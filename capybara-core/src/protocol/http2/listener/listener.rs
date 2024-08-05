use std::net::SocketAddr;

use anyhow::Error;
use async_trait::async_trait;
use futures::StreamExt;
use tokio::io::{AsyncRead, AsyncWrite, BufWriter, ReadHalf, WriteHalf};
use tokio_rustls::TlsAcceptor;
use tokio_util::codec::FramedRead;

use capybara_util::cachestr::Cachestr;

use crate::proto::{Listener, Signals};
use crate::protocol::http::HttpCodec;
use crate::protocol::http2::codec::Http2Codec;
use crate::protocol::http2::frame::{self, Frame, Settings, WindowUpdate};
use crate::transport::tcp;
use crate::Result;

pub struct Http2Listener {
    id: Cachestr,
    addr: SocketAddr,
    tls: Option<TlsAcceptor>,
}

#[async_trait]
impl Listener for Http2Listener {
    fn id(&self) -> &str {
        self.id.as_ref()
    }

    async fn listen(&self, signals: &mut Signals) -> crate::Result<()> {
        let l = tcp::TcpListenerBuilder::new(self.addr).build()?;
        info!("listener '{}' is listening on {:?}", &self.id, &self.addr);

        let (stream, addr) = l.accept().await?;
        debug!("accept a new http2 connection {:?}", &addr);

        let conn = Connection::new(stream);

        todo!()
    }
}

struct Connection<S> {
    downstream: (FramedRead<ReadHalf<S>, Http2Codec>, BufWriter<WriteHalf<S>>),
}

impl<S> Connection<S>
where
    S: AsyncWrite + AsyncRead + Sync + Send + 'static,
{
    fn new(stream: S) -> Self {
        let (rh, wh) = tokio::io::split(stream);

        let fr = FramedRead::with_capacity(rh, Http2Codec::default(), 8192);

        Self {
            downstream: (fr, BufWriter::with_capacity(8192, wh)),
        }
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

    async fn polling(&mut self) -> anyhow::Result<()> {
        if let Some(handshake) = self.handshake().await? {
            while let Some(next) = self.downstream.0.next().await {
                let next = next?;
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
