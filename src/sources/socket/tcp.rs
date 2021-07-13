use crate::{
    event::{Event, LogEvent, Value},
    internal_events::{SocketDecodeFrameFailed, SocketEventReceived, SocketMode},
    sources::util::{
        decoding::{DecodingBuilder, DecodingConfig},
        SocketListenAddr, TcpSource,
    },
    tcp::TcpKeepaliveConfig,
    tls::TlsConfig,
};
use bytes::Bytes;
use codec::BytesDelimitedCodec;
use getset::{CopyGetters, Getters, Setters};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone, Getters, CopyGetters, Setters)]
pub struct TcpConfig {
    #[get_copy = "pub"]
    address: SocketListenAddr,
    #[get_copy = "pub"]
    keepalive: Option<TcpKeepaliveConfig>,
    #[serde(default = "default_max_length")]
    #[getset(get_copy = "pub", set = "pub")]
    max_length: usize,
    #[serde(default = "default_shutdown_timeout_secs")]
    #[getset(get_copy = "pub", set = "pub")]
    shutdown_timeout_secs: u64,
    #[get = "pub"]
    host_key: Option<String>,
    #[getset(get = "pub", set = "pub")]
    tls: Option<TlsConfig>,
    #[get_copy = "pub"]
    receive_buffer_bytes: Option<usize>,
    decoding: Option<DecodingConfig>,
}

fn default_max_length() -> usize {
    bytesize::kib(100u64) as usize
}

fn default_shutdown_timeout_secs() -> u64 {
    30
}

impl TcpConfig {
    pub fn new(
        address: SocketListenAddr,
        keepalive: Option<TcpKeepaliveConfig>,
        max_length: usize,
        shutdown_timeout_secs: u64,
        host_key: Option<String>,
        tls: Option<TlsConfig>,
        receive_buffer_bytes: Option<usize>,
        decoding: Option<DecodingConfig>,
    ) -> Self {
        Self {
            address,
            keepalive,
            max_length,
            shutdown_timeout_secs,
            host_key,
            tls,
            receive_buffer_bytes,
            decoding,
        }
    }

    pub fn from_address(address: SocketListenAddr) -> Self {
        Self {
            address,
            keepalive: None,
            max_length: default_max_length(),
            shutdown_timeout_secs: default_shutdown_timeout_secs(),
            host_key: None,
            tls: None,
            receive_buffer_bytes: None,
            decoding: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RawTcpSource {
    pub config: TcpConfig,
}

pub struct RawTcpSourceContext {
    pub source_type: Bytes,
    pub host_key: String,
    pub decode: Box<dyn Fn(Bytes) -> crate::Result<Value> + Send + Sync>,
}

impl TcpSource for RawTcpSource {
    type Context = RawTcpSourceContext;
    type Error = std::io::Error;
    type Decoder = BytesDelimitedCodec;

    fn build_context(&self) -> crate::Result<Self::Context> {
        Ok(Self::Context {
            source_type: Bytes::from("socket"),
            host_key: self
                .config
                .host_key
                .clone()
                .unwrap_or_else(|| crate::config::log_schema().host_key().to_owned()),
            decode: self.config.decoding.build()?,
        })
    }

    fn decoder(&self) -> Self::Decoder {
        BytesDelimitedCodec::new_with_max_length(b'\n', self.config.max_length)
    }

    fn build_event(&self, context: &Self::Context, frame: Bytes, host: Bytes) -> Option<Event> {
        let byte_size = frame.len();

        emit!(SocketEventReceived {
            byte_size,
            mode: SocketMode::Tcp
        });

        let value = match (context.decode)(frame) {
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
            context.source_type.clone(),
        );
        log.insert(context.host_key.clone(), host);

        Some(Event::from(log))
    }
}

#[cfg(test)]
mod test {

    #[test]
    fn tcp_it_defaults_max_length() {
        let with: super::TcpConfig = toml::from_str(
            r#"
            address = "127.0.0.1:1234"
            max_length = 19
            "#,
        )
        .unwrap();

        let without: super::TcpConfig = toml::from_str(
            r#"
            address = "127.0.0.1:1234"
            "#,
        )
        .unwrap();

        assert_eq!(with.max_length, 19);
        assert_eq!(without.max_length, super::default_max_length());
    }
}
