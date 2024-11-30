use std::io;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use bytes::Bytes;
use deadpool::managed::Manager;
use futures::{Stream, StreamExt};
use once_cell::sync::Lazy;
use rustls::pki_types::ServerName;
use smallvec::SmallVec;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufWriter, ReadHalf, WriteHalf};
use tokio::sync::Notify;
use tokio::time::error::Elapsed;
use tokio_rustls::TlsAcceptor;
use tokio_util::codec::FramedRead;

use capybara_util::cachestr::Cachestr;

use crate::pipeline::http::{
    load, AnyString, HeaderOperator, HeadersContext, HttpContextFlags, HttpPipelineFactoryExt,
};
use crate::pipeline::{HttpContext, PipelineConf};
use crate::proto::{Listener, Signal, Signals, UpstreamKey};
use crate::protocol::http::codec::Flags;
use crate::protocol::http::misc::SERVER;
use crate::protocol::http::{
    misc, Headers, HttpCodec, HttpField, HttpFrame, RequestLine, Response, ResponseFlags,
    StatusLine,
};
use crate::transport::{tcp, Address, Addressable};
use crate::upstream::{Pool, Pools, Upstreams};
use crate::{CapybaraError, Result};

static HTML_408: Lazy<Bytes> = Lazy::new(|| render_html(include_str!("408.html")));
static HTML_500: Lazy<Bytes> = Lazy::new(|| render_html(include_str!("500.html")));
static HTML_502: Lazy<Bytes> = Lazy::new(|| render_html(include_str!("502.html")));
static HTML_504: Lazy<Bytes> = Lazy::new(|| render_html(include_str!("504.html")));

#[inline(always)]
fn render_html(html: &'static str) -> Bytes {
    if let Ok(template) = liquid::ParserBuilder::with_stdlib()
        .build()
        .unwrap()
        .parse(html)
    {
        let globals = liquid::object!({
            "server": SERVER.as_str(),
        });

        if let Ok(v) = template.render(&globals) {
            return Bytes::from(v);
        }
    }
    Bytes::from(html.as_bytes())
}

pub struct HttpListenerBuilder {
    addr: SocketAddr,
    tls: Option<TlsAcceptor>,
    id: Option<Cachestr>,
    pipelines: Vec<(Cachestr, PipelineConf)>,
    upstreams: Vec<(Cachestr, Arc<dyn Pools>)>,
    cfg: Config,
}

impl HttpListenerBuilder {
    pub fn id<I>(mut self, id: I) -> Self
    where
        I: AsRef<str>,
    {
        self.id.replace(Cachestr::from(id.as_ref()));
        self
    }

    pub fn pipeline<A>(mut self, name: A, c: &PipelineConf) -> Self
    where
        A: AsRef<str>,
    {
        self.pipelines
            .push((Cachestr::from(name.as_ref()), Clone::clone(c)));
        self
    }

    pub fn tls(mut self, tls_acceptor: TlsAcceptor) -> Self {
        self.tls.replace(tls_acceptor);
        self
    }

    pub fn client_header_timeout(mut self, timeout: Duration) -> Self {
        self.cfg.client_header_timeout = timeout;
        self
    }

    pub fn client_body_timeout(mut self, timeout: Duration) -> Self {
        self.cfg.client_body_timeout = timeout;
        self
    }

    pub fn proxy_connect_timeout(mut self, timeout: Duration) -> Self {
        self.cfg.proxy_connect_timeout = timeout;
        self
    }

    pub fn proxy_read_timeout(mut self, timeout: Duration) -> Self {
        self.cfg.proxy_read_timeout = timeout;
        self
    }

    pub fn proxy_send_timeout(mut self, timeout: Duration) -> Self {
        self.cfg.proxy_send_timeout = timeout;
        self
    }

    pub fn keepalive_timeout(mut self, timeout: Duration) -> Self {
        self.cfg.keepalive_timeout = timeout;
        self
    }

    pub fn upstream<K>(mut self, key: K, pools: Arc<dyn Pools>) -> Self
    where
        K: AsRef<str>,
    {
        self.upstreams.push((Cachestr::from(key.as_ref()), pools));
        self
    }

