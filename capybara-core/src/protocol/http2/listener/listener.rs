use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Error;
use arc_swap::ArcSwap;
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite, BufWriter, ReadHalf, WriteHalf};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::{mpsc, Notify};
use tokio_rustls::TlsAcceptor;
use tokio_util::codec::{FramedRead, FramedWrite};

use capybara_util::cachestr::Cachestr;

use crate::proto::{Listener, Signals};
use crate::protocol::http::HttpCodec;
use crate::protocol::http2::codec::Http2Codec;
use crate::protocol::http2::frame::{self, Frame, Settings, WindowUpdate};
use crate::transport::tcp;
use crate::Result;

#[derive(Default)]
struct Config {}

pub struct Http2ListenerBuilder {
    id: Option<Cachestr>,
    addr: SocketAddr,
    tls: Option<TlsAcceptor>,
    cfg: Config,
}

impl Http2ListenerBuilder {
    pub fn id(mut self, id: &str) -> Self {
        self.id.replace(Cachestr::from(id));
        self
    }

    pub fn tls(mut self, tls: TlsAcceptor) -> Self {
        self.tls.replace(tls);
        self
    }

    pub fn build(self) -> Result<Http2Listener> {
        let Self { id, addr, tls, cfg } = self;

        let closer = Arc::new(Notify::new());

        Ok(Http2Listener {
            id: id.unwrap_or_else(|| Cachestr::from(uuid::Uuid::new_v4().to_string())),
            tls,
            addr,
            closer,
            cfg: ArcSwap::from_pointee(cfg),
        })
    }
}

pub struct Http2Listener {
    id: Cachestr,
    addr: SocketAddr,
    tls: Option<TlsAcceptor>,
    closer: Arc<Notify>,
    cfg: ArcSwap<Config>,
}

impl Http2Listener {
    pub fn builder(addr: SocketAddr) -> Http2ListenerBuilder {
        Http2ListenerBuilder {
            id: None,
            tls: None,
            addr,
            cfg: Default::default(),
        }
    }
}

#[async_trait]
impl Listener for Http2Listener {
    fn id(&self) -> &str {
        self.id.as_ref()
    }

    async fn listen(&self, signals: &mut Signals) -> Result<()> {
        let l = tcp::TcpListenerBuilder::new(self.addr).build()?;
        info!("listener '{}' is listening on {:?}", &self.id, &self.addr);

        loop {
            let (stream, addr) = l.accept().await?;
            info!("accept a new http2 connection {:?}", &addr);

            let mut conn = Connection::new(stream);

            tokio::spawn(async move {
                if let Err(e) = conn.start_read().await {
                    error!("stopped: {}", e);
                }
            });
        }
    }
}

struct Connection<S> {
    downstream: (
        FramedRead<ReadHalf<S>, Http2Codec>,
        FramedWrite<WriteHalf<S>, Http2Codec>,
    ),
}

impl<S> Connection<S>
where
    S: AsyncWrite + AsyncRead + Sync + Send + 'static,
{
    fn new(stream: S) -> Self {
        let (rh, wh) = tokio::io::split(stream);

        let r = FramedRead::with_capacity(rh, Http2Codec::default(), 8192);
        let w = FramedWrite::new(wh, Http2Codec::default());

        Self { downstream: (r, w) }
    }

    async fn handshake(&mut self) -> Result<Option<Handshake>> {
        if let Some(first) = self.downstream.0.next().await {
            if matches!(first?, Frame::Magic) {
                if let Some(second) = self.downstream.0.next().await {
                    if let Frame::Settings(metadata, settings) = second? {
                        if let Some(third) = self.downstream.0.next().await {
                            if let Frame::WindowUpdate(metadata, window_update) = third? {
                                let handshake = Handshake {
                                    settings,
                                    window_update,
                                };
                                return Ok(Some(handshake));
                            }
                        }
                    }
                }
            }
        }
        Ok(None)
    }

    async fn write(&mut self, next: &Frame) -> anyhow::Result<()> {
        self.downstream.1.send(next).await?;
        Ok(())
    }

    async fn start_read(&mut self) -> anyhow::Result<()> {
        if let Some(handshake) = self.handshake().await? {
            while let Some(next) = self.downstream.0.next().await {
                let next = next?;

                info!("incoming frame: {:?}", &next);

                // TODO: handle frames
                match &next {
                    Frame::Data(metadata, _) => {}
                    Frame::Settings(_, _) => {}
                    Frame::Headers(metadata, _) => {}
                    Frame::Priority(_, _) => {}
                    Frame::RstStream(_, _) => {}
                    Frame::Ping(_, _) => {}
                    Frame::Goaway(_, _) => {}
                    Frame::WindowUpdate(_, _) => {}
                    Frame::Continuation(_, _) => {}
                    Frame::PushPromise(_, _) => {}
                    _ => (),
                }
            }
        }
        Ok(())
    }
}

struct Handshake {
    settings: Settings,
    window_update: WindowUpdate,
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[tokio::test]
    async fn test_http2_listener() -> anyhow::Result<()> {
        init();

        let (tx, mut rx) = mpsc::channel(1);

        // tokio::sync::mpsc::Receiver< crate::proto::Signal >

        let l = Http2Listener::builder("127.0.0.1:15006".parse().unwrap()).build()?;
        l.listen(&mut rx).await?;

        Ok(())
    }
}
