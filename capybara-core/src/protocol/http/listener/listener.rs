use std::net::{SocketAddr, ToSocketAddrs};
use std::num::ParseIntError;
use std::sync::Arc;

use anyhow::Error;
use arc_swap::{ArcSwap, Cache};
use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufWriter, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_rustls::TlsAcceptor;
use tokio_util::codec::FramedRead;
use uuid::uuid;

use crate::cachestr::Cachestr;
use crate::pipeline::http::{load, HttpPipelineFactoryExt};
use crate::pipeline::{HttpContext, HttpPipeline, HttpPipelineFactory, PipelineConf};
use crate::proto::{Listener, Signal, Signals};
use crate::protocol::http::codec::Flags;
use crate::protocol::http::{Headers, HttpCodec, HttpFrame, RequestLine};
use crate::resolver::{Resolver, DEFAULT_RESOLVER};
use crate::transport::{TcpListenerBuilder, TlsConnectorBuilder};
use crate::CapybaraError;
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

        Ok(HttpListener {
            id: id.unwrap_or_else(|| Cachestr::from(uuid::Uuid::new_v4().to_string())),
            tls,
            addr,
            pipelines: ArcSwap::from_pointee(pipelines),
        })
    }
}

pub struct HttpListener {
    id: Cachestr,
    addr: SocketAddr,
    tls: Option<TlsAcceptor>,
    pipelines: ArcSwap<Vec<(Cachestr, PipelineConf)>>,
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
            b = b.pipeline_box(next.generate_boxed()?);
        }

        Ok(b.build())
    }
}

