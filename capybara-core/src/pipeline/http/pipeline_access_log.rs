use crate::pipeline::{HttpContext, HttpPipeline, HttpPipelineFactory, PipelineConf};
use crate::protocol::http::{Headers, HttpField, RequestLine, StatusLine};
use crate::CapybaraError;
use async_trait::async_trait;
use bytes::{BufMut, BytesMut};
use capybara_util::cachestr::Cachestr;
use chrono::{DateTime, Local};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use regex::Regex;
use std::borrow::Cow;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

const BLANK: u8 = b'-';

#[derive(Debug, Clone)]
enum LogPart {
    Connection,
    ConnectionRequests,
    Request,
    RemoteAddr,
    RemoteUser,
    TimeLocal,
    RequestLength,
    BodyBytesSent,
    Status,
    RequestMethod,
    RequestPath,
    RequestURI,
    Host,
    HttpReferer,
    HttpUserAgent,
    RequestTime,
    XForwardedFor,
    UpstreamConnectTime,
    UpstreamHeaderTime,
    UpstreamResponseTime,
    GzipRatio,
    Space,
    Tab,
    Anything(Cachestr),
    HttpHeader(Cachestr),
}

pub(crate) struct LogContext {
    connection: u64,
    connection_requests: u64,
    begin: DateTime<Local>,
    end: DateTime<Local>,
    remote_addr: Option<SocketAddr>,
    body_bytes_recv: usize,
    body_bytes_sent: usize,
    status_code: u16,
    request_line: RequestLine,
    request_headers: Headers,

    gzip_ratio: Option<u16>,

    /// The time spent on establishing a connection with an upstream server
    upstream_connect_time: Option<Duration>,
    /// The time between establishing a connection and receiving the first byte of the response header from the upstream server
    upstream_header_time: Option<Duration>,
    /// The time between establishing a connection and receiving the last byte of the response body from the upstream server
    upstream_response_time: Option<Duration>,
}

#[derive(Clone)]
pub(crate) struct LogTemplate(Vec<LogPart>);

impl LogTemplate {
    pub(crate) fn write<W>(&self, w: &mut W, item: &LogContext)
    where
        W: BufMut + std::fmt::Write,
    {
        for next in self.0.iter() {
            match next {
                LogPart::RequestPath => w.put_slice(item.request_line.path_bytes()),
                LogPart::RequestMethod => w.put_slice(item.request_line.method().as_bytes()),
                LogPart::RequestURI => w.put_slice(item.request_line.uri()),
                LogPart::Request => {
                    let b = &item.request_line.b;
                    w.put_slice(&b[..b.len() - 2]);
                }
                LogPart::RemoteAddr => match &item.remote_addr {
                    None => w.put_u8(BLANK),
                    Some(addr) => {
                        let _ = write!(w, "{}", addr.ip());
                    }
                },
                LogPart::RemoteUser => match item
                    .request_headers
                    .get_by_field(HttpField::Authorization)
                {
                    None => w.put_u8(BLANK),
                    Some(user) => {
                        use base64::{engine::general_purpose, Engine as _};
                        const PREFIX: &str = "Basic ";
                        let mut found = false;
                        if user.starts_with(PREFIX.as_bytes()) {
                            if let Ok(v) = general_purpose::STANDARD.decode(&user[PREFIX.len()..]) {
                                if let Some(i) = memchr::memmem::find(&v[..], b":") {
                                    w.put_slice(&v[..i]);
                                    found = true;
                                }
                            }
                        }
                        if !found {
                            w.put_u8(BLANK);
                        }
                    }
                },
                LogPart::TimeLocal => {
                    let _ = write!(w, "{}", item.begin.format("%Y-%m-%dT%H:%M:%S.%3f%z"));
                }
                LogPart::BodyBytesSent => {
                    let _ = write!(w, "{}", item.body_bytes_sent);
                }
                LogPart::Status => {
                    let _ = write!(w, "{}", item.status_code);
                }
                LogPart::Host => match &item.request_headers.get_by_field(HttpField::Host) {
                    None => w.put_u8(BLANK),
                    Some(b) => w.put_slice(b),
                },
                LogPart::HttpReferer => {
                    match &item.request_headers.get_by_field(HttpField::Referer) {
                        None => w.put_u8(BLANK),
                        Some(b) => w.put_slice(b),
                    }
                }
                LogPart::HttpUserAgent => {
                    match item.request_headers.get_by_field(HttpField::UserAgent) {
                        None => w.put_u8(BLANK),
                        Some(b) => w.put_slice(b),
                    }
                }
                LogPart::RequestTime => {
                    let elapsed_millis = item
                        .end
                        .signed_duration_since(item.begin)
                        .num_milliseconds();
                    let _ = write!(w, "{}.{:03}", elapsed_millis / 1000, elapsed_millis % 1000);
                }
                LogPart::UpstreamConnectTime => match item.upstream_connect_time {
                    None => w.put_u8(BLANK),
                    Some(elapsed) => {
                        let _ = write!(w, "{:.3}", elapsed.as_secs_f32());
                    }
                },
                LogPart::UpstreamHeaderTime => match item.upstream_header_time {
                    None => w.put_u8(BLANK),
                    Some(elapsed) => {
                        let _ = write!(w, "{:.3}", elapsed.as_secs_f32());
                    }
                },
                LogPart::UpstreamResponseTime => match item.upstream_response_time {
                    None => w.put_u8(BLANK),
                    Some(elapsed) => {
                        let _ = write!(w, "{:.3}", elapsed.as_secs_f32());
                    }
                },
                LogPart::Anything(anything) => w.put_slice(anything.as_bytes()),
                LogPart::GzipRatio => match &item.gzip_ratio {
                    Some(r) => {
                        let _ = write!(w, "{}", r);
                    }
                    None => w.put_u8(BLANK),
                },
                LogPart::Space => w.put_u8(b' '),
                LogPart::Tab => w.put_u8(b'\t'),
                LogPart::XForwardedFor => {
                    match item.request_headers.get_by_field(HttpField::XForwardedFor) {
                        None => w.put_u8(BLANK),
                        Some(b) => w.put_slice(b),
                    }
                }
                LogPart::HttpHeader(k) => match item.request_headers.get_bytes(k.as_ref()) {
                    None => w.put_u8(BLANK),
                    Some(b) => w.put_slice(b),
                },
                LogPart::RequestLength => {
                    let request_length =
                        item.request_line.len() + item.request_headers.len() + item.body_bytes_recv;
                    let _ = write!(w, "{}", request_length);
                }
                LogPart::Connection => {
                    let _ = write!(w, "{}", item.connection);
                }
                LogPart::ConnectionRequests => {
                    let _ = write!(w, "{}", item.connection_requests);
                }
            }
        }
    }
}

