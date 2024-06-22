use bytes::{Bytes, BytesMut};

pub(crate) const CRLF: &[u8] = b"\r\n";

#[inline(always)]
pub(super) fn readline(buf: &mut BytesMut) -> Option<Bytes> {
    let mut br = false;
    for (i, b) in buf.iter().enumerate() {
        match b {
            b'\r' => br = true,
            b'\n' => {
                if br {
                    return Some(buf.split_to(i + 1).freeze());
                }
            }
            _ => br = false,
        }
    }

    None
}

#[inline(always)]
pub(super) fn is_space(b: &u8) -> bool {
    *b == b' '
}

#[inline(always)]
pub(super) fn is_ascii_space(b: u8) -> bool {
    // '\t', '\n', '\v', '\f', '\r', ' ', 0x85, 0xA0,
    matches!(b, b'\t' | b'\n' | 0x0b | 0x0c | b'\r' | b' ' | 0x85 | 0xA0)
}

#[inline(always)]
pub(crate) fn hash16(input: &[u8]) -> u16 {
    let mut hash = 0u16;
    // TODO: should we convert '-' to '_'?
    for next in input {
        hash = (hash << 5).wrapping_sub(hash);
        if *next >= b'A' && *next <= b'Z' {
            hash = hash.wrapping_add((*next | 0x20) as u16);
        } else {
            hash = hash.wrapping_add(*next as u16);
        }
    }
    hash
}
