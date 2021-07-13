use crate::{
    event::Event,
    internal_events::{SocketEventReceived, SocketMode},
    shutdown::ShutdownSignal,
    sources::{
        util::{build_unix_datagram_source, build_unix_stream_source},
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
        }
    }
}

/**
* Function to pass to build_unix_*_source, specific to the basic unix source.
* Takes a single line of a received message and builds an Event object.
**/
fn build_event(line: &str, host_key: &str, received_from: Option<Bytes>) -> Event {
    let byte_size = line.len();
    let mut event = Event::from(line);
    event.as_mut_log().insert(
        crate::config::log_schema().source_type_key(),
        Bytes::from("socket"),
    );
    if let Some(host) = received_from {
        event.as_mut_log().insert(host_key, host);
    }
    emit!(SocketEventReceived {
        byte_size,
        mode: SocketMode::Unix
    });
    event
}

pub(super) fn unix_datagram(
    path: PathBuf,
    max_length: usize,
    host_key: String,
    shutdown: ShutdownSignal,
    out: Pipeline,
) -> Source {
    build_unix_datagram_source(
        path,
        max_length,
        host_key,
        LinesCodec::new_with_max_length(max_length),
        shutdown,
        out,
        |bytes, host_key, received_from| {
            let line = std::str::from_utf8(&bytes).unwrap(); // Guaranteed to be valid UTF-8 by `LinesCodec`.
            Some(build_event(line, host_key, received_from))
        },
    )
}

pub(super) fn unix_stream(
    path: PathBuf,
    max_length: usize,
    host_key: String,
    shutdown: ShutdownSignal,
    out: Pipeline,
) -> Source {
    build_unix_stream_source(
        path,
        LinesCodec::new_with_max_length(max_length),
        host_key,
        shutdown,
        out,
        |bytes, host_key, received_from| {
            let line = std::str::from_utf8(&bytes).unwrap(); // Guaranteed to be valid UTF-8 by `LinesCodec`.
            Some(build_event(line, host_key, received_from))
        },
    )
}
