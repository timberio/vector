use super::util::{SocketListenAddr, TcpIsErrorFatal, TcpSource};
use crate::{
    config::{
        log_schema, DataType, GenerateConfig, Resource, SourceConfig, SourceContext,
        SourceDescription,
    },
    event::{Event, LogEvent, Value},
    tcp::TcpKeepaliveConfig,
    tls::{MaybeTlsSettings, TlsConfig},
    types,
};
use bytes::{Buf, Bytes, BytesMut};
use flate2::read::ZlibDecoder;
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use std::{
    collections::{BTreeMap, VecDeque},
    convert::TryFrom,
    io::{self, Read},
};
use tokio_util::codec::Decoder;

#[derive(Deserialize, Serialize, Debug)]
pub struct LogstashConfig {
    address: SocketListenAddr,
    keepalive: Option<TcpKeepaliveConfig>,
    tls: Option<TlsConfig>,
    receive_buffer_bytes: Option<usize>,
}

inventory::submit! {
    SourceDescription::new::<LogstashConfig>("logstash")
}

impl GenerateConfig for LogstashConfig {
    fn generate_config() -> toml::Value {
        toml::Value::try_from(Self {
            address: SocketListenAddr::SocketAddr("0.0.0.0:5044".parse().unwrap()),
            keepalive: None,
            tls: None,
            receive_buffer_bytes: None,
        })
        .unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "logstash")]
impl SourceConfig for LogstashConfig {
    async fn build(&self, cx: SourceContext) -> crate::Result<super::Source> {
        let source = LogstashSource {
            timestamp_converter: types::Conversion::Timestamp(cx.globals.timezone),
        };
        let shutdown_secs = 30;
        let tls = MaybeTlsSettings::from_config(&self.tls, true)?;
        source.run(
            self.address,
            self.keepalive,
            shutdown_secs,
            tls,
            self.receive_buffer_bytes,
            cx.shutdown,
            cx.out,
        )
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn source_type(&self) -> &'static str {
        "logstash"
    }

    fn resources(&self) -> Vec<Resource> {
        vec![self.address.into()]
    }
}

#[derive(Debug, Clone)]
struct LogstashSource {
    timestamp_converter: crate::types::Conversion,
}

impl TcpSource for LogstashSource {
    type Context = ();
    type Error = DecodeError;
    type Decoder = LogstashDecoder;

    fn build_context(&self) -> crate::Result<Self::Context> {
        Ok(())
    }

    fn decoder(&self) -> Self::Decoder {
        LogstashDecoder::new()
    }

    // https://github.com/logstash-plugins/logstash-input-beats/blob/master/PROTOCOL.md#ack-frame-type
    fn build_ack(&self, frame: &LogstashEventFrame) -> Bytes {
        let mut bytes: Vec<u8> = Vec::with_capacity(6);
        bytes.push(frame.protocol.into());
        bytes.push(LogstashFrameType::Ack.into());
        bytes.extend(frame.sequence_number.to_be_bytes().iter());
        Bytes::from(bytes)
    }

    fn build_event(
        &self,
        _context: &Self::Context,
        frame: LogstashEventFrame,
        host: Bytes,
    ) -> Option<Event> {
        let mut log = LogEvent::from(
            frame
                .fields
                .into_iter()
                .map(|(key, value)| (key, Value::from(value)))
                .collect::<BTreeMap<_, _>>(),
        );
        if log.get(log_schema().host_key()).is_none() {
            log.insert(log_schema().host_key(), host);
        }
        if log.get(log_schema().timestamp_key()).is_none() {
            // attempt to parse @timestamp if it exists; otherwise set to receipt time
            let timestamp = log
                .get("@timestamp")
                .and_then(|timestamp| {
                    self.timestamp_converter
                        .convert::<Value>(timestamp.as_bytes())
                        .ok()
                })
                .unwrap_or_else(|| Value::from(chrono::Utc::now()));
            log.insert(log_schema().timestamp_key(), timestamp);
        }
        Some(Event::from(log))
    }
}

#[derive(Debug)]
enum LogstashDecoderReadState {
    ReadProtocol,
    ReadType(LogstashProtocolVersion),
    ReadFrame(LogstashProtocolVersion, LogstashFrameType),
    PendingFrames(VecDeque<LogstashEventFrame>),
}

#[derive(Debug)]
struct LogstashDecoder {
    state: LogstashDecoderReadState,
}

