use std::borrow::Cow;
use std::io;

#[derive(thiserror::Error, Debug)]
pub enum CapybaraError {
    #[error("invalid HPACK index {0}")]
    InvalidHPackIndex(u8),

    #[error("invalid huffman code")]
    InvalidHuffmanCode,

    #[error("data store disconnected")]
    Disconnect(#[from] io::Error),
    #[error("invalid header (expected {expected:?}, found {found:?})")]
    InvalidHeader { expected: String, found: String },
    #[error("unknown internal error")]
    Unknown,

    #[error("invalid configuration '{0}'")]
    InvalidConfig(Cow<'static, str>),

    #[error("invalid tls configuration '{0}'")]
    InvalidTlsConfig(Cow<'static, str>),

    #[error("invalid tls sni '{0}'")]
    InvalidTlsSni(Cow<'static, str>),

    #[error("malformed tls config: {0}")]
    MalformedTlsConfig(anyhow::Error),

    #[error("invalid route")]
    InvalidRoute,

    #[error("invalid address '{0}'")]
    InvalidAddress(/* address */ Cow<'static, str>),

    #[error("invalid upstream pool")]
    InvalidUpstreamPool,

    #[error("malformed http packet: {0}")]
    MalformedHttpPacket(/* reason */ Cow<'static, str>),

    #[error("request header size exceed {0} bytes")]
    ExceedMaxHttpHeaderSize(usize),

    #[error("request body size exceed {0} bytes")]
    ExceedMaxHttpBodySize(usize),

    #[error("invalid connection")]
    InvalidConnection,

    #[error("invalid properties of pipeline '{0}': {1}")]
    InvalidPipelineConfig(/* property name */ Cow<'static, str>, anyhow::Error),

    #[error("invoke filter#{0} failed: {1}")]
    FilterExecutionFailure(/* filter index */ usize, anyhow::Error),

    #[error("no address resolved from '{0}'")]
    NoAddressResolved(/* domain */ Cow<'static, str>),

    #[error("cannot parse upstream from '{0}'")]
    InvalidUpstream(Cow<'static, str>),

    #[error("operation timeout")]
    Timeout,

    #[error(transparent)]
    Other(#[from] anyhow::Error), // source and Display delegate to anyhow::Error
}
