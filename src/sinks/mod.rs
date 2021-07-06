use futures::future::BoxFuture;
use snafu::Snafu;

pub mod util;

#[cfg(feature = "sinks-amqp")]
pub mod amqp;
#[cfg(feature = "sinks-aws_cloudwatch_logs")]
pub mod aws_cloudwatch_logs;
#[cfg(feature = "sinks-aws_cloudwatch_metrics")]
pub mod aws_cloudwatch_metrics;
#[cfg(feature = "sinks-aws_kinesis_firehose")]
pub mod aws_kinesis_firehose;
#[cfg(feature = "sinks-aws_kinesis_streams")]
pub mod aws_kinesis_streams;
#[cfg(feature = "sinks-aws_s3")]
pub mod aws_s3;
#[cfg(feature = "sinks-aws_sqs")]
pub mod aws_sqs;
#[cfg(feature = "sinks-azure_blob")]
pub mod azure_blob;
#[cfg(feature = "sinks-azure_monitor_logs")]
pub mod azure_monitor_logs;
#[cfg(feature = "sinks-blackhole")]
pub mod blackhole;
#[cfg(feature = "sinks-clickhouse")]
pub mod clickhouse;
#[cfg(feature = "sinks-console")]
pub mod console;
#[cfg(feature = "sinks-datadog")]
pub mod datadog;
#[cfg(feature = "sinks-elasticsearch")]
pub mod elasticsearch;
#[cfg(feature = "sinks-file")]
pub mod file;
#[cfg(feature = "sinks-gcp")]
pub mod gcp;
#[cfg(feature = "sinks-honeycomb")]
pub mod honeycomb;
#[cfg(feature = "sinks-http")]
pub mod http;
#[cfg(feature = "sinks-humio")]
pub mod humio;
#[cfg(any(feature = "sinks-influxdb", feature = "prometheus-integration-tests"))]
pub mod influxdb;
#[cfg(all(feature = "sinks-kafka", feature = "rdkafka"))]
pub mod kafka;
#[cfg(feature = "sinks-logdna")]
pub mod logdna;
#[cfg(feature = "sinks-loki")]
pub mod loki;
#[cfg(feature = "sinks-nats")]
pub mod nats;
#[cfg(feature = "sinks-new_relic_logs")]
pub mod new_relic_logs;
#[cfg(feature = "sinks-papertrail")]
pub mod papertrail;
#[cfg(feature = "sinks-prometheus")]
pub mod prometheus;
#[cfg(feature = "sinks-pulsar")]
pub mod pulsar;
#[cfg(feature = "sinks-redis")]
pub mod redis;
#[cfg(feature = "sinks-sematext")]
pub mod sematext;
#[cfg(feature = "sinks-socket")]
pub mod socket;
#[cfg(feature = "sinks-splunk_hec")]
pub mod splunk_hec;
#[cfg(feature = "sinks-statsd")]
pub mod statsd;
#[cfg(feature = "sinks-vector")]
pub mod vector;

pub use vector_core::sink::VectorSink;

pub type Healthcheck = BoxFuture<'static, crate::Result<()>>;

/// Common build errors
#[derive(Debug, Snafu)]
pub enum BuildError {
    #[snafu(display("Unable to resolve DNS for {:?}", address))]
    DnsFailure { address: String },
    #[snafu(display("DNS errored {}", source))]
    DnsError { source: crate::dns::DnsError },
    #[snafu(display("Socket address problem: {}", source))]
    SocketAddressError { source: std::io::Error },
    #[snafu(display("URI parse error: {}", source))]
    UriParseError { source: ::http::uri::InvalidUri },
}

/// Common healthcheck errors
#[derive(Debug, Snafu)]
pub enum HealthcheckError {
    #[snafu(display("Unexpected status: {}", status))]
    UnexpectedStatus { status: ::http::StatusCode },
}
