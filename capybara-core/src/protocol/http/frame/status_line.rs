use std::borrow::Cow;

use bytes::{Bytes, BytesMut};

use crate::protocol::http::misc;
use crate::Result;

#[derive(Debug, Clone, PartialEq)]
pub struct StatusLine(pub(crate) Bytes);

impl StatusLine {
    #[inline]
    pub fn read(buf: &mut BytesMut) -> Result<Option<StatusLine>> {
        Ok(misc::readline(buf).map(StatusLine))
    }

    pub fn status_code(&self) -> u16 {
        let mut code = 0;

        for (i, v) in self.0[..].split(misc::is_space).enumerate() {
            if i == 1 {
                code = std::str::from_utf8(v).unwrap().parse::<u16>().unwrap();
                break;
            }
        }
        code
    }

    pub fn reason_phrase(&self) -> Cow<str> {
        let pos = self.0.iter().rposition(|b| *b == b' ').unwrap();
        String::from_utf8_lossy(&self.0[pos + 1..self.0.len() - 2])
    }

    pub fn version(&self) -> Cow<str> {
        self.0
            .split(misc::is_space)
            .next()
            .map(String::from_utf8_lossy)
            .unwrap()
    }
}

impl Into<Bytes> for StatusLine {
    fn into(self) -> Bytes {
        self.0
    }
}

#[cfg(test)]
mod status_line_tests {
    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[test]
    fn test_http_status_line() {
        init();

        let status_line = b"HTTP/1.1 200 OK\r\nHos";

        let mut b = BytesMut::from(&status_line[..]);
        let status_line = StatusLine::read(&mut b);
        assert_eq!(3, b.len());
        assert!(
            status_line.as_ref().is_ok_and(|it| it.is_some()),
            "should parse status line!"
        );

        let l = status_line.unwrap().unwrap();
        assert_eq!("HTTP/1.1", l.version());
        assert_eq!(200, l.status_code());
        assert_eq!("OK", l.reason_phrase());
    }
}
