use bytes::{Bytes, BytesMut};
use codec::{BytesDelimitedCodec, SyslogDecoder};
use std::io;
use tokio_util::codec::Decoder;

#[derive(Debug, Clone)]
pub enum StreamDecoder {
    BytesDecoder(BytesDelimitedCodec),
    SyslogDecoder(SyslogDecoder),
}

impl Decoder for StreamDecoder {
    type Item = Bytes;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self {
            StreamDecoder::BytesDecoder(d) => d.decode(src),
            StreamDecoder::SyslogDecoder(d) => d.decode(src),
        }
    }

    fn decode_eof(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self {
            StreamDecoder::BytesDecoder(d) => d.decode_eof(buf),
            StreamDecoder::SyslogDecoder(d) => d.decode_eof(buf),
        }
    }
}