impl LogstashDecoder {
    fn new() -> Self {
        Self {
            state: LogstashDecoderReadState::ReadProtocol,
        }
    }
}

#[derive(Debug, Snafu)]
pub enum DecodeError {
    #[snafu(display("i/o error: {}", source))]
    IO { source: io::Error },
    #[snafu(display("Unknown logstash protocol version: {}", version))]
    UnknownProtocolVersion { version: char },
    #[snafu(display("Unknown logstash protocol message type: {}", frame_type))]
    UnknownFrameType { frame_type: char },
    #[snafu(display("Failed to decode JSON frame: {}", source))]
    JsonFrameFailedDecode { source: serde_json::Error },
    #[snafu(display("Failed to decompress compressed frame: {}", source))]
    DecompressionFailed { source: io::Error },
}

impl TcpIsErrorFatal for DecodeError {
    fn is_error_fatal(&self) -> bool {
        use DecodeError::*;

        match self {
            IO { .. } => true,
            UnknownProtocolVersion { .. } => true,
            UnknownFrameType { .. } => true,
            JsonFrameFailedDecode { .. } => false,
            DecompressionFailed { .. } => false,
        }
    }
}

impl From<io::Error> for DecodeError {
    fn from(source: io::Error) -> Self {
        DecodeError::IO { source }
    }
}

#[derive(Debug, Clone, Copy)]
enum LogstashProtocolVersion {
    V1, // 1
    V2, // 2
}

impl From<LogstashProtocolVersion> for u8 {
    fn from(frame_type: LogstashProtocolVersion) -> u8 {
        use LogstashProtocolVersion::*;

        match frame_type {
            V1 => b'1',
            V2 => b'2',
        }
    }
}

impl TryFrom<u8> for LogstashProtocolVersion {
    type Error = DecodeError;

