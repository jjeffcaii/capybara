use std::fmt::{Display, Formatter};
use std::net::SocketAddr;
use std::str::FromStr;

use async_trait::async_trait;
use rustls::ServerName;

use crate::cachestr::Cachestr;
use crate::{CapybaraError, Result};

#[derive(Clone, Hash, PartialEq)]
pub enum UpstreamKind {
    Tcp(SocketAddr),
    Tls(SocketAddr, ServerName),
    TcpHP(Cachestr, u16),
    TlsHP(Cachestr, u16, ServerName),
}

impl FromStr for UpstreamKind {
    type Err = CapybaraError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        fn is_tls_port(port: u16) -> bool {
            port == 443
        }

        if let Ok(addr) = s.parse::<SocketAddr>() {
            return Ok(if is_tls_port(addr.port()) {
                UpstreamKind::Tcp(addr)
            } else {
                UpstreamKind::Tls(addr, ServerName::IpAddress(addr.ip()))
            });
        }

        let mut sp = s.splitn(2, ':');

        match sp.next() {
            None => Err(CapybaraError::InvalidUpstream(s.to_string().into())),
            Some(first) => {
                let host = Cachestr::from(first);
                match sp.next() {
                    None => Ok(UpstreamKind::TcpHP(host, 80)),
                    Some(second) => {
                        let port: u16 = second
                            .parse()
                            .map_err(|e| CapybaraError::InvalidUpstream(s.to_string().into()))?;
                        if is_tls_port(port) {
                            let sni = ServerName::try_from(first).map_err(|_| {
                                CapybaraError::InvalidUpstream(s.to_string().into())
                            })?;
                            Ok(UpstreamKind::TlsHP(host, port, sni))
                        } else {
                            Ok(UpstreamKind::TcpHP(host, port))
                        }
                    }
                }
            }
        }
    }
}

impl Display for UpstreamKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            UpstreamKind::Tcp(addr) => write!(f, "tcp://{}", addr),
            UpstreamKind::Tls(addr, sni) => write!(f, "tls://{}?sni={:?}", addr, sni),
            UpstreamKind::TcpHP(addr, port) => write!(f, "tcp://{}:{}", addr, port),
            UpstreamKind::TlsHP(addr, port, sni) => {
                write!(f, "tls://{}:{}?sni={:?}", addr, port, sni)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Signal {
    Shutdown,
    Reload,
}

pub type Signals = tokio::sync::mpsc::Receiver<Signal>;

#[async_trait]
pub trait Listener: Send + Sync + 'static {
    async fn listen(&self, signals: &mut Signals) -> Result<()>;
}

#[async_trait]
pub trait Configurable<T>: Send + Sync + 'static {
    async fn configure(&self, c: T) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[test]
    fn test_upstream_kind() {
        init();

        let u = UpstreamKind::TlsHP(
            Cachestr::from("example.com"),
            443,
            ServerName::try_from("example.com").unwrap(),
        );
        info!("upstream: {}", &u);

        let res = "example.com:443".parse::<UpstreamKind>();
        assert!(res.is_ok_and(|it| "tls://example.com:443" == &it.to_string()))
    }
}
