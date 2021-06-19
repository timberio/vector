use crate::udp;
use crate::{
    config::{self, GenerateConfig, Resource, SourceConfig, SourceContext, SourceDescription},
    event::Event,
    internal_events::{StatsdEventReceived, StatsdInvalidRecord, StatsdSocketError},
    shutdown::ShutdownSignal,
    sources::util::{SocketListenAddr, TcpSource},
    tcp::TcpKeepaliveConfig,
    tls::{MaybeTlsSettings, TlsConfig},
    Pipeline,
};
use bytes::Bytes;
use codec::BytesDelimitedCodec;
use futures::{stream, SinkExt, StreamExt, TryFutureExt};
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use tokio::net::UdpSocket;
use tokio_util::{codec::BytesCodec, udp::UdpFramed};

pub mod parser;
#[cfg(unix)]
mod unix;

use parser::parse;
#[cfg(unix)]
use unix::{statsd_unix, UnixConfig};

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum StatsdConfig {
    Tcp(TcpConfig),
    Udp(UdpConfig),
    #[cfg(unix)]
    Unix(UnixConfig),
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct UdpConfig {
    address: SocketAddr,
    receive_buffer_bytes: Option<usize>,
}

impl UdpConfig {
    pub fn from_address(address: SocketAddr) -> Self {
        Self {
            address,
            receive_buffer_bytes: None,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
struct TcpConfig {
    address: SocketListenAddr,
    keepalive: Option<TcpKeepaliveConfig>,
    #[serde(default)]
    tls: Option<TlsConfig>,
    #[serde(default = "default_shutdown_timeout_secs")]
    shutdown_timeout_secs: u64,
    receive_buffer_bytes: Option<usize>,
}

impl TcpConfig {
    #[cfg(all(test, feature = "sinks-prometheus"))]
    pub fn from_address(address: SocketListenAddr) -> Self {
        Self {
            address,
            keepalive: None,
            tls: None,
            shutdown_timeout_secs: default_shutdown_timeout_secs(),
            receive_buffer_bytes: None,
        }
    }
}

fn default_shutdown_timeout_secs() -> u64 {
    30
}

inventory::submit! {
    SourceDescription::new::<StatsdConfig>("statsd")
}

impl GenerateConfig for StatsdConfig {
    fn generate_config() -> toml::Value {
        toml::Value::try_from(Self::Udp(UdpConfig::from_address(SocketAddr::V4(
            SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 8125),
        ))))
        .unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "statsd")]
impl SourceConfig for StatsdConfig {
    async fn build(&self, cx: SourceContext) -> crate::Result<super::Source> {
        match self {
            StatsdConfig::Udp(config) => {
                Ok(Box::pin(statsd_udp(config.clone(), cx.shutdown, cx.out)))
            }
            StatsdConfig::Tcp(config) => {
                let tls = MaybeTlsSettings::from_config(&config.tls, true)?;
                StatsdTcpSource.run(
                    config.address,
                    config.keepalive,
                    config.shutdown_timeout_secs,
                    tls,
                    config.receive_buffer_bytes,
                    cx.shutdown,
                    cx.out,
                )
            }
            #[cfg(unix)]
            StatsdConfig::Unix(config) => Ok(statsd_unix(config.clone(), cx.shutdown, cx.out)),
        }
    }

    fn output_type(&self) -> config::DataType {
        config::DataType::Metric
    }

    fn source_type(&self) -> &'static str {
        "statsd"
    }

    fn resources(&self) -> Vec<Resource> {
        match self.clone() {
            Self::Tcp(tcp) => vec![tcp.address.into()],
            Self::Udp(udp) => vec![Resource::udp(udp.address)],
            #[cfg(unix)]
            Self::Unix(_) => vec![],
        }
    }
}

pub(self) fn parse_event(line: &str) -> Option<Event> {
    match parse(line) {
        Ok(metric) => {
            emit!(StatsdEventReceived {
                byte_size: line.len()
            });
            Some(Event::Metric(metric))
        }
        Err(error) => {
            emit!(StatsdInvalidRecord { error, text: line });
            None
        }
    }
}

async fn statsd_udp(
    config: UdpConfig,
    shutdown: ShutdownSignal,
    mut out: Pipeline,
) -> Result<(), ()> {
    let socket = UdpSocket::bind(&config.address)
        .map_err(|error| emit!(StatsdSocketError::bind(error)))
        .await?;

    if let Some(receive_buffer_bytes) = config.receive_buffer_bytes {
        if let Err(error) = udp::set_receive_buffer_size(&socket, receive_buffer_bytes) {
            warn!(message = "Failed configuring receive buffer size on UDP socket.", %error);
        }
    }

    info!(
        message = "Listening.",
        addr = %config.address,
        r#type = "udp"
    );

    let mut stream = UdpFramed::new(socket, BytesCodec::new()).take_until(shutdown);
    while let Some(frame) = stream.next().await {
        match frame {
            Ok((bytes, _sock)) => {
                let packet = String::from_utf8_lossy(bytes.as_ref());
                let metrics = packet.lines().filter_map(parse_event).map(Ok);

                // Need `boxed` to resolve a lifetime issue
                // https://github.com/rust-lang/rust/issues/64552#issuecomment-669728225
                let mut metrics = stream::iter(metrics).boxed();
                if let Err(error) = out.send_all(&mut metrics).await {
                    error!(message = "Error sending metric.", %error);
                    break;
                }
            }
            Err(error) => {
                emit!(StatsdSocketError::read(error));
            }
        }
    }

    Ok(())
}

#[derive(Clone)]
struct StatsdTcpSource;

impl TcpSource for StatsdTcpSource {
    type Error = std::io::Error;
    type Decoder = BytesDelimitedCodec;

    fn decoder(&self) -> Self::Decoder {
        BytesDelimitedCodec::new(b'\n')
    }

    fn build_event(&self, line: Bytes, _host: Bytes) -> Option<Event> {
        let line = String::from_utf8_lossy(line.as_ref());
        parse_event(&line)
    }
}

#[cfg(feature = "sinks-prometheus")]
#[cfg(test)]
mod test {
    use super::*;
    use crate::{
        config,
        sinks::prometheus::exporter::PrometheusExporterConfig,
        test_util::{next_addr, start_topology},
    };
    use futures::channel::mpsc;
    use hyper::body::to_bytes as body_to_bytes;
    use tokio::io::AsyncWriteExt;
    use tokio::time::{sleep, Duration};

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<StatsdConfig>();
    }

    fn parse_count(lines: &[&str], prefix: &str) -> usize {
        lines
            .iter()
            .find(|s| s.starts_with(prefix))
            .map(|s| s.split_whitespace().nth(1).unwrap())
            .unwrap()
            .parse::<usize>()
            .unwrap()
    }

    #[tokio::test]
    async fn test_statsd_udp() {
        let in_addr = next_addr();
        let config = StatsdConfig::Udp(UdpConfig::from_address(in_addr));
        let (sender, mut receiver) = mpsc::channel(200);
        tokio::spawn(async move {
            let bind_addr = next_addr();
            let socket = UdpSocket::bind(bind_addr).await.unwrap();
            socket.connect(in_addr).await.unwrap();
            while let Some(bytes) = receiver.next().await {
                socket.send(bytes).await.unwrap();
            }
        });
        test_statsd(config, sender).await;
    }

    #[tokio::test]
    async fn test_statsd_tcp() {
        let in_addr = next_addr();
        let config = StatsdConfig::Tcp(TcpConfig::from_address(in_addr.into()));
        let (sender, mut receiver) = mpsc::channel(200);
        tokio::spawn(async move {
            while let Some(bytes) = receiver.next().await {
                tokio::net::TcpStream::connect(in_addr)
                    .await
                    .unwrap()
                    .write_all(bytes)
                    .await
                    .unwrap();
            }
        });
        test_statsd(config, sender).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_statsd_unix() {
        let in_path = tempfile::tempdir().unwrap().into_path().join("unix_test");
        let config = StatsdConfig::Unix(UnixConfig {
            path: in_path.clone(),
        });
        let (sender, mut receiver) = mpsc::channel(200);
        tokio::spawn(async move {
            while let Some(bytes) = receiver.next().await {
                tokio::net::UnixStream::connect(&in_path)
                    .await
                    .unwrap()
                    .write_all(bytes)
                    .await
                    .unwrap();
            }
        });
        test_statsd(config, sender).await;
    }

    async fn test_statsd(
        statsd_config: StatsdConfig,
        // could use unbounded channel,
        // but we want to reserve the order messages.
        mut sender: mpsc::Sender<&'static [u8]>,
    ) {
        let out_addr = next_addr();

        let mut config = config::Config::builder();
        config.add_source("in", statsd_config, None, None);
        config.add_sink(
            "out",
            &["in"],
            PrometheusExporterConfig {
                address: out_addr,
                tls: None,
                default_namespace: Some("vector".into()),
                buckets: vec![1.0, 2.0, 4.0],
                quantiles: vec![],
                flush_period_secs: 1,
            },
        );

        let (topology, _crash) = start_topology(config.build().unwrap(), false).await;

        // Give some time for the topology to start
        sleep(Duration::from_millis(100)).await;

        for _ in 0..100 {
            sender.send(
                b"foo:1|c|#a,b:b\nbar:42|g\nfoo:1|c|#a,b:c\nglork:3|h|@0.1\nmilliglork:3000|ms|@0.1\nset:0|s\nset:1|s\n"
            ).await.unwrap();
            // Space things out slightly to try to avoid dropped packets
            sleep(Duration::from_millis(10)).await;
        }

        // Give packets some time to flow through
        sleep(Duration::from_millis(100)).await;

        let client = hyper::Client::new();
        let response = client
            .get(format!("http://{}/metrics", out_addr).parse().unwrap())
            .await
            .unwrap();
        assert!(response.status().is_success());

        let body = body_to_bytes(response.into_body()).await.unwrap();
        let lines = std::str::from_utf8(&body)
            .unwrap()
            .lines()
            .collect::<Vec<_>>();

        // note that prometheus client reorders the labels
        let vector_foo1 = parse_count(&lines, "vector_foo{a=\"true\",b=\"b\"");
        let vector_foo2 = parse_count(&lines, "vector_foo{a=\"true\",b=\"c\"");
        // packets get lost :(
        assert!(vector_foo1 > 90);
        assert!(vector_foo2 > 90);

        let vector_bar = parse_count(&lines, "vector_bar");
        assert_eq!(42, vector_bar);

        assert_eq!(parse_count(&lines, "vector_glork_bucket{le=\"1\"}"), 0);
        assert_eq!(parse_count(&lines, "vector_glork_bucket{le=\"2\"}"), 0);
        assert!(parse_count(&lines, "vector_glork_bucket{le=\"4\"}") > 0);
        assert!(parse_count(&lines, "vector_glork_bucket{le=\"+Inf\"}") > 0);
        let glork_sum = parse_count(&lines, "vector_glork_sum");
        let glork_count = parse_count(&lines, "vector_glork_count");
        assert_eq!(glork_count * 3, glork_sum);

        assert_eq!(parse_count(&lines, "vector_milliglork_bucket{le=\"1\"}"), 0);
        assert_eq!(parse_count(&lines, "vector_milliglork_bucket{le=\"2\"}"), 0);
        assert!(parse_count(&lines, "vector_milliglork_bucket{le=\"4\"}") > 0);
        assert!(parse_count(&lines, "vector_milliglork_bucket{le=\"+Inf\"}") > 0);
        let milliglork_sum = parse_count(&lines, "vector_milliglork_sum");
        let milliglork_count = parse_count(&lines, "vector_milliglork_count");
        assert_eq!(milliglork_count * 3, milliglork_sum);

        // Set test
        // Flush could have occurred
        assert!(parse_count(&lines, "vector_set") <= 2);

        // Flush test
        {
            // Wait for flush to happen
            sleep(Duration::from_millis(2000)).await;

            let response = client
                .get(format!("http://{}/metrics", out_addr).parse().unwrap())
                .await
                .unwrap();
            assert!(response.status().is_success());

            let body = body_to_bytes(response.into_body()).await.unwrap();
            let lines = std::str::from_utf8(&body)
                .unwrap()
                .lines()
                .collect::<Vec<_>>();

            // Check rested
            assert_eq!(parse_count(&lines, "vector_set"), 0);

            // Re-check that set is also reset------------

            sender.send(b"set:0|s\nset:1|s\n").await.unwrap();
            // Give packets some time to flow through
            sleep(Duration::from_millis(100)).await;

            let response = client
                .get(format!("http://{}/metrics", out_addr).parse().unwrap())
                .await
                .unwrap();
            assert!(response.status().is_success());

            let body = body_to_bytes(response.into_body()).await.unwrap();
            let lines = std::str::from_utf8(&body)
                .unwrap()
                .lines()
                .collect::<Vec<_>>();

            // Set test
            assert_eq!(parse_count(&lines, "vector_set"), 2);
        }

        // Shut down server
        topology.stop().await;
    }
}
