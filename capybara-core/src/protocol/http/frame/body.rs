use std::collections::VecDeque;

use bytes::Bytes;
use tokio::io::AsyncWriteExt;

use super::chunked::{Chunks, Iter};

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Empty,
    Raw(Bytes),
    Chunked(Chunks),
    CompositeRaw(VecDeque<Bytes>),
    CompositeChunked(VecDeque<Chunks>),
}

impl Body {
    pub async fn write_to<W>(self, w: &mut W) -> anyhow::Result<()>
    where
        W: AsyncWriteExt + Unpin,
    {
        match self {
            Body::Empty => (),
            Body::Raw(mut raw) => {
                w.write_all_buf(&mut raw).await?;
            }
            Body::Chunked(chunked) => {
                let mut b = chunked.0;
                w.write_all_buf(&mut b).await?;
            }
            Body::CompositeRaw(all) => {
                for mut raw in all {
                    w.write_all_buf(&mut raw).await?;
                }
            }
            Body::CompositeChunked(all) => {
                for next in all {
                    let mut b: Bytes = next.into();
                    w.write_all_buf(&mut b).await?;
                }
            }
        }

        Ok(())
    }

    pub fn payload_iter(&self) -> impl Iterator<Item = &[u8]> {
        BodyPayloadIterator(match self {
            Body::Empty => PayloadIterKind::Empty,
            Body::Raw(it) => PayloadIterKind::Raw(Some(it)),
            Body::Chunked(it) => PayloadIterKind::Chunks(it.iter()),
            Body::CompositeRaw(items) => PayloadIterKind::RawVec(items.iter()),
            Body::CompositeChunked(items) => PayloadIterKind::ChunksVec(items.iter(), None),
        })
    }

    pub fn len(&self) -> usize {
        match self {
            Body::Empty => 0,
            Body::Raw(b) => b.len(),
            Body::Chunked(Chunks(b)) => b.len(),
            Body::CompositeRaw(all) => all.iter().map(|it| it.len()).sum(),
            Body::CompositeChunked(all) => all.iter().map(|it| it.len()).sum(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Body::Empty => true,
            Body::Raw(b) => b.is_empty(),
            Body::Chunked(Chunks(b)) => b.is_empty(),
            Body::CompositeRaw(all) => {
                for next in all.iter() {
                    if !next.is_empty() {
                        return false;
                    }
                }
                true
            }
            Body::CompositeChunked(all) => {
                for next in all.iter() {
                    if !next.is_empty() {
                        return false;
                    }
                }
                true
            }
        }
    }
}

impl std::ops::AddAssign for Body {
    fn add_assign(&mut self, rhs: Self) {
        // TODO: reduce mem copy
        let lhs = std::mem::replace(self, Body::Empty);
        *self = lhs + rhs;
    }
}

impl std::ops::Add for Body {
    type Output = Body;

    fn add(self, rhs: Self) -> Self::Output {
        if self == Body::Empty {
            return rhs;
        }
        if rhs == Body::Empty {
            return self;
        }

        match self {
            Body::Raw(left) => match rhs {
                Body::Raw(right) => Body::CompositeRaw(VecDeque::from([left, right])),
                Body::CompositeRaw(mut rights) => {
                    rights.push_front(left);
                    Body::CompositeRaw(rights)
                }
                _ => unreachable!(),
            },
            Body::Chunked(left) => match rhs {
                Body::Chunked(right) => Body::CompositeChunked(VecDeque::from([left, right])),
                Body::CompositeChunked(mut rights) => {
                    rights.push_front(left);
                    Body::CompositeChunked(rights)
                }
                _ => unreachable!(),
            },
            Body::CompositeRaw(mut left) => match rhs {
                Body::Raw(right) => {
                    left.push_back(right);
                    Body::CompositeRaw(left)
                }
                Body::CompositeRaw(rights) => {
                    let mut merge = VecDeque::with_capacity(left.len() + rights.len());
                    for next in left {
                        merge.push_back(next);
                    }
                    for next in rights {
                        merge.push_back(next);
                    }
                    Body::CompositeRaw(merge)
                }
                _ => unreachable!(),
            },
            Body::CompositeChunked(mut l) => match rhs {
                Body::Chunked(r) => {
                    l.push_back(r);
                    Body::CompositeChunked(l)
                }
                Body::CompositeChunked(r) => {
                    let mut merge = VecDeque::with_capacity(l.len() + r.len());
                    for next in l {
                        merge.push_back(next);
                    }
                    for next in r {
                        merge.push_back(next);
                    }
                    Body::CompositeChunked(merge)
                }
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
    }
}

impl IntoIterator for Body {
    type Item = Bytes;

    type IntoIter = BodyBytesIter;

    fn into_iter(self) -> Self::IntoIter {
        BodyBytesIter(self)
    }
}

pub struct BodyBytesIter(Body);

impl Iterator for BodyBytesIter {
    type Item = Bytes;

    fn next(&mut self) -> Option<Self::Item> {
        let body = std::mem::replace(&mut self.0, Body::Empty);
        match body {
            Body::Empty => None,
            Body::Raw(b) => Some(b),
            Body::Chunked(chunks) => {
                let b: Bytes = chunks.into();
                Some(b)
            }
            Body::CompositeRaw(mut raws) => match raws.pop_front() {
                None => None,
                Some(front) => {
                    match raws.len() {
                        0 => (),
                        1 => self.0 = Body::Raw(raws.pop_front().unwrap()),
                        _ => self.0 = Body::CompositeRaw(raws),
                    }
                    Some(front)
                }
            },
            Body::CompositeChunked(mut all) => match all.pop_front() {
                None => None,
                Some(front) => {
                    match all.len() {
                        0 => (),
                        1 => self.0 = Body::Chunked(all.pop_front().unwrap()),
                        _ => self.0 = Body::CompositeChunked(all),
                    }
                    Some(front.into_bytes())
                }
            },
        }
    }
}

enum PayloadIterKind<'a> {
    Empty,
    Raw(Option<&'a Bytes>),
    RawVec(std::collections::vec_deque::Iter<'a, Bytes>),
    Chunks(Iter<'a>),
    ChunksVec(
        std::collections::vec_deque::Iter<'a, Chunks>,
        Option<Iter<'a>>,
    ),
}

pub struct BodyPayloadIterator<'a>(PayloadIterKind<'a>);

impl<'a> Iterator for BodyPayloadIterator<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.0 {
            PayloadIterKind::Empty => None,
            PayloadIterKind::Raw(it) => it.take().map(|it| &it[..]),
            PayloadIterKind::RawVec(it) => it.next().map(|it| &it[..]),
            PayloadIterKind::Chunks(it) => it.next(),
            PayloadIterKind::ChunksVec(it, front) => loop {
                if let Some(it) = front {
                    if let Some(next) = it.next() {
                        return Some(next);
                    }
                }
                match it.next() {
                    None => return None,
                    Some(chunks) => {
                        front.replace(chunks.iter());
                    }
                }
            },
        }
    }
}

#[cfg(test)]
mod http_body_tests {
    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[test]
    fn test_payload_raw() {
        init();

        let raw = Body::Raw(Bytes::from("foo"));

        assert_eq!(3, raw.len());

        let mut it = raw.payload_iter();
        assert!(it.next().is_some_and(|it| it.eq(b"foo")));
        assert!(it.next().is_none());
    }

