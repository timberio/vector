use crate::{
    event::{Event, Value},
    internal_events::StatsdInvalidUtf8FrameReceived,
    shutdown::ShutdownSignal,
    sources::util::build_unix_stream_source,
    sources::Source,
    Pipeline,
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio_util::codec::LinesCodec;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct UnixConfig {
    pub path: PathBuf,
}

fn build_event(
    _: &str,
    _: Option<Bytes>,
    frame: Bytes,
    _: &(dyn Fn(Bytes) -> crate::Result<Value> + Send + Sync),
) -> Option<Event> {
    match std::str::from_utf8(&frame) {
        Ok(line) => super::parse_event(line),
        Err(error) => {
            emit!(StatsdInvalidUtf8FrameReceived { error });
            None
        }
    }
}

pub fn statsd_unix(
    config: UnixConfig,
    shutdown: ShutdownSignal,
    out: Pipeline,
) -> crate::Result<Source> {
    build_unix_stream_source(
        config.path,
        LinesCodec::new(),
        String::new(),
        None,
        shutdown,
        out,
        build_event,
    )
}
