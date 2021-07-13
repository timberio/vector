use super::util::{SocketListenAddr, TcpSource};
#[cfg(unix)]
use crate::sources::util::build_unix_stream_source;
use crate::udp;
use crate::{
    config::{
        log_schema, DataType, GenerateConfig, Resource, SourceConfig, SourceContext,
        SourceDescription,
    },
    event::{Event, Value},
    internal_events::{SocketMode, SyslogEventReceived, SyslogUdpReadError, SyslogUtf8Error},
    shutdown::ShutdownSignal,
    sources::util::decoding,
    tcp::TcpKeepaliveConfig,
    tls::{MaybeTlsSettings, TlsConfig},
    Pipeline,
};
use bytes::{Buf, Bytes, BytesMut};
use chrono::{Datelike, Utc};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::path::PathBuf;
use std::{io, net::SocketAddr, sync::Arc};
use syslog_loose::{IncompleteDate, Message, ProcId, Protocol};
use tokio::net::UdpSocket;
use tokio_util::{
    codec::{BytesCodec, Decoder, LinesCodec, LinesCodecError},
    udp::UdpFramed,
};

#[derive(Deserialize, Serialize, Debug)]
// TODO: add back when serde-rs/serde#1358 is addressed
// #[serde(deny_unknown_fields)]
pub struct SyslogConfig {
    #[serde(flatten)]
    mode: Mode,
    #[serde(default = "default_max_length")]
    max_length: usize,
    /// The host key of the log. (This differs from `hostname`)
    host_key: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Mode {
    Tcp {
        address: SocketListenAddr,
        keepalive: Option<TcpKeepaliveConfig>,
        tls: Option<TlsConfig>,
        receive_buffer_bytes: Option<usize>,
    },
    Udp {
        address: SocketAddr,
        receive_buffer_bytes: Option<usize>,
    },
    #[cfg(unix)]
    Unix { path: PathBuf },
}

pub fn default_max_length() -> usize {
    bytesize::kib(100u64) as usize
}

impl SyslogConfig {
    pub fn from_mode(mode: Mode) -> Self {
        Self {
            mode,
            host_key: None,
            max_length: default_max_length(),
        }
    }
}

inventory::submit! {
    SourceDescription::new::<SyslogConfig>("syslog")
}

impl GenerateConfig for SyslogConfig {
    fn generate_config() -> toml::Value {
        toml::Value::try_from(Self {
            mode: Mode::Tcp {
                address: SocketListenAddr::SocketAddr("0.0.0.0:514".parse().unwrap()),
                keepalive: None,
                tls: None,
                receive_buffer_bytes: None,
            },
            host_key: None,
            max_length: default_max_length(),
        })
        .unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "syslog")]
impl SourceConfig for SyslogConfig {
    async fn build(&self, cx: SourceContext) -> crate::Result<super::Source> {
        let host_key = self
            .host_key
            .clone()
            .unwrap_or_else(|| log_schema().host_key().to_string());

        match self.mode.clone() {
            Mode::Tcp {
                address,
                keepalive,
                tls,
                receive_buffer_bytes,
            } => {
                let source = Arc::new(SyslogTcpSource {
                    max_length: self.max_length,
                    host_key,
                });
                let shutdown_secs = 30;
                let tls = MaybeTlsSettings::from_config(&tls, true)?;
                source.run(
                    address,
                    keepalive,
                    shutdown_secs,
                    tls,
                    receive_buffer_bytes,
                    cx.shutdown,
                    cx.out,
                )
            }
            Mode::Udp {
                address,
                receive_buffer_bytes,
            } => Ok(udp(
                address,
                self.max_length,
                host_key,
                receive_buffer_bytes,
                cx.shutdown,
                cx.out,
            )),
            #[cfg(unix)]
            Mode::Unix { path } => Ok(build_unix_stream_source(
                path,
                SyslogDecoder::new(self.max_length),
                host_key,
                cx.shutdown,
                cx.out,
                |bytes, host_key, default_host| {
                    let bytes: &[u8] = &bytes;
                    let line = std::str::from_utf8(bytes).unwrap(); // Guaranteed to be valid UTF-8 by `LinesCodec`.
                    let mut event = event_from_str(line);
                    enrich_syslog_event(&mut event, host_key, default_host, bytes.len());
                    Some(event)
                },
            )),
        }
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn source_type(&self) -> &'static str {
        "syslog"
    }

