use bytes::{Buf, BytesMut};
use garde::rules::length::HasSimpleLength;
use tokio_util::codec::{Decoder, Encoder};

use super::frame::{Frame, FrameKind, Metadata, Ping, Priority, RstStream, Settings, WindowUpdate};
use super::hpack::Headers;

const MAGIC: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
const METADATA_SIZE: usize = 9;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum State {
    Magic,
    Metadata,
    Payload(usize),
}

impl Default for State {
    fn default() -> Self {
        Self::Magic
    }
}

#[derive(Default)]
pub struct Http2Codec {
    state: State,
    metadata: Option<Metadata>,
}

impl Http2Codec {
    pub fn without_magic() -> Http2Codec {
        Http2Codec {
            state: State::Metadata,
            metadata: None,
        }
    }
}

impl Http2Codec {
    fn read_frame_to(b: &mut BytesMut, to: usize, metadata: Metadata) -> Frame {
        let b = b.split_to(to).freeze();
        match metadata.kind() {
            FrameKind::Data => Frame::Data(metadata, b),
            FrameKind::Headers => Frame::Headers(metadata, Headers(b)),
            FrameKind::Priority => Frame::Priority(metadata, Priority(b)),
            FrameKind::RstStream => Frame::RstStream(metadata, RstStream(b)),
            FrameKind::Settings => Frame::Settings(metadata, Settings(b)),
            FrameKind::PushPromise => Frame::PushPromise(metadata, b),
            FrameKind::Ping => Frame::Ping(metadata, Ping(b)),
            FrameKind::Goaway => Frame::Goaway(metadata, b),
            FrameKind::WindowUpdate => Frame::WindowUpdate(metadata, WindowUpdate(b)),
            FrameKind::Continuation => Frame::Continuation(metadata, b),
        }
    }
}

impl Encoder<&Frame> for Http2Codec {
    type Error = anyhow::Error;

    fn encode(&mut self, item: &Frame, dst: &mut BytesMut) -> Result<(), Self::Error> {
        match item {
            Frame::Magic => {
                dst.reserve(MAGIC.len());
                dst.extend_from_slice(&MAGIC[..]);
            }
            Frame::Data(metadata, payload) => {
                dst.reserve(METADATA_SIZE + payload.len());
                dst.extend_from_slice(&metadata[..]);
                dst.extend_from_slice(&payload[..]);
            }
            Frame::Settings(metadata, payload) => {
                let b = &payload[..];
                dst.reserve(METADATA_SIZE + b.len());
                dst.extend_from_slice(&metadata[..]);
                dst.extend_from_slice(b);
            }
            Frame::Headers(metadata, payload) => {
                let b = &payload[..];
                dst.reserve(METADATA_SIZE + b.len());
                dst.extend_from_slice(&metadata[..]);
                dst.extend_from_slice(b);
            }
            Frame::Priority(metadata, payload) => {
                let b = &payload[..];
                dst.reserve(METADATA_SIZE + b.len());
                dst.extend_from_slice(&metadata[..]);
                dst.extend_from_slice(b);
            }
            Frame::RstStream(metadata, payload) => {
                let b = &payload[..];
                dst.reserve(METADATA_SIZE + b.len());
                dst.extend_from_slice(&metadata[..]);
                dst.extend_from_slice(b);
            }
            Frame::Ping(metadata, payload) => {
                let b = &payload[..];
                dst.reserve(METADATA_SIZE + payload.len());
                dst.extend_from_slice(&metadata[..]);
                dst.extend_from_slice(b);
            }
            Frame::Goaway(metadata, payload) => {
                dst.reserve(METADATA_SIZE + payload.len());
                dst.extend_from_slice(&metadata[..]);
                dst.extend_from_slice(&payload[..]);
            }
            Frame::WindowUpdate(metadata, payload) => {
                let b = &payload[..];
                dst.reserve(METADATA_SIZE + b.len());
                dst.extend_from_slice(&metadata[..]);
                dst.extend_from_slice(b);
            }
            Frame::Continuation(metadata, payload) => {
                dst.reserve(METADATA_SIZE + payload.len());
                dst.extend_from_slice(&metadata[..]);
                dst.extend_from_slice(&payload[..]);
            }
            Frame::PushPromise(metadata, payload) => {
                dst.reserve(METADATA_SIZE + payload.len());
                dst.extend_from_slice(&metadata[..]);
                dst.extend_from_slice(&payload[..]);
            }
        }

        Ok(())
    }
}

impl Decoder for Http2Codec {
    type Item = Frame;
    type Error = anyhow::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.state {
            State::Magic => {
                if src.remaining() < MAGIC.len() {
                    return Ok(None);
                }
                let magic = src.split_to(MAGIC.len());
                if magic[..] == MAGIC[..] {
                    self.state = State::Metadata;
                    return Ok(Some(Frame::Magic));
                }
                bail!("invalid MAGIC");
            }
            State::Metadata => {
                if src.remaining() < METADATA_SIZE {
                    return Ok(None);
                }

                // read metadata
                let metadata = {
                    let b = src.split_to(METADATA_SIZE).freeze();
                    Metadata(b)
                };

                let size = metadata.length();

                if src.remaining() < size {
                    self.state = State::Payload(size);
                    self.metadata.replace(metadata);
                    return Ok(None);
                }

                self.state = State::Metadata;

                Ok(Some(Self::read_frame_to(src, size, metadata)))
            }
            State::Payload(size) => {
                if src.remaining() < size {
                    return Ok(None);
                }
                self.state = State::Metadata;
                Ok(Some(Self::read_frame_to(
                    src,
                    size,
                    self.metadata.take().unwrap(),
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[tokio::test]
    async fn test_decode() -> anyhow::Result<()> {
        init();

        {
            let mut dec = Http2Codec::default();

            let mut b = hex2bytes("505249202a20485454502f322e300d0a0d0a534d0d0a0d0a000012040000000000000300000064000400a000000002000000000000040800000000003e7f00010000230105000000018286418a089d5c0b8170dc780f0304846125427f7a8825b650c3cbbcb83f53032a2f2a");

            while let Some(next) = dec.decode(&mut b)? {
                info!("client->server: {:?}", &next);
            }
        }

        {
            let mut dec = Http2Codec::without_magic();
            let mut b = hex2bytes("\
        000031010400000001885f92497ca58ae819aafb50938ec415305a99567b5c0231326196d07abe940b6a65b685040134a041700cdc6d953168df\
        00000c00010000000168656c6c6f20776f726c6421\
        ");
            while let Some(next) = dec.decode(&mut b)? {
                info!("server->client: {:?}", &next);
            }
        }

        Ok(())
    }

    fn hex2bytes(s: &str) -> BytesMut {
        let v = hex::decode(s).unwrap();
        BytesMut::from(&v[..])
    }
}