impl Default for LogTemplate {
    fn default() -> Self {
        r#"{{remote_addr}} - {{remote_user}} [{{time_local}}] "{{request}}" {{status}} {{body_bytes_sent}} "{{http_referer}}" "{{http_user_agent}}" {{request_time}}"#.parse().unwrap()
    }
}

impl FromStr for LogTemplate {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut result = vec![];
        let mut cur = 0usize;

        static REGEXP_VAR: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"\{\{[a-zA-Z0-9.\-_:]+}}").unwrap());

        for next in REGEXP_VAR.captures_iter(s) {
            if let Some(m) = next.get(0) {
                let start = m.start();
                let end = m.end();

                if start > cur {
                    result.push(LogPart::Anything(Cachestr::from(&s[cur..start])));
                }

                match m.as_str() {
                    "{{request}}" => result.push(LogPart::Request),
                    "{{remote_addr}}" => result.push(LogPart::RemoteAddr),
                    "{{remote_user" => result.push(LogPart::RemoteUser),
                    "{{time_local}}" => result.push(LogPart::TimeLocal),
                    "{{body_bytes_sent}}" => result.push(LogPart::BodyBytesSent),
                    "{{status}}" => result.push(LogPart::Status),
                    "{{http_referer}}" => result.push(LogPart::HttpReferer),
                    "{{http_user_agent}}" => result.push(LogPart::HttpUserAgent),
                    "{{request_time}}" => result.push(LogPart::RequestTime),
                    "{{http_x_forwarded_for}}" => result.push(LogPart::XForwardedFor),
                    "{{upstream_connect_time}}" => result.push(LogPart::UpstreamConnectTime),
                    "{{upstream_header_time}}" => result.push(LogPart::UpstreamHeaderTime),
                    "{{upstream_response_time}}" => result.push(LogPart::UpstreamResponseTime),
                    "{{gzip_ratio}}" => result.push(LogPart::GzipRatio),
                    "{{host}}" => result.push(LogPart::Host),
                    "{{request_method}}" => result.push(LogPart::RequestMethod),
                    "{{request_path}}" => result.push(LogPart::RequestPath),
                    "{{request_uri}}" => result.push(LogPart::RequestURI),
                    "{{request_length}}" => result.push(LogPart::RequestLength),
                    "{{connection}}" => result.push(LogPart::Connection),
                    "{{connection_requests}}" => result.push(LogPart::ConnectionRequests),
                    other => {
                        let origin = &other[1..];
                        let converted = if origin.contains('_') {
                            Cow::Owned(origin.replace('_', "-"))
                        } else {
                            Cow::Borrowed(origin)
                        };
                        match converted.strip_prefix("http_") {
                            Some(header) => {
                                result.push(LogPart::HttpHeader(Cachestr::from(header)));
                            }
                            None => result.push(LogPart::HttpHeader(Cachestr::from(converted))),
                        }
                    }
                }

                cur = end;
            }
        }

        if cur < s.len() {
            result.push(LogPart::Anything(Cachestr::from(&s[cur..])))
        }

        Ok(LogTemplate(result))
    }
}

struct HttpPipelineAccessLog {
    tpl: Arc<LogTemplate>,
    lc: Mutex<LogContext>,
}

