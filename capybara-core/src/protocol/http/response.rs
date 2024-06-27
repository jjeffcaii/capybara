use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use bitflags::bitflags;
use bytes::{BufMut, Bytes, BytesMut};
use once_cell::sync::Lazy;
use smallvec::SmallVec;
use tokio::io::AsyncWriteExt;

use crate::Result;

use super::frame::{HeadersBuilder, StatusLine};
use super::httpfield::HttpField;
use super::misc;

type DateBytes = SmallVec<[u8; 32]>;

static DATE: Lazy<ArcSwap<DateBytes>> = Lazy::new(|| ArcSwap::new(Arc::new(DateBytes::new())));
static DATE_TICKER_STARTED: Lazy<AtomicBool> = Lazy::new(AtomicBool::default);

async fn start_date_ticker() {
    use tokio::time::{sleep, Duration};
    // start the timer
    info!("start interval of http date generator ok");
    loop {
        // 1. compute the next truncated second, as the next launch time
        let sleep_nanos = 1_000_000_000i64 - (chrono::Utc::now().timestamp_subsec_nanos() as i64);
        if sleep_nanos > 0 {
            sleep(Duration::from_nanos(sleep_nanos as u64)).await;
            // 2. refresh date str every second
            DATE.store(Arc::new(generate_date_bytes()));
        } else {
            // sleep 1ms at least
            sleep(Duration::from_millis(1)).await;
        }
    }
}

#[inline(always)]
fn generate_date_bytes() -> DateBytes {
    // Sun, 08 Oct 2023 08:49:35 GMT
    let mut b = DateBytes::new();
    {
        use std::io::Write as _;
        write!(
            &mut b,
            "{}",
            chrono::Utc::now().format("%a, %d %b %Y %T GMT")
        )
        .ok();
    }
    b
}

#[inline]
async fn write_header_date<W>(w: &mut W, lowercase: bool) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    if lowercase {
        w.write_all(HttpField::Date.as_str().to_ascii_lowercase().as_bytes())
            .await?;
    } else {
        w.write_all(HttpField::Date.as_bytes()).await?;
    }

    w.write_all(b": ").await?;

    loop {
        {
            let loaded = DATE.load();
            if !loaded.is_empty() {
                w.write_all(&loaded[..]).await?;
                break;
            }
        }

        if let Ok(origin) =
            DATE_TICKER_STARTED.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        {
            if !origin {
                // generate when starting
                DATE.store(Arc::new(generate_date_bytes()));

                tokio::spawn(async {
                    start_date_ticker().await;
                });
            }
        }
    }

    w.write_all(misc::CRLF).await?;
    Ok(())
}

#[derive(Debug, Copy, Clone, Default, Hash, PartialEq, Eq)]
pub struct ResponseFlags(u8);

