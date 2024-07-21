use std::io;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use rustls::ServerName;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufWriter, ReadHalf, WriteHalf};
use tokio::sync::Notify;
use tokio_rustls::TlsAcceptor;
use tokio_util::codec::FramedRead;

use crate::cachestr::Cachestr;
use crate::pipeline::http::{
    load, AnyString, HeaderOperator, HeadersContext, HttpContextFlags, HttpPipelineFactoryExt,
};
use crate::pipeline::{HttpContext, PipelineConf};
use crate::proto::{Listener, Signal, Signals, UpstreamKey};
use crate::protocol::http::codec::Flags;
use crate::protocol::http::{
    misc, Headers, HttpCodec, HttpField, HttpFrame, RequestLine, Response, ResponseFlags,
    StatusLine,
};
use crate::transport::tcp;
use crate::upstream::{Pool, Upstreams};
use crate::Result;

pub struct HttpListenerBuilder {
    addr: SocketAddr,
    tls: Option<TlsAcceptor>,
    id: Option<Cachestr>,
    pipelines: Vec<(Cachestr, PipelineConf)>,
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

    pub fn build(self) -> Result<HttpListener> {
        let Self {
            addr,
            id,
            pipelines,
            tls,
        } = self;

        let closer = Arc::new(Notify::new());

        let upstreams = Upstreams::builder(Clone::clone(&closer)).build();

        Ok(HttpListener {
            id: id.unwrap_or_else(|| Cachestr::from(uuid::Uuid::new_v4().to_string())),
            tls,
            addr,
            pipelines: ArcSwap::from_pointee(pipelines),
            upstreams,
            closer,
        })
    }
}

pub struct HttpListener {
    id: Cachestr,
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

    fn build_context(
        client_addr: SocketAddr,
        factories: &[Box<dyn HttpPipelineFactoryExt>],
    ) -> Result<HttpContext> {
        let mut b = HttpContext::builder(client_addr);

        for next in factories {
            b = b.pipeline_arc(next.generate_arc()?);
        }

        Ok(b.build())
    }
}

#[async_trait]
impl Listener for HttpListener {
    async fn listen(&self, signals: &mut Signals) -> Result<()> {
        let l = tcp::TcpListenerBuilder::new(self.addr).build()?;
        let mut pipelines = self.build_pipeline_factories()?;
        info!("listener '{}' is listening on {:?}", &self.id, &self.addr);

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
                        }
                    }

                }
                accept = l.accept() => {
                    let (stream, addr) = accept?;

                    debug!("accept a new tcp stream {:?}", &addr);
                    let ctx = Self::build_context(addr, &pipelines[..])?;

                    match &self.tls {
                        None => {
                            let upstreams = Clone::clone(&self.upstreams);
                            let mut handler = Handler::new(ctx, stream, Clone::clone(&self.closer), upstreams);
                            tokio::spawn(async move {
                                if let Err(e) = handler.handle().await {
                                    error!("http handler end: {}", e);
                                }
                            });
                        }
                        Some(tls) => {
                            if let Ok(stream) = tls.accept(stream).await {
                                let upstreams = Clone::clone(&self.upstreams);
                                let mut handler = Handler::new(ctx, stream, Clone::clone(&self.closer), upstreams);
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
}

impl<S> Handler<S> {
    const BUFF_SIZE: usize = 8192; // 8KB
}

impl<S> Handler<S>
where
    S: AsyncWrite + AsyncRead + Sync + Send + 'static,
{
    #[inline]
    fn new(ctx: HttpContext, stream: S, closer: Arc<Notify>, upstreams: Upstreams) -> Self {
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
        match self.downstream.0.next().await {
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

                let mut has_upstream = false;

                if let Some(uk) = self.ctx.upstream() {
                    has_upstream = true;
                    match &*uk {
                        UpstreamKey::Tls(_, sni) => self.set_request_sni(sni),
                        UpstreamKey::TlsHP(_, _, sni) => self.set_request_sni(sni),
                        UpstreamKey::TcpHP(host, port) => {
                            // check if http port
                            let host = if *port == 80 {
                                AnyString::Cache(Clone::clone(host))
                            } else {
                                AnyString::String(format!("{}:{}", host, port))
                            };
                            self.ctx
                                .request()
                                .headers()
                                ._replace(HttpField::Host.into(), host);
                        }
                        _ => (),
                    }
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
                                    Some(v) if v.eq_ignore_ascii_case(b"close") => Status::Close,
                                    _ => Status::KeepAlive,
                                }
                            }
                        };

                        if !has_upstream {
                            if let Some(kind) = self.ctx.upstream() {
                                match &*kind {
                                    UpstreamKey::Tls(_, sni) => self.set_request_sni(sni),
                                    UpstreamKey::TlsHP(_, _, sni) => self.set_request_sni(sni),
                                    UpstreamKey::TcpHP(host, port) => {
                                        // check if http port
                                        let host = if *port == 80 {
                                            AnyString::Cache(Clone::clone(host))
                                        } else {
                                            AnyString::String(format!("{}:{}", host, port))
                                        };
                                        self.ctx
                                            .request()
                                            .headers()
                                            ._replace(HttpField::Host.into(), host);
                                    }
                                    _ => (),
                                }
                            }
                        }

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
                            let pool = self.upstreams.get(uk).await?;
                            match &*pool {
                                Pool::Tcp(pool) => {
                                    let mut upstream = pool.get().await?;
                                    self.transfer(upstream.as_mut(), request_line, request_headers)
                                        .await?
                                }
                                Pool::Tls(pool) => {
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
                                .body(include_bytes!("502.html"))
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
    use crate::pipeline::PipelineConf;
    use crate::proto::{Listener, Signal};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::sync::Notify;

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
