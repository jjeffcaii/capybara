use std::result::Result as StdResult;

use bitflags::bitflags;
use bytes::BytesMut;
use tokio_util::codec::Decoder;

use crate::error::CapybaraError::ExceedMaxHttpBodySize;
use crate::protocol::http::httpfield::HttpField;

use super::frame::{Body, Chunks, Headers, HttpFrame, RequestLine, StatusLine};

const INFINITY: isize = isize::MAX;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Flags(u32);

bitflags! {
    impl Flags: u32 {
        const NO_CONTENT = 1 << 0;
        const RESPONSE = 1 << 1;
        const FUSE_BODY = 1 << 2;
        const LOWER_CASE_HEADER = 1 << 3;
    }
}

#[derive(Debug)]
enum State {
    Ready,
    HeadlineFinish,
    HeadersFinish,
}

#[derive(Debug)]
pub struct HttpCodec {
    flags: Flags,
    body_size: isize,
    state: State,
    body: Body,
    read_size: usize,
    max_header_size: Option<usize>,
    max_body_size: Option<usize>,
}

impl HttpCodec {
    pub fn new(
        flags: Flags,
        max_header_size: Option<usize>,
        max_body_size: Option<usize>,
    ) -> HttpCodec {
        HttpCodec {
            flags,
            state: State::Ready,
            body_size: -1,
            body: Body::Empty,
            read_size: 0,
            max_header_size,
            max_body_size,
        }
    }

    #[inline(always)]
    fn reset(&mut self) {
        self.state = State::Ready;
        self.body_size = -1;
        self.body = Body::Empty;
        self.read_size = 0;
    }

    #[inline]
    fn pickup_size(&mut self, headers: &Headers) -> anyhow::Result<()> {
        if self.flags.contains(Flags::NO_CONTENT) || self.body_size != -1 {
            return Ok(());
        }

        if let Some(v) = headers.get_by_field(HttpField::TransferEncoding) {
            if v.eq_ignore_ascii_case(b"chunked") {
                self.body_size = INFINITY;
                return Ok(());
            }
        }

        if let Some(v) = headers.get_by_field(HttpField::ContentLength) {
            let s = unsafe { std::str::from_utf8_unchecked(v) };
            let size: isize = s.parse()?;

            let max_body_size = self.remaining_max_body_size();
            if size as usize > max_body_size {
                bail!(ExceedMaxHttpBodySize(max_body_size));
            }

            self.body_size = size;
        }

        Ok(())
    }

    #[inline(always)]
    fn remaining_max_header_size(&self) -> usize {
        self.max_header_size
            .map(|n| n - self.read_size)
            .unwrap_or(usize::MAX)
    }

    #[inline(always)]
    fn remaining_max_body_size(&self) -> usize {
        self.max_body_size
            .map(|n| n - self.read_size)
            .unwrap_or(usize::MAX)
    }
}

impl Decoder for HttpCodec {
    type Item = HttpFrame;

    type Error = anyhow::Error;

    fn decode(&mut self, src: &mut BytesMut) -> StdResult<Option<Self::Item>, Self::Error> {
        let result = match self.state {
            State::Ready => {
                if self.flags.contains(Flags::RESPONSE) {
                    StatusLine::read(src)?.map(|status_line| {
                        // 204 NO CONTENT
                        if status_line.status_code() == 204 {
                            self.body_size = 0;
                        }
                        self.state = State::HeadlineFinish;
                        HttpFrame::StatusLine(status_line)
                    })
                } else {
                    RequestLine::read(src, self.remaining_max_header_size())?.map(|request_line| {
                        self.state = State::HeadlineFinish;

                        self.read_size += request_line.len();

                        HttpFrame::RequestLine(request_line)
                    })
                }
            }
            State::HeadlineFinish => {
                let max_header_size = self.remaining_max_header_size();
                let lowercase = self.flags.contains(Flags::LOWER_CASE_HEADER);
                match Headers::read_ext(src, max_header_size, lowercase)? {
                    Some(headers) => {
                        // reset the read_size for incoming body size computation.
                        self.read_size = 0usize;

                        self.pickup_size(&headers)?;
                        self.state = State::HeadersFinish;
                        Some(HttpFrame::Headers(headers))
                    }
                    None => None,
                }
            }
            State::HeadersFinish => {
                if self.body_size < 1 {
                    self.reset();

                    Some(HttpFrame::CompleteBody(Body::Empty))
                } else if self.body_size == INFINITY {
                    match Chunks::read(src, self.remaining_max_body_size())? {
                        None => None,
                        Some((chunks, eof)) => {
                            if eof {
                                let existing = std::mem::replace(&mut self.body, Body::Empty);
                                self.reset();

                                match existing {
                                    Body::Empty => {
                                        Some(HttpFrame::CompleteBody(Body::Chunked(chunks)))
                                    }
                                    other => {
                                        Some(HttpFrame::CompleteBody(other + Body::Chunked(chunks)))
                                    }
                                }
                            } else {
                                self.read_size += chunks.len();

                                let body = Body::Chunked(chunks);

                                if self.flags.contains(Flags::FUSE_BODY) {
                                    self.body += body;
                                    None
                                } else {
                                    Some(HttpFrame::PartialBody(body))
                                }
                            }
                        }
                    }
                } else {
                    let actual = src.len();
                    let expect = self.body_size as usize;
                    if actual >= expect {
                        let existing = std::mem::replace(&mut self.body, Body::Empty);

                        self.reset();

                        let b = src.split_to(expect).freeze();

                        match existing {
                            Body::Empty => Some(HttpFrame::CompleteBody(Body::Raw(b))),
                            other => Some(HttpFrame::CompleteBody(other + Body::Raw(b))),
                        }
                    } else if actual == 0 {
                        None
                    } else {
                        self.body_size = (expect - actual) as isize;
                        let body = Body::Raw(src.split().freeze());

                        if self.flags.contains(Flags::FUSE_BODY) {
                            self.body += body;
                            None
                        } else {
                            Some(HttpFrame::PartialBody(body))
                        }
                    }
                }
            }
        };
        Ok(result)
    }
}
