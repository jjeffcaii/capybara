use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::error::CapybaraError::{ExceedMaxHttpBodySize, MalformedHttpPacket};
use crate::Result;

#[derive(Default, Clone)]
pub struct ChunksBuilder {
    b: BytesMut,
}

impl ChunksBuilder {
    pub fn append<B>(&mut self, b: B)
    where
        B: Buf,
    {
        use std::fmt::Write;

        let size = b.remaining();
        if size > 0 {
            write!(&mut self.b, "{:x}\r\n", size).ok();
            self.b.put(b);
            self.b.put(&b"\r\n"[..]);
        }
    }

    pub fn clear(&mut self) {
        self.b.clear();
    }

    pub fn build(&mut self) -> Chunks {
        self.b.put(&b"0\r\n\r\n"[..]);
        Chunks(self.b.split().freeze())
    }

    pub fn build_incomplete(&mut self) -> Chunks {
        Chunks(self.b.split().freeze())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chunks(pub(crate) Bytes);

impl Chunks {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0[..]
    }

    pub fn into_bytes(self) -> Bytes {
        self.0
    }

    pub fn builder() -> ChunksBuilder {
        ChunksBuilder {
            b: BytesMut::default(),
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> Iter {
        Iter { b: self.0.as_ref() }
    }

    #[inline]
    pub fn read(buf: &mut BytesMut, max_size: usize) -> Result<Option<(Chunks, bool)>> {
        let mut cr = false;
        let mut length = -1;
        let mut at = 0;
        let mut eof = false;
        for (i, b) in buf.iter().enumerate() {
            if i >= max_size {
                return Err(ExceedMaxHttpBodySize(max_size));
            }
            if length > 0 {
                length -= 1;
            }

            match *b {
                b'\r' => {
                    if length == 0 {
                        return Err(MalformedHttpPacket("chunk should start with CRLF".into()));
                    }
                    cr = true;
                }
                b'\n' => {
                    if !cr {
                        if length == 0 {
                            return Err(MalformedHttpPacket("chunk should start with CRLF".into()));
                        }
                    } else {
                        cr = false;
                        if length == -1 {
                            let b = &buf[at..i - 1];
                            let chunk_size_str = unsafe { std::str::from_utf8_unchecked(b) };
                            match isize::from_str_radix(chunk_size_str, 16) {
                                Ok(l) => length = l,
                                Err(_) => {
                                    return Err(MalformedHttpPacket(
                                        format!(
                                        "invalid chunk length string '{}', should be in hex format",
                                        chunk_size_str
                                    )
                                        .into(),
                                    ))
                                }
                            }
                            if length == 0 {
                                eof = true;
                            }
                            length += 2;
                        } else if length == 0 {
                            at = i + 1;
                            if eof {
                                break;
                            }
                            length = -1;
                        }
                    }
                }
                _ => {
                    if length == 0 {
                        return Err(MalformedHttpPacket("invalid http chunk body".into()));
                    }
                    cr = false;
                }
            }
        }

        if at < 1 {
            Ok(None)
        } else {
            let chunked = Chunks::from(buf.split_to(at).freeze());
            Ok(Some((chunked, eof)))
        }
    }
}

pub struct Iter<'a> {
    b: &'a [u8],
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        let mut is_prev_r = false;
        let mut offset = 0;
        let mut size = -1isize;
        for (i, b) in self.b.iter().enumerate() {
            match b {
                b'\r' => is_prev_r = true,
                b'\n' => {
                    if is_prev_r {
                        let hex = unsafe { std::str::from_utf8_unchecked(&self.b[..i - 1]) };
                        size = isize::from_str_radix(hex, 16).unwrap();
                        offset = i;
                        break;
                    }
                    is_prev_r = false;
                }
                _ => is_prev_r = false,
            }
        }

        if size == -1 {
            None
        } else {
            let size = size as usize;
            let ret = Some(&self.b[offset + 1..offset + 1 + size]);
            self.b = &self.b[offset + size + 3..];
            ret
        }
    }
}

impl AsRef<[u8]> for Chunks {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl From<Bytes> for Chunks {
    fn from(value: Bytes) -> Self {
        Self(value)
    }
}

impl Into<Bytes> for Chunks {
    fn into(self) -> Bytes {
        self.0
    }
}

#[cfg(test)]
mod chunks_tests {
    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[test]
    fn encode_chunked() {
        init();

        let mut b = Chunks::builder();
        b.append(&b"hello,"[..]);
        b.append(&b"chunked!"[..]);

        let chunks = b.build();

        let b: &[u8] = chunks.as_ref();

        assert_eq!(b"6\r\nhello,\r\n8\r\nchunked!\r\n0\r\n\r\n", b);
    }

    #[test]
    fn http_complete_chunked() {
        init();

        let data = b"6\r\nhello,\r\n8\r\nchunked!\r\n0\r\n\r\n";

        let mut b = BytesMut::from(&data[..]);
        let res = Chunks::read(&mut b, usize::MAX);
        assert!(b.is_empty());
        assert!(res.is_ok());
        let res = res.unwrap();
        assert!(res.is_some());

        let (b, eof) = res.unwrap();
        assert!(eof);
        assert_eq!(&data[..], b.as_ref());

        for next in b.iter() {
            info!("next: |{}|", unsafe { std::str::from_utf8_unchecked(next) });
        }
    }

    #[test]
    fn http_partial_chunked() {
        init();
        let mut b = BytesMut::from(&b"6\r\nhello,\r\n8\r\nchunked!\r\nxxx"[..]);
        let res = Chunks::read(&mut b, usize::MAX);
        assert_eq!(3, b.len());
        assert!(res.is_ok());

        let res = res.unwrap();
        assert!(res.is_some());

        let (c, eof) = res.unwrap();
        assert!(!eof, "should has more data");

        for next in c.iter() {
            info!("next: |{}|", unsafe { std::str::from_utf8_unchecked(next) });
        }
    }
}
