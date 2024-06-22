use std::borrow::Cow;
use std::fmt::{Debug, Formatter, Write};

use bytes::{BufMut, Bytes, BytesMut};
use small_map::SmallMap;
use smallvec::SmallVec;

use crate::error::Error::{ExceedMaxHttpHeaderSize, MalformedHttpPacket};
use crate::protocol::http::httpfield::HttpField;
use crate::protocol::http::misc;
use crate::Result;

/// store the position of header in the whole headers bytes.
///
/// For example: `Content-Type: application/json\r\n`
///              |            |                    |
///              +---> begin  +---> colon          +---> end
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Position {
    begin_: u16,
    end_: u16,
    colon: u16,
    v_begin: u16,
    v_end: u16,
}

impl Position {
    #[inline(always)]
    fn new(begin: usize, end: usize, colon: usize, v_begin: usize, v_end: usize) -> Position {
        Position {
            begin_: begin as u16,
            end_: end as u16,
            colon: colon as u16,
            v_begin: v_begin as u16,
            v_end: v_end as u16,
        }
    }

    #[inline(always)]
    fn begin(&self) -> usize {
        self.begin_ as usize
    }

    #[inline(always)]
    fn end(&self) -> usize {
        self.end_ as usize
    }

    #[inline(always)]
    fn breakpoint(&self) -> usize {
        self.colon as usize
    }

    #[inline(always)]
    fn value_begin(&self) -> usize {
        self.v_begin as usize
    }

    #[inline(always)]
    fn value_end(&self) -> usize {
        self.v_end as usize
    }

    fn len(&self) -> usize {
        (self.end_ - self.begin_) as usize
    }
}

const N: usize = 32;

#[derive(Default)]
pub(crate) struct Indices(SmallMap<N, u16, SmallVec<[u16; 1]>, ahash::RandomState>);

impl Indices {
    #[inline(always)]
    fn new() -> Self {
        Self(SmallMap::new())
    }

    fn set(&mut self, key: u16, value: u16) {
        match self.0.get_mut(&key) {
            None => {
                let mut v = SmallVec::<[u16; 1]>::new();
                v.push(value);
                self.0.insert(key, v);
            }
            Some(v) => v.push(value),
        }
    }

    fn get(&self, key: &u16) -> Option<&SmallVec<[u16; 1]>> {
        self.0.get(key)
    }
}

pub(crate) type Positions = SmallVec<[Position; N]>;

/// headers represents multiple http headers.
pub struct Headers {
    /// raw bytes
    pub(crate) b: Bytes,
    /// position anchors
    pub(crate) pos: Positions,
    pub(crate) indices: Indices,
}

impl Clone for Headers {
    fn clone(&self) -> Self {
        let mut indices = Indices::default();
        for (i, pos) in self.pos.iter().enumerate() {
            let key = &self.b[pos.begin()..pos.breakpoint()];
            indices.set(misc::hash16(key), i as u16);
        }
        Self {
            b: Clone::clone(&self.b),
            pos: Clone::clone(&self.pos),
            indices,
        }
    }

    fn clone_from(&mut self, source: &Self) {
        self.b = Clone::clone(&source.b);
        Clone::clone_from(&mut self.pos, &source.pos);

        let mut indices = Indices::default();
        for (i, pos) in source.pos.iter().enumerate() {
            let key = &source.b[pos.begin()..pos.breakpoint()];
            indices.set(misc::hash16(key), i as u16);
        }
        self.indices = indices;
    }
}

impl Debug for Headers {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Headers").field("b", &self.b).finish()
    }
}

impl Headers {
    pub fn builder() -> HeadersBuilder {
        Default::default()
    }

    #[inline]
    pub fn read(buf: &mut BytesMut, max_size: usize) -> Result<Option<Self>> {
        Self::read_ext(buf, max_size, false)
    }