    pub fn build(self) -> Result<HttpListener> {
        let Self {
            addr,
            id,
            pipelines,
            tls,
            upstreams,
            cfg,
        } = self;

        let closer = Arc::new(Notify::new());

        let mut ub = Upstreams::builder(Clone::clone(&closer));
        for (k, v) in upstreams {
            ub = ub.keyed(k, v);
        }

        Ok(HttpListener {
            id: id.unwrap_or_else(|| Cachestr::from(uuid::Uuid::new_v4().to_string())),
            tls,
            addr,
            pipelines: ArcSwap::from_pointee(pipelines),
            upstreams: ub.build(),
            cfg: ArcSwap::from_pointee(cfg),
            closer,
        })
    }
}

struct Config {
    client_header_timeout: Duration,
    client_body_timeout: Duration,
    keepalive_timeout: Duration,
    proxy_connect_timeout: Duration,
    proxy_read_timeout: Duration,
    proxy_send_timeout: Duration,
    upstreams_retries: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            client_header_timeout: Duration::from_secs(60),
            client_body_timeout: Duration::from_secs(60),
            keepalive_timeout: Duration::from_secs(75),
            proxy_connect_timeout: Duration::from_secs(60),
            proxy_read_timeout: Duration::from_secs(60),
            proxy_send_timeout: Duration::from_secs(60),
            upstreams_retries: 0,
        }
    }
}

pub struct HttpListener {
    id: Cachestr,
    cfg: ArcSwap<Config>,
    addr: SocketAddr,
    tls: Option<TlsAcceptor>,
    pipelines: ArcSwap<Vec<(Cachestr, PipelineConf)>>,
    upstreams: Upstreams,
    closer: Arc<Notify>,
}

impl HttpListener {
    pub fn builder(addr: SocketAddr) -> HttpListenerBuilder {
        HttpListenerBuilder {
            addr,
            id: None,
            tls: None,
            pipelines: Default::default(),
            upstreams: Default::default(),
            cfg: Default::default(),
        }
    }

    #[inline]
    fn build_pipeline_factories(&self) -> Result<Vec<Box<dyn HttpPipelineFactoryExt>>> {
        let r = self.pipelines.load();

        let mut factories = Vec::with_capacity(r.len());

        for (k, v) in r.iter() {
            factories.push(load(k.as_ref(), v)?);
        }

        Ok(factories)
    }

    #[inline(always)]
    fn build_context(
        flags: HttpContextFlags,
        client_addr: SocketAddr,
        factories: &[Box<dyn HttpPipelineFactoryExt>],
    ) -> Result<HttpContext> {
        let mut b = HttpContext::builder(client_addr).flags(flags);

        for next in factories {
            b = b.pipeline_arc(next.generate_arc()?);
        }

        Ok(b.build())
    }

    #[inline(always)]
    fn build_http_context_flags(&self) -> HttpContextFlags {
        let mut f = HttpContextFlags::default();
        if self.tls.is_some() {
            f.set(HttpContextFlags::HTTPS, true);
        }
        f
    }
}

#[async_trait]
impl Listener for HttpListener {
    fn id(&self) -> &str {
        self.id.as_ref()
    }