    fn resources(&self) -> Vec<Resource> {
        match self.mode.clone() {
            Mode::Tcp { address, .. } => vec![address.into()],
            Mode::Udp { address, .. } => vec![Resource::udp(address)],
            #[cfg(unix)]
            Mode::Unix { .. } => vec![],
        }
    }
}

struct SyslogParser;

impl decoding::Parser for SyslogParser {
    fn parse(&self, bytes: Bytes) -> crate::Result<Event> {
        let bytes: &[u8] = &bytes;
        let line = std::str::from_utf8(bytes)?;
        Ok(event_from_str(line))
    }
}

#[derive(Debug, Clone)]
struct SyslogTcpSource {
    max_length: usize,
    host_key: String,
}

impl TcpSource for SyslogTcpSource {
    type Error = LinesCodecError;
    type Item = Event;
    type Decoder = decoding::Decoder<Self::Error, SyslogParser>;

    fn create_decoder(&self) -> Self::Decoder {
        decoding::Decoder::new(Box::new(SyslogDecoder::new(self.max_length)), SyslogParser)
    }

    fn handle_event(&self, event: &mut Event, host: Bytes, byte_size: usize) {
        enrich_syslog_event(event, &self.host_key, Some(host), byte_size);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    NotDiscarding,
    Discarding(usize),
    DiscardingToEol,
}

/// Decodes according to `Octet Counting` in https://tools.ietf.org/html/rfc6587
#[derive(Clone, Debug)]
struct SyslogDecoder {
    other: LinesCodec,
    octet_decoding: Option<State>,
}

impl SyslogDecoder {
    fn new(max_length: usize) -> Self {
        Self {
            other: LinesCodec::new_with_max_length(max_length),
            octet_decoding: None,
        }
    }

    fn octet_decode(
        &mut self,
        state: State,
        src: &mut BytesMut,
    ) -> Result<Option<Bytes>, LinesCodecError> {
        // Encoding scheme:
        //
        // len ' ' data
        // |    |  | len number of bytes that contain syslog message
        // |    |
        // |    | Separating whitespace
        // |
        // | ASCII decimal number of unknown length

        let space_pos = src.iter().position(|&b| b == b' ');

        // If we are discarding, discard to the next newline.
        let newline_pos = src.iter().position(|&b| b == b'\n');

        match (state, newline_pos, space_pos) {
            (State::Discarding(chars), _, _) if src.len() >= chars => {
                // We have a certain number of chars to discard.
                // There are enough chars in this frame to discard
                src.advance(chars);
                self.octet_decoding = None;
                Err(LinesCodecError::Io(io::Error::new(
                    io::ErrorKind::Other,
                    "Frame length limit exceeded",
                )))
            }

            (State::Discarding(chars), _, _) => {
                // We have a certain number of chars to discard.
                // There aren't enough in this frame so we need to discard
                // The entire frame and adjust the amount to discard accordingly.
                self.octet_decoding = Some(State::Discarding(src.len() - chars));
                src.advance(src.len());
                Ok(None)
            }

            (State::DiscardingToEol, Some(offset), _) => {
                // When discarding we keep discarding to the next newline.
                src.advance(offset + 1);
                self.octet_decoding = None;
                Err(LinesCodecError::Io(io::Error::new(
                    io::ErrorKind::Other,
                    "Frame length limit exceeded",
                )))
            }

            (State::DiscardingToEol, None, _) => {
                // There is no newline in this frame. Since we don't have a set number of
                // chars we want to discard, we need to discard to the next newline.
                // Advance as far as we can to discard the entire frame.
                src.advance(src.len());
                Ok(None)
            }

            (State::NotDiscarding, _, Some(space_pos)) if space_pos < self.other.max_length() => {
                // Everything looks good. We aren't discarding, we have a space that is not beyond our
                // maximum length. Attempt to parse the bytes as a number which will hopefully
                // give us a sensible length for our message.
                let len: usize = match std::str::from_utf8(&src[..space_pos])
                    .map_err(|_| ())
                    .and_then(|num| num.parse().map_err(|_| ()))
                {
                    Ok(len) => len,
                    Err(_) => {
                        // It was not a sensible number.
                        // Advance the buffer past the erroneous bytes
                        // to prevent us getting stuck in an infinite loop.
                        src.advance(space_pos + 1);
                        self.octet_decoding = None;
                        return Err(LinesCodecError::Io(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Unable to decode message len as number",
                        )));
                    }
                };

                let from = space_pos + 1;
                let to = from + len;

                if len > self.other.max_length() {
                    // The length is greater than we want.
                    // We need to discard the entire message.
                    self.octet_decoding = Some(State::Discarding(len));
                    src.advance(space_pos + 1);

                    Ok(None)
                } else if let Some(msg) = src.get(from..to) {
                    let bytes = Bytes::copy_from_slice(msg);
                    src.advance(to);
                    self.octet_decoding = None;
                    Ok(Some(bytes))
                } else {
                    // We have an acceptable number of bytes in this message, but all the data
                    // was not in the frame, return None to indicate we want more data before we
                    // do anything else.
                    Ok(None)
                }
            }

            (State::NotDiscarding, Some(newline_pos), _) => {
                // Beyond maximum length, advance to the newline.
                src.advance(newline_pos + 1);
                Err(LinesCodecError::Io(io::Error::new(
                    io::ErrorKind::Other,
                    "Frame length limit exceeded",
                )))
            }

            (State::NotDiscarding, None, _) if src.len() < self.other.max_length() => {
                // We aren't discarding, but there is no useful character to tell us what to do next,
                // we are still not beyond the max length, so just return None to indicate we need to
                // wait for more data.
                Ok(None)
            }

            (State::NotDiscarding, None, _) => {
                // There is no newline in this frame and we have more data than we want to handle.
                // Advance as far as we can to discard the entire frame.
                self.octet_decoding = Some(State::DiscardingToEol);
                src.advance(src.len());
                Ok(None)
            }
        }
    }

