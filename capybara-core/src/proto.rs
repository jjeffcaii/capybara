use std::fmt::{Display, Formatter};
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

use async_trait::async_trait;
use rustls::ServerName;

use crate::cachestr::Cachestr;
use crate::{CapybaraError, Result};

#[derive(Clone, Hash, Eq, PartialEq)]
pub enum UpstreamKey {
    Tcp(SocketAddr),
    Tls(SocketAddr, ServerName),
    TcpHP(Cachestr, u16),
    TlsHP(Cachestr, u16, ServerName),
    Tag(Cachestr),
}

impl FromStr for UpstreamKey {
    type Err = CapybaraError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        // 1. ip+port: 1.2.3.4:8080
        // 2. host+port: example.com:80
        // 3. with schema: tcp://1.2.3.4:8080, tls://example.com:8443, http://1.2.3.4, https://example.com
        // TODO: 4. identify: my_upstream

        fn is_tls_port(port: u16) -> bool {
            port == 443
        }

        fn host_and_port(s: &str) -> Result<(&str, Option<u16>)> {
            let mut sp = s.splitn(2, ':');

            match sp.next() {
                None => Err(CapybaraError::InvalidUpstream(s.to_string().into())),
                Some(first) => match sp.next() {
                    Some(second) => match second.parse::<u16>() {
                        Ok(port) => Ok((first, Some(port))),
                        Err(_) => Err(CapybaraError::InvalidUpstream(s.to_string().into())),
                    },
                    None => Ok((first, None)),
                },
            }
        }

        fn to_sni(sni: &str) -> Result<ServerName> {
            ServerName::try_from(sni)
                .map_err(|_| CapybaraError::InvalidTlsSni(sni.to_string().into()))
        }

        // FIXME: too many duplicated codes

        if let Some(suffix) = s.strip_prefix("upstream://") {
            return if suffix.is_empty() {
                Err(CapybaraError::InvalidUpstream(s.to_string().into()))
            } else {
                Ok(UpstreamKey::Tag(Cachestr::from(suffix)))
            };
        }

        if let Some(suffix) = s.strip_prefix("tcp://") {
            let (host, port) = host_and_port(suffix)?;
            let port = port.ok_or_else(|| CapybaraError::InvalidUpstream(s.to_string().into()))?;
            return Ok(match host.parse::<IpAddr>() {
                Ok(ip) => UpstreamKey::Tcp(SocketAddr::new(ip, port)),
                Err(_) => UpstreamKey::TcpHP(Cachestr::from(host), port),
            });
        }

        if let Some(suffix) = s.strip_prefix("tls://") {
            let (host, port) = host_and_port(suffix)?;
            let port = port.ok_or_else(|| CapybaraError::InvalidUpstream(s.to_string().into()))?;
            return Ok(match host.parse::<IpAddr>() {
                Ok(ip) => UpstreamKey::Tls(SocketAddr::new(ip, port), ServerName::IpAddress(ip)),
                Err(_) => UpstreamKey::TlsHP(Cachestr::from(host), port, to_sni(host)?),
            });
        }

        if let Some(suffix) = s.strip_prefix("http://") {
            let (host, port) = host_and_port(suffix)?;
            let port = port.unwrap_or(80);
            return Ok(match host.parse::<IpAddr>() {
                Ok(ip) => UpstreamKey::Tcp(SocketAddr::new(ip, port)),
                Err(_) => UpstreamKey::TcpHP(Cachestr::from(host), port),
            });
        }

        if let Some(suffix) = s.strip_prefix("https://") {
            let (host, port) = host_and_port(suffix)?;
            let port = port.unwrap_or(443);
            return Ok(match host.parse::<IpAddr>() {
                Ok(ip) => UpstreamKey::Tls(SocketAddr::new(ip, port), ServerName::IpAddress(ip)),
                Err(_) => UpstreamKey::TlsHP(Cachestr::from(host), port, to_sni(host)?),
            });
        }

        let (host, port) = host_and_port(s)?;
        let port = port.ok_or_else(|| CapybaraError::InvalidUpstream(s.to_string().into()))?;
        Ok(match host.parse::<IpAddr>() {
            Ok(ip) => UpstreamKey::Tcp(SocketAddr::new(ip, port)),
            Err(_) => UpstreamKey::TcpHP(Cachestr::from(host), port),
        })
    }
}

impl Display for UpstreamKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            UpstreamKey::Tcp(addr) => write!(f, "tcp://{}", addr),
            UpstreamKey::Tls(addr, sni) => {
                if let ServerName::DnsName(name) = sni {
                    return write!(f, "tls://{}?sni={}", addr, name.as_ref());
                }
                write!(f, "tls://{}", addr)
            }
            UpstreamKey::TcpHP(addr, port) => write!(f, "tcp://{}:{}", addr, port),
            UpstreamKey::TlsHP(addr, port, sni) => {
                if let ServerName::DnsName(name) = sni {
                    return write!(f, "tls://{}:{}?sni={}", addr, port, name.as_ref());
                }
                write!(f, "tls://{}:{}", addr, port)
            }
            UpstreamKey::Tag(tag) => write!(f, "upstream://{}", tag.as_ref()),
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
    fn id(&self) -> &str;
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

        for (s, expect) in [
            ("upstream://some-upstream", "upstream://some-upstream"),
            // ip+port
            ("127.0.0.1:8080", "tcp://127.0.0.1:8080"),
            // host+port
            ("example.com:8080", "tcp://example.com:8080"),
            // schema+ip
            ("http://127.0.0.1", "tcp://127.0.0.1:80"),
            ("https://127.0.0.1", "tls://127.0.0.1:443"),
            // schema+ip+port
            ("tcp://127.0.0.1:8080", "tcp://127.0.0.1:8080"),
            ("tls://127.0.0.1:8443", "tls://127.0.0.1:8443"),
            ("http://127.0.0.1:8080", "tcp://127.0.0.1:8080"),
            ("https://127.0.0.1:8443", "tls://127.0.0.1:8443"),
            // schema+host
            ("http://example.com", "tcp://example.com:80"),
            (
                "https://example.com",
                "tls://example.com:443?sni=example.com",
            ),
            // schema+host+port
            ("tcp://localhost:8080", "tcp://localhost:8080"),
            ("tls://localhost:8443", "tls://localhost:8443?sni=localhost"),
            ("http://localhost:8080", "tcp://localhost:8080"),
            (
                "https://localhost:8443",
                "tls://localhost:8443?sni=localhost",
            ),
        ] {
            assert!(s.parse::<UpstreamKey>().is_ok_and(|it| {
                let actual = it.to_string();
                actual == expect
            }));
        }
    }
}
