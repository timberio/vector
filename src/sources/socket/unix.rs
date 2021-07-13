use crate::{
    event::{Event, LogEvent, Value},
    internal_events::{SocketDecodeFrameFailed, SocketEventReceived, SocketMode},
    shutdown::ShutdownSignal,
    sources::{
        util::{build_unix_datagram_source, build_unix_stream_source, decoding::DecodingConfig},
        Source,
    },
    Pipeline,
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio_util::codec::LinesCodec;

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct UnixConfig {
    pub path: PathBuf,
    #[serde(default = "default_max_length")]
    pub max_length: usize,
    pub host_key: Option<String>,
    pub decoding: Option<DecodingConfig>,
}

fn default_max_length() -> usize {
    bytesize::kib(100u64) as usize
}

impl UnixConfig {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            max_length: default_max_length(),
            host_key: None,
            decoding: None,
        }
    }
}

/**
* Function to pass to build_unix_*_source, specific to the basic unix source.
* Takes a single line of a received message and builds an Event object.
**/
fn build_event(
    host_key: &str,
    received_from: Option<Bytes>,
    frame: Bytes,
    decode: &(dyn Fn(Bytes) -> crate::Result<Value> + Send + Sync),
) -> Option<Event> {
    let byte_size = frame.len();

    emit!(SocketEventReceived {
        byte_size,
        mode: SocketMode::Unix
    });

    let value = match decode(frame) {
        Ok(value) => value,
        Err(error) => {
            emit!(SocketDecodeFrameFailed {
                mode: SocketMode::Tcp,
                error
            });
            return None;
        }
    };

    let mut log = LogEvent::from(value);

    log.insert(
        crate::config::log_schema().source_type_key(),
        Bytes::from("socket"),
    );
    if let Some(host) = received_from {
        log.insert(host_key, host);
    }

    Some(Event::from(log))
}

pub(super) fn unix_datagram(
    path: PathBuf,
    max_length: usize,
    host_key: String,
    decoding: Option<DecodingConfig>,
    shutdown: ShutdownSignal,
    out: Pipeline,
) -> crate::Result<Source> {
    build_unix_datagram_source(
        path,
        max_length,
        host_key,
        LinesCodec::new_with_max_length(max_length),
        decoding,
        shutdown,
        out,
        build_event,
    )
}

pub(super) fn unix_stream(
    path: PathBuf,
    max_length: usize,
    host_key: String,
    decoding: Option<DecodingConfig>,
    shutdown: ShutdownSignal,
    out: Pipeline,
) -> crate::Result<Source> {
    build_unix_stream_source(
        path,
        LinesCodec::new_with_max_length(max_length),
        host_key,
        decoding,
        shutdown,
        out,
        build_event,
    )
}
