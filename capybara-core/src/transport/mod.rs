use std::fmt::{Display, Formatter};
use std::net::SocketAddr;

use capybara_util::cachestr::Cachestr;
pub use tcp::TcpListenerBuilder;
pub use tls::{TlsAcceptorBuilder, TlsConnectorBuilder};

pub mod tcp;
pub mod tls;

pub trait Addressable {
    fn address(&self) -> &Address;
}

#[derive(Clone)]
pub enum Address {
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
