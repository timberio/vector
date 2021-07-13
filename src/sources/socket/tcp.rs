use crate::{
    event::Event,
    internal_events::{SocketEventReceived, SocketMode},
    sources::util::{SocketListenAddr, TcpSource},
    tcp::TcpKeepaliveConfig,
    tls::TlsConfig,
};
use bytes::Bytes;
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
    ) -> Self {
        Self {
            address,
            keepalive,
            max_length,
            shutdown_timeout_secs,
            host_key,
            tls,
            receive_buffer_bytes,
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
        }
    }
}

pub struct RawTcpSource<D: tokio_util::codec::Decoder<Item = (Event, usize)>> {
    config: TcpConfig,
    create_decoder: Box<dyn Fn() -> D + Send + Sync>,
}

impl<D: tokio_util::codec::Decoder<Item = (Event, usize)>> RawTcpSource<D> {
    pub fn new(config: TcpConfig, create_decoder: Box<dyn Fn() -> D + Send + Sync>) -> Self {
        Self {
            config,
            create_decoder,
        }
    }
}

impl<D> TcpSource for RawTcpSource<D>
where
    D: tokio_util::codec::Decoder<Item = (Event, usize)> + Send + Sync + 'static,
    D::Error: From<std::io::Error>
        + crate::sources::util::TcpIsErrorFatal
        + std::fmt::Debug
        + std::fmt::Display
        + Send,
{
    type Error = D::Error;
    type Item = Event;
    type Decoder = D;

    fn create_decoder(&self) -> Self::Decoder {
        (self.create_decoder)()
    }

    fn handle_event(&self, event: &mut Event, host: Bytes, byte_size: usize) {
        event.as_mut_log().insert(
            crate::config::log_schema().source_type_key(),
            Bytes::from("socket"),
        );

        let host_key = (self.config.host_key.clone())
            .unwrap_or_else(|| crate::config::log_schema().host_key().to_string());

        event.as_mut_log().insert(host_key, host);

        emit!(SocketEventReceived {
            byte_size,
            mode: SocketMode::Tcp
        });
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