    /// None if this is not octet counting encoded
    fn checked_decode(
        &mut self,
        src: &mut BytesMut,
    ) -> Option<Result<Option<Bytes>, LinesCodecError>> {
        if let Some(&first_byte) = src.get(0) {
            if (49..=57).contains(&first_byte) {
                // First character is non zero number so we can assume that
                // octet count framing is used.
                trace!("Octet counting encoded event detected.");
                self.octet_decoding = Some(State::NotDiscarding);
            }
        }

        self.octet_decoding
            .map(|state| self.octet_decode(state, src))
    }
}

impl Decoder for SyslogDecoder {
    type Item = Bytes;
    type Error = LinesCodecError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(ret) = self.checked_decode(src) {
            ret
        } else {
            // Octet counting isn't used so fallback to newline codec.
            self.other
                .decode(src)
                .map(|line| line.map(|line| line.into()))
        }
    }

    fn decode_eof(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(ret) = self.checked_decode(buf) {
            ret
        } else {
            // Octet counting isn't used so fallback to newline codec.
            self.other
                .decode_eof(buf)
                .map(|line| line.map(|line| line.into()))
        }
    }
}

pub fn udp(
    addr: SocketAddr,
    _max_length: usize,
    host_key: String,
    receive_buffer_bytes: Option<usize>,
    shutdown: ShutdownSignal,
    out: Pipeline,
) -> super::Source {
    let out = out.sink_map_err(|error| error!(message = "Error sending line.", %error));

    Box::pin(async move {
        let socket = UdpSocket::bind(&addr)
            .await
            .expect("Failed to bind to UDP listener socket");

        if let Some(receive_buffer_bytes) = receive_buffer_bytes {
            if let Err(error) = udp::set_receive_buffer_size(&socket, receive_buffer_bytes) {
                warn!(message = "Failed configuring receive buffer size on UDP socket.", %error);
            }
        }

        info!(
            message = "Listening.",
            addr = %addr,
            r#type = "udp"
        );

        let _ = UdpFramed::new(socket, BytesCodec::new())
            .take_until(shutdown)
            .filter_map(|frame| {
                let host_key = host_key.clone();
                async move {
                    match frame {
                        Ok((bytes, received_from)) => {
                            let received_from = received_from.ip().to_string().into();

                            std::str::from_utf8(&bytes)
                                .map_err(|error| {
                                    emit!(SyslogUtf8Error {
                                        mode: SocketMode::Udp,
                                        error
                                    })
                                })
                                .ok()
                                .map(|s| {
                                    let mut event = event_from_str(s);
                                    enrich_syslog_event(
                                        &mut event,
                                        &host_key,
                                        Some(received_from),
                                        bytes.len(),
                                    );
                                    Ok(event)
                                })
                        }
                        Err(error) => {
                            emit!(SyslogUdpReadError { error });
                            None
                        }
                    }
                }
            })
            .forward(out)
            .await;

        info!("Finished sending.");
        Ok(())
    })
}

