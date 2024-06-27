use std::fmt::{Display, Formatter};
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::AsyncRead;
use tokio::net::TcpStream;

use crate::proto::UpstreamKey;
use crate::resolver::DEFAULT_RESOLVER;
use crate::transport::{tcp, tls};
use crate::Result;

pub(crate) enum ClientStream {
    Tcp(TcpStream),
    Tls(tls::TlsStream<TcpStream>),
}

impl Display for ClientStream {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientStream::Tcp(stream) => write!(f, "tcp://{}", stream.peer_addr().unwrap()),
            ClientStream::Tls(stream) => {
                let (c, _) = stream.get_ref();
                write!(f, "tls://{}", c.peer_addr().unwrap())
            }
        }
    }
}

pub(crate) async fn establish(upstream: &UpstreamKey, buff_size: usize) -> Result<ClientStream> {
    let stream = match upstream {
        UpstreamKey::Tcp(addr) => ClientStream::Tcp(
            tcp::TcpStreamBuilder::new(*addr)
                .buff_size(buff_size)
                .build()?,
        ),
        UpstreamKey::Tls(addr, sni) => {
            let stream = tcp::TcpStreamBuilder::new(*addr)
                .buff_size(buff_size)
                .build()?;
            let c = tls::TlsConnectorBuilder::new().build()?;
            ClientStream::Tls(c.connect(Clone::clone(sni), stream).await?)
        }
        UpstreamKey::TcpHP(domain, port) => {
            let ip = resolve(domain.as_ref()).await?;
            let addr = SocketAddr::new(ip, *port);
            ClientStream::Tcp(
                tcp::TcpStreamBuilder::new(addr)
                    .buff_size(buff_size)
                    .build()?,
            )
        }
        UpstreamKey::TlsHP(domain, port, sni) => {
            let ip = resolve(domain.as_ref()).await?;
            let addr = SocketAddr::new(ip, *port);
            let stream = tcp::TcpStreamBuilder::new(addr)
                .buff_size(buff_size)
                .build()?;
            let c = tls::TlsConnectorBuilder::new().build()?;
            let stream = c.connect(Clone::clone(sni), stream).await?;
            ClientStream::Tls(stream)
        }
    };

    debug!("establish {} ok: {}", upstream, &stream);

    Ok(stream)
}

#[inline]
async fn resolve(host: &str) -> Result<IpAddr> {
    if let Ok(addr) = host.parse::<IpAddr>() {
        return Ok(addr);
    }
    DEFAULT_RESOLVER.resolve_one(host).await
}