    #[inline]
    pub fn read_ext(buf: &mut BytesMut, max_size: usize, lowercase: bool) -> Result<Option<Self>> {
        let mut offset = 0;
        let mut is_prev_r = false;
        let mut n = 0;
        let mut positions = Positions::new();
        let mut eof = false;
        let mut colon_offset = -1isize;
        let mut indices = Indices::new();

        let mut hash = 0u16;

        let mut blanks = 0usize;
        let mut blanks0 = None;

        for (i, b) in buf.iter_mut().enumerate() {
            if i >= max_size {
                return Err(ExceedMaxHttpHeaderSize(max_size));
            }

            if colon_offset == -1 {
                match *b {
                    b':' => {
                        colon_offset = i as isize;
                        is_prev_r = false;
                    }
                    b'\r' => is_prev_r = true,
                    b'\n' => {
                        if is_prev_r {
                            is_prev_r = false;
                            n = i + 1;

                            // bingo
                            if i - offset == 1 {
                                eof = true;
                                break;
                            }
                            return Err(MalformedHttpPacket("incomplete CRLF".into()));
                        }
                    }
                    other => {
                        // https://datatracker.ietf.org/doc/html/rfc822#section-3.1.2
                        if other <= 0x20 || other >= 0x7f {
                            return Err(MalformedHttpPacket(
                                format!("invalid header field character '{}'", other).into(),
                            ));
                        }
                        is_prev_r = false;

                        // TODO: should we convert '-' to '_'?
                        hash = (hash << 5).wrapping_sub(hash);

                        if other.is_ascii_uppercase() {
                            // convert header name to lower-case
                            if lowercase {
                                *b = other | 0x20;
                            }
                            hash = hash.wrapping_add((other | 0x20) as u16);
                        } else {
                            hash = hash.wrapping_add(other as u16);
                        }
                    }
                }
            } else {
                match *b {
                    b'\r' => {
                        blanks += 1;
                        is_prev_r = true;
                    }
                    b'\n' => {
                        blanks += 1;
                        if is_prev_r {
                            is_prev_r = false;
                            n = i + 1;

                            // bingo
                            if i - offset == 1 {
                                eof = true;
                                break;
                            }

                            indices.set(hash, positions.len() as u16);
                            positions.push(Position::new(
                                offset,
                                i + 1,
                                colon_offset as usize,
                                colon_offset as usize + 1 + blanks0.unwrap_or_default(),
                                i + 1 - blanks,
                            ));

                            offset = i + 1;

                            // cleanup
                            colon_offset = -1;
                            hash = 0;
                            blanks = 0;
                            blanks0 = None;
                        }
                    }
                    b' ' | b'\t' | 0x0b | 0x0c | 0x85 | 0xA0 => {
                        is_prev_r = false;
                        blanks += 1;
                    }
                    _ => {
                        if blanks0.is_none() {
                            blanks0.replace(blanks);
                        }
                        blanks = 0;

                        is_prev_r = false;
                    }
                }
            }
        }

        Ok(if eof {
            Some(Headers {
                b: buf.split_to(n).freeze(),
                pos: positions,
                indices,
            })
        } else {
            None
        })
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.pos.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.pos.is_empty()
    }

    #[inline]
    pub fn get_content_length(&self) -> Result<Option<usize>> {
        match self.get_by_field(HttpField::ContentLength) {
            Some(b) => {
                let s = unsafe { std::str::from_utf8_unchecked(b) };
                match s.parse::<usize>() {
                    Ok(n) => Ok(Some(n)),
                    Err(e) => Err(MalformedHttpPacket(
                        format!("invalid content-length '{}'", s).into(),
                    )),
                }
            }
            None => Ok(None),
        }
    }

    #[inline]
    pub fn nth(&self, i: usize) -> Option<(&[u8], &[u8])> {
        if let Some(pos) = self.pos.get(i) {
            let key = &self.b[pos.begin()..pos.breakpoint()];

            let val = {
                let mut i = pos.breakpoint() + 1;
                let mut j = pos.end() - 2;

                while misc::is_ascii_space(self.b[j - 1]) && i < j {
                    j -= 1;
                }

                while misc::is_ascii_space(self.b[i]) && i < j {
                    i += 1;
                }

                &self.b[i..j]
            };

            return Some((key, val));
        }

        None
    }

