use rustls::pki_types::ServerName;
use std::fmt::{Display, Formatter};
use std::net::{IpAddr, SocketAddr};
use tokio::net::TcpStream;

use crate::proto::{Addr, UpstreamKey};
use crate::resolver::DEFAULT_RESOLVER;
use crate::transport::{tcp, tls};
use crate::{CapybaraError, Result};

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
        UpstreamKey::Tcp(addr) => match addr {
            Addr::SocketAddr(addr) => ClientStream::Tcp(
                tcp::TcpStreamBuilder::new(*addr)
                    .buff_size(buff_size)
                    .build()?,
            ),
            Addr::Host(host, port) => {
                let ip = resolve(host.as_ref()).await?;
                let addr = SocketAddr::new(ip, *port);
                ClientStream::Tcp(
                    tcp::TcpStreamBuilder::new(addr)
                        .buff_size(buff_size)
                        .build()?,
                )
            }
        },
        UpstreamKey::Tls(addr) => {
            match addr {
                Addr::SocketAddr(addr) => {
                    let stream = tcp::TcpStreamBuilder::new(*addr)
                        .buff_size(buff_size)
                        .build()?;
                    let c = tls::TlsConnectorBuilder::new().build()?;

                    let sni = ServerName::from(addr.ip());
                    ClientStream::Tls(c.connect(sni, stream).await?)
                }
                Addr::Host(host, port) => {
                    let ip = resolve(host.as_ref()).await?;
                    let addr = SocketAddr::new(ip, *port);
                    let stream = tcp::TcpStreamBuilder::new(addr)
                        .buff_size(buff_size)
                        .build()?;
                    let c = tls::TlsConnectorBuilder::new().build()?;
                    // TODO: how to reduce creating times of sni?
                    let sni = ServerName::try_from(host.as_ref())
                        .map_err(|e| CapybaraError::Other(e.into()))?
                        .to_owned();
                    let stream = c.connect(sni, stream).await?;
                    ClientStream::Tls(stream)
                }
            }
        }
        UpstreamKey::Tag(tag) => {
            todo!("establish with tag is not supported yet")
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
