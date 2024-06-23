use std::borrow::Cow;
use std::io;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("data store disconnected")]
    Disconnect(#[from] io::Error),
    #[error("invalid header (expected {expected:?}, found {found:?})")]
    InvalidHeader { expected: String, found: String },
    #[error("unknown internal error")]
    Unknown,

    #[error("invalid configuration '{0}'")]
    InvalidConfig(Cow<'static, str>),

    #[error("invalid route")]
    InvalidRoute,

    #[error("malformed http packet: {0}")]
    MalformedHttpPacket(/* reason */ Cow<'static, str>),

    #[error("request header size exceed {0} bytes")]
    ExceedMaxHttpHeaderSize(usize),

    #[error("request body size exceed {0} bytes")]
    ExceedMaxHttpBodySize(usize),

    #[error("malformed socks5 packet")]
    MalformedSocks5Packet,

    #[error("invalid connection")]
    InvalidConnection,

    #[error("invalid socks5 authentication")]
    InvalidSocks5Authentication,

    #[error("establish socks5 connection failed: {0}")]
    Socks5ConnectionFailure(/* message */ Cow<'static, str>),

    #[error("invalid properties of filter '{0}': {1}")]
    InvalidFilterConfig(/* property name */ Cow<'static, str>, anyhow::Error),

    #[error("invoke filter#{0} failed: {1}")]
    FilterExecutionFailure(/* filter index */ usize, anyhow::Error),

    #[error("invoke filter#{0} panic")]
    FilterExecutionPanic(/* filter index. */ usize),

    #[error("no address resolved from '{0}'")]
    NoAddressResolved(/* domain */ Cow<'static, str>),

    #[error(transparent)]
    Other(#[from] anyhow::Error), // source and Display delegate to anyhow::Error
}
