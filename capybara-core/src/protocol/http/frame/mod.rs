pub use body::Body;
pub use chunked::Chunks;
pub use header::{Headers, HeadersBuilder};
pub use query::{Queries, Query};
pub use request_line::RequestLine;
pub use status_line::StatusLine;

mod body;
mod chunked;
mod header;
mod query;
mod request_line;
mod status_line;

#[derive(Debug)]
pub enum HttpFrame {
    RequestLine(RequestLine),
    StatusLine(StatusLine),
    Headers(Headers),
    CompleteBody(Body),
    PartialBody(Body),
}

impl HttpFrame {
    #[inline]
    pub fn is_complete(&self) -> bool {
        matches!(self, Self::CompleteBody(_))
    }
}
