use std::fmt::{self, Formatter};

use bytes::Bytes;
use smallvec::{smallvec, SmallVec};
use strum_macros::{EnumIter, FromRepr};

use super::hpack::Headers;

const FLAG_PADDING: u8 = 0x08;
const FLAG_PADDED: u8 = 0x08;
const FLAG_END_STREAM: u8 = 0x01;
const FLAG_HEADERS_PRIORITY: u8 = 0x20;
const FLAG_HEADERS_END_HEADERS: u8 = 0x04;
const FLAG_SETTINGS_ACK: u8 = 0x01;

#[derive(Debug)]
pub enum Frame {
    Magic,
    Data(Metadata, Bytes),
    Settings(Metadata, Settings),
    Headers(Metadata, Headers),
    Priority(Metadata, Priority),
    RstStream(Metadata, RstStream),
    Ping(Metadata, Ping),
    Goaway(Metadata, Bytes),
    WindowUpdate(Metadata, WindowUpdate),
    Continuation(Metadata, Bytes),
    PushPromise(Metadata, Bytes),
}

#[derive(Clone)]
pub struct Metadata(pub(crate) Bytes);

impl Metadata {
    pub fn length(&self) -> usize {
        ((self.0[0] as usize) << 16) + ((self.0[1] as usize) << 8) + (self.0[2] as usize)
    }

    pub fn kind(&self) -> FrameKind {
        FrameKind::from_repr(self.0[3]).unwrap()
    }

    pub fn flags(&self) -> u8 {
        self.0[4]
    }

    pub fn stream_id(&self) -> u32 {
        read_u32(&self.0[5..])
    }
}

impl fmt::Debug for Metadata {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        use std::io::Write as _;
        let mut b: SmallVec<[u8; 64]> = smallvec![];
        let kind = self.kind();
        let flags = self.flags();

        match &kind {
            FrameKind::Data => {
                if flags & FLAG_PADDED != 0 {
                    write!(&mut b, "|PADDED").ok();
                }
                if flags & FLAG_END_STREAM != 0 {
                    write!(&mut b, "|END_STREAM").ok();
                }
            }
            FrameKind::Headers => {
                if flags & FLAG_HEADERS_PRIORITY != 0 {
                    write!(&mut b, "|PRIORITY").ok();
                }
                if flags & FLAG_PADDED != 0 {
                    write!(&mut b, "|PADDED").ok();
                }
                if flags & FLAG_HEADERS_END_HEADERS != 0 {
                    write!(&mut b, "|END_HEADERS").ok();
                }
                if flags & FLAG_END_STREAM != 0 {
                    write!(&mut b, "|END_STREAM").ok();
                }
            }
            FrameKind::PushPromise => {
                if flags & FLAG_PADDED != 0 {
                    write!(&mut b, "|PADDED").ok();
                }
                if flags & FLAG_HEADERS_END_HEADERS != 0 {
                    write!(&mut b, "|END_HEADERS").ok();
                }
            }
            FrameKind::Settings | FrameKind::Ping => {
                if flags & FLAG_SETTINGS_ACK != 0 {
                    write!(&mut b, "|ACK").ok();
                }
            }
            FrameKind::Continuation => {
                if flags & FLAG_HEADERS_END_HEADERS != 0 {
                    write!(&mut b, "|END_HEADERS").ok();
                }
            }
            _ => (),
        }

        write!(
            f,
            "Metadata({},{},{})",
            kind,
            if b.is_empty() {
                "-"
            } else {
                unsafe { std::str::from_utf8_unchecked(&b[1..]) }
            },
            self.stream_id(),
        )?;
        Ok(())
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, EnumIter, FromRepr)]
#[repr(u8)]
pub enum FrameKind {
    Data = 0x00,
    Headers = 0x01,
    Priority = 0x02,
    RstStream = 0x03,
    Settings = 0x04,
    PushPromise = 0x05,
    Ping = 0x06,
    Goaway = 0x07,
    WindowUpdate = 0x08,
    Continuation = 0x09,
}

impl fmt::Display for FrameKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            FrameKind::Data => write!(f, "DATA"),
            FrameKind::Headers => write!(f, "HEADERS"),
            FrameKind::Priority => write!(f, "PRIORITY"),
            FrameKind::RstStream => write!(f, "RST_STREAM"),
            FrameKind::Settings => write!(f, "SETTINGS"),
            FrameKind::PushPromise => write!(f, "PUSH_PROMISE"),
            FrameKind::Ping => write!(f, "PING"),
            FrameKind::Goaway => write!(f, "GOAWAY"),
            FrameKind::WindowUpdate => write!(f, "WINDOW_UPDATE"),
            FrameKind::Continuation => write!(f, "CONTINUATION"),
        }
    }
}

#[derive(Clone)]
pub struct Priority(pub(crate) Bytes);

impl Priority {
    pub fn exclusive(&self) -> bool {
        self.0[0] & 0x80 != 0
    }