    pub fn iter(&self) -> Iter {
        Iter {
            headers: self,
            cursor: 0,
        }
    }

    #[inline]
    pub fn get_bytes(&self, key: &str) -> Option<&[u8]> {
        if key.is_empty() {
            return None;
        }

        let hash = misc::hash16(key.as_bytes());

        if let Some(it) = self.indices.get(&hash) {
            for i in it.iter().rev() {
                if let Some(pos) = self.pos.get(*i as usize) {
                    if key
                        .as_bytes()
                        .eq_ignore_ascii_case(&self.b[pos.begin()..pos.breakpoint()])
                    {
                        return Some(&self.b[pos.value_begin()..pos.value_end()]);
                    }
                }
            }
        }

        None
    }

    #[inline]
    pub fn get_by_field(&self, h: HttpField) -> Option<&[u8]> {
        let magic: u16 = h.into();
        let key = h.as_bytes();
        if let Some(it) = self.indices.get(&magic) {
            for i in it.iter().rev() {
                if let Some(pos) = self.pos.get(*i as usize) {
                    if key.eq_ignore_ascii_case(&self.b[pos.begin()..pos.breakpoint()]) {
                        return Some(&self.b[pos.value_begin()..pos.value_end()]);
                    }
                }
            }
        }

        None
    }

    #[inline]
    pub fn get(&self, key: &str) -> Option<Cow<str>> {
        if key.is_empty() {
            return None;
        }

        let magic = misc::hash16(key.as_bytes());

        if let Some(it) = self.indices.get(&magic) {
            for i in it.iter().rev() {
                if let Some(pos) = self.pos.get(*i as usize) {
                    if key
                        .as_bytes()
                        .eq_ignore_ascii_case(&self.b[pos.begin()..pos.breakpoint()])
                    {
                        return Some(String::from_utf8_lossy(
                            &self.b[pos.value_begin()..pos.value_end()],
                        ));
                    }
                }
            }
        }

        None
    }

    #[inline]
    pub fn position(&self, key: &str) -> Option<usize> {
        if key.is_empty() {
            return None;
        }

        let magic = misc::hash16(key.as_bytes());

        if let Some(v) = self.indices.get(&magic) {
            for i in v.iter() {
                let j = *i as usize;
                if let Some(pos) = self.pos.get(j) {
                    if key
                        .as_bytes()
                        .eq_ignore_ascii_case(&self.b[pos.begin()..pos.breakpoint()])
                    {
                        return Some(j);
                    }
                }
            }
        }

        None
    }

    /// search positions from key ignore case.
    #[inline]
    pub fn positions(&self, key: &str) -> SmallVec<[usize; 1]> {
        let mut ret = SmallVec::<[usize; 1]>::new();
        if key.is_empty() {
            return ret;
        }

        let h = misc::hash16(key.as_bytes());

        if let Some(indices) = self.indices.get(&h) {
            for i in indices {
                let j = *i as usize;
                if let Some(pos) = self.pos.get(j) {
                    if key
                        .as_bytes()
                        .eq_ignore_ascii_case(&self.b[pos.begin()..pos.breakpoint()])
                    {
                        ret.push(j);
                    }
                }
            }
        }

        ret
    }
}

impl Into<Bytes> for Headers {
    fn into(self) -> Bytes {
        self.b
    }
}

impl AsRef<[u8]> for Headers {
    fn as_ref(&self) -> &[u8] {
        self.b.as_ref()
    }
}

#[derive(Default)]
pub struct HeadersBuilder {
    b: Option<BytesMut>,
    pos: Positions,
    indices: Indices,
}

impl HeadersBuilder {
    pub fn len(&self) -> usize {
        self.pos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pos.is_empty()
    }