bitflags! {
    impl ResponseFlags: u8 {
        const LOWERCASE_DATE_HEADER = 1 << 0;
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status_line: StatusLine,
    pub headers: Bytes,
    pub body: Option<Bytes>,
}

impl Response {
    pub fn builder() -> ResponseBuilder {
        Default::default()
    }

    pub async fn write_to<W>(&self, w: &mut W, flags: ResponseFlags) -> Result<()>
    where
        W: AsyncWriteExt + Unpin,
    {
        w.write_all(&self.status_line.0[..]).await?;

        write_header_date(w, flags.contains(ResponseFlags::LOWERCASE_DATE_HEADER)).await?;

        w.write_all(&self.headers[..]).await?;
        if let Some(body) = &self.body {
            w.write_all(&body[..]).await?;
        }
        Ok(())
    }
}

#[derive(Debug, Copy, Clone, Default, Hash, PartialEq, Eq)]
struct ResponseBuilderFlags(u16);

bitflags! {
    impl ResponseBuilderFlags: u16 {
        const FLAG_DNT_SERVER = 1 << 0;
        const FLAG_NO_HTTP11 = 1 << 1;
        const FLAG_NO_KEEPALIVE = 1 << 2;
        const FLAG_LOWERCASE_HEADER = 1 << 3;
    }
}

pub struct ResponseBuilder {
    headers: HeadersBuilder,
    body_buf: BytesMut,
    code: u16,
    flag: ResponseBuilderFlags,
}

impl Default for ResponseBuilder {
    fn default() -> Self {
        ResponseBuilder {
            code: 200,
            headers: Default::default(),
            body_buf: Default::default(),
            flag: ResponseBuilderFlags::default(),
        }
    }
}

impl ResponseBuilder {
    pub fn status_code(mut self, status_code: u16) -> Self {
        self.code = status_code;
        self
    }

    pub fn use_lowercase_header(mut self) -> Self {
        self.flag |= ResponseBuilderFlags::FLAG_LOWERCASE_HEADER;
        self
    }

    pub fn content_type<T>(mut self, typ: T) -> Self
    where
        T: AsRef<str>,
    {
        if self
            .flag
            .contains(ResponseBuilderFlags::FLAG_LOWERCASE_HEADER)
        {
            self.headers = self.headers.put(
                HttpField::ContentType.as_str().to_ascii_lowercase(),
                typ.as_ref(),
            );
        } else {
            self.headers = self
                .headers
                .put(HttpField::ContentType.as_str(), typ.as_ref());
        }

        self
    }

    pub fn header<K, V>(mut self, key: K, value: V) -> Self
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        if self
            .flag
            .contains(ResponseBuilderFlags::FLAG_LOWERCASE_HEADER)
        {
            self.headers = self.headers.put(key.as_ref().to_ascii_lowercase(), value);
        } else {
            self.headers = self.headers.put(key, value);
        }

        self
    }

    pub fn body<B>(mut self, value: B) -> Self
    where
        B: AsRef<[u8]>,
    {
        let value = value.as_ref();
        self.body_buf.put(value);
        self
    }

    pub fn disable_server(mut self) -> Self {
        self.flag |= ResponseBuilderFlags::FLAG_DNT_SERVER;
        self
    }

    pub fn build(mut self) -> Response {
        let lowercase_headers = self
            .flag
            .contains(ResponseBuilderFlags::FLAG_LOWERCASE_HEADER);

        let size = self.body_buf.len();

        if lowercase_headers {
            self.headers = self.headers.put(
                HttpField::ContentLength.as_str().to_ascii_lowercase(),
                size.to_string(),
            );
        } else {
            self.headers = self
                .headers
                .put(HttpField::ContentLength.as_str(), size.to_string());
        }

        // set 'Server: leap/x.y.z' if custom server header is enabled.
        if !self.flag.contains(ResponseBuilderFlags::FLAG_DNT_SERVER) {
            if lowercase_headers {
                self.headers = self.headers.put(
                    HttpField::Server.as_str().to_ascii_lowercase(),
                    misc::SERVER.as_str(),
                );
            } else {
                self.headers = self
                    .headers
                    .put(HttpField::Server.as_str(), misc::SERVER.as_str());
            }
        }

        // set 'Connection: close' if keepalive is disabled.
        if self.flag.contains(ResponseBuilderFlags::FLAG_NO_KEEPALIVE) {
            if lowercase_headers {
                self.headers = self
                    .headers
                    .put(HttpField::Connection.as_str().to_ascii_lowercase(), "close");
            } else {
                self.headers = self.headers.put(HttpField::Connection.as_str(), "close");
            }
        }

        self.headers = self.headers.complete();

        let Self {
            code,
            headers,
            body_buf: body_,
            ..
        } = self;

        let mut status_line = BytesMut::with_capacity(32);

        if self.flag.contains(ResponseBuilderFlags::FLAG_NO_HTTP11) {
            status_line.put(&b"HTTP/1.0 "[..]);
        } else {
            status_line.put(&b"HTTP/1.1 "[..]);
        }

        // https://developer.mozilla.org/en-US/docs/Web/HTTP/Status
        match code {
            100 => status_line.put(&b"100 Continue"[..]),
            101 => status_line.put(&b"101 Switching Protocols"[..]),
            102 => status_line.put(&b"102 Processing"[..]),
            103 => status_line.put(&b"103 Early Hints"[..]),
            200 => status_line.put(&b"200 OK"[..]),
            201 => status_line.put(&b"201 Created"[..]),
            202 => status_line.put(&b"202 Accepted"[..]),
            203 => status_line.put(&b"203 Non-Authoritative Information"[..]),
            204 => status_line.put(&b"204 No Content"[..]),
            205 => status_line.put(&b"205 Reset Content"[..]),
            206 => status_line.put(&b"206 Partial Content"[..]),
            207 => status_line.put(&b"207 Multi-Status"[..]),
            208 => status_line.put(&b"208 Already Reported"[..]),
            226 => status_line.put(&b"226 IM Used"[..]),
            300 => status_line.put(&b"300 Multiple Choices"[..]),
            301 => status_line.put(&b"301 Moved Permanently"[..]),
            302 => status_line.put(&b"302 Found"[..]),
            303 => status_line.put(&b"303 See Other"[..]),
            304 => status_line.put(&b"304 Not Modified"[..]),
            307 => status_line.put(&b"307 Temporary Redirect"[..]),
            308 => status_line.put(&b"308 Permanent Redirect"[..]),
            400 => status_line.put(&b"400 Bad Request"[..]),
            401 => status_line.put(&b"401 Unauthorized"[..]),
            402 => status_line.put(&b"402 Payment Required"[..]),
            403 => status_line.put(&b"403 Forbidden"[..]),
            404 => status_line.put(&b"404 Not Found"[..]),
            405 => status_line.put(&b"405 Method Not Allowed"[..]),
            406 => status_line.put(&b"406 Not Acceptable"[..]),
            407 => status_line.put(&b"407 Proxy Authentication Required"[..]),
            408 => status_line.put(&b"408 Request Timeout"[..]),
            409 => status_line.put(&b"409 Conflict"[..]),
            410 => status_line.put(&b"410 Gone"[..]),
            411 => status_line.put(&b"411 Length Required"[..]),
            412 => status_line.put(&b"412 Precondition Failed"[..]),
            413 => status_line.put(&b"413 Content Too Large"[..]),
            414 => status_line.put(&b"414 URI Too Long"[..]),
            415 => status_line.put(&b"415 Unsupported Media Type"[..]),
            416 => status_line.put(&b"416 Range Not Satisfiable"[..]),
            417 => status_line.put(&b"417 Expectation Failed"[..]),
            418 => status_line.put(&b"418 I'm a teapot"[..]),
            421 => status_line.put(&b"421 Misdirected Request"[..]),
            422 => status_line.put(&b"422 Unprocessable Content"[..]),
            423 => status_line.put(&b"423 Locked"[..]),
            424 => status_line.put(&b"424 Failed Dependency"[..]),
            425 => status_line.put(&b"425 Too Early"[..]),
            426 => status_line.put(&b"426 Upgrade Required"[..]),
            428 => status_line.put(&b"428 Precondition Required"[..]),
            429 => status_line.put(&b"429 Too Many Requests"[..]),
            431 => status_line.put(&b"431 Request Header Fields Too Large"[..]),
            451 => status_line.put(&b"451 Unavailable For Legal Reasons"[..]),
            500 => status_line.put(&b"500 Internal Server Error"[..]),
            501 => status_line.put(&b"501 Not Implemented"[..]),
            502 => status_line.put(&b"502 Bad Gateway"[..]),
            503 => status_line.put(&b"503 Service Unavailable"[..]),
            504 => status_line.put(&b"504 Gateway Timeout"[..]),
            505 => status_line.put(&b"505 HTTP Version Not Supported"[..]),
            506 => status_line.put(&b"506 Variant Also Negotiates"[..]),
            507 => status_line.put(&b"507 Insufficient Storage"[..]),
            508 => status_line.put(&b"508 Loop Detected"[..]),
            510 => status_line.put(&b"510 Not Extended"[..]),
            511 => status_line.put(&b"511 Network Authentication Required"[..]),

            other => {
                use std::fmt::Write as _;
                write!(&mut status_line, "{} UNKNOWN", other).unwrap();
            }
        };

        status_line.put(misc::CRLF);

        let headers = headers.build();
        let body = body_.freeze();

        Response {
            status_line: StatusLine(status_line.freeze()),
            headers: headers.into(),
            body: if body.is_empty() { None } else { Some(body) },
        }
    }
}

#[cfg(test)]
mod response_tests {
    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[test]
    fn response_builder() {
        init();

        let resp = Response::builder()
            .header("X-Ray-Id", "foobar")
            .header("Content-Type", "text/plain")
            .body(b"hello world")
            .build();

        let mut b = BytesMut::new();

        b.put_slice(&resp.status_line.0);
        b.put_slice(&resp.headers);
        if let Some(body) = &resp.body {
            b.put_slice(body);
        }

        let b = b.freeze();

        info!("response: {:?}", b);
    }
}
