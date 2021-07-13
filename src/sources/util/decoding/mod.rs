use crate::{event::Event, internal_events::DecoderParseFailed};
use bytes::{Bytes, BytesMut};

pub trait Parser {
    fn parse(&self, bytes: Bytes) -> crate::Result<Event>;
}

pub struct Decoder<
    Error: From<std::io::Error>,
    Parser: super::decoding::Parser,
    Item: Into<Bytes> = Bytes,
> {
    framer: Box<dyn tokio_util::codec::Decoder<Item = Item, Error = Error> + Send + Sync>,
    parser: Parser,
}

impl<Error, Parser, Item> Decoder<Error, Parser, Item>
where
    Error: From<std::io::Error>,
    Parser: super::decoding::Parser,
    Item: Into<Bytes>,
{
    pub fn new(
        framer: Box<dyn tokio_util::codec::Decoder<Item = Item, Error = Error> + Send + Sync>,
        parser: Parser,
    ) -> Self {
        Self { framer, parser }
    }
}

impl<Error, Parser, Item> tokio_util::codec::Decoder for Decoder<Error, Parser, Item>
where
    Error: From<std::io::Error>,
    Parser: super::decoding::Parser,
    Item: Into<Bytes>,
{
    type Item = (Event, usize);
    type Error = Error;

    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.framer.decode(buf).map(|result| {
            result.and_then(|frame| {
                let bytes = frame.into();
                let byte_size = bytes.len();
                match self.parser.parse(bytes) {
                    Ok(event) => Some((event, byte_size)),
                    Err(error) => {
                        emit!(DecoderParseFailed { error });
                        None
                    }
                }
            })
        })
    }

    fn decode_eof(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.framer.decode_eof(buf).map(|result| {
            result.and_then(|frame| {
                let bytes = frame.into();
                let byte_size = bytes.len();
                match self.parser.parse(bytes) {
                    Ok(event) => Some((event, byte_size)),
                    Err(error) => {
                        emit!(DecoderParseFailed { error });
                        None
                    }
                }
            })
        })
    }
}
