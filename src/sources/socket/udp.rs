use crate::{
    event::{Event, LogEvent},
    internal_events::{
        SocketDecodeFrameFailed, SocketEventReceived, SocketMode, SocketReceiveError,
    },
    shutdown::ShutdownSignal,
    sources::{
        util::decoding::{DecodingBuilder, DecodingConfig},
        Source,
    },
    udp, Pipeline,
};
use bytes::{Bytes, BytesMut};
use codec::BytesDelimitedCodec;
use futures::SinkExt;
use getset::{CopyGetters, Getters};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tokio_util::codec::Decoder;

/// UDP processes messages per packet, where messages are separated by newline.
#[derive(Deserialize, Serialize, Debug, Clone, Getters, CopyGetters)]
#[serde(deny_unknown_fields)]
pub struct UdpConfig {
    #[get_copy = "pub"]
    address: SocketAddr,
    #[serde(default = "default_max_length")]
    #[get_copy = "pub"]
    max_length: usize,
    #[get = "pub"]
    host_key: Option<String>,
    #[get_copy = "pub"]
    receive_buffer_bytes: Option<usize>,
    #[get = "pub"]
    decoding: Option<DecodingConfig>,
}

fn default_max_length() -> usize {
    bytesize::kib(100u64) as usize
}

impl UdpConfig {
    pub fn from_address(address: SocketAddr) -> Self {
        Self {
            address,
            max_length: default_max_length(),
            host_key: None,
            receive_buffer_bytes: None,
            decoding: None,
        }
    }
}

pub fn udp(
    address: SocketAddr,
    max_length: usize,
    host_key: String,
    receive_buffer_bytes: Option<usize>,
    decoding: Option<DecodingConfig>,
    mut shutdown: ShutdownSignal,
    out: Pipeline,
) -> crate::Result<Source> {
    let decode = decoding.build()?;
    let mut out = out.sink_map_err(|error| error!(message = "Error sending event.", %error));

    Ok(Box::pin(async move {
        let socket = UdpSocket::bind(&address)
            .await
            .expect("Failed to bind to udp listener socket");

        if let Some(receive_buffer_bytes) = receive_buffer_bytes {
            if let Err(error) = udp::set_receive_buffer_size(&socket, receive_buffer_bytes) {
                warn!(message = "Failed configuring receive buffer size on UDP socket.", %error);
            }
        }

        let max_length = if let Some(receive_buffer_bytes) = receive_buffer_bytes {
            std::cmp::min(max_length, receive_buffer_bytes)
        } else {
            max_length
        };

        info!(message = "Listening.", address = %address);

        let mut buf = BytesMut::with_capacity(max_length);
        loop {
            buf.resize(max_length, 0);
            tokio::select! {
                recv = socket.recv_from(&mut buf) => {
                    let (byte_size, address) = recv.map_err(|error| {
                        emit!(SocketReceiveError {
                            error,
                            mode: SocketMode::Udp
                        });
                    })?;

                    let mut payload = buf.split_to(byte_size);

                    // UDP processes messages per payload, where messages are separated by newline
                    // and stretch to end of payload.
                    let mut decoder = BytesDelimitedCodec::new(b'\n');
                    while let Ok(Some(frame)) = decoder.decode_eof(&mut payload) {
                        emit!(SocketEventReceived { byte_size, mode:SocketMode::Udp });

                        let value = match decode(frame) {
                            Ok(value) => value,
                            Err(error) => {
                                emit!(SocketDecodeFrameFailed {
                                    mode: SocketMode::Tcp,
                                    error
                                });
                                continue;
                            }
                        };

                        let mut log = LogEvent::from(value);

                        log.insert(
                            crate::config::log_schema().source_type_key(),
                            Bytes::from("socket")
                        );
                        log.insert(host_key.clone(), address.to_string());

                        let event = Event::from(log);

                        tokio::select!{
                            result = out.send(event) => match result {
                                Ok(()) => (),
                                Err(()) => return Ok(()),
                            },
                            _ = &mut shutdown => return Ok(()),
                        }
                    }
                }
                _ = &mut shutdown => return Ok(()),
            }
        }
    }))
}
