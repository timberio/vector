#[macro_use]
extern crate tracing;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::{cmp, io, usize};
use tokio_util::codec::{Decoder, Encoder};

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct BytesDelimitedCodec {
    delim: u8,
    max_length: usize,
    is_discarding: bool,
    next_index: usize,
}

impl BytesDelimitedCodec {
    /// Returns a `BytesDelimitedCodec` with the specified delimiter.
    pub fn new(delim: u8) -> Self {
        BytesDelimitedCodec {
            delim,
            max_length: usize::MAX,
            is_discarding: false,
            next_index: 0,
        }
    }

    /// Returns a `BytesDelimitedCodec` with a maximum frame length limit.
    pub fn new_with_max_length(delim: u8, max_length: usize) -> Self {
        BytesDelimitedCodec {
            max_length,
            ..BytesDelimitedCodec::new(delim)
        }
    }

    /// Returns the maximum frame length when decoding.
    pub fn max_length(&self) -> usize {
        self.max_length
    }
}

impl Decoder for BytesDelimitedCodec {
    type Item = Bytes;
    type Error = io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<Bytes>, io::Error> {
        loop {
            // Determine how far into the buffer we'll search for a newline. If
            // there's no max_length set, we'll read to the end of the buffer.
            let read_to = cmp::min(self.max_length.saturating_add(1), buf.len());

            let newline_pos = buf[self.next_index..read_to]
                .iter()
                .position(|b| *b == self.delim);

            match (self.is_discarding, newline_pos) {
                (true, Some(offset)) => {
                    // If we found a newline, discard up to that offset and
                    // then stop discarding. On the next iteration, we'll try
                    // to read a line normally.
                    buf.advance(offset + self.next_index + 1);
                    self.is_discarding = false;
                    self.next_index = 0;
                }
                (true, None) => {
                    // Otherwise, we didn't find a newline, so we'll discard
                    // everything we read. On the next iteration, we'll continue
                    // discarding up to max_len bytes unless we find a newline.
                    buf.advance(read_to);
                    self.next_index = 0;
                    if buf.is_empty() {
                        return Ok(None);
                    }
                }
                (false, Some(pos)) => {
                    // We found a correct frame

                    let newpos_index = pos + self.next_index;
                    self.next_index = 0;
                    let mut frame = buf.split_to(newpos_index + 1);

                    trace!(
                        message = "Decoding the frame.",
                        bytes_proccesed = frame.len()
                    );

                    let frame = frame.split_to(frame.len() - 1);

                    return Ok(Some(frame.freeze()));
                }
                (false, None) if buf.len() > self.max_length => {
                    // We reached the max length without finding the
                    // delimiter so must discard the rest until we
                    // reach the next delimiter
                    self.is_discarding = true;
                    warn!(
                        message = "Discarding frame larger than max_length.",
                        buf_len = buf.len(),
                        max_length = self.max_length,
                        internal_log_rate_secs = 30
                    );
                    return Ok(None);
                }
                (false, None) => {
                    // We didn't find the delimiter and didn't
                    // reach the max frame length.
                    self.next_index = read_to;
                    return Ok(None);
                }
            }
        }
    }

    fn decode_eof(&mut self, buf: &mut BytesMut) -> Result<Option<Bytes>, io::Error> {
        let frame = match self.decode(buf)? {
            Some(frame) => Some(frame),
            None if !buf.is_empty() && !self.is_discarding => {
                let frame = buf.split_to(buf.len());
                self.next_index = 0;

                Some(frame.into())
            }
            _ => None,
        };

        Ok(frame)
    }
}

impl<T> Encoder<T> for BytesDelimitedCodec
where
    T: AsRef<[u8]>,
{
    type Error = io::Error;

    fn encode(&mut self, item: T, buf: &mut BytesMut) -> Result<(), io::Error> {
        let item = item.as_ref();
        buf.reserve(item.len() + 1);
        buf.put(item);
        buf.put_u8(self.delim);
        Ok(())
    }
}

/// Decodes according to `Octet Counting` in https://tools.ietf.org/html/rfc6587
#[derive(Clone, Debug)]
pub struct SyslogDecoder {
    other: BytesDelimitedCodec,
    octet_decoding: Option<State>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    NotDiscarding,
    Discarding(usize),
    DiscardingToEol,
}

impl SyslogDecoder {
    pub fn new(max_length: usize) -> Self {
        Self {
            other: BytesDelimitedCodec::new_with_max_length(b'\n', max_length),
            octet_decoding: None,
        }
    }