    async fn listen(&self, signals: &mut Signals) -> Result<()> {
        let l = tcp::TcpListenerBuilder::new(self.addr).build()?;
        let mut pipelines = self.build_pipeline_factories()?;
        info!("listener '{}' is listening on {:?}", &self.id, &self.addr);

        let mut flags = self.build_http_context_flags();

        loop {
            tokio::select! {
                signal = signals.recv() => {
                    info!("listener '{}' got signal {:?}", &self.id, &signal);
                    match signal {
                        None => {
                            info!("listener '{}' is stopping....", &self.id);
                            return Ok(());
                        }
                        Some(Signal::Shutdown) => {
                            info!("listener '{}' is stopping...", &self.id);
                            return Ok(());
                        }
                        Some(Signal::Reload) => {
                            info!("listener '{}' is reloading...", &self.id);
                            // TODO: reload the current listener
                            pipelines = self.build_pipeline_factories()?;
                            flags = self.build_http_context_flags();
                        }
                    }

                }
                accept = l.accept() => {
                    let (stream, addr) = accept?;

                    debug!("accept a new tcp stream {:?}", &addr);
                    let ctx = Self::build_context(flags, addr, &pipelines[..])?;
                    let cfg = self.cfg.load_full();
                    let closer = Clone::clone(&self.closer);

                    match &self.tls {
                        None => {
                            let upstreams = Clone::clone(&self.upstreams);
                            let mut handler = Handler::new(
                                ctx,
                                cfg,
                                stream,
                                closer,
                                upstreams,
                            );
                            tokio::spawn(async move {
                                if let Err(e) = handler.handle().await {
                                    error!("http handler end: {}", e);
                                }
                            });
                        }
                        Some(tls) => {
                            if let Ok(stream) = tls.accept(stream).await {
                                let upstreams = Clone::clone(&self.upstreams);
                                let mut handler = Handler::new(
                                    ctx,
                                    cfg,
                                    stream,
                                    closer,
                                    upstreams,
                                );
                                tokio::spawn(async move {
                                    if let Err(e) = handler.handle().await {
                                        error!("http handler end: {}", e);
                                    }
                                });
                            }
                        }
                    }
                }
            }
        }
    }
}

struct Handshake {
    request_line: RequestLine,
    request_headers: Headers,
    status: Status,
}

enum Status {
    Close,
    KeepAlive,
}

struct Handler<S> {
    downstream: (FramedRead<ReadHalf<S>, HttpCodec>, BufWriter<WriteHalf<S>>),
    deny_headers: roaring::RoaringBitmap,
    ctx: HttpContext,
    upstreams: Upstreams,
    cfg: Arc<Config>,
}

impl<S> Handler<S> {
    const BUFF_SIZE: usize = 8192; // 8KB
}

