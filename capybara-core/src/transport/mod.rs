use std::fmt::{Display, Formatter};
use std::net::SocketAddr;

pub use tcp::TcpListenerBuilder;
pub use tls::{TlsAcceptorBuilder, TlsConnectorBuilder};

use crate::cachestr::Cachestr;

pub mod tcp;
pub mod tls;

#[derive(Clone)]
pub(super) enum Address {
    Direct(SocketAddr),
    Domain(Cachestr, u16),
}

impl Display for Address {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Address::Direct(addr) => write!(f, "{}", addr),
            Address::Domain(domain, port) => write!(f, "{}:{}", domain, port),
        }
    }
}
