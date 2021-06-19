#![cfg(all(feature = "sources-syslog", feature = "sinks-socket"))]

use bytes::Bytes;
use rand::{thread_rng, Rng};
use serde::Deserialize;
use serde_json::Value;
use sinks::socket::{self, SocketSinkConfig};
use sinks::util::{encoding::EncodingConfig, tcp::TcpSinkConfig, Encoding};
use std::{collections::HashMap, fmt, str::FromStr};
#[cfg(unix)]
use tokio::io::AsyncWriteExt;
use tokio_util::codec::BytesCodec;
use vector::{
    config, sinks,
    sources::syslog::{Mode, SyslogConfig},
    test_util::{
        next_addr, random_maps, random_string, send_encodable, send_lines, start_topology,
        trace_init, wait_for_tcp, CountReceiver,
    },
};

#[tokio::test]
async fn test_tcp_syslog() {
    let num_messages: usize = 10000;

    let in_addr = next_addr();
    let out_addr = next_addr();

    let mut config = config::Config::builder();
    config.add_source(
        "in",
        SyslogConfig::from_mode(Mode::Tcp {
            address: in_addr.into(),
            keepalive: None,
            tls: None,
            receive_buffer_bytes: None,
        }),
        None,
        None,
    );
    config.add_sink("out", &["in"], tcp_json_sink(out_addr.to_string()));

    let output_lines = CountReceiver::receive_lines(out_addr);

    let (topology, _crash) = start_topology(config.build().unwrap(), false).await;
    // Wait for server to accept traffic
    wait_for_tcp(in_addr).await;

    let input_messages: Vec<SyslogMessageRfc5424> = (0..num_messages)
        .map(|i| SyslogMessageRfc5424::random(i, 30, 4, 3, 3))
        .collect();

    let input_lines: Vec<String> = input_messages.iter().map(|msg| msg.to_string()).collect();

    send_lines(in_addr, input_lines).await.unwrap();

    // Shut down server
    topology.stop().await;

    let output_lines = output_lines.await;
    assert_eq!(output_lines.len(), num_messages);

    let output_messages: Vec<SyslogMessageRfc5424> = output_lines
        .iter()
        .map(|s| {
            let mut value = Value::from_str(s).unwrap();
            value.as_object_mut().unwrap().remove("hostname"); // Vector adds this field which will cause a parse error.
            value.as_object_mut().unwrap().remove("source_ip"); // Vector adds this field which will cause a parse error.
            serde_json::from_value(value).unwrap()
        })
        .collect();
    assert_eq!(output_messages, input_messages);
}

#[cfg(unix)]
#[tokio::test]
async fn test_unix_stream_syslog() {
    use futures::{stream, SinkExt, StreamExt};
    use tokio::{net::UnixStream, task::yield_now};
    use tokio_util::codec::{FramedWrite, LinesCodec};

    let num_messages: usize = 10000;

    let in_path = tempfile::tempdir().unwrap().into_path().join("stream_test");
    let out_addr = next_addr();

    let mut config = config::Config::builder();
    config.add_source(
        "in",
        SyslogConfig::from_mode(Mode::Unix {
            path: in_path.clone(),
        }),
        None,
        None,
    );
    config.add_sink("out", &["in"], tcp_json_sink(out_addr.to_string()));

    let output_lines = CountReceiver::receive_lines(out_addr);

    let (topology, _crash) = start_topology(config.build().unwrap(), false).await;

    // Wait for server to accept traffic
    while std::os::unix::net::UnixStream::connect(&in_path).is_err() {
        yield_now().await;
    }

    let input_messages: Vec<SyslogMessageRfc5424> = (0..num_messages)
        .map(|i| SyslogMessageRfc5424::random(i, 30, 4, 3, 3))
        .collect();

    let stream = UnixStream::connect(&in_path).await.unwrap();
    let mut sink = FramedWrite::new(stream, LinesCodec::new());

    let lines: Vec<String> = input_messages.iter().map(|msg| msg.to_string()).collect();
    let mut lines = stream::iter(lines).map(Ok);
    sink.send_all(&mut lines).await.unwrap();

    let stream = sink.get_mut();
    stream.shutdown().await.unwrap();

    // Otherwise some lines will be lost
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

    // Shut down server
    topology.stop().await;

    let output_lines = output_lines.await;
    assert_eq!(output_lines.len(), num_messages);

    let output_messages: Vec<SyslogMessageRfc5424> = output_lines
        .iter()
        .map(|s| {
            let mut value = Value::from_str(s).unwrap();
            value.as_object_mut().unwrap().remove("hostname"); // Vector adds this field which will cause a parse error.
            value.as_object_mut().unwrap().remove("source_ip"); // Vector adds this field which will cause a parse error.
            serde_json::from_value(value).unwrap()
        })
        .collect();
    assert_eq!(output_messages, input_messages);
}