    pub fn put<K, V>(mut self, key: K, value: V) -> Self
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let key = key.as_ref().trim();
        let value = value.as_ref().trim();
        let kh = misc::hash16(key.as_ref());

        let (offset, size) = match &mut self.b {
            Some(b) => {
                let offset = b.len();
                b.write_str(key).ok();
                b.write_str(": ").ok();
                b.write_str(value).ok();
                b.write_str(unsafe { std::str::from_utf8_unchecked(misc::CRLF) })
                    .ok();
                (offset, b.len())
            }
            None => {
                let mut b = BytesMut::with_capacity(128);
                b.write_str(key).ok();
                b.write_str(": ").ok();
                b.write_str(value).ok();
                b.write_str(unsafe { std::str::from_utf8_unchecked(misc::CRLF) })
                    .ok();
                let size = b.len();
                self.b.replace(b);
                (0, size)
            }
        };

        self.indices.set(kh, self.pos.len() as u16);
        let colon = offset + key.len();
        self.pos
            .push(Position::new(offset, size, colon, colon + 2, size - 2));
        self
    }

    pub fn complete(mut self) -> Self {
        match &mut self.b {
            Some(b) => b.put_slice(misc::CRLF),
            None => {
                self.b.replace(BytesMut::from(misc::CRLF));
            }
        }
        self
    }

    pub fn build(self) -> Headers {
        let Self { b, pos, indices } = self;
        Headers {
            b: match b {
                Some(b) => b.freeze(),
                None => Bytes::new(),
            },
            pos,
            indices,
        }
    }
}

pub struct Iter<'a> {
    headers: &'a Headers,
    cursor: usize,
}

/// Header represents a line of http header.
/// For example:
/// ```
///    let raw_header_bytes = &b"Host: nginx/1.8.0\r\n"[..];
/// ```
pub struct Header<'a>(&'a [u8]);

impl Header<'_> {
    pub fn key(&self) -> &[u8] {
        let pos = self.get_position();
        &self.0[..pos]
    }

    pub fn value(&self) -> &[u8] {
        let mut pos = self.get_position() + 1;
        let raw = &self.0[pos..];
        for (i, b) in raw.iter().enumerate() {
            match b {
                b' ' => (),
                _ => {
                    pos += i;
                    break;
                }
            }
        }
        &self.0[pos..self.0.len() - 2]
    }

    pub fn key_str(&self) -> Cow<str> {
        String::from_utf8_lossy(self.key())
    }

    pub fn value_str(&self) -> Cow<str> {
        String::from_utf8_lossy(self.value())
    }

    #[inline]
    fn get_position(&self) -> usize {
        self.0.iter().position(|it| *it == b':').unwrap()
    }
}

impl AsRef<[u8]> for Header<'_> {
    fn as_ref(&self) -> &[u8] {
        self.0
    }
}

impl IntoIterator for Headers {
    type Item = Bytes;
    type IntoIter = RawHeaderIter;

    fn into_iter(self) -> Self::IntoIter {
        Self::IntoIter {
            headers: self,
            cur: 0,
        }
    }
}

pub struct RawHeaderIter {
    headers: Headers,
    cur: usize,
}

impl Iterator for RawHeaderIter {
    type Item = Bytes;

    fn next(&mut self) -> Option<Self::Item> {
        match self.headers.pos.get(self.cur) {
            None => None,
            Some(pos) => {
                let item = self.headers.b.split_to(pos.len());
                self.cur += 1;
                Some(item)
            }
        }
    }
}

impl<'a> Iterator for Iter<'a> {
    type Item = Header<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor >= self.headers.pos.len() {
            None
        } else {
            let pos = self.headers.pos.get(self.cursor).unwrap();
            self.cursor += 1;
            let b = &self.headers.b[pos.begin()..pos.end()];
            Some(Header(b))
        }
    }
}