    pub fn stream_dependency(&self) -> u32 {
        read_u32(&self.0[..])
    }

    pub fn weight(&self) -> u8 {
        self.0[4]
    }
}

impl fmt::Debug for Priority {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Priority({},{},{})",
            self.exclusive(),
            self.stream_dependency(),
            self.weight()
        )
    }
}

#[derive(Clone)]
pub struct RstStream(pub(crate) Bytes);

impl RstStream {
    pub fn error_code(&self) -> u32 {
        read_u32(&self.0[..])
    }
}

impl fmt::Debug for RstStream {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "RstStream({})", self.error_code())
    }
}

#[derive(Clone)]
pub struct Settings(pub(crate) Bytes);

impl Settings {
    pub fn iter(&self) -> impl Iterator<Item = (Identifier, u32)> + '_ {
        SettingIter(&self.0[..])
    }
}

impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut iter = self.iter();

        write!(f, "Settings[")?;

        if let Some((k, v)) = iter.next() {
            write!(f, "{:?}={}", k, v)?;
            for (k, v) in iter {
                write!(f, ",{:?}={}", k, v)?;
            }
        }

        write!(f, "]")?;

        Ok(())
    }
}

struct SettingIter<'a>(&'a [u8]);

impl<'a> Iterator for SettingIter<'a> {
    type Item = (Identifier, u32);

    fn next(&mut self) -> Option<Self::Item> {
        if self.0.len() < 6 {
            return None;
        }
        let k = match ((self.0[0] as u16) << 8) + (self.0[1] as u16) {
            0x01 => Identifier::SettingsHeaderTableSize,
            0x02 => Identifier::SettingsEnablePush,
            0x03 => Identifier::SettingsMaxConcurrentStreams,
            0x04 => Identifier::SettingsInitialWindowSize,
            0x05 => Identifier::SettingsMaxFrameSize,
            0x06 => Identifier::SettingsMaxHeaderListSize,
            other => Identifier::Unknown(other),
        };

        let v = read_u32(&self.0[2..]);

        self.0 = &self.0[6..];

        Some((k, v))
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, EnumIter)]
pub enum Identifier {
    SettingsHeaderTableSize,      // 0x01,
    SettingsEnablePush,           // 0x02,
    SettingsMaxConcurrentStreams, // 0x03,
    SettingsInitialWindowSize,    // 0x04,
    SettingsMaxFrameSize,         // 0x05,
    SettingsMaxHeaderListSize,    // 0x06,
    Unknown(u16),
}

#[derive(Clone)]
pub struct WindowUpdate(pub(crate) Bytes);

impl WindowUpdate {
    pub fn window_size_increment(&self) -> u32 {
        read_u32(&self.0[..])
    }
}

impl fmt::Debug for WindowUpdate {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "WindowUpdate({})", self.window_size_increment())
    }
}

#[derive(Clone)]
pub struct Ping(pub(crate) Bytes);

impl Ping {
    pub fn opaque_data(&self) -> u64 {
        read_u64(&self.0[..])
    }
}

impl fmt::Debug for Ping {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Ping(0x{:08x})", self.opaque_data())
    }
}

#[inline]
fn read_u32(b: &[u8]) -> u32 {
    assert!(b.len() >= 4);
    ((b[0] as u32) << 24) + ((b[1] as u32) << 16) + ((b[2] as u32) << 8) + (b[3] as u32)
}

#[inline]
fn read_u64(b: &[u8]) -> u64 {
    assert!(b.len() >= 8);
    ((b[0] as u64) << 56)
        + ((b[1] as u64) << 48)
        + ((b[2] as u64) << 40)
        + ((b[3] as u64) << 32)
        + ((b[4] as u64) << 24)
        + ((b[5] as u64) << 16)
        + ((b[6] as u64) << 8)
        + (b[7] as u64)
}

#[cfg(test)]
mod tests {
    use bytes::{Bytes, BytesMut};

    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[test]
    fn test_decode_settings() {
        init();
        //  SETTINGS
        let b = hex2bytes("000012040000000000000300000064000400a00000000200000000").split_to(9);
        let f = Metadata(b);
        assert_eq!(18, f.length());
        assert_eq!(FrameKind::Settings, f.kind());
        assert_eq!(0, f.stream_id());
    }

    #[test]
    fn test_decode_headers() {
        init();
        let b = hex2bytes("0000230105000000018286418a089d5c0b8170dc780f0304846125427f7a8825b650c3cbbcb83f53032a2f2a").split_to(9);
        let f = Metadata(b);
        assert_eq!(35, f.length());
        assert_eq!(FrameKind::Headers, f.kind());
        assert_eq!(1, f.stream_id());
    }

    fn hex2bytes(s: &str) -> Bytes {
        let v = hex::decode(s).unwrap();
        BytesMut::from(&v[..]).freeze()
    }
}
