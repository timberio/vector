pub mod adaptive_concurrency;
pub mod batch;
pub mod buffer;
pub mod encoding;
pub mod http;
pub mod retries;
pub mod service;
pub mod sink;
pub mod socket_bytes_sink;
pub mod statistic;
pub mod tcp;
#[cfg(test)]
pub mod test;
pub mod udp;
#[cfg(all(
    any(
        feature = "sinks-socket",
        feature = "sinks-statsd",
        feature = "sinks-syslog"
    ),
    unix
))]
pub mod unix;
pub mod uri;

use crate::event::{Event, EventMetadata, LogEvent};
use bytes::Bytes;
use encoding::{EncodingConfig, EncodingConfiguration};
use serde::{Deserialize, Serialize};
use snafu::Snafu;
use std::borrow::Cow;

pub use batch::{Batch, BatchConfig, BatchSettings, BatchSize, PushResult};
pub use buffer::json::{BoxedRawValue, JsonArrayBuffer};
pub use buffer::metrics::MetricEntry;
pub use buffer::partition::Partition;
pub use buffer::vec::{EncodedLength, VecBuffer};
pub use buffer::{Buffer, Compression, PartitionBuffer, PartitionInnerBuffer};
pub use service::{
    Concurrency, ServiceBuilderExt, TowerBatchedSink, TowerPartitionSink, TowerRequestConfig,
    TowerRequestLayer, TowerRequestSettings,
};
pub use sink::{BatchSink, PartitionBatchSink, StreamSink};
pub use uri::UriSerde;

#[derive(Debug, Snafu)]
enum SinkBuildError {
    #[snafu(display("Missing host in address field"))]
    MissingHost,
    #[snafu(display("Missing port in address field"))]
    MissingPort,
}

#[derive(Debug)]
pub struct EncodedEvent<I> {
    pub item: I,
    pub metadata: Option<EventMetadata>,
}

impl<I> EncodedEvent<I> {
    /// Create a trivial input with no metadata. This method will be
    /// removed when all sinks are converted.
    pub fn new(item: I) -> Self {
        Self {
            item,
            metadata: None,
        }
    }

    /// Helper function to set up the metadata.
    pub fn with_metadata(self, log: LogEvent) -> Self {
        let (_fields, metadata) = log.into_parts();
        Self {
            item: self.item,
            metadata: Some(metadata),
        }
    }

    // This should be:
    // ```impl<F, I: From<F>> From<EncodedEvent<F>> for EncodedEvent<I>```
    // however, the compiler rejects that due to conflicting
    // implementations of `From` due to the generic
    // ```impl<T> From<T> for T```
    pub fn from<F>(that: EncodedEvent<F>) -> Self
    where
        I: From<F>,
    {
        Self {
            item: I::from(that.item),
            metadata: that.metadata,
        }
    }
}

/**
 * Enum representing different ways to encode events as they are sent into a Sink.
 */
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    Text,
    Json,
}

/**
* Encodes the given event into raw bytes that can be sent into a Sink, according to
* the given encoding. If there are any errors encoding the event, logs a warning
* and returns None.
**/
pub fn encode_event(
    mut event: Event,
    encoding: &EncodingConfig<Encoding>,
) -> Option<EncodedEvent<Bytes>> {
    encoding.apply_rules(&mut event);
    let log = event.into_log();

    let b = match encoding.codec() {
        Encoding::Json => serde_json::to_vec(&log),
        Encoding::Text => {
            let bytes = log
                .get(crate::config::log_schema().message_key())
                .map(|v| v.as_bytes().to_vec())
                .unwrap_or_default();
            Ok(bytes)
        }
    };
    let (_fields, metadata) = log.into_parts();

    b.map(|mut b| {
        b.push(b'\n');
        EncodedEvent {
            item: Bytes::from(b),
            metadata: Some(metadata),
        }
    })
    .map_err(|error| error!(message = "Unable to encode.", %error))
    .ok()
}

/// Joins namespace with name via delimiter if namespace is present.
pub fn encode_namespace<'a>(
    namespace: Option<&str>,
    delimiter: char,
    name: impl Into<Cow<'a, str>>,
) -> String {
    let name = name.into();
    namespace
        .map(|namespace| format!("{}{}{}", namespace, delimiter, name))
        .unwrap_or_else(|| name.into_owned())
}