    fn try_from(frame_type: u8) -> Result<LogstashProtocolVersion, DecodeError> {
        use LogstashProtocolVersion::*;

        match frame_type {
            b'1' => Ok(V1),
            b'2' => Ok(V2),
            version => Err(DecodeError::UnknownProtocolVersion {
                version: version as char,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum LogstashFrameType {
    Ack,        // A
    WindowSize, // W
    Data,       // D
    Json,       // J
    Compressed, // C
}

impl From<LogstashFrameType> for u8 {
    fn from(frame_type: LogstashFrameType) -> u8 {
        use LogstashFrameType::*;

        match frame_type {
            Ack => b'A',
            WindowSize => b'W',
            Data => b'D',
            Json => b'J',
            Compressed => b'C',
        }
    }
}

impl TryFrom<u8> for LogstashFrameType {
    type Error = DecodeError;

    fn try_from(frame_type: u8) -> Result<LogstashFrameType, DecodeError> {
        use LogstashFrameType::*;

        match frame_type {
            b'A' => Ok(Ack),
            b'W' => Ok(WindowSize),
            b'D' => Ok(Data),
            b'J' => Ok(Json),
            b'C' => Ok(Compressed),
            frame_type => Err(DecodeError::UnknownFrameType {
                frame_type: frame_type as char,
            }),
        }
    }
}

/// Normalized event from logstash frame
#[derive(Debug)]
struct LogstashEventFrame {
    protocol: LogstashProtocolVersion,
    sequence_number: u32,
    fields: BTreeMap<String, serde_json::Value>,
}

// Based on spec at: https://github.com/logstash-plugins/logstash-input-beats/blob/master/PROTOCOL.md
// And implementation from logstash: https://github.com/logstash-plugins/logstash-input-beats/blob/27bad62a26a81fc000a9d21495b8dc7174ab63e9/src/main/java/org/logstash/beats/BeatsParser.java
impl Decoder for LogstashDecoder {
    type Item = LogstashEventFrame;
    type Error = DecodeError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // This implements a sort of simple state machine to read the frames from the wire
        //
        // Each matched arm with either:
        // * Return that there is not enough data
        // * Return an error
        // * Read some bytes and advance the state
        loop {
            self.state = match self.state {
                // if we have any unsent frames, send them before reading new logstash frame
                LogstashDecoderReadState::PendingFrames(ref mut frames) => {
                    match frames.pop_front() {
                        Some(frame) => return Ok(Some(frame)),
                        None => LogstashDecoderReadState::ReadProtocol,
                    }
                }
                LogstashDecoderReadState::ReadProtocol => {
                    if src.remaining() < 1 {
                        return Ok(None);
                    }

                    use LogstashProtocolVersion::*;

                    match LogstashProtocolVersion::try_from(src.get_u8())? {
                        V1 => LogstashDecoderReadState::ReadType(LogstashProtocolVersion::V1),
                        V2 => LogstashDecoderReadState::ReadType(LogstashProtocolVersion::V2),
                    }
                }
                LogstashDecoderReadState::ReadType(protocol) => {
                    if src.remaining() < 1 {
                        return Ok(None);
                    }

                    use LogstashFrameType::*;

                    match LogstashFrameType::try_from(src.get_u8())? {
                        WindowSize => LogstashDecoderReadState::ReadFrame(
                            protocol,
                            LogstashFrameType::WindowSize,
                        ),
                        Data => {
                            LogstashDecoderReadState::ReadFrame(protocol, LogstashFrameType::Data)
                        }
                        Json => {
                            LogstashDecoderReadState::ReadFrame(protocol, LogstashFrameType::Json)
                        }
                        Compressed => LogstashDecoderReadState::ReadFrame(
                            protocol,
                            LogstashFrameType::Compressed,
                        ),
                        Ack => {
                            LogstashDecoderReadState::ReadFrame(protocol, LogstashFrameType::Ack)
                        }
                    }
                }
                // The window size indicates how many events the writer will send before waiting
                // for acks. As we forward events as we get them, and ack as they are receieved, we
                // do not need to keep track of this.
                //
                // https://github.com/logstash-plugins/logstash-input-beats/blob/master/PROTOCOL.md#window-size-frame-type
                LogstashDecoderReadState::ReadFrame(_protocol, LogstashFrameType::WindowSize) => {
                    if src.remaining() < 4 {
                        return Ok(None);
                    }

                    let _window_size = src.get_u32();

                    LogstashDecoderReadState::ReadProtocol
                }
                // we shouldn't receive acks from the writer, just skip
                //
                // https://github.com/logstash-plugins/logstash-input-beats/blob/master/PROTOCOL.md#ack-frame-type
                LogstashDecoderReadState::ReadFrame(_protocol, LogstashFrameType::Ack) => {
                    if src.remaining() < 4 {
                        return Ok(None);
                    }

                    let _sequence_number = src.get_u32();

                    LogstashDecoderReadState::ReadProtocol
                }
                // https://github.com/logstash-plugins/logstash-input-beats/blob/master/PROTOCOL.md#data-frame-type
                LogstashDecoderReadState::ReadFrame(protocol, LogstashFrameType::Data) => {
                    let mut rest = src.as_ref();

                    if rest.remaining() < 8 {
                        return Ok(None);
                    }
                    let sequence_number = rest.get_u32();
                    let pair_count = rest.get_u32();

                    let mut fields: BTreeMap<String, serde_json::Value> = BTreeMap::new();
                    for _ in 0..pair_count {
                        if src.remaining() < 4 {
                            return Ok(None);
                        }
                        let key_length = rest.get_u32() as usize;

                        if rest.remaining() < key_length {
                            return Ok(None);
                        }
                        let (key, right) = rest.split_at(key_length);
                        rest = right;

                        if src.remaining() < 4 {
                            return Ok(None);
                        }
                        let value_length = rest.get_u32() as usize;
                        if rest.remaining() < value_length {
                            return Ok(None);
                        }
                        let (value, right) = rest.split_at(value_length);
                        rest = right;

                        fields.insert(
                            String::from_utf8_lossy(key).to_string(),
                            String::from_utf8_lossy(value).into(),
                        );
                    }

                    let remaining = rest.remaining();

                    src.advance(src.remaining() - remaining);

                    let frames = vec![LogstashEventFrame {
                        protocol,
                        sequence_number,
                        fields,
                    }]
                    .into();

                    LogstashDecoderReadState::PendingFrames(frames)
                }
                // https://github.com/logstash-plugins/logstash-input-beats/blob/master/PROTOCOL.md#json-frame-type
                LogstashDecoderReadState::ReadFrame(protocol, LogstashFrameType::Json) => {
                    let mut rest = src.as_ref();

                    if rest.remaining() < 8 {
                        return Ok(None);
                    }
                    let sequence_number = rest.get_u32();
                    let payload_size = rest.get_u32() as usize;

                    if rest.remaining() < payload_size {
                        return Ok(None);
                    }

                    let (slice, right) = rest.split_at(payload_size);
                    rest = right;

                    let fields_result: Result<BTreeMap<String, serde_json::Value>, _> =
                        serde_json::from_slice(slice).context(JsonFrameFailedDecode {});

                    let remaining = rest.remaining();

                    src.advance(src.remaining() - remaining);

                    match fields_result {
                        Ok(fields) => {
                            let frames = vec![LogstashEventFrame {
                                protocol,
                                sequence_number,
                                fields,
                            }]
                            .into();

                            LogstashDecoderReadState::PendingFrames(frames)
                        }
                        Err(err) => return Err(err),
                    }
                }
                // https://github.com/logstash-plugins/logstash-input-beats/blob/master/PROTOCOL.md#compressed-frame-type
                LogstashDecoderReadState::ReadFrame(_protocol, LogstashFrameType::Compressed) => {
                    let mut rest = src.as_ref();

                    if rest.remaining() < 4 {
                        return Ok(None);
                    }
                    let payload_size = rest.get_u32() as usize;

                    if rest.remaining() < payload_size {
                        src.reserve(payload_size as usize);
                        return Ok(None);
                    }

                    let (slice, right) = rest.split_at(payload_size);
                    rest = right;

                    let mut buf = {
                        let mut buf = Vec::new();

                        let res = ZlibDecoder::new(io::Cursor::new(slice))
                            .read_to_end(&mut buf)
                            .context(DecompressionFailed)
                            .map(|_| BytesMut::from(&buf[..]));

                        let remaining = rest.remaining();

                        src.advance(src.remaining() - remaining);

                        res
                    }?;

                    let mut decoder = LogstashDecoder::new();

                    let mut frames = VecDeque::new();

                    while let Some(s) = decoder.decode(&mut buf)? {
                        frames.push_back(s);
                    }

                    LogstashDecoderReadState::PendingFrames(frames)
                }
            };
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<LogstashConfig>();
    }
}

#[cfg(all(test, feature = "logstash-integration-tests"))]
mod integration_tests {
    use super::*;
    use crate::{
        config::SourceContext,
        docker::docker,
        test_util::{collect_n, next_addr_for_ip, trace_init, wait_for_tcp},
        tls::TlsOptions,
        Pipeline,
    };
    use bollard::{
        container::{Config as ContainerConfig, CreateContainerOptions},
        image::{CreateImageOptions, ListImagesOptions},
        models::HostConfig,
        Docker,
    };
    use futures::{channel::mpsc, StreamExt};
    use std::{collections::HashMap, fs::File, io::Write, net::SocketAddr, time::Duration};
    use tokio::time::timeout;
    use uuid::Uuid;

    #[tokio::test]
    async fn beats_heartbeat() {
        trace_init();

        let image = "docker.elastic.co/beats/heartbeat";
        let tag = "7.12.1";

        let docker = docker(None, None).unwrap();

        let (out, address) = source(None).await;

        pull_image(&docker, image, tag).await;

        let dir = tempfile::tempdir().unwrap();
        let mut file = File::create(dir.path().join("heartbeat.yml")).unwrap();
        write!(
            &mut file,
            r#"
heartbeat.monitors:
- type: http
  schedule: '@every 1s'
  urls:
    - https://google.com

output.logstash:
  hosts: ['host.docker.internal:{}']
"#,
            address.port()
        )
        .unwrap();

        let options = Some(CreateContainerOptions {
            name: format!("vector_test_logstash_{}", Uuid::new_v4()),
        });
        let config = ContainerConfig {
            image: Some(format!("{}:{}", image, tag)),
            // adding `-strict.perms=false to the default cmd as otherwise heartbeat was
            // complaining about the file permissions when running in CI
            // https://www.elastic.co/guide/en/beats/libbeat/5.3/config-file-permissions.html
            cmd: Some(vec![
                String::from("-environment=container"),
                String::from("-strict.perms=false"),
            ]),
            host_config: Some(HostConfig {
                network_mode: Some(String::from("host")),
                extra_hosts: Some(vec![String::from("host.docker.internal:host-gateway")]),
                binds: Some(vec![format!(
                    "{}/heartbeat.yml:{}",
                    dir.path().display(),
                    "/usr/share/heartbeat/heartbeat.yml"
                )]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let container = docker.create_container(options, config).await.unwrap();

        docker
            .start_container::<String>(&container.id, None)
            .await
            .unwrap();

        let events = timeout(Duration::from_secs(60), collect_n(out, 1))
            .await
            .unwrap();

        remove_container(&docker, &container.id).await;

        assert!(!events.is_empty());

        let log = events[0].as_log();
        assert_eq!(
            log.get("@metadata.beat"),
            Some(String::from("heartbeat").into()).as_ref()
        );
        assert_eq!(log.get("summary.up"), Some(1.into()).as_ref());
        assert!(log.get("timestamp").is_some());
        assert!(log.get("host").is_some());
    }

    #[tokio::test]
    async fn logstash() {
        trace_init();

        let image = "docker.elastic.co/logstash/logstash";
        let tag = "7.13.1";

        let docker = docker(None, None).unwrap();

        let (out, address) = source(Some(TlsConfig {
            enabled: Some(true),
            options: TlsOptions {
                crt_file: Some("tests/data/host.docker.internal.crt".into()),
                key_file: Some("tests/data/host.docker.internal.key".into()),
                ..Default::default()
            },
        }))
        .await;

        pull_image(&docker, image, tag).await;

        let dir = tempfile::tempdir().unwrap();
        let mut file = File::create(dir.path().join("logstash.conf")).unwrap();
        write!(
            &mut file,
            "{}",
            r#"
input {
  generator {
    count => 5
    message => "Hello World"
  }
}
output {
  lumberjack {
    hosts => "host.docker.internal"
    ssl_certificate => "/tmp/logstash.crt"
    port => PORT
  }
}
"#
            .replace("PORT", &address.port().to_string())
        )
        .unwrap();

        let options = Some(CreateContainerOptions {
            name: format!("vector_test_logstash_{}", Uuid::new_v4()),
        });
        let config = ContainerConfig {
            image: Some(format!("{}:{}", image, tag)),
            host_config: Some(HostConfig {
                network_mode: Some(String::from("host")),
                extra_hosts: Some(vec![String::from("host.docker.internal:host-gateway")]),
                binds: Some(vec![
                    "/dev/null:/usr/share/logstash/config/logstash.yml".to_string(), // tries to contact elasticsearch by default
                    format!(
                        "{}/logstash.conf:{}",
                        dir.path().display(),
                        "/usr/share/logstash/pipeline/logstash.conf"
                    ),
                    format!(
                        "{}/tests/data/host.docker.internal.crt:/tmp/logstash.crt",
                        std::env::current_dir().unwrap().display()
                    ),
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let container = docker.create_container(options, config).await.unwrap();

        docker
            .start_container::<String>(&container.id, None)
            .await
            .unwrap();

        let events = timeout(Duration::from_secs(60), collect_n(out, 1))
            .await
            .unwrap();

        remove_container(&docker, &container.id).await;

        assert!(!events.is_empty());

        let log = events[0].as_log();
        assert!(log
            .get("line")
            .unwrap()
            .to_string_lossy()
            .contains("Hello World"));
        assert!(log.get("host").is_some());
    }

    async fn pull_image(docker: &Docker, image: &str, tag: &str) {
        let mut filters = HashMap::new();
        filters.insert(
            String::from("reference"),
            vec![format!("{}:{}", image, tag)],
        );

        let options = Some(ListImagesOptions {
            filters,
            ..Default::default()
        });

        let images = docker.list_images(options).await.unwrap();
        if images.is_empty() {
            // If not found, pull it
            let options = Some(CreateImageOptions {
                from_image: image,
                tag,
                ..Default::default()
            });

            docker
                .create_image(options, None, None)
                .for_each(|item| async move {
                    let info = item.unwrap();
                    if let Some(error) = info.error {
                        panic!("{:?}", error);
                    }
                })
                .await
        }
    }

    async fn source(tls: Option<TlsConfig>) -> (mpsc::Receiver<Event>, SocketAddr) {
        let (sender, recv) = Pipeline::new_test();
        let address = next_addr_for_ip(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
        tokio::spawn(async move {
            LogstashConfig {
                address: address.into(),
                tls,
                keepalive: None,
                receive_buffer_bytes: None,
            }
            .build(SourceContext::new_test(sender))
            .await
            .unwrap()
            .await
            .unwrap()
        });
        wait_for_tcp(address).await;
        (recv, address)
    }

    async fn remove_container(docker: &Docker, id: &str) {
        trace!("Stopping container.");

        let _ = docker
            .stop_container(id, None)
            .await
            .map_err(|e| error!(%e));

        trace!("Removing container.");

        // Don't panic, as this is unrelated to the test
        let _ = docker
            .remove_container(id, None)
            .await
            .map_err(|e| error!(%e));
    }
}