#[tokio::test]
async fn test_octet_counting_syslog() {
    trace_init();

    let num_messages: usize = 10000;

    let in_addr = next_addr();
    let out_addr = next_addr();

    let mut config = config::Config::builder();
    config.add_source(
        "in",
        SyslogConfig::from_mode(Mode::Tcp {
            address: in_addr.into(),
            keepalive: None,
            tls: None,
            receive_buffer_bytes: None,
        }),
        None,
        None,
    );
    config.add_sink("out", &["in"], tcp_json_sink(out_addr.to_string()));

    let output_lines = CountReceiver::receive_lines(out_addr);

    let (topology, _crash) = start_topology(config.build().unwrap(), false).await;
    // Wait for server to accept traffic
    wait_for_tcp(in_addr).await;

    let input_messages: Vec<SyslogMessageRfc5424> = (0..num_messages)
        .map(|i| {
            let mut msg = SyslogMessageRfc5424::random(i, 30, 4, 3, 3);
            msg.message.push('\n');
            msg.message.push_str(&random_string(30));
            msg
        })
        .collect();

    let codec = BytesCodec::new();
    let input_lines: Vec<Bytes> = input_messages
        .iter()
        .map(|msg| {
            let s = msg.to_string();
            format!("{} {}", s.len(), s).into()
        })
        .collect();

    send_encodable(in_addr, codec, input_lines).await.unwrap();

    // Shut down server
    topology.stop().await;

    let output_lines = output_lines.await;
    assert_eq!(output_lines.len(), num_messages);

    let output_messages: Vec<SyslogMessageRfc5424> = output_lines
        .iter()
        .map(|s| {
            let mut value = Value::from_str(s).unwrap();
            value.as_object_mut().unwrap().remove("hostname"); // Vector adds this field which will cause a parse error.
            value.as_object_mut().unwrap().remove("source_ip"); // Vector adds this field which will cause a parse error.
            serde_json::from_value(value).unwrap()
        })
        .collect();
    assert_eq!(output_messages, input_messages);
}

#[derive(Deserialize, PartialEq, Clone, Debug)]
struct SyslogMessageRfc5424 {
    msgid: String,
    severity: Severity,
    facility: Facility,
    version: u8,
    timestamp: String,
    host: String,
    source_type: String,
    appname: String,
    procid: usize,
    message: String,
    #[serde(flatten)]
    structured_data: StructuredData,
}

impl SyslogMessageRfc5424 {
    fn random(
        id: usize,
        msg_len: usize,
        field_len: usize,
        max_map_size: usize,
        max_children: usize,
    ) -> Self {
        let msg = random_string(msg_len);
        let structured_data = random_structured_data(max_map_size, max_children, field_len);

        let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        //"secfrac" can contain up to 6 digits, but TCP sinks uses `AutoSi`

        Self {
            msgid: format!("test{}", id),
            severity: Severity::LOG_INFO,
            facility: Facility::LOG_USER,
            version: 1,
            timestamp,
            host: "hogwarts".to_owned(),
            source_type: "syslog".to_owned(),
            appname: "harry".to_owned(),
            procid: thread_rng().gen_range(0..32768),
            structured_data,
            message: msg,
        }
    }
}

impl fmt::Display for SyslogMessageRfc5424 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "<{}>{} {} {} {} {} {} {} {}",
            encode_priority(self.severity, self.facility),
            self.version,
            self.timestamp,
            self.host,
            self.appname,
            self.procid,
            self.msgid,
            format_structured_data_rfc5424(&self.structured_data),
            self.message
        )
    }
}

#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
#[derive(Copy, Clone, Deserialize, PartialEq, Debug)]
pub enum Severity {
    #[serde(rename(deserialize = "emergency"))]
    LOG_EMERG,
    #[serde(rename(deserialize = "alert"))]
    LOG_ALERT,
    #[serde(rename(deserialize = "critical"))]
    LOG_CRIT,
    #[serde(rename(deserialize = "error"))]
    LOG_ERR,
    #[serde(rename(deserialize = "warn"))]
    LOG_WARNING,
    #[serde(rename(deserialize = "notice"))]
    LOG_NOTICE,
    #[serde(rename(deserialize = "info"))]
    LOG_INFO,
    #[serde(rename(deserialize = "debug"))]
    LOG_DEBUG,
}

#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
#[derive(Copy, Clone, PartialEq, Deserialize, Debug)]
pub enum Facility {
    #[serde(rename(deserialize = "kernel"))]
    LOG_KERN = 0 << 3,
    #[serde(rename(deserialize = "user"))]
    LOG_USER = 1 << 3,
    #[serde(rename(deserialize = "mail"))]
    LOG_MAIL = 2 << 3,
    #[serde(rename(deserialize = "daemon"))]
    LOG_DAEMON = 3 << 3,
    #[serde(rename(deserialize = "auth"))]
    LOG_AUTH = 4 << 3,
    #[serde(rename(deserialize = "syslog"))]
    LOG_SYSLOG = 5 << 3,
}

type StructuredData = HashMap<String, HashMap<String, String>>;

fn random_structured_data(
    max_map_size: usize,
    max_children: usize,
    field_len: usize,
) -> StructuredData {
    let amount = thread_rng().gen_range(0..max_children);

    random_maps(max_map_size, field_len)
        .filter(|m| !m.is_empty()) //syslog_rfc5424 ignores empty maps, tested separately
        .take(amount)
        .enumerate()
        .map(|(i, map)| (format!("id{}", i), map))
        .collect()
}

fn format_structured_data_rfc5424(data: &StructuredData) -> String {
    if data.is_empty() {
        "-".to_string()
    } else {
        let mut res = String::new();
        for (id, params) in data {
            res = res + "[" + id;
            for (name, value) in params {
                res = res + " " + name + "=\"" + value + "\"";
            }
            res += "]";
        }

        res
    }
}

fn encode_priority(severity: Severity, facility: Facility) -> u8 {
    facility as u8 | severity as u8
}

fn tcp_json_sink(address: String) -> SocketSinkConfig {
    SocketSinkConfig::new(
        socket::Mode::Tcp(TcpSinkConfig::from_address(address)),
        EncodingConfig::from(Encoding::Json),
    )
}