/// Function used to resolve the year for syslog messages that don't include the year.
/// If the current month is January, and the syslog message is for December, it will take the previous year.
/// Otherwise, take the current year.
fn resolve_year((month, _date, _hour, _min, _sec): IncompleteDate) -> i32 {
    let now = Utc::now();
    if now.month() == 1 && month == 12 {
        now.year() - 1
    } else {
        now.year()
    }
}

/**
* Function to pass to build_unix_stream_source, specific to the Unix mode of the syslog source.
* Handles the logic of parsing and decoding the syslog message format.
**/
// TODO: many more cases to handle:
// octet framing (i.e. num bytes as ascii string prefix) with and without delimiters
// null byte delimiter in place of newline
fn event_from_str(line: &str) -> Event {
    let line = line.trim();
    let parsed = syslog_loose::parse_message_with_year(line, resolve_year);
    let mut event = Event::from(parsed.msg);

    insert_fields_from_syslog(&mut event, parsed);

    event
}

fn enrich_syslog_event(
    event: &mut Event,
    host_key: &str,
    default_host: Option<Bytes>,
    byte_size: usize,
) {
    let log = event.as_mut_log();

    log.insert(log_schema().source_type_key(), Bytes::from("syslog"));

    if let Some(default_host) = &default_host {
        log.insert("source_ip", default_host.clone());
    }

    let parsed_hostname = log.get("hostname").map(|hostname| hostname.as_bytes());
    if let Some(parsed_host) = parsed_hostname.or(default_host) {
        log.insert(host_key, parsed_host);
    }

    let timestamp = log
        .get("timestamp")
        .and_then(|timestamp| timestamp.as_timestamp().copied())
        .unwrap_or_else(Utc::now);
    log.insert(log_schema().timestamp_key(), timestamp);

    emit!(SyslogEventReceived { byte_size });

    trace!(
        message = "Processing one event.",
        event = ?event
    );
}