#[async_trait]
impl HttpPipeline for HttpPipelineAccessLog {
    async fn handle_request_line(
        &self,
        ctx: &mut HttpContext,
        request_line: &mut RequestLine,
    ) -> anyhow::Result<()> {
        let cloned = Clone::clone(request_line);

        {
            let mut w = self.lc.lock();
            w.request_line = cloned;
            w.begin = Local::now();
        }

        match ctx.next() {
            None => Ok(()),
            Some(next) => next.handle_request_line(ctx, request_line).await,
        }
    }

    async fn handle_request_headers(
        &self,
        ctx: &mut HttpContext,
        headers: &mut Headers,
    ) -> anyhow::Result<()> {
        let cloned = Clone::clone(headers);

        {
            let mut w = self.lc.lock();
            w.request_headers = cloned;
        }

        match ctx.next() {
            None => Ok(()),
            Some(next) => next.handle_request_headers(ctx, headers).await,
        }
    }

    async fn handle_status_line(
        &self,
        ctx: &mut HttpContext,
        status_line: &mut StatusLine,
    ) -> anyhow::Result<()> {
        let status_code = status_line.status_code();
        {
            let mut w = self.lc.lock();
            w.status_code = status_code;
        }

        match ctx.next() {
            None => Ok(()),
            Some(next) => next.handle_status_line(ctx, status_line).await,
        }
    }

    async fn handle_response_headers(
        &self,
        ctx: &mut HttpContext,
        headers: &mut Headers,
    ) -> anyhow::Result<()> {
        let mut b = BytesMut::with_capacity(2048);
        {
            let mut w = self.lc.lock();
            w.end = Local::now();
            self.tpl.write(&mut b, &w);
        }

        let b = b.freeze();
        let s = unsafe { std::str::from_utf8_unchecked(&b[..]) };

        info!("-----{}", s);

        match ctx.next() {
            None => Ok(()),
            Some(next) => next.handle_response_headers(ctx, headers).await,
        }
    }
}

struct HttpPipelineAccessLogFactory {
    tpl: Arc<LogTemplate>,
}

impl HttpPipelineFactory for HttpPipelineAccessLogFactory {
    type Item = HttpPipelineAccessLog;

    fn generate(&self) -> anyhow::Result<Self::Item> {
        let lc = LogContext {
            connection: 0,
            connection_requests: 0,
            begin: Default::default(),
            end: Default::default(),
            remote_addr: None,
            body_bytes_recv: 0,
            body_bytes_sent: 0,
            status_code: 0,
            request_line: RequestLine::builder().build(),
            request_headers: Headers::builder().build(),
            gzip_ratio: None,
            upstream_connect_time: None,
            upstream_header_time: None,
            upstream_response_time: None,
        };
        Ok(HttpPipelineAccessLog {
            tpl: Clone::clone(&self.tpl),
            lc: Mutex::new(lc),
        })
    }
}

impl TryFrom<&PipelineConf> for HttpPipelineAccessLogFactory {
    type Error = anyhow::Error;

    fn try_from(value: &PipelineConf) -> Result<Self, Self::Error> {
        const KEY_FORMAT: &str = "format";

        let mut tpl = None;
        if let Some(v) = value.get(KEY_FORMAT) {
            let s = v
                .as_str()
                .ok_or_else(|| CapybaraError::InvalidConfig(KEY_FORMAT.into()))?;
            tpl.replace(LogTemplate::from_str(s)?);
        }

        Ok(Self {
            tpl: Arc::new(tpl.unwrap_or_default()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[tokio::test]
    async fn test_accesslog() -> anyhow::Result<()> {
        init();

        let c: PipelineConf = {
            // language=yaml
            let s = r#"
#            format: x
            "#;
            serde_yaml::from_str(s).unwrap()
        };

        let factory = HttpPipelineAccessLogFactory::try_from(&c)?;

        let p = factory.generate()?;

        // route with query
        {
            let mut ctx = HttpContext::fake();
            let mut rl = RequestLine::builder().uri("/hello?srv=3").build();
            let mut headers = Headers::builder()
                .put(HttpField::Host.as_str(), "example.com")
                .put(HttpField::Referer.as_str(), "fake-referer")
                .build();
            let mut status_line = {
                let status_line = b"HTTP/1.1 200 OK\r\nHos";
                let mut b = BytesMut::from(&status_line[..]);
                StatusLine::read(&mut b)?.unwrap()
            };
            let mut headers2 = Headers::builder()
                .put("Content-Type", "application/json")
                .build();

            assert!(p.handle_request_line(&mut ctx, &mut rl).await.is_ok());
            assert!(p
                .handle_request_headers(&mut ctx, &mut headers)
                .await
                .is_ok());

            {
                use tokio::time;
                time::sleep(time::Duration::from_millis(123)).await;
            }

            assert!(p
                .handle_status_line(&mut ctx, &mut status_line)
                .await
                .is_ok());
            assert!(p
                .handle_response_headers(&mut ctx, &mut headers2)
                .await
                .is_ok());
        }

        Ok(())
    }
}