impl<S> Handler<S>
where
    S: AsyncWrite + AsyncRead + Sync + Send + 'static,
{
    #[inline]
    fn new(
        ctx: HttpContext,
        cfg: Arc<Config>,
        stream: S,
        closer: Arc<Notify>,
        upstreams: Upstreams,
    ) -> Self {
        let (rh, wh) = tokio::io::split(stream);

        let downstream = (
            {
                let f = Flags::default();
                FramedRead::with_capacity(rh, HttpCodec::new(f, None, None), Self::BUFF_SIZE)
            },
            BufWriter::with_capacity(Self::BUFF_SIZE, wh),
        );

        Self {
            ctx,
            cfg,
            downstream,
            upstreams,
            deny_headers: Default::default(),
        }
    }

    #[inline]
    async fn write_headers<W>(
        w: &mut W,
        headers: Headers,
        hc: &mut HeadersContext,
        deny_headers: &mut roaring::RoaringBitmap,
    ) -> Result<()>
    where
        W: AsyncWriteExt + Unpin,
    {
        if hc.is_empty() {
            let mut b: Bytes = headers.into();
            w.write_all_buf(&mut b).await?;
            return Ok(());
        }

        let extra_headers: Option<Bytes> = {
            let mut bu = Headers::builder();
            for (name, operators) in hc.inner.iter() {
                for op in operators.iter() {
                    match op {
                        HeaderOperator::Drop => {
                            for pos in headers.positions(name.as_ref()) {
                                deny_headers.insert(pos as u32);
                            }
                        }
                        HeaderOperator::Add(val) => {
                            bu = bu.put(name.as_ref(), val.as_ref());
                        }
                    }
                }
            }

            if bu.is_empty() {
                None
            } else {
                Some(bu.build().into())
            }
        };

        if let Some(mut extra_headers) = extra_headers {
            w.write_all_buf(&mut extra_headers).await?;
        }

        // 1. no blacklist headers
        if deny_headers.is_empty() {
            let mut b: Bytes = headers.into();
            w.write_all_buf(&mut b).await?;
            return Ok(());
        }

        // 2. deny blacklist headers
        let length = headers.len();
        for (i, mut b) in headers.into_iter().enumerate() {
            if !deny_headers.contains(i as u32) {
                w.write_all_buf(&mut b).await?;
            }
        }

        w.write_all(misc::CRLF).await?;
        w.flush().await?;

        deny_headers.clear();

        Ok(())
    }

    #[inline]
    async fn write_response_half(
        &mut self,
        status_line: StatusLine,
        headers: Headers,
    ) -> Result<()> {
        {
            let mut b: Bytes = status_line.into();
            self.downstream.1.write_all_buf(&mut b).await?;
        }

        self.ctx.response().headers()._replace(
            HttpField::Server.into(),
            AnyString::Arc(Clone::clone(&misc::SERVER)),
        );

        Self::write_headers(
            &mut self.downstream.1,
            headers,
            self.ctx.response().headers(),
            &mut self.deny_headers,
        )
        .await?;

        Ok(())
    }

    #[inline]
    async fn write_request_half<W>(
        &mut self,
        w: &mut W,
        request_line: RequestLine,
        headers: Headers,
    ) -> Result<()>
    where
        W: AsyncWriteExt + Unpin,
    {
        {
            let mut b: Bytes = request_line.into();
            w.write_all_buf(&mut b).await?;
        }

        Self::write_headers(
            w,
            headers,
            self.ctx.request().headers(),
            &mut self.deny_headers,
        )
        .await?;

        Ok(())
    }

    #[inline]
    async fn exhaust_requests(&mut self) -> Result<()> {
        if self
            .ctx
            .flags()
            .contains(HttpContextFlags::DOWNSTREAM_EXHAUSTED)
        {
            return Ok(());
        }

        while let Some(req) = self.downstream.0.next().await {
            let req = req?;
            if req.is_complete() {
                self.ctx
                    .flags_mut()
                    .set(HttpContextFlags::DOWNSTREAM_EXHAUSTED, true);
                break;
            }
        }

        Ok(())
    }

    #[inline]
    async fn handshake(&mut self) -> Result<Option<Handshake>> {
        use tokio::time;

        match time::timeout(self.cfg.client_header_timeout, self.downstream.0.next()).await {
            Ok(first_result) => {
                match first_result {
                    Some(first) => {
                        // check io error
                        if let Err(e) = &first {
                            if let Some(e) = e.downcast_ref::<io::Error>() {
                                if matches!(e.kind(), ErrorKind::UnexpectedEof) {
                                    return Ok(None);
                                }
                            }
                        }

                        let HttpFrame::RequestLine(mut request_line) = first? else {
                            unreachable!()
                        };

                        if let Some(p) = self.ctx.pipeline() {
                            p.handle_request_line(&mut self.ctx, &mut request_line)
                                .await?;
                        }

                        match self.downstream.0.next().await {
                            Some(second) => {
                                let HttpFrame::Headers(mut headers) = second? else {
                                    unreachable!()
                                };

                                if let Some(p) = self.ctx.pipeline() {
                                    p.handle_request_headers(&mut self.ctx, &mut headers)
                                        .await?;
                                }

                                let status = {
                                    let connection = headers.get_by_field(HttpField::Connection);
                                    if request_line.nohttp11() {
                                        match connection {
                                            Some(v) if v.eq_ignore_ascii_case(b"keep-alive") => {
                                                Status::KeepAlive
                                            }
                                            _ => Status::Close,
                                        }
                                    } else {
                                        match connection {
                                            Some(v) if v.eq_ignore_ascii_case(b"close") => {
                                                Status::Close
                                            }
                                            _ => Status::KeepAlive,
                                        }
                                    }
                                };

                                Ok(Some(Handshake {
                                    request_line,
                                    request_headers: headers,
                                    status,
                                }))
                            }
                            None => Ok(None),
                        }
                    }
                    None => Ok(None),
                }
            }
            Err(_) => Err(CapybaraError::Timeout),
        }
    }

    async fn exchange<R>(&mut self, upstream: &mut R) -> anyhow::Result<()>
    where
        R: Stream<Item = anyhow::Result<HttpFrame>> + Unpin,
    {
        if let Some(first) = upstream.next().await {
            let HttpFrame::StatusLine(mut status_line) = first? else {
                unreachable!()
            };

            if let Some(p) = self.ctx.pipeline() {
                p.handle_status_line(&mut self.ctx, &mut status_line)
                    .await?;
            }

            if let Some(second) = upstream.next().await {
                let HttpFrame::Headers(mut headers) = second? else {
                    unreachable!()
                };

                if let Some(p) = self.ctx.pipeline() {
                    p.handle_response_headers(&mut self.ctx, &mut headers)
                        .await?;
                }

                self.write_response_half(status_line, headers).await?;

                loop {
                    match upstream.next().await {
                        None => break,
                        Some(next) => match next? {
                            HttpFrame::CompleteBody(body) => {
                                body.write_to(&mut self.downstream.1).await?;
                                self.downstream.1.flush().await?;
                                break;
                            }
                            HttpFrame::PartialBody(body) => {
                                body.write_to(&mut self.downstream.1).await?;
                                self.downstream.1.flush().await?;
                            }
                            _ => unreachable!(),
                        },
                    }
                }
            }
        }

        Ok(())
    }

    #[inline]
    async fn transfer<U>(
        &mut self,
        upstream: &mut U,
        request_line: RequestLine,
        request_headers: Headers,
    ) -> anyhow::Result<()>
    where
        U: AsyncWrite + AsyncRead + Unpin,
    {
        let (r, w) = tokio::io::split(upstream);
        let mut w = BufWriter::with_capacity(Self::BUFF_SIZE, w);
        self.write_request_half(&mut w, request_line, request_headers)
            .await?;

        while let Some(next) = self.downstream.0.next().await {
            let next = next?;

            match next {
                HttpFrame::PartialBody(body) => {
                    body.write_to(&mut w).await?;
                    w.flush().await?;
                }
                HttpFrame::CompleteBody(body) => {
                    body.write_to(&mut w).await?;
                    w.flush().await?;

                    let codec = HttpCodec::new(Flags::RESPONSE, None, None);
                    let mut r = FramedRead::with_capacity(r, codec, Self::BUFF_SIZE);

                    self.exchange(&mut r).await?;

                    break;
                }
                _ => unreachable!(),
            }
        }

        Ok(())
    }

    async fn handle(&mut self) -> anyhow::Result<()> {
        loop {
            self.ctx.reset();
            self.deny_headers.clear();

            match self.handshake().await {
                Ok(Some(Handshake {
                    request_line,
                    request_headers,
                    status,
                })) => match self.ctx.immediate_response.take() {
                    Some(respond) => {
                        self.exhaust_requests().await?;
                        respond
                            .write_to(&mut self.downstream.1, ResponseFlags::default())
                            .await?;
                        self.downstream.1.flush().await?;
                        if matches!(status, Status::Close) {
                            break;
                        }
                    }
                    None => match self.ctx.upstream() {
                        Some(uk) => {
                            let pool = self.upstreams.get(uk, 0).await?;
                            match &*pool {
                                Pool::Tcp(pool) => {
                                    if !self.ctx.request().headers()._exist(HttpField::Host.into())
                                    {
                                        if let Address::Host(host, port) = pool.manager().address()
                                        {
                                            let host = if *port == 80 {
                                                AnyString::Cache(Clone::clone(host))
                                            } else {
                                                let host = {
                                                    use std::io::Write;
                                                    let mut b = SmallVec::<[u8; 128]>::new();
                                                    write!(
                                                        &mut b[..],
                                                        "{}:{}",
                                                        host.as_ref(),
                                                        port
                                                    )
                                                    .ok();
                                                    Cachestr::from(unsafe {
                                                        std::str::from_utf8_unchecked(&b[..])
                                                    })
                                                };
                                                AnyString::Cache(host)
                                            };

                                            self.ctx
                                                .request()
                                                .headers()
                                                ._replace(HttpField::Host.into(), host);
                                        }
                                    }

                                    let mut upstream = pool.get().await?;
                                    self.transfer(upstream.as_mut(), request_line, request_headers)
                                        .await?
                                }
                                Pool::Tls(pool) => {
                                    if !self.ctx.request().headers()._exist(HttpField::Host.into())
                                    {
                                        if let Address::Host(host, port) = pool.manager().address()
                                        {
                                            let host = if *port == 443 {
                                                AnyString::Cache(Clone::clone(host))
                                            } else {
                                                let host = {
                                                    let mut b = SmallVec::<[u8; 128]>::new();
                                                    use std::io::Write;
                                                    write!(
                                                        &mut b[..],
                                                        "{}:{}",
                                                        host.as_ref(),
                                                        port
                                                    )
                                                    .ok();
                                                    Cachestr::from(unsafe {
                                                        std::str::from_utf8_unchecked(&b[..])
                                                    })
                                                };
                                                AnyString::Cache(host)
                                            };
                                            self.ctx
                                                .request()
                                                .headers()
                                                ._replace(HttpField::Host.into(), host);
                                        }
                                    }

                                    let mut upstream = pool.get().await?;
                                    self.transfer(upstream.as_mut(), request_line, request_headers)
                                        .await?
                                }
                            }

                            if matches!(status, Status::Close) {
                                break;
                            }
                        }
                        None => {
                            self.exhaust_requests().await?;
                            let resp = Response::builder()
                                .status_code(502)
                                .body(&HTML_502[..])
                                .content_type("text/html")
                                .build();
                            resp.write_to(&mut self.downstream.1, ResponseFlags::default())
                                .await?;
                            self.downstream.1.flush().await?;
                        }
                    },
                },
                Ok(None) => {
                    debug!("outbound connection has no more data");
                    break;
                }
                Err(e) => {
                    error!("http handshake failed: {}", e);

                    // TODO: how to get the root cause???
                    if let Some((code, html)) = match &e {
                        CapybaraError::InvalidUpstream(_) => Some((502, &HTML_502[..])),
                        CapybaraError::Timeout => Some((408, &HTML_408[..])),
                        _ => Some((500, &HTML_500[..])),
                    } {
                        let resp = Response::builder()
                            .status_code(code)
                            .body(html)
                            .content_type("text/html")
                            .header(HttpField::Connection.as_str(), "close")
                            .build();
                        resp.write_to(&mut self.downstream.1, ResponseFlags::default())
                            .await?;
                        self.downstream.1.flush().await?;
                    }

                    // TODO: cleanup
                    break;
                }
            }
        }

        Ok(())
    }

    #[inline(always)]
    fn set_request_sni(&mut self, sni: &ServerName) {
        if let ServerName::DnsName(name) = sni {
            self.ctx
                .request()
                .headers()
                .replace(HttpField::Host.as_str(), name.as_ref());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::sync::Notify;

    use crate::pipeline::PipelineConf;
    use crate::proto::{Listener, Signal};

    use super::*;

    async fn init() {
        pretty_env_logger::try_init_timed().ok();
        crate::setup().await;
    }

    #[tokio::test]
    async fn test_http_listener() -> anyhow::Result<()> {
        init().await;

        use tokio::sync::mpsc;

        let (tx, mut rx) = mpsc::channel(1);
        let closed = Arc::new(Notify::new());

        {
            let c: PipelineConf = {
                // language=yaml
                let s = r#"
            routes:
            - route: httpbin.org:80
              matches:
              - location: path
                match: /anything*
            "#;
                serde_yaml::from_str(s).unwrap()
            };

            let l = HttpListener::builder("127.0.0.1:8080".parse()?)
                .id("fake-http-listener")
                .pipeline("capybara.pipelines.http.router", &c)
                .build()?;

            let closed = Clone::clone(&closed);

            tokio::spawn(async move {
                let _ = l.listen(&mut rx).await;
                closed.notify_one();
            });
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let mut c = TcpStream::connect("127.0.0.1:8080").await?;
        c.write_all(&b"GET /anything HTTP/1.1\r\nHost: httpbin.org\r\nAccept: *\r\nConnection: close\r\n\r\n"[..]).await?;
        c.flush().await?;

        let mut v = vec![];
        c.read_to_end(&mut v).await?;
        info!("response: {}", String::from_utf8_lossy(&v[..]));

        let _ = tx.send(Signal::Shutdown).await;

        closed.notified().await;

        Ok(())
    }
}
