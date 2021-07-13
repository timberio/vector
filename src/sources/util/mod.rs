#[cfg(any(feature = "sources-http", feature = "sources-datadog"))]
mod body_decoding;
#[cfg(feature = "sources-utils-decoding")]
pub mod decoding;
mod encoding_config;
#[cfg(any(feature = "sources-file", feature = "sources-kafka"))]
pub(crate) mod finalizer;
#[cfg(all(unix, feature = "sources-dnstap"))]
pub mod framestream;
#[cfg(feature = "sources-utils-http")]
mod http;
pub mod multiline_config;
#[cfg(all(feature = "sources-utils-tls", feature = "listenfd"))]
mod tcp;
#[cfg(all(unix, feature = "sources-socket"))]
mod unix_datagram;
#[cfg(all(unix, feature = "sources-utils-unix"))]
mod unix_stream;

#[cfg(any(feature = "sources-http", feature = "sources-datadog"))]
pub(crate) use self::body_decoding::{decode_body, Encoding};
#[cfg(any(feature = "sources-http", feature = "sources-heroku_logs"))]
pub(crate) use self::http::add_query_parameters;
#[cfg(feature = "sources-prometheus")]
pub(crate) use self::http::decode;
#[cfg(feature = "sources-utils-http")]
pub(crate) use self::http::{ErrorMessage, HttpSource, HttpSourceAuthConfig};
pub use encoding_config::EncodingConfig;
pub use multiline_config::MultilineConfig;
#[cfg(all(feature = "sources-utils-tls", feature = "listenfd"))]
pub use tcp::{IsErrorFatal as TcpIsErrorFatal, SocketListenAddr, TcpSource};
#[cfg(all(unix, feature = "sources-socket",))]
pub use unix_datagram::build_unix_datagram_source;
#[cfg(all(unix, feature = "sources-utils-unix",))]
pub use unix_stream::build_unix_stream_source;
