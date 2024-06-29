use std::net::SocketAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use tokio::net::TcpStream;
use tokio::sync::Notify;

use crate::cachestr::Cachestr;
use crate::error::CapybaraError;
use crate::pipeline::stream::load;
use crate::pipeline::stream::StreamPipelineFactoryExt;
use crate::pipeline::{PipelineConf, StreamContext};
use crate::proto::{Listener, Signal, Signals};
use crate::resolver::DEFAULT_RESOLVER;
use crate::transport::TcpListenerBuilder;
use crate::upstream::ClientStream;

pub struct StreamListenerBuilder {
    addr: SocketAddr,
    id: Option<Cachestr>,
    pipelines: Vec<(Cachestr, PipelineConf)>,
}

impl StreamListenerBuilder {
    pub fn id<A>(mut self, id: A) -> Self
    where
        A: AsRef<str>,
    {
        self.id.replace(Cachestr::from(id.as_ref()));
        self
    }

    pub fn pipeline<N>(mut self, name: N, c: &PipelineConf) -> Self
    where
        N: AsRef<str>,
    {
        self.pipelines
            .push((Cachestr::from(name.as_ref()), Clone::clone(c)));
        self
    }

    pub fn build(self) -> crate::Result<StreamListener> {
        let Self {
            addr,
            id,
            pipelines,
        } = self;

        Ok(StreamListener {
            id: id.unwrap_or_else(|| Cachestr::from(uuid::Uuid::new_v4().to_string())),
            addr,
            pipelines: ArcSwap::from_pointee(pipelines),
        })
    }
}

pub struct StreamListener {
    id: Cachestr,
    addr: SocketAddr,
    pipelines: ArcSwap<Vec<(Cachestr, PipelineConf)>>,
}

impl StreamListener {
    pub fn builder(addr: SocketAddr) -> StreamListenerBuilder {
        StreamListenerBuilder {
            id: None,
            addr,
            pipelines: vec![],
        }
    }

    #[inline]
    fn build_pipeline_factories(&self) -> anyhow::Result<Vec<Box<dyn StreamPipelineFactoryExt>>> {
        let r = self.pipelines.load();

        let mut factories = Vec::with_capacity(r.len());
        for (k, v) in r.iter() {
            let factory = load(k, v)?;
            factories.push(factory);
        }

        Ok(factories)
    }

    fn build_context(
        client_addr: SocketAddr,
        factories: &[Box<dyn StreamPipelineFactoryExt>],
    ) -> anyhow::Result<StreamContext> {
        let mut b = StreamContext::builder(client_addr);

        for factory in factories {
            let next = factory.generate_arc()?;
            b = b.pipeline_arc(next);
        }

        Ok(b.build())
    }
}

#[async_trait]
impl Listener for StreamListener {
    async fn listen(&self, signals: &mut Signals) -> crate::Result<()> {
        let l = TcpListenerBuilder::new(self.addr).build()?;

        info!("'{}' is listening on {}", &self.id, &self.addr);

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
                    let (stream,addr) = accept?;

                    let ctx = Self::build_context(addr,&pipelines[..])?;
                    let closer = Clone::clone(&closer);

                    let mut h = Handler::new(ctx,stream);
                    tokio::spawn(async move {
                        if let Err(e) = h.handle(closer).await{
                            error!("stream handler occurs an error: {}", e);
                        }
                    });
                }
            }
        }
    }
}

struct Handler {
    ctx: StreamContext,
    downstream: TcpStream,
}

impl Handler {
    const BUFF_SIZE: usize = 8192;

    fn new(ctx: StreamContext, downstream: TcpStream) -> Self {
        Self { ctx, downstream }
    }

    async fn resolve(upstream: &str) -> anyhow::Result<SocketAddr> {
        if let Ok(addr) = upstream.parse::<SocketAddr>() {
            return Ok(addr);
        }

        let host_and_port = upstream.split(':').collect::<Vec<&str>>();
        if host_and_port.len() != 2 {
            bail!(CapybaraError::NoAddressResolved(
                upstream.to_string().into()
            ))
        }
        let port: u16 = host_and_port.last().unwrap().parse()?;
        let ip = DEFAULT_RESOLVER
            .resolve_one(host_and_port.first().unwrap())
            .await?;

        Ok(SocketAddr::new(ip, port))
    }

    async fn handle(&mut self, _closer: Arc<Notify>) -> anyhow::Result<()> {
        if let Some(p) = self.ctx.pipeline() {
            p.handle_connect(&mut self.ctx).await?;
        }

        let mut upstream = match self.ctx.upstream() {
            None => bail!(CapybaraError::InvalidRoute),
            Some(upstream) => crate::upstream::establish(&upstream, Self::BUFF_SIZE).await?,
        };

        use tokio::io::copy_bidirectional_with_sizes as copy;

        let (in_bytes, out_bytes) = match &mut upstream {
            ClientStream::Tcp(inner) => {
                copy(
                    &mut self.downstream,
                    inner,
                    Self::BUFF_SIZE,
                    Self::BUFF_SIZE,
                )
                .await?
            }
            ClientStream::Tls(inner) => {
                copy(
                    &mut self.downstream,
                    inner,
                    Self::BUFF_SIZE,
                    Self::BUFF_SIZE,
                )
                .await?
            }
        };

        debug!("copy bidirectional ok: in={}, out={}", in_bytes, out_bytes);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn init() {
        pretty_env_logger::try_init_timed().ok();
        crate::setup().await;
    }

    #[tokio::test]
    async fn test_stream_listener() -> anyhow::Result<()> {
        init().await;

        let c: PipelineConf = {
            // language=yaml
            let s = r#"
            upstream: 'httpbin.org:80'
            "#;

            serde_yaml::from_str(s).unwrap()
        };

        let (_tx, mut rx) = tokio::sync::mpsc::channel(1);

        let l = StreamListener::builder("127.0.0.1:9999".parse().unwrap())
            .id("fake-stream-listener")
            .pipeline("capybara.pipelines.stream.router", &c)
            .build()?;

        l.listen(&mut rx).await?;

        Ok(())
    }
}