#[cfg(test)]
mod http_headers_tests {
    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[test]
    fn test_malformed_headers() {
        init();
        let mut b = BytesMut::from(&b"x-your-header\r\n"[..]);

        let res = Headers::read(&mut b, usize::MAX);
        assert!(res.is_err());

        let mut b = BytesMut::from(&b"Content-Type\x00: text/plain\r\n"[..]);
        let res = Headers::read(&mut b, usize::MAX);
        assert!(res.is_err());
    }

    #[test]
    fn test_read_empty_headers() {
        init();

        let mut b = BytesMut::from(&b"\r\nFoo"[..]);

        let res = Headers::read(&mut b, usize::MAX);
        assert_eq!(3, b.len());
        assert!(res.is_ok_and(|it| it.is_some_and(|it| {
            if it.is_empty() {
                let bb: Bytes = it.into();
                bb.as_ref().eq(&b"\r\n"[..])
            } else {
                false
            }
        })));
    }

    #[test]
    fn test_get() {
        init();

        let mut b = BytesMut::from(
            &b"\
        Accept: *\r\n\
        foo:\r\n\
        bar:123\r\n\
        qux:    456\r\n\
        blank:       \r\n\
        dog:   \r  \n dog  \r   \r\n\
        bear:   b e a r    \r\n\
        space1: \r\n\
        space2:  \r\n\
        \r\n"[..],
        );

        let h = Headers::read_ext(&mut b, usize::MAX, true)
            .unwrap()
            .unwrap();
        assert!(h.get("foo").is_some_and(|it| {
            let s = it.as_ref();
            s.is_empty()
        }));
        assert!(h.get("accept").is_some_and(|it| "*".eq(it.as_ref())));
        assert!(h.get("bar").is_some_and(|it| "123".eq(it.as_ref())));
        assert!(h.get("qux").is_some_and(|it| "456".eq(it.as_ref())));
        assert!(h.get("blank").is_some_and(|it| "".eq(it.as_ref())));
        assert!(h.get("dog").is_some_and(|it| "dog".eq(it.as_ref())));
        assert!(h.get("bear").is_some_and(|it| "b e a r".eq(it.as_ref())));
        assert!(h.get("space1").is_some_and(|it| "".eq(it.as_ref())));
        assert!(h.get("space2").is_some_and(|it| "".eq(it.as_ref())));

        assert!(h
            .get_by_field(HttpField::Accept)
            .is_some_and(|it| it.eq(&b"*"[..])));

        info!("headers: {:?}", &h);
    }

    #[test]
    fn test_read_partial_headers() {
        init();

        let mut b = BytesMut::from(&b"Host: localhost:8080\r\nContent-Type: text/html\r\nAcc"[..]);

        let res = Headers::read(&mut b, usize::MAX);
        assert!(res.is_ok_and(|h| h.is_none()), "not enough headers bytes");
    }

    #[test]
    fn test_headers_positions() {
        init();

        let mut b = BytesMut::from(
            &b"\
        Host: localhost:8080\r\n\
        Foo: 1\r\n\
        Content-Type: text/html\r\n\
        Foo: 2\r\n\
        Accept: *\r\n\
        \r\n"[..],
        );

        let headers = Headers::read(&mut b, usize::MAX).unwrap().unwrap();

        let pos = headers.positions("fOo");
        assert_eq!(2, pos.len());

        let mut i = pos.into_iter();
        let next = headers.nth(i.next().unwrap());
        assert!(next.is_some_and(|(_, v)| b"1".eq(v)));
        let next = headers.nth(i.next().unwrap());
        assert!(next.is_some_and(|(_, v)| b"2".eq(v)));

        assert!(i.next().is_none());
    }

