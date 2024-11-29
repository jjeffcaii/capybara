use async_trait::async_trait;
use rustls::pki_types::ServerName;
use std::fmt::{Display, Formatter};
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

use capybara_util::cachestr::Cachestr;

use crate::{CapybaraError, Result};

#[derive(Clone, Hash, Eq, PartialEq)]
pub enum UpstreamKey {
    Tcp(Addr),
    Tls(Addr),
    Tag(Cachestr),
}

#[derive(Clone, Hash, Eq, PartialEq)]
pub enum Addr {
    SocketAddr(SocketAddr),
    Host(Cachestr, u16),
}

impl Addr {
    fn parse_from(s: &str, default_port: Option<u16>) -> Result<Self> {
        let (host, port) = host_and_port(s)?;

        let port = match port {
            None => {
                default_port.ok_or_else(|| CapybaraError::InvalidUpstream(s.to_string().into()))?
            }
            Some(port) => port,
        };

        if let Ok(addr) = host.parse::<IpAddr>() {
            return Ok(Addr::SocketAddr(SocketAddr::new(addr, port)));
        }

        Ok(Addr::Host(Cachestr::from(host), port))
    }
}

impl Display for Addr {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Addr::SocketAddr(addr) => write!(f, "{}", addr),
            Addr::Host(host, port) => write!(f, "{}:{}", host, port),
        }
    }
}

#[inline]
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

        fn to_sni(sni: &str) -> Result<ServerName<'static>> {
            ServerName::try_from(sni)
                .map_err(|_| CapybaraError::InvalidTlsSni(sni.to_string().into()))
                .map(|it| it.to_owned())
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
            let addr = Addr::parse_from(suffix, None)?;
            return Ok(UpstreamKey::Tcp(addr));
        }

        if let Some(suffix) = s.strip_prefix("tls://") {
            let addr = Addr::parse_from(suffix, Some(443))?;
            return Ok(UpstreamKey::Tls(addr));
        }

        if let Some(suffix) = s.strip_prefix("http://") {
            let addr = Addr::parse_from(suffix, Some(80))?;
            return Ok(UpstreamKey::Tcp(addr));
        }

        if let Some(suffix) = s.strip_prefix("https://") {
            let addr = Addr::parse_from(suffix, Some(443))?;
            return Ok(UpstreamKey::Tls(addr));
        }

        let (host, port) = host_and_port(s)?;
        let port = port.ok_or_else(|| CapybaraError::InvalidUpstream(s.to_string().into()))?;
        let addr = match host.parse::<IpAddr>() {
            Ok(ip) => Addr::SocketAddr(SocketAddr::new(ip, port)),
            Err(_) => Addr::Host(Cachestr::from(host), port),
        };

        Ok(UpstreamKey::Tcp(addr))
    }
}

impl Display for UpstreamKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            UpstreamKey::Tcp(addr) => write!(f, "tcp://{}", addr),
            UpstreamKey::Tls(addr) => write!(f, "tls://{}", addr),
            UpstreamKey::Tag(tag) => write!(f, "upstream://{}", tag),
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
            ("https://example.com", "tls://example.com:443"),
            // schema+host+port
            ("tcp://localhost:8080", "tcp://localhost:8080"),
            ("tls://localhost:8443", "tls://localhost:8443"),
            ("http://localhost:8080", "tcp://localhost:8080"),
            ("https://localhost:8443", "tls://localhost:8443"),
        ] {
            assert!(s.parse::<UpstreamKey>().is_ok_and(|it| {
                let actual = it.to_string();
                actual == expect
            }));
        }
    }
}
