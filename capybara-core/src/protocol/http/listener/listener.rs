use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;

use anyhow::Error;
use arc_swap::Cache;
use futures::StreamExt;
use tokio::io::{AsyncRead, AsyncWrite, BufWriter, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_util::codec::FramedRead;
use uuid::uuid;

use crate::cachestr::Cachestr;
use crate::proto::{Listener, Signal, SignalReceiver};
use crate::protocol::http::codec::Flags;
use crate::protocol::http::{HttpCodec, HttpFrame};
use crate::transport::TcpListenerBuilder;
use crate::Result;

pub struct HttpListenerBuilder {
    addr: SocketAddr,
    id: Option<Cachestr>,
}

impl HttpListenerBuilder {
    pub fn id<I>(mut self, id: I) -> Self
    where
        I: AsRef<str>,
    {
        self.id.replace(Cachestr::from(id.as_ref()));
        self
    }

    pub fn build(self) -> Result<HttpListener> {
        let Self { addr, id } = self;

        Ok(HttpListener {
            id: id.unwrap_or_else(|| Cachestr::from(uuid::Uuid::new_v4().to_string())),
            addr,
        })
    }
}

pub struct HttpListener {
    id: Cachestr,
    addr: SocketAddr,
}

impl HttpListener {
    pub fn builder(addr: SocketAddr) -> HttpListenerBuilder {
        HttpListenerBuilder { addr, id: None }
    }
}

impl Listener for HttpListener {
    async fn listen(&self, signal_receiver: &mut SignalReceiver) -> Result<()> {
        let l = TcpListenerBuilder::new(self.addr).build()?;
        let closer = Arc::new(Notify::new());

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
                        }
                    }

                }
                accept = l.accept() => {
                    let (stream, addr) = accept?;
                    info!("accept a new tcp stream {:?}", &addr);
                    let mut handler = Handler::new(stream, addr, Clone::clone(&closer));
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
    addr: SocketAddr,
    downstream: (FramedRead<ReadHalf<S>, HttpCodec>, BufWriter<WriteHalf<S>>),
}

impl<S> Handler<S> {
    const BUFF_SIZE: usize = 8192; // 8KB
}

impl<S> Handler<S>
where
    S: AsyncWrite + AsyncRead + Sync + Send + 'static,
{
    fn new(stream: S, addr: SocketAddr, closer: Arc<Notify>) -> Self {
        let (rh, wh) = tokio::io::split(stream);

        let downstream = (
            {
                let f = Flags::default();
                FramedRead::with_capacity(rh, HttpCodec::new(f, None, None), Self::BUFF_SIZE)
            },
            BufWriter::with_capacity(Self::BUFF_SIZE, wh),
        );

        Self { addr, downstream }
    }

    async fn handle(&mut self) -> anyhow::Result<()> {
        loop {
            match self.downstream.0.next().await {
                Some(next) => {
                    let next = next?;

                    match next {
                        HttpFrame::RequestLine(_) => {}
                        HttpFrame::Headers(_) => {}
                        HttpFrame::CompleteBody(_) => {}
                        HttpFrame::PartialBody(_) => {}
                        _ => unreachable!(),
                    }

                    info!("next http frame: {:?}", next);
                }
                None => {
                    debug!("no more frame for {:?}", &self.addr);
                    break;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::proto::{Listener, Signal};
    use crate::protocol::http::listener::listener::HttpListener;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[tokio::test]
    async fn test_http_listener() -> anyhow::Result<()> {
        init();

        use tokio::sync::mpsc;

        let (tx, mut rx) = mpsc::channel(1);

        let l = HttpListener::builder("127.0.0.1:8080".parse()?).build()?;

        tokio::spawn(async move {
            let _ = l.listen(&mut rx).await;
        });

        let _ = tx.send(Signal::Shutdown).await;

        Ok(())
    }
}
