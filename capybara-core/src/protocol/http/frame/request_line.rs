use std::borrow::Cow;
use std::fmt::{Debug, Display, Formatter};
use std::str::FromStr;

use bytes::{BufMut, Bytes, BytesMut};

use crate::error::CapybaraError::{ExceedMaxHttpHeaderSize, MalformedHttpPacket};
use crate::Result;

enum ParseState {
    MethodBegin,
    MethodFinish,
    PathBegin,
    QueriesBegin,
    VersionBegin,
    VersionEnd,
    End,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Method {
    CONNECT,
    DELETE,
    GET,
    HEAD,
    OPTIONS,
    PATCH,
    POST,
    PUT,
    TRACE,
}

impl FromStr for Method {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        TryFrom::try_from(s)
    }
}

impl TryFrom<&str> for Method {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        if value.len() < 3 || value.len() > 7 {
            bail!("invalid method '{}'", value);
        }
        match value {
            "CONNECT" => Ok(Self::CONNECT),
            "DELETE" => Ok(Self::DELETE),
            "GET" => Ok(Self::GET),
            "HEAD" => Ok(Self::HEAD),
            "OPTIONS" => Ok(Self::OPTIONS),
            "PATCH" => Ok(Self::PATCH),
            "POST" => Ok(Self::POST),
            "PUT" => Ok(Self::PUT),
            "TRACE" => Ok(Self::TRACE),
            other => bail!("invalid method '{}'", other),
        }
    }
}

impl Method {
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Method::CONNECT => "CONNECT",
            Method::DELETE => "DELETE",
            Method::GET => "GET",
            Method::HEAD => "HEAD",
            Method::OPTIONS => "OPTIONS",
            Method::PATCH => "PATCH",
            Method::POST => "POST",
            Method::PUT => "PUT",
            Method::TRACE => "TRACE",
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.as_str().as_bytes()
    }
}

