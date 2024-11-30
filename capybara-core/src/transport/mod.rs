use std::fmt::Display;

pub use tcp::TcpListenerBuilder;
pub use tls::{TlsAcceptorBuilder, TlsConnectorBuilder};

pub mod tcp;
pub mod tls;

pub type Address = crate::proto::Addr;

pub trait Addressable {
    fn address(&self) -> &Address;
}