    #[test]
    fn test_composite_raw() {
        init();

        let body = Body::CompositeRaw(VecDeque::from([
            Bytes::from("foo"),
            Bytes::from("bar"),
            Bytes::from("qux"),
        ]));

        assert_eq!(9, body.len());

        let mut it = body.payload_iter();
        assert!(it.next().is_some_and(|it| it.eq(b"foo")));
        assert!(it.next().is_some_and(|it| it.eq(b"bar")));
        assert!(it.next().is_some_and(|it| it.eq(b"qux")));
        assert!(it.next().is_none());
    }

    #[test]
    fn test_chunked() {
        init();

        let c = {
            let mut cb = Chunks::builder();
            cb.append(&b"foo"[..]);
            cb.append(&b"bar"[..]);
            cb.append(&b"qux"[..]);
            cb.build()
        };

        let body = Body::Chunked(c);

        assert_eq!(29, body.len());

        let mut it = body.payload_iter();
        assert!(it.next().is_some_and(|it| it.eq(b"foo")));
        assert!(it.next().is_some_and(|it| it.eq(b"bar")));
        assert!(it.next().is_some_and(|it| it.eq(b"qux")));
        assert!(it.next().is_some_and(|it| it.eq(b"")));
        assert!(it.next().is_none());
    }

    #[test]
    fn test_composite_chunked() {
        init();

        let first = {
            let mut cb = Chunks::builder();
            cb.append(&b"foo"[..]);
            cb.append(&b"bar"[..]);
            cb.append(&b"qux"[..]);
            cb.build_incomplete()
        };

        let second = {
            let mut cb = Chunks::builder();
            cb.append(&b"cat"[..]);
            cb.append(&b"dog"[..]);
            cb.append(&b"egg"[..]);
            cb.build_incomplete()
        };

        let third = {
            let mut cb = Chunks::builder();
            cb.append(&b"xxx"[..]);
            cb.append(&b"yyy"[..]);
            cb.append(&b"zzz"[..]);
            cb.build()
        };

        let body = Body::CompositeChunked(VecDeque::from([first, second, third]));

        assert_eq!(24 + 24 + 29, body.len());

        let mut it = body.payload_iter();
        assert!(it.next().is_some_and(|it| it.eq(b"foo")));
        assert!(it.next().is_some_and(|it| it.eq(b"bar")));
        assert!(it.next().is_some_and(|it| it.eq(b"qux")));
        assert!(it.next().is_some_and(|it| it.eq(b"cat")));
        assert!(it.next().is_some_and(|it| it.eq(b"dog")));
        assert!(it.next().is_some_and(|it| it.eq(b"egg")));
        assert!(it.next().is_some_and(|it| it.eq(b"xxx")));
        assert!(it.next().is_some_and(|it| it.eq(b"yyy")));
        assert!(it.next().is_some_and(|it| it.eq(b"zzz")));
        assert!(it.next().is_some_and(|it| it.eq(b"")));
        assert!(it.next().is_none());
    }
}
