use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;

use anyhow::Error;
use arc_swap::{ArcSwap, Cache};
use futures::StreamExt;
use tokio::io::{AsyncRead, AsyncWrite, BufWriter, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_util::codec::FramedRead;
use uuid::uuid;

use crate::cachestr::Cachestr;
use crate::pipeline::http::{load, HttpPipelineFactoryExt};
use crate::pipeline::{HttpContext, HttpPipeline, HttpPipelineFactory, PipelineConf};
use crate::proto::{Listener, Signal, SignalReceiver};
use crate::protocol::http::codec::Flags;
use crate::protocol::http::{HttpCodec, HttpFrame};
use crate::transport::TcpListenerBuilder;
use crate::Result;

pub struct HttpListenerBuilder {
    addr: SocketAddr,
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

    pub fn build(self) -> Result<HttpListener> {
        let Self {
            addr,
            id,
            pipelines,
        } = self;

        Ok(HttpListener {
            id: id.unwrap_or_else(|| Cachestr::from(uuid::Uuid::new_v4().to_string())),
            addr,
            pipelines: ArcSwap::from_pointee(pipelines),
        })
    }
}

pub struct HttpListener {
    id: Cachestr,
    addr: SocketAddr,
    pipelines: ArcSwap<Vec<(Cachestr, PipelineConf)>>,
}

impl HttpListener {
    pub fn builder(addr: SocketAddr) -> HttpListenerBuilder {
        HttpListenerBuilder {
            addr,
            id: None,
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

impl Listener for HttpListener {
    async fn listen(&self, signal_receiver: &mut SignalReceiver) -> Result<()> {
        let l = TcpListenerBuilder::new(self.addr).build()?;
        let closer = Arc::new(Notify::new());

        let mut pipelines = self.build_pipeline_factories()?;

        loop {
            tokio::select! {
                signal = signal_receiver.recv() => {
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
                    info!("accept a new tcp stream {:?}", &addr);
                    let ctx = Self::build_context(addr,&pipelines[..])?;
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

    async fn handle(&mut self) -> anyhow::Result<()> {
        loop {
            match self.downstream.0.next().await {
                Some(next) => {
                    let mut next = next?;

                    match &mut next {
                        HttpFrame::RequestLine(request_line) => {
                            if let Some(p) = self.ctx.reset_pipeline() {
                                p.handle_request_line(&self.ctx, request_line).await?;
                            }
                        }
                        HttpFrame::Headers(headers) => {
                            if let Some(p) = self.ctx.reset_pipeline() {
                                p.handle_request_headers(&self.ctx, headers).await?;
                            }
                        }
                        HttpFrame::CompleteBody(_) => {}
                        HttpFrame::PartialBody(_) => {}
                        _ => unreachable!(),
                    }

                    info!("next http frame: {:?}", &next);
                }
                None => {
                    debug!("no more frame for {:?}", self.ctx.client_addr());
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
            id: 123
            "#;
            serde_yaml::from_str(s).unwrap()
        };

        let l = HttpListener::builder("127.0.0.1:8080".parse()?)
            .id("fake-http-listener")
            .pipeline("capybara.pipelines.http.noop", &c)
            .build()?;

        tokio::spawn(async move {
            let _ = l.listen(&mut rx).await;
        });

        let _ = tokio::signal::ctrl_c().await;

        let _ = tx.send(Signal::Shutdown).await;

        Ok(())
    }
}