    #[test]
    fn test_read_complete_headers() {
        init();

        let origin = b"Host: localhost:8080\r\nAccept-Encoding: gzip, deflate\r\nAccept: */*\r\nConnection: keep-alive\r\nUser-Agent: HTTPie/3.2.2\r\n\r\nfoo";
        let mut b = BytesMut::from(&origin[..]);

        let res = Headers::read(&mut b, usize::MAX);
        assert!(res.is_ok());
        let res = res.unwrap();
        assert!(res.is_some(), "part should not be None");
        assert_eq!(3, b.len());
        assert!(res.is_some(), "bad complete headers result");

        let headers = res.unwrap();

        assert_eq!(5, headers.len());

        let check = |k: &str, v: &str| {
            assert_eq!(Some(v.into()), headers.get(k).map(|it| it.to_string()));
        };

        check("host", "localhost:8080");
        check("accept-encoding", "gzip, deflate");
        check("accept", "*/*");
        check("connection", "keep-alive");
        check("user-agent", "HTTPie/3.2.2");

        let chk = |header: Option<Header>, key: &str, val: &str| {
            assert!(header.is_some());
            let header = header.unwrap();
            let k = header.key_str();
            let v = header.value_str();
            assert!(key.eq_ignore_ascii_case(k.as_ref()));
            assert_eq!(val, v.as_ref());
            assert_eq!(key.len() + 4 + val.len(), header.0.len());

            let s = format!("{}: {}\r\n", key, val);
            let raw: &[u8] = header.as_ref();
            assert!(raw.eq_ignore_ascii_case(s.as_bytes()));
        };

        let mut hi = headers.iter();
        chk(hi.next(), "host", "localhost:8080");
        chk(hi.next(), "accept-encoding", "gzip, deflate");
        chk(hi.next(), "accept", "*/*");
        chk(hi.next(), "connection", "keep-alive");
        chk(hi.next(), "user-agent", "HTTPie/3.2.2");
        assert!(hi.next().is_none());
        assert!(hi.next().is_none());

        let chk2 = |header: Option<Bytes>, expect: &str| {
            assert!(header.is_some());
            let header = header.unwrap();
            assert!(expect.as_bytes().eq_ignore_ascii_case(header.as_ref()));
        };

        let raw = headers.as_ref();
        assert_eq!(&origin[..origin.len() - 3], raw);

        let mut iter = headers.into_iter();

        chk2(iter.next(), "host: localhost:8080\r\n");
        chk2(iter.next(), "accept-encoding: gzip, deflate\r\n");
        chk2(iter.next(), "accept: */*\r\n");
        chk2(iter.next(), "connection: keep-alive\r\n");
        chk2(iter.next(), "user-agent: HTTPie/3.2.2\r\n");
        assert!(iter.next().is_none());
        assert!(iter.next().is_none());
    }

    #[test]
    fn test_build_headers() {
        let raw = &b"foo: aaa\r\nbar: bbb\r\n\r\n"[..];
        let mut b = BytesMut::from(raw);
        let h0 = Headers::read(&mut b, usize::MAX).unwrap().unwrap();
        assert!(b.is_empty());

        let bu = Headers::builder()
            .put("foo", "aaa")
            .put("bar", "bbb")
            .complete();
        assert_eq!(2, bu.len());
        let h = bu.build();

        assert_eq!(&h0.pos, &h.pos, "generated positions should be same");

        let s0 = unsafe { std::str::from_utf8_unchecked(h0.b.as_ref()) };
        let s1 = unsafe { std::str::from_utf8_unchecked(h.b.as_ref()) };
        assert_eq!(s0, s1);

        let foo = h.get("foo").map(|it| it.to_string());
        let bar = h.get("bar").map(|it| it.to_string());
        let qux = h.get("qux").map(|it| it.to_string());

        assert_eq!(Some("aaa".to_string()), foo);
        assert_eq!(Some("bbb".to_string()), bar);
        assert!(qux.is_none());

        assert!(h.nth(0).is_some_and(|(k, v)| k.eq(b"foo") && v.eq(b"aaa")));
        assert!(h.nth(1).is_some_and(|(k, v)| k.eq(b"bar") && v.eq(b"bbb")));
        assert!(h.nth(2).is_none());
    }
}