impl AsRef<str> for Method {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// HTTP Method, 0000XXXX
const MAGIC_METHOD_CONNECT: u8 = 0b00000000;
const MAGIC_METHOD_DELETE: u8 = 0b00000001;
const MAGIC_METHOD_GET: u8 = 0b00000010;
const MAGIC_METHOD_HEAD: u8 = 0b00000011;
const MAGIC_METHOD_OPTIONS: u8 = 0b00000100;
const MAGIC_METHOD_POST: u8 = 0b00000101;
const MAGIC_METHOD_PUT: u8 = 0b00000110;
const MAGIC_METHOD_TRACE: u8 = 0b00000111;
const MAGIC_METHOD_PATCH: u8 = 0b00001000;

/// other flags: XXXX0000
const MAGIC_NOHTTP11: u8 = 0b00010000;

#[derive(Debug, Clone)]
struct QueriesIndex {
    offset: u32,
}

#[derive(Debug, Clone)]
pub struct RequestLine {
    pub(crate) b: Bytes,
    idx: Option<QueriesIndex>,
    magic: u8,
}

impl RequestLine {
    pub fn builder<'a>() -> RequestLineBuilder<'a> {
        RequestLineBuilder {
            uri_: None,
            method_: Method::GET,
            dnthttp11: false,
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.b.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn read(buf: &mut BytesMut, max_size: usize) -> Result<Option<RequestLine>> {
        const C: u8 = 1;
        const CO: u8 = 2;
        const CON: u8 = 3;
        const CONN: u8 = 4;
        const CONNE: u8 = 5;
        const CONNEC: u8 = 6;
        const CONNECT: u8 = 7;

        const D: u8 = 10;
        const DE: u8 = 11;
        const DEL: u8 = 12;
        const DELE: u8 = 13;
        const DELET: u8 = 14;
        const DELETE: u8 = 15;

        const G: u8 = 20;
        const GE: u8 = 21;
        const GET: u8 = 22;

        const H: u8 = 30;
        const HE: u8 = 31;
        const HEA: u8 = 32;
        const HEAD: u8 = 33;

        const O: u8 = 40;
        const OP: u8 = 41;
        const OPT: u8 = 42;
        const OPTI: u8 = 43;
        const OPTIO: u8 = 44;
        const OPTION: u8 = 45;
        const OPTIONS: u8 = 46;

        const P: u8 = 50;
        const PO: u8 = 51;
        const POS: u8 = 52;
        const POST: u8 = 53;

        const PU: u8 = 54;
        const PUT: u8 = 55;

        const PA: u8 = 56;
        const PAT: u8 = 57;
        const PATC: u8 = 58;
        const PATCH: u8 = 59;

        const T: u8 = 60;
        const TR: u8 = 61;
        const TRA: u8 = 62;
        const TRAC: u8 = 63;
        const TRACE: u8 = 64;

        const HT: u8 = 100;
        const HTT: u8 = 101;
        const HTTP: u8 = 102;
        const HTTP_: u8 = 103;
        const HTTP_1: u8 = 104;
        const HTTP_1_: u8 = 105;
        const HTTP_1_0: u8 = 106;
        const HTTP_1_1: u8 = 107;

        // method machine state
        let mut ms = 0u8;
        // version machine state
        let mut vs = 0u8;

        // the beginning parse state
        let mut s = ParseState::MethodBegin;

        let mut queries_offset: Option<u32> = None;
        let mut magic = 0u8;

        for (i, b) in buf.iter().enumerate() {
            if i > max_size {
                return Err(ExceedMaxHttpHeaderSize(max_size));
            }

            match s {
                ParseState::MethodBegin => match i {
                    0 => match *b {
                        b'C' => ms = C,
                        b'D' => ms = D,
                        b'G' => ms = G,
                        b'H' => ms = H,
                        b'O' => ms = O,
                        b'P' => ms = P,
                        b'T' => ms = T,
                        _ => return Err(MalformedHttpPacket("invalid http method".into())),
                    },
                    1 => match *b {
                        b'A' => match ms {
                            P => ms = PA,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'O' => match ms {
                            C => ms = CO,
                            P => ms = PO,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'E' => match ms {
                            D => ms = DE,
                            G => ms = GE,
                            H => ms = HE,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'U' => match ms {
                            P => ms = PU,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'R' => match ms {
                            T => ms = TR,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'P' => match ms {
                            O => ms = OP,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        _ => return Err(MalformedHttpPacket("invalid http method".into())),
                    },
                    2 => match *b {
                        b'A' => match ms {
                            HE => ms = HEA,
                            TR => ms = TRA,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'L' => match ms {
                            DE => ms = DEL,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'N' => match ms {
                            CO => ms = CON,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'S' => match ms {
                            PO => ms = POS,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'T' => match ms {
                            OP => ms = OPT,
                            PA => ms = PAT,
                            GE => ms = GET,
                            PU => ms = PUT,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        _ => return Err(MalformedHttpPacket("invalid http method".into())),
                    },
                    3 => match *b {
                        b' ' => match ms {
                            GET => {
                                s = ParseState::MethodFinish;
                                magic |= MAGIC_METHOD_GET;
                            }
                            PUT => {
                                s = ParseState::MethodFinish;
                                magic |= MAGIC_METHOD_PUT;
                            }
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'N' => match ms {
                            CON => ms = CONN,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'E' => match ms {
                            DEL => ms = DELE,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'I' => match ms {
                            OPT => ms = OPTI,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'D' => match ms {
                            HEA => ms = HEAD,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'T' => match ms {
                            POS => ms = POST,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'C' => match ms {
                            PAT => ms = PATC,
                            TRA => ms = TRAC,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        _ => return Err(MalformedHttpPacket("invalid http method".into())),
                    },
                    4 => match *b {
                        b' ' => match ms {
                            HEAD => {
                                s = ParseState::MethodFinish;
                                magic |= MAGIC_METHOD_HEAD;
                            }
                            POST => {
                                s = ParseState::MethodFinish;
                                magic |= MAGIC_METHOD_POST;
                            }
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'E' => match ms {
                            CONN => ms = CONNE,
                            TRAC => ms = TRACE,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'H' => match ms {
                            PATC => ms = PATCH,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'T' => match ms {
                            DELE => ms = DELET,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'O' => match ms {
                            OPTI => ms = OPTIO,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        _ => return Err(MalformedHttpPacket("invalid http method".into())),
                    },
                    5 => match *b {
                        b' ' => match ms {
                            TRACE => {
                                s = ParseState::MethodFinish;
                                magic |= MAGIC_METHOD_TRACE;
                            }
                            PATCH => {
                                s = ParseState::MethodFinish;
                                magic |= MAGIC_METHOD_PATCH;
                            }
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'C' => match ms {
                            CONNE => ms = CONNEC,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'E' => match ms {
                            DELET => ms = DELETE,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'N' => match ms {
                            OPTIO => ms = OPTION,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        _ => return Err(MalformedHttpPacket("invalid http method".into())),
                    },
                    6 => match *b {
                        b' ' => match ms {
                            DELETE => {
                                s = ParseState::MethodFinish;
                                magic |= MAGIC_METHOD_DELETE;
                            }
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'T' => match ms {
                            CONNEC => ms = CONNECT,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        b'S' => match ms {
                            OPTION => ms = OPTIONS,
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        _ => return Err(MalformedHttpPacket("invalid http method".into())),
                    },
                    7 => match *b {
                        b' ' => match ms {
                            CONNECT => {
                                s = ParseState::MethodFinish;
                                magic |= MAGIC_METHOD_CONNECT;
                            }
                            OPTIONS => {
                                s = ParseState::MethodFinish;
                                magic |= MAGIC_METHOD_OPTIONS;
                            }
                            _ => return Err(MalformedHttpPacket("invalid http method".into())),
                        },
                        _ => return Err(MalformedHttpPacket("invalid http method".into())),
                    },
                    _ => return Err(MalformedHttpPacket("invalid http method".into())),
                },
                ParseState::MethodFinish => match *b {
                    b'/' => s = ParseState::PathBegin,
                    _ => {
                        return Err(MalformedHttpPacket(
                            "invalid http path, a well-known path should start with '/'".into(),
                        ))
                    }
                },
                ParseState::PathBegin => match *b {
                    b'?' => {
                        queries_offset.replace(i as u32);
                        s = ParseState::QueriesBegin;
                    }
                    b' ' => s = ParseState::VersionBegin,
                    _ => (),
                },
                ParseState::QueriesBegin => {
                    match *b {
                        b' ' => s = ParseState::VersionBegin,
                        b'&' => {
                            // TODO: 是否需要在这里直接切queries
                        }
                        _ => (),
                    }
                }
                ParseState::VersionBegin => match vs {
                    0 => match *b {
                        b'H' => vs = H,
                        _ => return Err(MalformedHttpPacket("invalid http version".into())),
                    },
                    H => match *b {
                        b'T' => vs = HT,
                        _ => return Err(MalformedHttpPacket("invalid http version".into())),
                    },
                    HT => match *b {
                        b'T' => vs = HTT,
                        _ => return Err(MalformedHttpPacket("invalid http version".into())),
                    },
                    HTT => match *b {
                        b'P' => vs = HTTP,
                        _ => return Err(MalformedHttpPacket("invalid http version".into())),
                    },
                    HTTP => match *b {
                        b'/' => vs = HTTP_,
                        _ => return Err(MalformedHttpPacket("invalid http version".into())),
                    },
                    HTTP_ => match *b {
                        b'1' => vs = HTTP_1,
                        _ => {
                            return Err(MalformedHttpPacket(
                                "only 'HTTP/1.0' and 'HTTP/1.1' are supported".into(),
                            ))
                        }
                    },
                    HTTP_1 => match *b {
                        b'.' => vs = HTTP_1_,
                        _ => return Err(MalformedHttpPacket("invalid http version".into())),
                    },
                    HTTP_1_ => match *b {
                        b'0' => {
                            vs = HTTP_1_0;
                            s = ParseState::VersionEnd;
                            magic |= MAGIC_NOHTTP11;
                        }
                        b'1' => {
                            vs = HTTP_1_1;
                            s = ParseState::VersionEnd;
                        }
                        _ => {
                            return Err(MalformedHttpPacket(
                                "only 'HTTP/1.0' and 'HTTP/1.1' are supported".into(),
                            ))
                        }
                    },
                    _ => unreachable!(),
                },
                ParseState::VersionEnd => match *b {
                    b'\r' => s = ParseState::End,
                    _ => {
                        return Err(MalformedHttpPacket(
                            "CRLF should be after http version".into(),
                        ))
                    }
                },
                ParseState::End => match *b {
                    b'\n' => {
                        let b = buf.split_to(i + 1).freeze();
                        return Ok(Some(Self {
                            b,
                            idx: queries_offset.map(|offset| QueriesIndex { offset }),
                            magic,
                        }));
                    }
                    _ => {
                        return Err(MalformedHttpPacket(
                            "CRLF should be after http version".into(),
                        ))
                    }
                },
            }
        }

        Ok(None)
    }

    #[inline]
    pub fn method(&self) -> Method {
        match self.magic & 0b00001111 {
            MAGIC_METHOD_CONNECT => Method::CONNECT,
            MAGIC_METHOD_DELETE => Method::DELETE,
            MAGIC_METHOD_GET => Method::GET,
            MAGIC_METHOD_HEAD => Method::HEAD,
            MAGIC_METHOD_OPTIONS => Method::OPTIONS,
            MAGIC_METHOD_PATCH => Method::PATCH,
            MAGIC_METHOD_POST => Method::POST,
            MAGIC_METHOD_PUT => Method::PUT,
            MAGIC_METHOD_TRACE => Method::TRACE,
            _ => unreachable!(),
        }
    }

    pub fn path(&self) -> Cow<str> {
        let offset = self.method().as_str().len() + 1;
        match &self.idx {
            None => String::from_utf8_lossy(&self.b[offset..self.b.len() - 11]),
            Some(idx) => String::from_utf8_lossy(&self.b[offset..idx.offset as usize]),
        }
    }

    /// Returns the request path in a well-known format which trims the slash.
    /// For example, 'GET /////your/api/path/' will get the result of '/your/api/path'.
    pub fn path_no_slash(&self) -> Cow<str> {
        let mut offset = self.method().as_str().len() + 1;
        let mut end = match &self.idx {
            None => self.b.len() - 11,
            Some(idx) => idx.offset as usize,
        };

        loop {
            if end - offset <= 1 {
                break;
            }
            match self.b.get(end - 1) {
                Some(b'/') => end -= 1,
                _ => break,
            }
        }

        if matches!(self.b.get(offset), Some(b'/')) {
            loop {
                if end - offset <= 1 {
                    break;
                }

                if let Some(b'/') = self.b.get(offset + 1) {
                    offset += 1;
                    continue;
                }

                break;
            }
        }

        String::from_utf8_lossy(&self.b[offset..end])
    }

    pub fn path_bytes(&self) -> &[u8] {
        let offset = self.method().as_str().len() + 1;
        match &self.idx {
            None => &self.b[offset..self.b.len() - 11],
            Some(idx) => &self.b[offset..idx.offset as usize],
        }
    }

    pub fn uri(&self) -> &[u8] {
        let offset = self.method().as_str().len() + 1;
        &self.b[offset..self.b.len() - 11]
    }

    pub fn query_bytes(&self) -> Option<&[u8]> {
        if let Some(idx) = &self.idx {
            return Some(&self.b[idx.offset as usize + 1..self.b.len() - 11]);
        }
        None
    }

    pub fn query(&self) -> Option<Cow<str>> {
        if let Some(idx) = &self.idx {
            return Some(String::from_utf8_lossy(
                &self.b[idx.offset as usize + 1..self.b.len() - 11],
            ));
        }
        None
    }

    pub fn version(&self) -> &[u8] {
        &self.b[self.b.len() - 10..self.b.len() - 2]
    }

    pub fn nohttp11(&self) -> bool {
        self.magic & MAGIC_NOHTTP11 == MAGIC_NOHTTP11
    }
}

impl Display for RequestLine {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(unsafe { std::str::from_utf8_unchecked(&self.b[..]) })
    }
}

pub struct RequestLineBuilder<'a> {
    method_: Method,
    uri_: Option<&'a str>,
    dnthttp11: bool,
}

impl<'a> RequestLineBuilder<'a> {
    pub fn method(mut self, method: Method) -> Self {
        self.method_ = method;
        self
    }

    pub fn http10(mut self) -> Self {
        self.dnthttp11 = true;
        self
    }

    pub fn uri(mut self, uri: &'a str) -> Self {
        self.uri_.replace(uri);
        self
    }

    pub fn build(self) -> RequestLine {
        let Self {
            method_,
            uri_,
            dnthttp11,
        } = self;

        let mut b = BytesMut::new();

        let mut magic = match method_ {
            Method::CONNECT => MAGIC_METHOD_CONNECT,
            Method::DELETE => MAGIC_METHOD_DELETE,
            Method::GET => MAGIC_METHOD_GET,
            Method::HEAD => MAGIC_METHOD_HEAD,
            Method::OPTIONS => MAGIC_METHOD_OPTIONS,
            Method::POST => MAGIC_METHOD_POST,
            Method::PUT => MAGIC_METHOD_PUT,
            Method::TRACE => MAGIC_METHOD_TRACE,
            Method::PATCH => MAGIC_METHOD_PATCH,
        };
        let mut q = None;

        b.put(method_.as_bytes());
        b.put_u8(b' ');
        match uri_ {
            Some(uri) => {
                if let Some(offset) = uri.find('?') {
                    q.replace(b.len() + offset);
                }
                b.put(uri.as_bytes())
            }
            None => b.put_u8(b'/'),
        }

        if dnthttp11 {
            magic |= MAGIC_NOHTTP11;
            b.put(&b" HTTP/1.0\r\n"[..]);
        } else {
            b.put(&b" HTTP/1.1\r\n"[..]);
        }

        RequestLine {
            b: b.freeze(),
            magic,
            idx: q.map(|offset| QueriesIndex {
                offset: offset as u32,
            }),
        }
    }
}

impl Into<Bytes> for RequestLine {
    fn into(self) -> Bytes {
        self.b
    }
}

#[cfg(test)]
mod request_line_tests {
    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[test]
    fn test_build_request_line() {
        let uri = "////delete/?id=1";
        let rl = RequestLine::builder()
            .method(Method::DELETE)
            .uri(uri)
            .http10()
            .build();
        assert_eq!(&b"DELETE ////delete/?id=1 HTTP/1.0\r\n"[..], &rl.b[..]);
        assert!(rl.query_bytes().is_some_and(|it| b"id=1".eq(it)));
        assert_eq!("////delete/", rl.path().as_ref());
        assert_eq!("/delete", rl.path_no_slash().as_ref());
        assert_eq!(uri.as_bytes(), rl.uri());
        assert_eq!(&b"HTTP/1.0"[..], rl.version());
    }

    #[test]
    fn test_read_request_line() {
        init();

        {
            let mut b = BytesMut::from(&b"GET /get/?foo=bar HTTP/1.1\r\nHos"[..]);
            let res = RequestLine::read(&mut b, usize::MAX);
            assert_eq!(3, b.len());

            assert!(
                res.as_ref().is_ok_and(|it| it.is_some()),
                "should return a request line"
            );

            let rl = res.unwrap().unwrap();

            assert_eq!(Method::GET, rl.method());
            assert_eq!(&b"/get/?foo=bar"[..], rl.uri());
            assert_eq!("/get/", rl.path());
            assert_eq!("/get", rl.path_no_slash());
            assert_eq!(Some(Cow::from("foo=bar")), rl.query());
            assert_eq!(&b"HTTP/1.1"[..], rl.version());

            info!("request_line: {}", &rl);
        }

        {
            let mut b = BytesMut::from(&b"HEAD / HTTP/1.0\r\n"[..]);
            let res = RequestLine::read(&mut b, usize::MAX);
            assert!(b.is_empty());

            assert!(
                res.as_ref().is_ok_and(|it| it.is_some()),
                "should return a request line"
            );

            let rl = res.unwrap().unwrap();

            assert_eq!(Method::HEAD, rl.method());
            assert_eq!(&b"/"[..], rl.uri());
            assert_eq!("/", rl.path());
            assert_eq!("/", rl.path_no_slash());
            assert!(rl.query().is_none());
            assert_eq!(&b"HTTP/1.0"[..], rl.version());

            info!("request_line: {}", &rl);
        }
    }

    #[test]
    fn test_request_line_all() {
        init();

        // well format
        for (method, raw) in [
            (Method::CONNECT, "CONNECT /connect HTTP/1.1\r\nHos"),
            (Method::DELETE, "DELETE /delete HTTP/1.1\r\nHos"),
            (Method::GET, "GET /get HTTP/1.1\r\nHos"),
            (Method::HEAD, "HEAD /head HTTP/1.1\r\nHos"),
            (Method::OPTIONS, "OPTIONS /options HTTP/1.1\r\nHos"),
            (Method::PATCH, "PATCH /patch HTTP/1.1\r\nHos"),
            (Method::POST, "POST /post HTTP/1.1\r\nHos"),
            (Method::PUT, "PUT /put HTTP/1.1\r\nHos"),
            (Method::TRACE, "TRACE /trace HTTP/1.1\r\nHos"),
        ] {
            let desc = raw.replace("\r\n", r"\r\n");
            info!("RequestLine::read < {}", desc);
            let mut b = BytesMut::from(raw.as_bytes());
            let res = RequestLine::read(&mut b, usize::MAX);
            assert_eq!(3, b.len());
            assert!(
                res.as_ref().is_ok_and(|it| it.is_some()),
                "should return a request line"
            );

            let uri = &format!("/{}", method.as_str().to_lowercase());

            let rl = res.unwrap().unwrap();
            assert_eq!(method, rl.method());
            assert_eq!(uri.as_bytes(), rl.uri());
            assert_eq!(&*uri, &*rl.path());
            assert_eq!(&*uri, &*rl.path_no_slash());
            assert_eq!(None, rl.query());
            assert_eq!(&b"HTTP/1.1"[..], rl.version());
        }

        // invalid format
        for raw in [
            "get /get HTTP/1.1\r\nHos",
            " GET /get HTTP/1.1\r\nHos",
            "FUCK /fuck HTTP/1.1\r\nHos",
            "GET /g e t HTTP/1.1\r\nHos",
            "GET /get  HTTP/1.1\r\nHos",
            "GET /get  HTTP/ 1.1\rHos",
            "GET /get  HTTP/ 1.1\r\nHos",
            "GETT /get  HTTP/ 1.1\r\nHos",
        ] {
            let desc = raw.replace("\r\n", r"\r\n");
            info!("RequestLine::read < {}", desc);
            let mut b = BytesMut::from(raw.as_bytes());
            let res = RequestLine::read(&mut b, usize::MAX);
            assert!(res.is_err(), "{} should be invalid", desc);
        }
    }
}