    fn octet_decode(
        &mut self,
        state: State,
        src: &mut BytesMut,
    ) -> Result<Option<Bytes>, io::Error> {
        // Encoding scheme:
        //
        // len ' ' data
        // |    |  | len number of bytes that contain syslog message
        // |    |
        // |    | Separating whitespace
        // |
        // | ASCII decimal number of unknown length

        let space_pos = src.iter().position(|&b| b == b' ');

        // If we are discarding, discard to the next newline.
        let newline_pos = src.iter().position(|&b| b == b'\n');

        match (state, newline_pos, space_pos) {
            (State::Discarding(chars), _, _) if src.len() >= chars => {
                // We have a certain number of chars to discard.
                // There are enough chars in this frame to discard
                src.advance(chars);
                self.octet_decoding = None;
                Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Frame length limit exceeded",
                ))
            }

            (State::Discarding(chars), _, _) => {
                // We have a certain number of chars to discard.
                // There aren't enough in this frame so we need to discard
                // The entire frame and adjust the amount to discard accordingly.
                self.octet_decoding = Some(State::Discarding(src.len() - chars));
                src.advance(src.len());
                Ok(None)
            }

            (State::DiscardingToEol, Some(offset), _) => {
                // When discarding we keep discarding to the next newline.
                src.advance(offset + 1);
                self.octet_decoding = None;
                Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Frame length limit exceeded",
                ))
            }

            (State::DiscardingToEol, None, _) => {
                // There is no newline in this frame. Since we don't have a set number of
                // chars we want to discard, we need to discard to the next newline.
                // Advance as far as we can to discard the entire frame.
                src.advance(src.len());
                Ok(None)
            }

            (State::NotDiscarding, _, Some(space_pos)) if space_pos < self.other.max_length() => {
                // Everything looks good. We aren't discarding, we have a space that is not beyond our
                // maximum length. Attempt to parse the bytes as a number which will hopefully
                // give us a sensible length for our message.
                let len: usize = match std::str::from_utf8(&src[..space_pos])
                    .map_err(|_| ())
                    .and_then(|num| num.parse().map_err(|_| ()))
                {
                    Ok(len) => len,
                    Err(_) => {
                        // It was not a sensible number.
                        // Advance the buffer past the erroneous bytes
                        // to prevent us getting stuck in an infinite loop.
                        src.advance(space_pos + 1);
                        self.octet_decoding = None;
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Unable to decode message len as number",
                        ));
                    }
                };

                let from = space_pos + 1;
                let to = from + len;

                if len > self.other.max_length() {
                    // The length is greater than we want.
                    // We need to discard the entire message.
                    self.octet_decoding = Some(State::Discarding(len));
                    src.advance(space_pos + 1);

                    Ok(None)
                } else if let Some(msg) = src.get(from..to) {
                    let s = match std::str::from_utf8(msg) {
                        Ok(s) => Bytes::from(s.to_string()),
                        Err(_) => {
                            // The data was not valid UTF8 :-(.
                            // Advance the buffer past the erroneous bytes
                            // to prevent us getting stuck in an infinite loop.
                            src.advance(to);
                            self.octet_decoding = None;
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "Unable to decode message as UTF8",
                            ));
                        }
                    };

                    // We have managed to read the entire message as valid UTF8!
                    src.advance(to);
                    self.octet_decoding = None;
                    Ok(Some(s))
                } else {
                    // We have an acceptable number of bytes in this message, but all the data
                    // was not in the frame, return None to indicate we want more data before we
                    // do anything else.
                    Ok(None)
                }
            }

            (State::NotDiscarding, Some(newline_pos), _) => {
                // Beyond maximum length, advance to the newline.
                src.advance(newline_pos + 1);
                Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Frame length limit exceeded",
                ))
            }

            (State::NotDiscarding, None, _) if src.len() < self.other.max_length() => {
                // We aren't discarding, but there is no useful character to tell us what to do next,
                // we are still not beyond the max length, so just return None to indicate we need to
                // wait for more data.
                Ok(None)
            }

            (State::NotDiscarding, None, _) => {
                // There is no newline in this frame and we have more data than we want to handle.
                // Advance as far as we can to discard the entire frame.
                self.octet_decoding = Some(State::DiscardingToEol);
                src.advance(src.len());
                Ok(None)
            }
        }
    }

    /// None if this is not octet counting encoded
    fn checked_decode(&mut self, src: &mut BytesMut) -> Option<Result<Option<Bytes>, io::Error>> {
        if let Some(&first_byte) = src.get(0) {
            if (49..=57).contains(&first_byte) {
                // First character is non zero number so we can assume that
                // octet count framing is used.
                trace!("Octet counting encoded event detected.");
                self.octet_decoding = Some(State::NotDiscarding);
            }
        }

        self.octet_decoding
            .map(|state| self.octet_decode(state, src))
    }
}

impl Decoder for SyslogDecoder {
    type Item = Bytes;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(ret) = self.checked_decode(src) {
            ret
        } else {
            // Octet counting isn't used so fallback to newline codec.
            self.other.decode(src)
        }
    }

    fn decode_eof(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(ret) = self.checked_decode(buf) {
            ret
        } else {
            // Octet counting isn't used so fallback to newline codec.
            self.other.decode_eof(buf)
        }
    }
}
