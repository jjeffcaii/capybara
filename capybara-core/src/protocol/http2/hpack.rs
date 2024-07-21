use std::fmt;
use std::fmt::Formatter;
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use once_cell::sync::Lazy;
use strum::FromRepr;
use strum_macros::EnumIter;

use crate::cachestr::Cachestr;
use crate::CapybaraError;

static STATIC_TABLE_ENTRIES: Lazy<Vec<Arc<HeaderField>>> = Lazy::new(|| {
    [
        (":authority", ""),
        (":method", "GET"),
        (":method", "POST"),
        (":path", "/"),
        (":path", "/index.html"),
        (":schema", "http"),
        (":schema", "https"),
        (":status", "200"),
        (":status", "204"),
        (":status", "206"),
        (":status", "304"),
        (":status", "400"),
        (":status", "404"),
        (":status", "500"),
        ("accept-charset", ""),
        ("accept-encoding", "gzip, deflate"),
        ("accept-language", ""),
        ("accept-ranges", ""),
        ("accept", ""),
        ("access-control-allow-origin", ""),
        ("age", ""),
        ("allow", ""),
        ("authorization", ""),
        ("cache-control", ""),
        ("content-disposition", ""),
        ("content-encoding", ""),
        ("content-language", ""),
        ("content-length", ""),
        ("content-location", ""),
        ("content-range", ""),
        ("content-type", ""),
        ("cookie", ""),
        ("date", ""),
        ("etag", ""),
        ("expect", ""),
        ("expires", ""),
        ("from", ""),
        ("host", ""),
        ("if-match", ""),
        ("if-modified-since", ""),
        ("if-none-match", ""),
        ("if-range", ""),
        ("if-unmodified-since", ""),
        ("last-modified", ""),
        ("link", ""),
        ("location", ""),
        ("max-forwards", ""),
        ("proxy-authenticate", ""),
        ("proxy-authorization", ""),
        ("range", ""),
        ("referer", ""),
        ("refresh", ""),
        ("retry-after", ""),
        ("server", ""),
        ("set-cookie", ""),
        ("strict-transport-security", ""),
        ("transfer-encoding", ""),
        ("user-agent", ""),
        ("vary", ""),
        ("via", ""),
        ("www-authenticate", ""),
    ]
    .iter()
    .map(|(k, v)| {
        let f = HeaderField {
            name: Cachestr::from(*k),
            value: if v.is_empty() {
                None
            } else {
                Some(Cachestr::from(*v))
            },
        };
        Arc::new(f)
    })
    .collect()
});

#[derive(Clone)]
pub struct Headers(pub(crate) Bytes);

impl Headers {
    pub fn iter(&self) -> impl Iterator<Item = Result<Arc<HeaderField>, CapybaraError>> + '_ {
        HeaderFieldIter { b: &self.0[..] }
    }
}

impl fmt::Debug for Headers {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Headers{{")?;
        let mut iter = self.iter();
        if let Some(first) = iter.next() {
            match first {
                Ok(h) => write!(f, "{}", h)?,
                Err(e) => write!(f, "Err({})", e)?,
            }
        }

        for next in iter {
            match next {
                Ok(h) => write!(f, ",{}", h)?,
                Err(e) => write!(f, ",Err({})", e)?,
            }
        }
        write!(f, "}}")?;
        Ok(())
    }
}

pub enum HStr<'a> {
    Raw(&'a [u8]),
    Huffman(&'a [u8]),
}

impl<'a> Into<Cachestr> for HStr<'a> {
    fn into(self) -> Cachestr {
        match self {
            HStr::Raw(b) => {
                let s = unsafe { std::str::from_utf8_unchecked(b) };
                Cachestr::from(s)
            }
            HStr::Huffman(b) => {
                let b = {
                    let mut bb = BytesMut::new();
                    super::huffman::decode(b, &mut bb).unwrap().freeze()
                };
                let s = unsafe { std::str::from_utf8_unchecked(&b[..]) };
                Cachestr::from(s)
            }
        }
    }
}

pub struct HeaderFieldIter<'a> {
    b: &'a [u8],
}

impl<'a> HeaderFieldIter<'a> {
    fn read_vstr(&mut self) -> Result<HStr<'a>, CapybaraError> {
        let length = (self.b[0] & 0x7f) as usize;
        let vstr = if self.b[0] & 0x80 != 0 {
            // huffman
            HStr::Huffman(&self.b[1..1 + length])
        } else {
            HStr::Raw(&self.b[1..1 + length])
        };

        self.b = &self.b[1 + length..];

        Ok(vstr)
    }
}

impl<'a> Iterator for HeaderFieldIter<'a> {
    type Item = Result<Arc<HeaderField>, CapybaraError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.b.is_empty() {
            return None;
        }

        let first = self.b[0];
        self.b = &self.b[1..];

        if first & 0x80 != 0 {
            // 0b1xxxxxxx
            // Indexed representation.
            // https://httpwg.org/specs/rfc7541.html#rfc.section.6.1
            let cur = first & 0x7f;
            return Some(
                StaticTableIndex::from_repr(cur)
                    .ok_or_else(|| CapybaraError::InvalidHPackIndex(cur))
                    .map(|it| it.as_header_field()),
            );
        }

        if first & 0xc0 == 0x40 {
            // 0b01xxxxxx
            // https://httpwg.org/specs/rfc7541.html#rfc.section.6.2.1
            let v = first & 0x3f;
            return match StaticTableIndex::from_repr(v) {
                None => Some(Err(CapybaraError::InvalidHPackIndex(v))),
                Some(idx) => {
                    let value = self.read_vstr().unwrap();
                    let hf = idx.as_header_field();
                    let f = HeaderField {
                        name: Clone::clone(&hf.name),
                        value: Some(value.into()),
                    };
                    return Some(Ok(f.into()));
                }
            };
        }

