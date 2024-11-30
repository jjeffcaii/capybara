use async_trait::async_trait;
use capybara_util::cachestr::Cachestr;
use once_cell::sync::Lazy;
use std::fmt::{Display, Formatter};
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

use crate::{CapybaraError, Result};

pub const PORT_HTTP: u16 = 80;
pub const PORT_TLS: u16 = 443;

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum Addr {
    SocketAddr(SocketAddr),
    Host(/* host */ Cachestr, /* port */ u16),
}

impl Addr {
    #[inline]
    fn parse_host_port(s: &str) -> Option<(&str, Option<u16>)> {
        let mut res = None;
        let mut sp = s.splitn(2, ':');
        if let Some(first) = sp.next() {
            let mut port = None;

            if let Some(second) = sp.next() {
                match second.parse::<u16>() {
                    Ok(n) => {
                        port = Some(n);
                    }
                    Err(_) => return None,
                }
            }

            res = Some((first, port));
        }

        res
    }

    #[inline]
    fn parse_with_port(s: &str, default_port: u16) -> Option<Self> {
        match Self::parse_host_port(s) {
            None => None,
            Some((host, port)) => {
                let port = port.unwrap_or(default_port);
                if port < 1 {
                    return None;
                }

                // return if target is a valid ip
                if let Ok(addr) = host.parse::<IpAddr>() {
                    return Some(Addr::SocketAddr(SocketAddr::new(addr, port)));
                }

                // return if the target is a valid domain
                if is_valid_domain(host) {
                    return Some(Addr::Host(Cachestr::from(host), port));
                }

                None
            }
        }
    }
}

impl FromStr for Addr {
    type Err = CapybaraError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Self::parse_with_port(s, 0)
            .ok_or_else(|| CapybaraError::InvalidAddress(s.to_string().into()))
    }
}

impl From<SocketAddr> for Addr {
    fn from(value: SocketAddr) -> Self {
        Self::SocketAddr(value)
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

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum UpstreamKey {
    /// TCP address
    Tcp(Addr),
    /// TLS address
    Tls(Addr),
    /// a tag which links to an upstream
    Tag(Cachestr),
}

impl FromStr for UpstreamKey {
    type Err = CapybaraError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        // 1. ip+port: 1.2.3.4:8080
        // 2. host+port: example.com:80
        // 3. with schema: tcp://1.2.3.4:8080, tls://example.com:8443, http://1.2.3.4, https://example.com
        // TODO: 4. identify: my_upstream

        // FIXME: too many duplicated codes
        if let Some(suffix) = s.strip_prefix("upstream://") {
            return if is_valid_identify(suffix) {
                Ok(UpstreamKey::Tag(Cachestr::from(suffix)))
            } else {
                Err(CapybaraError::InvalidUpstream(s.to_string().into()))
            };
        }

        if let Some(suffix) = s.strip_prefix("tcp://") {
            let addr = Addr::parse_with_port(suffix, 0)
                .ok_or_else(|| CapybaraError::InvalidUpstream(s.to_string().into()))?;
            return Ok(UpstreamKey::Tcp(addr));
        }

        if let Some(suffix) = s.strip_prefix("tls://") {
            let addr = Addr::parse_with_port(suffix, PORT_TLS)
                .ok_or_else(|| CapybaraError::InvalidUpstream(s.to_string().into()))?;
            return Ok(UpstreamKey::Tls(addr));
        }

        if let Some(suffix) = s.strip_prefix("http://") {
            let addr = Addr::parse_with_port(suffix, PORT_HTTP)
                .ok_or_else(|| CapybaraError::InvalidUpstream(s.to_string().into()))?;
            return Ok(UpstreamKey::Tcp(addr));
        }

        if let Some(suffix) = s.strip_prefix("https://") {
            let addr = Addr::parse_with_port(suffix, PORT_TLS)
                .ok_or_else(|| CapybaraError::InvalidUpstream(s.to_string().into()))?;
            return Ok(UpstreamKey::Tls(addr));
        }

        if let Some(addr) = Addr::parse_with_port(s, 0) {
            return Ok(UpstreamKey::Tcp(addr));
        }

        if is_valid_identify(s) {
            return Ok(UpstreamKey::Tag(Cachestr::from(s)));
        }

        Err(CapybaraError::InvalidUpstream(s.to_string().into()))
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

fn is_valid_domain(s: &str) -> bool {
    static DOMAIN_REGEXP: Lazy<regex::Regex> =
        Lazy::new(|| regex::Regex::new("^[a-zA-Z0-9-]*(\\.[a-zA-Z0-9-]*)*$").unwrap());

    DOMAIN_REGEXP.is_match(s)
}

fn is_valid_identify(s: &str) -> bool {
    static IDENTIFY_REGEXP: Lazy<regex::Regex> =
        Lazy::new(|| regex::Regex::new("^[a-zA-Z][a-zA-Z0-9_-]*$").unwrap());

    IDENTIFY_REGEXP.is_match(s)
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
            // tag
            ("some-upstream", "upstream://some-upstream"),
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
            ("http://localhost", "tcp://localhost:80"),
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