fn insert_fields_from_syslog(event: &mut Event, parsed: Message<&str>) {
    let log = event.as_mut_log();

    if let Some(host) = parsed.hostname {
        log.insert("hostname", host.to_string());
    }
    if let Some(severity) = parsed.severity {
        log.insert("severity", severity.as_str().to_owned());
    }
    if let Some(facility) = parsed.facility {
        log.insert("facility", facility.as_str().to_owned());
    }
    if let Protocol::RFC5424(version) = parsed.protocol {
        log.insert("version", version as i64);
    }
    if let Some(app_name) = parsed.appname {
        log.insert("appname", app_name.to_owned());
    }
    if let Some(msg_id) = parsed.msgid {
        log.insert("msgid", msg_id.to_owned());
    }
    if let Some(procid) = parsed.procid {
        let value: Value = match procid {
            ProcId::PID(pid) => pid.into(),
            ProcId::Name(name) => name.to_string().into(),
        };
        log.insert("procid", value);
    }

    for element in parsed.structured_data.into_iter() {
        for (name, value) in element.params.into_iter() {
            let key = format!("{}.{}", element.id, name);
            log.insert(key, value.to_string());
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{config::log_schema, event::Event};
    use bytes::BufMut;
    use chrono::prelude::*;
    use shared::assert_event_data_eq;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<SyslogConfig>();
    }

    #[test]
    fn config_tcp() {
        let config: SyslogConfig = toml::from_str(
            r#"
            mode = "tcp"
            address = "127.0.0.1:1235"
          "#,
        )
        .unwrap();
        assert!(matches!(config.mode, Mode::Tcp { .. }));
    }

    #[test]
    fn config_tcp_with_receive_buffer_size() {
        let config: SyslogConfig = toml::from_str(
            r#"
            mode = "tcp"
            address = "127.0.0.1:1235"
            receive_buffer_bytes = 256
          "#,
        )
        .unwrap();

        let receive_buffer_bytes = match config.mode {
            Mode::Tcp {
                receive_buffer_bytes,
                ..
            } => receive_buffer_bytes,
            _ => panic!("expected Mode::Tcp"),
        };

        assert_eq!(receive_buffer_bytes, Some(256));
    }

    #[test]
    fn config_tcp_keepalive_empty() {
        let config: SyslogConfig = toml::from_str(
            r#"
            mode = "tcp"
            address = "127.0.0.1:1235"
          "#,
        )
        .unwrap();

        let keepalive = match config.mode {
            Mode::Tcp { keepalive, .. } => keepalive,
            _ => panic!("expected Mode::Tcp"),
        };

        assert_eq!(keepalive, None);
    }

    #[test]
    fn config_tcp_keepalive_full() {
        let config: SyslogConfig = toml::from_str(
            r#"
            mode = "tcp"
            address = "127.0.0.1:1235"
            keepalive.time_secs = 7200
          "#,
        )
        .unwrap();

        let keepalive = match config.mode {
            Mode::Tcp { keepalive, .. } => keepalive,
            _ => panic!("expected Mode::Tcp"),
        };

        let keepalive = keepalive.expect("keepalive config not set");

        assert_eq!(keepalive.time_secs, Some(7200));
    }

    #[test]
    fn config_udp() {
        let config: SyslogConfig = toml::from_str(
            r#"
            mode = "udp"
            address = "127.0.0.1:1235"
            max_length = 32187
          "#,
        )
        .unwrap();
        assert!(matches!(config.mode, Mode::Udp { .. }));
    }

    #[test]
    fn config_udp_with_receive_buffer_size() {
        let config: SyslogConfig = toml::from_str(
            r#"
            mode = "udp"
            address = "127.0.0.1:1235"
            max_length = 32187
            receive_buffer_bytes = 256
          "#,
        )
        .unwrap();

        let receive_buffer_bytes = match config.mode {
            Mode::Udp {
                receive_buffer_bytes,
                ..
            } => receive_buffer_bytes,
            _ => panic!("expected Mode::Udp"),
        };

        assert_eq!(receive_buffer_bytes, Some(256));
    }

    #[cfg(unix)]
    #[test]
    fn config_unix() {
        let config: SyslogConfig = toml::from_str(
            r#"
            mode = "unix"
            path = "127.0.0.1:1235"
          "#,
        )
        .unwrap();
        assert!(matches!(config.mode, Mode::Unix { .. }));
    }

    fn enriched_syslog(
        line: &str,
        host_key: &str,
        default_host: Option<Bytes>,
        byte_size: usize,
    ) -> Event {
        let mut event = event_from_str(line);
        enrich_syslog_event(&mut event, host_key, default_host, byte_size);
        event
    }

    #[test]
    fn syslog_ng_network_syslog_protocol() {
        // this should also match rsyslog omfwd with template=RSYSLOG_SyslogProtocol23Format
        let msg = "i am foobar";
        let raw = format!(
            r#"<13>1 2019-02-13T19:48:34+00:00 74794bfb6795 root 8449 - {}{} {}"#,
            r#"[meta sequenceId="1" sysUpTime="37" language="EN"]"#,
            r#"[origin ip="192.168.0.1" software="test"]"#,
            msg
        );

        let mut expected = Event::from(msg);

        {
            let expected = expected.as_mut_log();
            expected.insert(
                log_schema().timestamp_key(),
                chrono::Utc.ymd(2019, 2, 13).and_hms(19, 48, 34),
            );
            expected.insert(log_schema().source_type_key(), "syslog");
            expected.insert("host", "74794bfb6795");
            expected.insert("hostname", "74794bfb6795");

            expected.insert("meta.sequenceId", "1");
            expected.insert("meta.sysUpTime", "37");
            expected.insert("meta.language", "EN");
            expected.insert("origin.software", "test");
            expected.insert("origin.ip", "192.168.0.1");

            expected.insert("severity", "notice");
            expected.insert("facility", "user");
            expected.insert("version", 1);
            expected.insert("appname", "root");
            expected.insert("procid", 8449);
        }

        assert_event_data_eq!(
            enriched_syslog(&raw, &"host".to_string(), None, raw.len()),
            expected
        );
    }

    #[test]
    fn handles_incorrect_sd_element() {
        let msg = "qwerty";
        let raw = format!(
            r#"<13>1 2019-02-13T19:48:34+00:00 74794bfb6795 root 8449 - {} {}"#,
            r#"[incorrect x]"#, msg
        );

        let mut expected = Event::from(msg);
        {
            let expected = expected.as_mut_log();
            expected.insert(
                log_schema().timestamp_key(),
                chrono::Utc.ymd(2019, 2, 13).and_hms(19, 48, 34),
            );
            expected.insert(log_schema().host_key(), "74794bfb6795");
            expected.insert("hostname", "74794bfb6795");
            expected.insert(log_schema().source_type_key(), "syslog");
            expected.insert("severity", "notice");
            expected.insert("facility", "user");
            expected.insert("version", 1);
            expected.insert("appname", "root");
            expected.insert("procid", 8449);
        }

        let event = enriched_syslog(&raw, &"host".to_string(), None, raw.len());
        assert_event_data_eq!(event, expected);

        let raw = format!(
            r#"<13>1 2019-02-13T19:48:34+00:00 74794bfb6795 root 8449 - {} {}"#,
            r#"[incorrect x=]"#, msg
        );

        let event = enriched_syslog(&raw, &"host".to_string(), None, raw.len());
        assert_event_data_eq!(event, expected);
    }

    #[test]
    fn handles_empty_sd_element() {
        fn there_is_map_called_empty(event: Event) -> bool {
            event
                .as_log()
                .all_fields()
                .find(|(key, _)| (&key[..]).starts_with("empty"))
                == None
        }

        let msg = format!(
            r#"<13>1 2019-02-13T19:48:34+00:00 74794bfb6795 root 8449 - {} qwerty"#,
            r#"[empty]"#
        );

        let event = enriched_syslog(&msg, &"host".to_string(), None, msg.len());
        assert!(there_is_map_called_empty(event));

        let msg = format!(
            r#"<13>1 2019-02-13T19:48:34+00:00 74794bfb6795 root 8449 - {} qwerty"#,
            r#"[non_empty x="1"][empty]"#
        );

        let event = enriched_syslog(&msg, &"host".to_string(), None, msg.len());
        assert!(there_is_map_called_empty(event));

        let msg = format!(
            r#"<13>1 2019-02-13T19:48:34+00:00 74794bfb6795 root 8449 - {} qwerty"#,
            r#"[empty][non_empty x="1"]"#
        );

        let event = enriched_syslog(&msg, &"host".to_string(), None, msg.len());
        assert!(there_is_map_called_empty(event));

        let msg = format!(
            r#"<13>1 2019-02-13T19:48:34+00:00 74794bfb6795 root 8449 - {} qwerty"#,
            r#"[empty not_really="testing the test"]"#
        );

        let event = enriched_syslog(&msg, &"host".to_string(), None, msg.len());
        assert!(!there_is_map_called_empty(event));
    }

    #[test]
    fn handles_weird_whitespace() {
        // this should also match rsyslog omfwd with template=RSYSLOG_SyslogProtocol23Format
        let raw = r#"
            <13>1 2019-02-13T19:48:34+00:00 74794bfb6795 root 8449 - [meta sequenceId="1"] i am foobar
            "#;
        let cleaned = r#"<13>1 2019-02-13T19:48:34+00:00 74794bfb6795 root 8449 - [meta sequenceId="1"] i am foobar"#;

        assert_event_data_eq!(
            enriched_syslog(&raw, &"host".to_string(), None, raw.len()),
            enriched_syslog(&cleaned, &"host".to_string(), None, cleaned.len())
        );
    }

    #[test]
    fn syslog_ng_default_network() {
        let msg = "i am foobar";
        let raw = format!(r#"<13>Feb 13 20:07:26 74794bfb6795 root[8539]: {}"#, msg);
        let event = enriched_syslog(&raw, &"host".to_string(), None, raw.len());

        let mut expected = Event::from(msg);
        {
            let value = event.as_log().get("timestamp").unwrap();
            let year = value.as_timestamp().unwrap().naive_local().year();

            let expected = expected.as_mut_log();
            let expected_date: DateTime<Utc> =
                chrono::Local.ymd(year, 2, 13).and_hms(20, 7, 26).into();
            expected.insert(log_schema().timestamp_key(), expected_date);
            expected.insert(log_schema().host_key(), "74794bfb6795");
            expected.insert(log_schema().source_type_key(), "syslog");
            expected.insert("hostname", "74794bfb6795");
            expected.insert("severity", "notice");
            expected.insert("facility", "user");
            expected.insert("appname", "root");
            expected.insert("procid", 8539);
        }

        assert_event_data_eq!(event, expected);
    }

    #[test]
    fn rsyslog_omfwd_tcp_default() {
        let msg = "start";
        let raw = format!(
            r#"<190>Feb 13 21:31:56 74794bfb6795 liblogging-stdlog:  [origin software="rsyslogd" swVersion="8.24.0" x-pid="8979" x-info="http://www.rsyslog.com"] {}"#,
            msg
        );
        let event = enriched_syslog(&raw, &"host".to_string(), None, raw.len());

        let mut expected = Event::from(msg);
        {
            let value = event.as_log().get("timestamp").unwrap();
            let year = value.as_timestamp().unwrap().naive_local().year();

            let expected = expected.as_mut_log();
            let expected_date: DateTime<Utc> =
                chrono::Local.ymd(year, 2, 13).and_hms(21, 31, 56).into();
            expected.insert(log_schema().timestamp_key(), expected_date);
            expected.insert(log_schema().source_type_key(), "syslog");
            expected.insert("host", "74794bfb6795");
            expected.insert("hostname", "74794bfb6795");
            expected.insert("severity", "info");
            expected.insert("facility", "local7");
            expected.insert("appname", "liblogging-stdlog");
            expected.insert("origin.software", "rsyslogd");
            expected.insert("origin.swVersion", "8.24.0");
            expected.insert("origin.x-pid", "8979");
            expected.insert("origin.x-info", "http://www.rsyslog.com");
        }

        assert_event_data_eq!(event, expected);
    }

    #[test]
    fn rsyslog_omfwd_tcp_forward_format() {
        let msg = "start";
        let raw = format!(
            r#"<190>2019-02-13T21:53:30.605850+00:00 74794bfb6795 liblogging-stdlog:  [origin software="rsyslogd" swVersion="8.24.0" x-pid="9043" x-info="http://www.rsyslog.com"] {}"#,
            msg
        );

        let mut expected = Event::from(msg);
        {
            let expected = expected.as_mut_log();
            expected.insert(
                log_schema().timestamp_key(),
                chrono::Utc
                    .ymd(2019, 2, 13)
                    .and_hms_micro(21, 53, 30, 605_850),
            );
            expected.insert(log_schema().source_type_key(), "syslog");
            expected.insert("host", "74794bfb6795");
            expected.insert("hostname", "74794bfb6795");
            expected.insert("severity", "info");
            expected.insert("facility", "local7");
            expected.insert("appname", "liblogging-stdlog");
            expected.insert("origin.software", "rsyslogd");
            expected.insert("origin.swVersion", "8.24.0");
            expected.insert("origin.x-pid", "9043");
            expected.insert("origin.x-info", "http://www.rsyslog.com");
        }

        assert_event_data_eq!(
            enriched_syslog(&raw, &"host".to_string(), None, raw.len()),
            expected
        );
    }

    #[test]
    fn non_octet_decode_works_with_multiple_frames() {
        let mut decoder = SyslogDecoder::new(128);
        let mut buffer = BytesMut::with_capacity(16);

        buffer.put(&b"<57>Mar 25 21:47:46 gleichner6005 quaerat[2444]: There were "[..]);
        let result = decoder.decode(&mut buffer);
        assert_eq!(Ok(None), result.map_err(|_| true));

        buffer.put(&b"8 penguins in the shop.\n"[..]);
        let result = decoder.decode(&mut buffer);
        assert_eq!(
            Some("<57>Mar 25 21:47:46 gleichner6005 quaerat[2444]: There were 8 penguins in the shop.".into()),
            result.unwrap()
        );
    }

    #[test]
    fn octet_decode_works_with_multiple_frames() {
        let mut decoder = SyslogDecoder::new(30);
        let mut buffer = BytesMut::with_capacity(16);

        buffer.put(&b"28 abcdefghijklm"[..]);
        let result = decoder.decode(&mut buffer);
        assert_eq!(Ok(None), result.map_err(|_| false));

        // Sending another frame starting with a number should not cause it to
        // try to decode a new message.
        buffer.put(&b"3 nopqrstuvwxyz"[..]);
        let result = decoder.decode(&mut buffer);
        assert_eq!(Some("abcdefghijklm3 nopqrstuvwxyz".into()), result.unwrap());
    }

    #[test]
    fn octet_decode_moves_past_invalid_length() {
        let mut decoder = SyslogDecoder::new(16);
        let mut buffer = BytesMut::with_capacity(16);

        // An invalid syslog message that starts with a digit so we think it is starting with the len.
        buffer.put(&b"232>1 zork"[..]);
        let result = decoder.decode(&mut buffer);

        assert!(result.is_err());
        assert_eq!(b"zork"[..], buffer);
    }

    #[test]
    fn octet_decode_moves_past_invalid_utf8() {
        let mut decoder = SyslogDecoder::new(16);
        let mut buffer = BytesMut::with_capacity(16);

        // An invalid syslog message containing invalid utf8 bytes.
        buffer.put(&[b'4', b' ', 0xf0, 0x28, 0x8c, 0xbc][..]);
        let result = decoder.decode(&mut buffer);

        assert!(result.is_err());
        assert_eq!(b""[..], buffer);
    }

    #[test]
    fn octet_decode_moves_past_exceeded_frame_length() {
        let mut decoder = SyslogDecoder::new(16);
        let mut buffer = BytesMut::with_capacity(32);

        buffer.put(&b"32thisshouldbelongerthanthmaxframeasizewhichmeansthesyslogparserwillnotbeabletodecodeit\n"[..]);
        let result = decoder.decode(&mut buffer);

        assert!(result.is_err());
        assert_eq!(b""[..], buffer);
    }

    #[test]
    fn octet_decode_rejects_exceeded_frame_length() {
        let mut decoder = SyslogDecoder::new(16);
        let mut buffer = BytesMut::with_capacity(32);

        buffer.put(&b"26 abcdefghijklmnopqrstuvwxyzand here we are"[..]);
        let result = decoder.decode(&mut buffer);
        assert_eq!(Ok(None), result.map_err(|_| false));
        let result = decoder.decode(&mut buffer);

        assert!(result.is_err());
        assert_eq!(b"and here we are"[..], buffer);
    }

    #[test]
    fn octet_decode_rejects_exceeded_frame_length_multiple_frames() {
        let mut decoder = SyslogDecoder::new(16);
        let mut buffer = BytesMut::with_capacity(32);

        buffer.put(&b"26 abc"[..]);
        let _result = decoder.decode(&mut buffer);

        buffer.put(&b"defghijklmnopqrstuvwxyzand here we are"[..]);
        let result = decoder.decode(&mut buffer);

        println!("{:?}", result);
        assert!(result.is_err());
        assert_eq!(b"and here we are"[..], buffer);
    }

    #[test]
    fn octet_decode_moves_past_exceeded_frame_length_multiple_frames() {
        let mut decoder = SyslogDecoder::new(16);
        let mut buffer = BytesMut::with_capacity(32);

        buffer.put(&b"32thisshouldbelongerthanthmaxframeasizewhichmeansthesyslogparserwillnotbeabletodecodeit"[..]);
        let _ = decoder.decode(&mut buffer);

        assert_eq!(decoder.octet_decoding, Some(State::DiscardingToEol));
        buffer.put(&b"wemustcontinuetodiscard\n32 something valid"[..]);
        let result = decoder.decode(&mut buffer);

        assert!(result.is_err());
        assert_eq!(b"32 something valid"[..], buffer);
    }
}