        if first & 0xf0 == 0 {
            // 0b0000xxxx
            // 6.2.2 Literal Header Field without Indexing
            // https://httpwg.org/specs/rfc7541.html#rfc.section.6.2.2
            let v = first & 0x0f;
            return match StaticTableIndex::from_repr(v) {
                None => Some(Err(CapybaraError::InvalidHPackIndex(v))),
                Some(idx) => {
                    let hf = idx.as_header_field();
                    let value = self.read_vstr().unwrap();

                    let f = HeaderField {
                        name: Clone::clone(&hf.name),
                        value: Some(value.into()),
                    };

                    Some(Ok(f.into()))
                }
            };
        }

        if first & 0xf0 == 0x10 {
            // 6.2.3 Literal Header Field never Indexed
            // 0b0001xxxx: top four bits are 0001
            // https://httpwg.org/specs/rfc7541.html#rfc.section.6.2.3
            let v = first & 0x0f;
            return match StaticTableIndex::from_repr(v) {
                None => Some(Err(CapybaraError::InvalidHPackIndex(v))),
                Some(idx) => {
                    let hf = idx.as_header_field();
                    let value = self.read_vstr().unwrap();

                    let f = HeaderField {
                        name: Clone::clone(&hf.name),
                        value: Some(value.into()),
                    };

                    Some(Ok(f.into()))
                }
            };
        }

        if first == 0xc0 {
            // 0b01000000
            // Figure 6: Literal Header Field with Incremental Indexing — Indexed Name
            let name = self.read_vstr().unwrap();
            let value = self.read_vstr().unwrap();
            let next = HeaderField {
                name: name.into(),
                value: Some(value.into()),
            };
            return Some(Ok(next.into()));
        }

        warn!("todo: {:x}", first);

        todo!()
    }
}

// https://datatracker.ietf.org/doc/html/rfc7541#appendix-A
#[derive(Copy, Clone, FromRepr, EnumIter, Ord, PartialOrd, Eq, PartialEq, Hash)]
#[repr(u8)]
pub enum StaticTableIndex {
    Authority = 1,
    GET = 2,
    POST = 3,
    PathSlash = 4,
    PathIndexHtml = 5,
    SchemaHttp = 6,
    SchemaHttps = 7,
    Status200 = 8,
    Status204 = 9,
    Status206 = 10,
    Status304 = 11,
    Status400 = 12,
    Status404 = 13,
    Status500 = 14,
    AcceptCharset = 15,
    AcceptEncoding = 16,
    AcceptLanguage = 17,
    AcceptRanges = 18,
    Accept = 19,
    AccessControlAllowOrigin = 20,
    Age = 21,
    Allow = 22,
    Authorization = 23,
    CacheControl = 24,
    ContentDisposition = 25,
    ContentEncoding = 26,
    ContentLanguage = 27,
    ContentLength = 28,
    ContentLocation = 29,
    ContentRange = 30,
    ContentType = 31,
    Cookie = 32,
    Date = 33,
    Etag = 34,
    Expect = 35,
    Expires = 36,
    From = 37,
    Host = 38,
    IfMatch = 39,
    IfModifiedSince = 40,
    IfNoneMatch = 41,
    IfRange = 42,
    IfUnmodifiedSince = 43,
    LastModified = 44,
    Link = 45,
    Location = 46,
    MaxForwards = 47,
    ProxyAuthenticate = 48,
    ProxyAuthorization = 49,
    Range = 50,
    Referer = 51,
    Refresh = 52,
    RetryAfter = 53,
    Server = 54,
    SetCookie = 55,
    StrictTransportSecurity = 56,
    TransferEncoding = 57,
    UserAgent = 58,
    Vary = 59,
    Via = 60,
    WWWAuthenticate = 61,
}

impl StaticTableIndex {
    pub fn as_header_field(&self) -> Arc<HeaderField> {
        let idx = *self as usize;
        Clone::clone(STATIC_TABLE_ENTRIES.get(idx - 1).unwrap())
    }
}

#[derive(Debug)]
pub struct HeaderField {
    name: Cachestr,
    value: Option<Cachestr>,
}

impl fmt::Display for HeaderField {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name.as_ref())?;
        if let Some(value) = &self.value {
            write!(f, "={}", value.as_ref())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use strum::IntoEnumIterator;

    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[test]
    fn test_iter() {
        init();

        for b in [
            "goZBigidXAuBcNx4DwMEhGElQn96iCW2UMPLvLg/UwMqLyo=",
            "iF+SSXyliugZqvtQk47EFTBamVZ7XAIxMmGW0Hq+lAtqZbaFBAE0oEFwDNxtlTFo3w==",
        ] {
            info!("----------------------");
            let headers = Headers(get_bytes(b));
            for next in headers.iter() {
                match next {
                    Ok(next) => info!("next: {}", &next),
                    Err(e) => error!("next: {}", e),
                }
            }
        }
    }

    #[test]
    fn test_static_table_entries() {
        init();
        for next in StaticTableIndex::iter() {
            let idx = next as usize - 1;
            assert!(STATIC_TABLE_ENTRIES.get(idx).is_some_and(|it| {
                info!("#{}: {}", next as usize, it);
                true
            }));
        }
    }

    fn get_bytes(s: &str) -> Bytes {
        use base64::{prelude::BASE64_STANDARD, Engine as _};
        let v = BASE64_STANDARD.decode(s).unwrap();
        Bytes::from(v)
    }
}