#[async_trait]
impl Listener for HttpListener {
    async fn listen(&self, signals: &mut Signals) -> Result<()> {
        let l = TcpListenerBuilder::new(self.addr).build()?;
        let closer = Arc::new(Notify::new());

        let mut pipelines = self.build_pipeline_factories()?;

        loop {
            tokio::select! {
                signal = signals.recv() => {
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

                    match &self.tls{
                        None => {
                            let mut handler = Handler::new(ctx, stream, Clone::clone(&closer));
                            tokio::spawn(async move {
                                if let Err(e) = handler.handle().await {
                                    error!("http handler end: {}", e);
                                }
                            });
                        }
                        Some(tls) => {
                            let stream = tls.accept(stream).await?;
                            let mut handler = Handler::new(ctx, stream, Clone::clone(&closer));
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

struct Handshake {
    upstream: Option<TcpStream>,
    request_line: RequestLine,
    request_headers: Headers,
}

struct Handler<S> {
    downstream: (FramedRead<ReadHalf<S>, HttpCodec>, BufWriter<WriteHalf<S>>),
    ctx: HttpContext,
}

impl<S> Handler<S> {
    const BUFF_SIZE: usize = 8192; // 8KB
}

impl<S> Handler<S>
where
    S: AsyncWrite + AsyncRead + Sync + Send + 'static,
{
    fn new(ctx: HttpContext, stream: S, closer: Arc<Notify>) -> Self {
        let (rh, wh) = tokio::io::split(stream);

        let downstream = (
            {
                let f = Flags::default();
                FramedRead::with_capacity(rh, HttpCodec::new(f, None, None), Self::BUFF_SIZE)
            },
            BufWriter::with_capacity(Self::BUFF_SIZE, wh),
        );

        Self { ctx, downstream }
    }

    async fn resolve(upstream: Cachestr) -> Result<SocketAddr> {
        if let Ok(addr) = upstream.parse::<SocketAddr>() {
            return Ok(addr);
        }

        let mut sp = upstream.splitn(2, ':');

        match sp.next() {
            None => Err(CapybaraError::NoAddressResolved(
                upstream.to_string().into(),
            )),
            Some(host) => {
                let ip = DEFAULT_RESOLVER.resolve_one(host).await?;
                match sp.next() {
                    None => Ok(SocketAddr::new(ip, 80)),
                    Some(port) => match port.parse::<u16>() {
                        Ok(port) => Ok(SocketAddr::new(ip, port)),
                        Err(_e) => Err(CapybaraError::NoAddressResolved(
                            upstream.to_string().into(),
                        )),
                    },
                }
            }
        }
    }

    async fn establish(upstream: Cachestr) -> Result<TcpStream> {
        let addr = Self::resolve(upstream).await?;

        use crate::transport::tcp;

        let stream = tcp::dial(addr, None, Self::BUFF_SIZE)?;

        debug!("establish tcpconn {} ok", addr);

        Ok(stream)
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
        let mut b: Bytes = request_line.into();
        w.write_all_buf(&mut b).await?;

        let mut b: Bytes = headers.into();
        w.write_all_buf(&mut b).await?;

        w.flush().await?;

        Ok(())
    }

    #[inline]
    async fn handshake(&mut self) -> Result<Option<Handshake>> {
        match self.downstream.0.next().await {
            Some(first) => {
                let HttpFrame::RequestLine(mut request_line) = first? else {
                    unreachable!()
                };

                if let Some(p) = self.ctx.reset_pipeline() {
                    p.handle_request_line(&self.ctx, &mut request_line).await?;
                }

                let mut upstream = None;

                if let Some(u) = self.ctx.upstream() {
                    upstream.replace(Self::establish(u).await?);
                }

                match self.downstream.0.next().await {
                    Some(second) => {
                        let HttpFrame::Headers(mut headers) = second? else {
                            unreachable!()
                        };

                        if let Some(p) = self.ctx.reset_pipeline() {
                            p.handle_request_headers(&self.ctx, &mut headers).await?;
                        }

                        if upstream.is_none() {
                            if let Some(u) = self.ctx.upstream() {
                                upstream.replace(Self::establish(u).await?);
                            }
                        }

                        Ok(Some(Handshake {
                            upstream,
                            request_line,
                            request_headers: headers,
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

            if let Some(p) = self.ctx.reset_pipeline() {
                p.handle_status_line(&self.ctx, &mut status_line).await?;
            }

            if let Some(second) = upstream.next().await {
                let HttpFrame::Headers(mut headers) = second? else {
                    unreachable!()
                };

                if let Some(p) = self.ctx.reset_pipeline() {
                    p.handle_response_headers(&self.ctx, &mut headers).await?;
                }

                // write status_line
                {
                    let mut b: Bytes = status_line.into();
                    self.downstream.1.write_all_buf(&mut b).await?;
                }

                // write response headers
                {
                    let mut b: Bytes = headers.into();
                    self.downstream.1.write_all_buf(&mut b).await?;
                }

                self.downstream.1.flush().await?;

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

    async fn handle(&mut self) -> anyhow::Result<()> {
        loop {
            match self.handshake().await {
                Ok(Some(Handshake {
                    upstream,
                    request_line,
                    request_headers,
                })) => {
                    match upstream {
                        Some(mut upstream) => {
                            match upstream.peer_addr() {
                                Ok(ref peer) if peer.port() == 443 => {
                                    let tls_connector = TlsConnectorBuilder::default().build()?;
                                    let sni = {
                                        let mut sni = None;

                                        if let Some(u) = self.ctx.upstream() {
                                            if let Some(host) = u.split(':').next() {
                                                if let Ok(v) = rustls::ServerName::try_from(host) {
                                                    sni.replace(v);
                                                }
                                            }
                                        }

                                        sni.unwrap_or_else(|| {
                                            rustls::ServerName::IpAddress(peer.ip())
                                        })
                                    };
                                    let mut upstream = tls_connector.connect(sni, upstream).await?;
                                    let (r, w) = tokio::io::split(&mut upstream);
                                    let mut w = BufWriter::with_capacity(Self::BUFF_SIZE, w);
                                    self.write_request_half(&mut w, request_line, request_headers)
                                        .await?;

                                    while let Some(next) = self.downstream.0.next().await {
                                        let next = next?;

                                        match next {
                                            HttpFrame::CompleteBody(body) => {
                                                body.write_to(&mut w).await?;
                                                w.flush().await?;

                                                let mut r = {
                                                    let codec =
                                                        HttpCodec::new(Flags::RESPONSE, None, None);
                                                    FramedRead::with_capacity(
                                                        r,
                                                        codec,
                                                        Self::BUFF_SIZE,
                                                    )
                                                };
                                                self.exchange(&mut r).await?;

                                                break;
                                            }
                                            HttpFrame::PartialBody(body) => {
                                                body.write_to(&mut w).await?;
                                                w.flush().await?;
                                            }
                                            _ => unreachable!(),
                                        }
                                    }
                                }
                                _ => {
                                    let (r, w) = tokio::io::split(&mut upstream);
                                    let mut w = BufWriter::with_capacity(Self::BUFF_SIZE, w);
                                    self.write_request_half(&mut w, request_line, request_headers)
                                        .await?;

                                    while let Some(next) = self.downstream.0.next().await {
                                        let next = next?;

                                        match next {
                                            HttpFrame::CompleteBody(body) => {
                                                body.write_to(&mut w).await?;
                                                w.flush().await?;

                                                let mut r = {
                                                    let codec =
                                                        HttpCodec::new(Flags::RESPONSE, None, None);
                                                    FramedRead::with_capacity(
                                                        r,
                                                        codec,
                                                        Self::BUFF_SIZE,
                                                    )
                                                };
                                                self.exchange(&mut r).await?;

                                                break;
                                            }
                                            HttpFrame::PartialBody(body) => {
                                                body.write_to(&mut w).await?;
                                                w.flush().await?;
                                            }
                                            _ => unreachable!(),
                                        }
                                    }
                                }
                            };
                        }
                        None => {
                            // TODO: 502 bad gateway
                            bail!("502 bad gateway!");
                        }
                    }
                }
                Ok(None) => {
                    debug!("outbound connection has no more data");
                    break;
                }
                Err(e) => {
                    error!("handshake failed: {}", e);
                    // TODO: cleanup
                    break;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::pipeline::PipelineConf;
    use crate::proto::{Listener, Signal};
    use crate::protocol::http::listener::listener::HttpListener;

    async fn init() {
        pretty_env_logger::try_init_timed().ok();
        crate::setup().await;
    }

    #[tokio::test]
    async fn test_http_listener() -> anyhow::Result<()> {
        init().await;

        use tokio::sync::mpsc;

        let (tx, mut rx) = mpsc::channel(1);

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

        tokio::spawn(async move {
            let _ = l.listen(&mut rx).await;
        });

        let _ = tokio::signal::ctrl_c().await;

        let _ = tx.send(Signal::Shutdown).await;

        Ok(())
    }
}
