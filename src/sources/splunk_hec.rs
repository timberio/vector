use crate::{
    config::{log_schema, DataType, Resource, SourceConfig, SourceContext, SourceDescription},
    event::{Event, LogEvent, Value},
    internal_events::{
        SplunkHecEventReceived, SplunkHecRequestBodyInvalid, SplunkHecRequestError,
        SplunkHecRequestReceived,
    },
    tls::{MaybeTlsSettings, TlsConfig},
    Pipeline,
};
use bytes::{Buf, Bytes};
use chrono::{DateTime, TimeZone, Utc};
use flate2::read::MultiGzDecoder;
use futures::{stream, FutureExt, SinkExt, StreamExt, TryFutureExt};
use http::StatusCode;
use serde::{de, Deserialize, Serialize};
use serde_json::{de::IoRead, json, Deserializer, Value as JsonValue};
use snafu::Snafu;
use std::{
    collections::HashMap,
    future,
    io::Read,
    net::{Ipv4Addr, SocketAddr},
};

use warp::{filters::BoxedFilter, path, reject::Rejection, reply::Response, Filter, Reply};

// Event fields unique to splunk_hec source
pub const CHANNEL: &str = "splunk_channel";
pub const INDEX: &str = "splunk_index";
pub const SOURCE: &str = "splunk_source";
pub const SOURCETYPE: &str = "splunk_sourcetype";

/// Accepts HTTP requests.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields, default)]
pub struct SplunkConfig {
    /// Local address on which to listen
    #[serde(default = "default_socket_address")]
    address: SocketAddr,
    /// Splunk HEC token
    token: Option<String>,
    tls: Option<TlsConfig>,
}

inventory::submit! {
    SourceDescription::new::<SplunkConfig>("splunk_hec")
}

impl_generate_config_from_default!(SplunkConfig);

impl SplunkConfig {
    #[cfg(test)]
    pub fn on(address: SocketAddr) -> Self {
        SplunkConfig {
            address,
            ..Self::default()
        }
    }
}

impl Default for SplunkConfig {
    fn default() -> Self {
        SplunkConfig {
            address: default_socket_address(),
            token: None,
            tls: None,
        }
    }
}

fn default_socket_address() -> SocketAddr {
    SocketAddr::new(Ipv4Addr::new(0, 0, 0, 0).into(), 8088)
}

#[async_trait::async_trait]
#[typetag::serde(name = "splunk_hec")]
impl SourceConfig for SplunkConfig {
    async fn build(&self, cx: SourceContext) -> crate::Result<super::Source> {
        let source = SplunkSource::new(self);

        let event_service = source.event_service(cx.out.clone());
        let raw_service = source.raw_service(cx.out);
        let health_service = source.health_service();
        let options = SplunkSource::options();

        let services = path!("services" / "collector" / ..)
            .and(
                warp::path::full()
                    .map(|path: warp::filters::path::FullPath| {
                        emit!(SplunkHecRequestReceived {
                            path: path.as_str()
                        });
                    })
                    .untuple_one(),
            )
            .and(
                event_service
                    .or(raw_service)
                    .unify()
                    .or(health_service)
                    .unify()
                    .or(options)
                    .unify(),
            )
            .or_else(finish_err);

        let tls = MaybeTlsSettings::from_config(&self.tls, true)?;
        let listener = tls.bind(&self.address).await?;

        let shutdown = cx.shutdown;
        Ok(Box::pin(async move {
            let span = crate::trace::current_span();
            warp::serve(services.with(warp::trace(move |_info| span.clone())))
                .serve_incoming_with_graceful_shutdown(
                    listener.accept_stream(),
                    shutdown.map(|_| ()),
                )
                .await;

            Ok(())
        }))
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn source_type(&self) -> &'static str {
        "splunk_hec"
    }

    fn resources(&self) -> Vec<Resource> {
        vec![Resource::tcp(self.address)]
    }
}

/// Shared data for responding to requests.
struct SplunkSource {
    credentials: Option<Bytes>,
}

impl SplunkSource {
    fn new(config: &SplunkConfig) -> Self {
        SplunkSource {
            credentials: config
                .token
                .as_ref()
                .map(|token| format!("Splunk {}", token).into()),
        }
    }

    fn event_service(&self, out: Pipeline) -> BoxedFilter<(Response,)> {
        let splunk_channel_query_param = warp::query::<HashMap<String, String>>()
            .map(|qs: HashMap<String, String>| qs.get("channel").map(|v| v.to_owned()));
        let splunk_channel_header = warp::header::optional::<String>("x-splunk-request-channel");

        let splunk_channel = splunk_channel_header
            .and(splunk_channel_query_param)
            .map(|header: Option<String>, query_param| header.or(query_param));

        warp::post()
            .and(path!("event").or(path!("event" / "1.0")))
            .and(self.authorization())
            .and(splunk_channel)
            .and(warp::addr::remote())
            .and(warp::header::optional::<String>("X-Forwarded-For"))
            .and(self.gzip())
            .and(warp::body::bytes())
            .and_then(
                move |_,
                      _,
                      channel: Option<String>,
                      remote: Option<SocketAddr>,
                      xff: Option<String>,
                      gzip: bool,
                      body: Bytes| {
                    let mut out = out
                        .clone()
                        .sink_map_err(|_| Rejection::from(ApiError::ServerShutdown));
                    async move {
                        let reader: Box<dyn Read + Send> = if gzip {
                            Box::new(MultiGzDecoder::new(body.reader()))
                        } else {
                            Box::new(body.reader())
                        };

                        let events = stream::iter(EventIterator::new(reader, channel, remote, xff));

                        // `fn send_all` can be used once https://github.com/rust-lang/futures-rs/issues/2402
                        // is resolved.
                        let res = events.forward(&mut out).await;

                        out.flush().await?;

                        res
                    }
                },
            )
            .map(finish_ok)
            .boxed()
    }

    fn raw_service(&self, out: Pipeline) -> BoxedFilter<(Response,)> {
        let splunk_channel_query_param = warp::query::<HashMap<String, String>>()
            .map(|qs: HashMap<String, String>| qs.get("channel").map(|v| v.to_owned()));
        let splunk_channel_header = warp::header::optional::<String>("x-splunk-request-channel");

        let splunk_channel = splunk_channel_header
            .and(splunk_channel_query_param)
            .and_then(|header: Option<String>, query_param| async move {
                header
                    .or(query_param)
                    .ok_or_else(|| Rejection::from(ApiError::MissingChannel))
            });

        warp::post()
            .and(path!("raw" / "1.0").or(path!("raw")))
            .and(self.authorization())
            .and(splunk_channel)
            .and(warp::addr::remote())
            .and(warp::header::optional::<String>("X-Forwarded-For"))
            .and(self.gzip())
            .and(warp::body::bytes())
            .and_then(
                move |_,
                      _,
                      channel: String,
                      remote: Option<SocketAddr>,
                      xff: Option<String>,
                      gzip: bool,
                      body: Bytes| {
                    let out = out.clone();
                    async move {
                        let event = future::ready(raw_event(body, gzip, channel, remote, xff));
                        futures::stream::once(event)
                            .forward(
                                out.sink_map_err(|_| Rejection::from(ApiError::ServerShutdown)),
                            )
                            .map_ok(|_| ())
                            .await
                    }
                },
            )
            .map(finish_ok)
            .boxed()
    }

    fn health_service(&self) -> BoxedFilter<(Response,)> {
        let credentials = self.credentials.clone();
        let authorize =
            warp::header::optional("Authorization").and_then(move |token: Option<String>| {
                let credentials = credentials.clone();
                async move {
                    match (token, credentials) {
                        (_, None) => Ok(()),
                        (Some(token), Some(password)) if token.as_bytes() == password.as_ref() => {
                            Ok(())
                        }
                        _ => Err(Rejection::from(ApiError::BadRequest)),
                    }
                }
            });

        warp::get()
            .and(path!("health" / "1.0").or(path!("health")))
            .and(authorize)
            .map(move |_, _| warp::reply().into_response())
            .boxed()
    }

    fn options() -> BoxedFilter<(Response,)> {
        let post = warp::options()
            .and(
                path!("event")
                    .or(path!("event" / "1.0"))
                    .or(path!("raw" / "1.0"))
                    .or(path!("raw")),
            )
            .map(|_| warp::reply::with_header(warp::reply(), "Allow", "POST").into_response());

        let get = warp::options()
            .and(path!("health").or(path!("health" / "1.0")))
            .map(|_| warp::reply::with_header(warp::reply(), "Allow", "GET").into_response());

        post.or(get).unify().boxed()
    }

    /// Authorize request
    fn authorization(&self) -> BoxedFilter<((),)> {
        let credentials = self.credentials.clone();
        warp::header::optional("Authorization")
            .and_then(move |token: Option<String>| {
                let credentials = credentials.clone();
                async move {
                    match (token, credentials) {
                        (_, None) => Ok(()),
                        (Some(token), Some(password)) if token.as_bytes() == password.as_ref() => {
                            Ok(())
                        }
                        (Some(_), Some(_)) => Err(Rejection::from(ApiError::InvalidAuthorization)),
                        (None, Some(_)) => Err(Rejection::from(ApiError::MissingAuthorization)),
                    }
                }
            })
            .boxed()
    }

    /// Is body encoded with gzip
    fn gzip(&self) -> BoxedFilter<(bool,)> {
        warp::header::optional::<String>("Content-Encoding")
            .and_then(|encoding: Option<String>| async move {
                match encoding {
                    Some(s) if s.as_bytes() == b"gzip" => Ok(true),
                    Some(_) => Err(Rejection::from(ApiError::UnsupportedEncoding)),
                    None => Ok(false),
                }
            })
            .boxed()
    }
}
/// Constructs one or more events from json-s coming from reader.
/// If errors, it's done with input.
struct EventIterator<R: Read> {
    /// Remaining request with JSON events
    data: R,
    /// Count of sent events
    events: usize,
    /// Optional channel from headers
    channel: Option<Value>,
    /// Default time
    time: Time,
    /// Remaining extracted default values
    extractors: [DefaultExtractor; 4],
}

impl<R: Read> EventIterator<R> {
    fn new(
        data: R,
        channel: Option<String>,
        remote: Option<SocketAddr>,
        remote_addr: Option<String>,
    ) -> Self {
        EventIterator {
            data,
            events: 0,
            channel: channel.map(Value::from),
            time: Time::Now(Utc::now()),
            extractors: [
                // Extract the host field with the given priority:
                // 1. The host field is present in the event payload
                // 2. The x-forwarded-for header is present in the incoming request
                // 3. Use the `remote`: SocketAddr value provided by warp
                DefaultExtractor::new_with(
                    "host",
                    log_schema().host_key(),
                    remote_addr
                        .or_else(|| remote.map(|addr| addr.to_string()))
                        .map(Value::from),
                ),
                DefaultExtractor::new("index", INDEX),
                DefaultExtractor::new("source", SOURCE),
                DefaultExtractor::new("sourcetype", SOURCETYPE),
            ],
        }
    }

    /// As serde_json::from_reader, but doesn't require that all data has to be consumed,
    /// nor that it has to exist.
    fn from_reader_take<T>(&mut self) -> Result<Option<T>, serde_json::Error>
    where
        T: de::DeserializeOwned,
    {
        use serde_json::de::Read;
        let mut reader = IoRead::new(&mut self.data);
        match reader.peek()? {
            None => Ok(None),
            Some(_) => Deserialize::deserialize(&mut Deserializer::new(reader)).map(Some),
        }
    }

    fn build_event(&mut self, mut json: JsonValue) -> Result<Event, Rejection> {
        // Construct Event from parsed json event
        let mut event = Event::new_empty_log();
        let log = event.as_mut_log();

        // Add source type
        log.insert(log_schema().source_type_key(), Bytes::from("splunk_hec"));

        // Process event field
        match json.get_mut("event") {
            Some(event) => match event.take() {
                JsonValue::String(string) => {
                    if string.is_empty() {
                        return Err(ApiError::EmptyEventField { event: self.events }.into());
                    }
                    log.insert(log_schema().message_key(), string);
                }
                JsonValue::Object(mut object) => {
                    if object.is_empty() {
                        return Err(ApiError::EmptyEventField { event: self.events }.into());
                    }

                    // Add 'line' value as 'event::schema().message_key'
                    if let Some(line) = object.remove("line") {
                        match line {
                            // This don't quite fit the meaning of a event::schema().message_key
                            JsonValue::Array(_) | JsonValue::Object(_) => {
                                log.insert("line", line);
                            }
                            _ => {
                                log.insert(log_schema().message_key(), line);
                            }
                        }
                    }

                    for (key, value) in object {
                        log.insert(key, value);
                    }
                }
                _ => return Err(ApiError::InvalidDataFormat { event: self.events }.into()),
            },
            None => return Err(ApiError::MissingEventField { event: self.events }.into()),
        }

        // Process channel field
        if let Some(JsonValue::String(guid)) = json.get_mut("channel").map(JsonValue::take) {
            log.insert(CHANNEL, guid);
        } else if let Some(guid) = self.channel.as_ref() {
            log.insert(CHANNEL, guid.clone());
        }

        // Process fields field
        if let Some(JsonValue::Object(object)) = json.get_mut("fields").map(JsonValue::take) {
            for (key, value) in object {
                log.insert(key, value);
            }
        }

        // Process time field
        let parsed_time = match json.get_mut("time").map(JsonValue::take) {
            Some(JsonValue::Number(time)) => Some(Some(time)),
            Some(JsonValue::String(time)) => Some(time.parse::<serde_json::Number>().ok()),
            _ => None,
        };
        match parsed_time {
            None => (),
            Some(Some(t)) => {
                if let Some(t) = t.as_u64() {
                    let time = parse_timestamp(t as i64)
                        .ok_or(ApiError::InvalidDataFormat { event: self.events })?;

                    self.time = Time::Provided(time);
                } else if let Some(t) = t.as_f64() {
                    self.time = Time::Provided(Utc.timestamp(
                        t.floor() as i64,
                        (t.fract() * 1000.0 * 1000.0 * 1000.0) as u32,
                    ));
                } else {
                    return Err(ApiError::InvalidDataFormat { event: self.events }.into());
                }
            }
            Some(None) => return Err(ApiError::InvalidDataFormat { event: self.events }.into()),
        }

        // Add time field
        match self.time.clone() {
            Time::Provided(time) => log.insert(log_schema().timestamp_key(), time),
            Time::Now(time) => log.insert(log_schema().timestamp_key(), time),
        };

        // Extract default extracted fields
        for de in self.extractors.iter_mut() {
            de.extract(log, &mut json);
        }

        emit!(SplunkHecEventReceived);
        self.events += 1;

        Ok(event)
    }
}

impl<R: Read> Iterator for EventIterator<R> {
    type Item = Result<Event, Rejection>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.from_reader_take::<JsonValue>() {
            Ok(Some(json)) => Some(self.build_event(json)),
            Ok(None) => {
                if self.events == 0 {
                    Some(Err(ApiError::NoData.into()))
                } else {
                    None
                }
            }
            Err(error) => {
                emit!(SplunkHecRequestBodyInvalid {
                    error: error.into()
                });
                Some(Err(
                    ApiError::InvalidDataFormat { event: self.events }.into()
                ))
            }
        }
    }
}

/// Parse a `i64` unix timestamp that can either be in seconds, milliseconds or
/// nanoseconds.
///
/// This attempts to parse timestamps based on what cutoff range they fall into.
/// For seconds to be parsed the timestamp must be less than the unix epoch of
/// the year `2400`. For this to parse milliseconds the time must be smaller
/// than the year `10,000` in unix epoch milliseconds. If the value is larger
/// than both we attempt to parse it as nanoseconds.
///
/// Returns `None` if `t` is negative.
fn parse_timestamp(t: i64) -> Option<DateTime<Utc>> {
    // Utc.ymd(2400, 1, 1).and_hms(0,0,0).timestamp();
    const SEC_CUTOFF: i64 = 13569465600;
    // Utc.ymd(10_000, 1, 1).and_hms(0,0,0).timestamp_millis();
    const MILLISEC_CUTOFF: i64 = 253402300800000;

    // Timestamps can't be negative!
    if t < 0 {
        return None;
    }

    let ts = if t < SEC_CUTOFF {
        Utc.timestamp(t, 0)
    } else if t < MILLISEC_CUTOFF {
        Utc.timestamp_millis(t)
    } else {
        Utc.timestamp_nanos(t)
    };

    Some(ts)
}

/// Maintains last known extracted value of field and uses it in the absence of field.
struct DefaultExtractor {
    field: &'static str,
    to_field: &'static str,
    value: Option<Value>,
}

impl DefaultExtractor {
    fn new(field: &'static str, to_field: &'static str) -> Self {
        DefaultExtractor {
            field,
            to_field,
            value: None,
        }
    }

    fn new_with(
        field: &'static str,
        to_field: &'static str,
        value: impl Into<Option<Value>>,
    ) -> Self {
        DefaultExtractor {
            field,
            to_field,
            value: value.into(),
        }
    }

    fn extract(&mut self, log: &mut LogEvent, value: &mut JsonValue) {
        // Process json_field
        if let Some(JsonValue::String(new_value)) = value.get_mut(self.field).map(JsonValue::take) {
            self.value = Some(new_value.into());
        }

        // Add data field
        if let Some(index) = self.value.as_ref() {
            log.insert(self.to_field, index.clone());
        }
    }
}

/// For tracking origin of the timestamp
#[derive(Clone, Debug)]
enum Time {
    /// Backup
    Now(DateTime<Utc>),
    /// Provided in the request
    Provided(DateTime<Utc>),
}

/// Creates event from raw request
fn raw_event(
    bytes: Bytes,
    gzip: bool,
    channel: String,
    remote: Option<SocketAddr>,
    xff: Option<String>,
) -> Result<Event, Rejection> {
    // Process gzip
    let message: Value = if gzip {
        let mut data = Vec::new();
        match MultiGzDecoder::new(bytes.reader()).read_to_end(&mut data) {
            Ok(0) => return Err(ApiError::NoData.into()),
            Ok(_) => Value::from(Bytes::from(data)),
            Err(error) => {
                emit!(SplunkHecRequestBodyInvalid { error });
                return Err(ApiError::InvalidDataFormat { event: 0 }.into());
            }
        }
    } else {
        bytes.into()
    };

    // Construct event
    let mut event = Event::new_empty_log();
    let log = event.as_mut_log();

    // Add message
    log.insert(log_schema().message_key(), message);

    // Add channel
    log.insert(CHANNEL, channel);

    // host-field priority for raw endpoint:
    // - x-forwarded-for is set to `host` field first, if present. If not present:
    // - set remote addr to host field
    if let Some(remote_address) = xff {
        log.insert(log_schema().host_key(), remote_address);
    } else if let Some(remote) = remote {
        log.insert(log_schema().host_key(), remote.to_string());
    }

    // Add timestamp
    log.insert(log_schema().timestamp_key(), Utc::now());

    // Add source type
    event
        .as_mut_log()
        .try_insert(log_schema().source_type_key(), Bytes::from("splunk_hec"));

    emit!(SplunkHecEventReceived);

    Ok(event)
}

#[derive(Clone, Copy, Debug, Snafu)]
pub(crate) enum ApiError {
    MissingAuthorization,
    InvalidAuthorization,
    UnsupportedEncoding,
    MissingChannel,
    NoData,
    InvalidDataFormat { event: usize },
    ServerShutdown,
    EmptyEventField { event: usize },
    MissingEventField { event: usize },
    BadRequest,
}

impl warp::reject::Reject for ApiError {}

/// Cached bodies for common responses
mod splunk_response {
    use bytes::Bytes;
    use lazy_static::lazy_static;
    use serde_json::{json, Value};

    fn json_to_bytes(value: Value) -> Bytes {
        serde_json::to_string(&value).unwrap().into()
    }

    lazy_static! {
        pub static ref INVALID_AUTHORIZATION: Bytes =
            json_to_bytes(json!({"text":"Invalid authorization","code":3}));
        pub static ref MISSING_CREDENTIALS: Bytes =
            json_to_bytes(json!({"text":"Token is required","code":2}));
        pub static ref NO_DATA: Bytes = json_to_bytes(json!({"text":"No data","code":5}));
        pub static ref SUCCESS: Bytes = json_to_bytes(json!({"text":"Success","code":0}));
        pub static ref SERVER_ERROR: Bytes =
            json_to_bytes(json!({"text":"Internal server error","code":8}));
        pub static ref SERVER_SHUTDOWN: Bytes =
            json_to_bytes(json!({"text":"Server is shutting down","code":9}));
        pub static ref UNSUPPORTED_MEDIA_TYPE: Bytes =
            json_to_bytes(json!({"text":"unsupported content encoding"}));
        pub static ref NO_CHANNEL: Bytes =
            json_to_bytes(json!({"text":"Data channel is missing","code":10}));
    }
}

fn finish_ok(_: ()) -> Response {
    response_json(StatusCode::OK, splunk_response::SUCCESS.as_ref())
}

async fn finish_err(rejection: Rejection) -> Result<(Response,), Rejection> {
    if let Some(&error) = rejection.find::<ApiError>() {
        emit!(SplunkHecRequestError { error });
        Ok((match error {
            ApiError::MissingAuthorization => response_json(
                StatusCode::UNAUTHORIZED,
                splunk_response::MISSING_CREDENTIALS.as_ref(),
            ),
            ApiError::InvalidAuthorization => response_json(
                StatusCode::UNAUTHORIZED,
                splunk_response::INVALID_AUTHORIZATION.as_ref(),
            ),
            ApiError::UnsupportedEncoding => response_json(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                splunk_response::UNSUPPORTED_MEDIA_TYPE.as_ref(),
            ),
            ApiError::MissingChannel => response_json(
                StatusCode::BAD_REQUEST,
                splunk_response::NO_CHANNEL.as_ref(),
            ),
            ApiError::NoData => {
                response_json(StatusCode::BAD_REQUEST, splunk_response::NO_DATA.as_ref())
            }
            ApiError::ServerShutdown => response_json(
                StatusCode::SERVICE_UNAVAILABLE,
                splunk_response::SERVER_SHUTDOWN.as_ref(),
            ),
            ApiError::InvalidDataFormat { event } => event_error("Invalid data format", 6, event),
            ApiError::EmptyEventField { event } => {
                event_error("Event field cannot be blank", 13, event)
            }
            ApiError::MissingEventField { event } => {
                event_error("Event field is required", 12, event)
            }
            ApiError::BadRequest => empty_response(StatusCode::BAD_REQUEST),
        },))
    } else {
        Err(rejection)
    }
}

/// Response without body
fn empty_response(code: StatusCode) -> Response {
    let mut res = Response::default();
    *res.status_mut() = code;
    res
}

/// Response with body
fn response_json(code: StatusCode, body: impl Serialize) -> Response {
    warp::reply::with_status(warp::reply::json(&body), code).into_response()
}

/// Error happened during parsing of events
fn event_error(text: &str, code: u16, event: usize) -> Response {
    let body = json!({
        "text":text,
        "code":code,
        "invalid-event-number":event
    });
    match serde_json::to_string(&body) {
        Ok(string) => response_json(StatusCode::BAD_REQUEST, string),
        Err(error) => {
            // This should never happen.
            error!(message = "Error encoding json body.", %error);
            response_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                splunk_response::SERVER_ERROR.clone(),
            )
        }
    }
}

#[cfg(feature = "sinks-splunk_hec")]
#[cfg(test)]
mod tests {
    use super::{parse_timestamp, SplunkConfig};
    use crate::{
        config::{log_schema, SinkConfig, SinkContext, SourceConfig, SourceContext},
        event::Event,
        sinks::{
            splunk_hec::{Encoding, HecSinkConfig},
            util::{encoding::EncodingConfig, BatchConfig, Compression, TowerRequestConfig},
            Healthcheck, VectorSink,
        },
        test_util::{collect_n, next_addr, trace_init, wait_for_tcp},
        Pipeline,
    };
    use chrono::{TimeZone, Utc};
    use futures::{channel::mpsc, stream, StreamExt};
    use std::{future::ready, net::SocketAddr};

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<SplunkConfig>();
    }

    /// Splunk token
    const TOKEN: &str = "token";

    async fn source() -> (mpsc::Receiver<Event>, SocketAddr) {
        source_with(Some(TOKEN.to_owned())).await
    }

    async fn source_with(token: Option<String>) -> (mpsc::Receiver<Event>, SocketAddr) {
        let (sender, recv) = Pipeline::new_test();
        let address = next_addr();
        tokio::spawn(async move {
            SplunkConfig {
                address,
                token,
                tls: None,
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

    async fn sink(
        address: SocketAddr,
        encoding: impl Into<EncodingConfig<Encoding>>,
        compression: Compression,
    ) -> (VectorSink, Healthcheck) {
        HecSinkConfig {
            token: TOKEN.to_owned(),
            endpoint: format!("http://{}", address),
            host_key: "host".to_owned(),
            indexed_fields: vec![],
            index: None,
            sourcetype: None,
            source: None,
            encoding: encoding.into(),
            compression,
            batch: BatchConfig::default(),
            request: TowerRequestConfig::default(),
            tls: None,
        }
        .build(SinkContext::new_test())
        .await
        .unwrap()
    }

    async fn start(
        encoding: impl Into<EncodingConfig<Encoding>>,
        compression: Compression,
    ) -> (VectorSink, mpsc::Receiver<Event>) {
        let (source, address) = source().await;
        let (sink, health) = sink(address, encoding, compression).await;
        assert!(health.await.is_ok());
        (sink, source)
    }

    async fn channel_n(
        messages: Vec<impl Into<Event> + Send + 'static>,
        sink: VectorSink,
        source: mpsc::Receiver<Event>,
    ) -> Vec<Event> {
        let n = messages.len();

        tokio::spawn(async move {
            sink.run(stream::iter(messages).map(|x| x.into()))
                .await
                .unwrap();
        });

        let events = collect_n(source, n).await;
        assert_eq!(n, events.len());

        events
    }

    #[derive(Clone, Copy, Debug)]
    enum Channel<'a> {
        Header(&'a str),
        QueryParam(&'a str),
    }

    #[derive(Default)]
    struct SendWithOpts<'a> {
        channel: Option<Channel<'a>>,
        forwarded_for: Option<String>,
    }

    async fn post(address: SocketAddr, api: &str, message: &str) -> u16 {
        let channel = Channel::Header("channel");
        let options = SendWithOpts {
            channel: Some(channel),
            forwarded_for: None,
        };
        send_with(address, api, message, TOKEN, &options).await
    }

    async fn send_with<'a>(
        address: SocketAddr,
        api: &str,
        message: &str,
        token: &str,
        opts: &SendWithOpts<'_>,
    ) -> u16 {
        let mut b = reqwest::Client::new()
            .post(&format!("http://{}/{}", address, api))
            .header("Authorization", format!("Splunk {}", token));

        b = match opts.channel {
            Some(c) => match c {
                Channel::Header(v) => b.header("x-splunk-request-channel", v),
                Channel::QueryParam(v) => b.query(&[("channel", v)]),
            },
            None => b,
        };

        b = match &opts.forwarded_for {
            Some(f) => b.header("X-Forwarded-For", f),
            None => b,
        };

        b.body(message.to_owned())
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    #[tokio::test]
    async fn no_compression_text_event() {
        trace_init();

        let message = "gzip_text_event";
        let (sink, source) = start(Encoding::Text, Compression::None).await;

        let event = channel_n(vec![message], sink, source).await.remove(0);

        assert_eq!(event.as_log()[log_schema().message_key()], message.into());
        assert!(event.as_log().get(log_schema().timestamp_key()).is_some());
        assert_eq!(
            event.as_log()[log_schema().source_type_key()],
            "splunk_hec".into()
        );
    }

    #[tokio::test]
    async fn one_simple_text_event() {
        trace_init();

        let message = "one_simple_text_event";
        let (sink, source) = start(Encoding::Text, Compression::gzip_default()).await;

        let event = channel_n(vec![message], sink, source).await.remove(0);

        assert_eq!(event.as_log()[log_schema().message_key()], message.into());
        assert!(event.as_log().get(log_schema().timestamp_key()).is_some());
        assert_eq!(
            event.as_log()[log_schema().source_type_key()],
            "splunk_hec".into()
        );
    }

    #[tokio::test]
    async fn multiple_simple_text_event() {
        trace_init();

        let n = 200;
        let (sink, source) = start(Encoding::Text, Compression::None).await;

        let messages = (0..n)
            .map(|i| format!("multiple_simple_text_event_{}", i))
            .collect::<Vec<_>>();
        let events = channel_n(messages.clone(), sink, source).await;

        for (msg, event) in messages.into_iter().zip(events.into_iter()) {
            assert_eq!(event.as_log()[log_schema().message_key()], msg.into());
            assert!(event.as_log().get(log_schema().timestamp_key()).is_some());
            assert_eq!(
                event.as_log()[log_schema().source_type_key()],
                "splunk_hec".into()
            );
        }
    }

    #[tokio::test]
    async fn one_simple_json_event() {
        trace_init();

        let message = "one_simple_json_event";
        let (sink, source) = start(Encoding::Json, Compression::gzip_default()).await;

        let event = channel_n(vec![message], sink, source).await.remove(0);

        assert_eq!(event.as_log()[log_schema().message_key()], message.into());
        assert!(event.as_log().get(log_schema().timestamp_key()).is_some());
        assert_eq!(
            event.as_log()[log_schema().source_type_key()],
            "splunk_hec".into()
        );
    }

    #[tokio::test]
    async fn multiple_simple_json_event() {
        trace_init();

        let n = 200;
        let (sink, source) = start(Encoding::Json, Compression::gzip_default()).await;

        let messages = (0..n)
            .map(|i| format!("multiple_simple_json_event{}", i))
            .collect::<Vec<_>>();
        let events = channel_n(messages.clone(), sink, source).await;

        for (msg, event) in messages.into_iter().zip(events.into_iter()) {
            assert_eq!(event.as_log()[log_schema().message_key()], msg.into());
            assert!(event.as_log().get(log_schema().timestamp_key()).is_some());
            assert_eq!(
                event.as_log()[log_schema().source_type_key()],
                "splunk_hec".into()
            );
        }
    }

    #[tokio::test]
    async fn json_event() {
        trace_init();

        let (sink, source) = start(Encoding::Json, Compression::gzip_default()).await;

        let mut event = Event::new_empty_log();
        event.as_mut_log().insert("greeting", "hello");
        event.as_mut_log().insert("name", "bob");
        sink.run(stream::once(ready(event))).await.unwrap();

        let event = collect_n(source, 1).await.remove(0);
        assert_eq!(event.as_log()["greeting"], "hello".into());
        assert_eq!(event.as_log()["name"], "bob".into());
        assert!(event.as_log().get(log_schema().timestamp_key()).is_some());
        assert_eq!(
            event.as_log()[log_schema().source_type_key()],
            "splunk_hec".into()
        );
    }

    #[tokio::test]
    async fn line_to_message() {
        trace_init();

        let (sink, source) = start(Encoding::Json, Compression::gzip_default()).await;

        let mut event = Event::new_empty_log();
        event.as_mut_log().insert("line", "hello");
        sink.run(stream::once(ready(event))).await.unwrap();

        let event = collect_n(source, 1).await.remove(0);
        assert_eq!(event.as_log()[log_schema().message_key()], "hello".into());
    }

    #[tokio::test]
    async fn raw() {
        trace_init();

        let message = "raw";
        let (source, address) = source().await;

        assert_eq!(200, post(address, "services/collector/raw", message).await);

        let event = collect_n(source, 1).await.remove(0);
        assert_eq!(event.as_log()[log_schema().message_key()], message.into());
        assert_eq!(event.as_log()[&super::CHANNEL], "channel".into());
        assert!(event.as_log().get(log_schema().timestamp_key()).is_some());
        assert_eq!(
            event.as_log()[log_schema().source_type_key()],
            "splunk_hec".into()
        );
    }

    #[tokio::test]
    async fn channel_header() {
        trace_init();

        let message = "raw";
        let (source, address) = source().await;

        let opts = SendWithOpts {
            channel: Some(Channel::Header("guid")),
            forwarded_for: None,
        };

        assert_eq!(
            200,
            send_with(address, "services/collector/raw", message, TOKEN, &opts).await
        );

        let event = collect_n(source, 1).await.remove(0);
        assert_eq!(event.as_log()[&super::CHANNEL], "guid".into());
    }

    #[tokio::test]
    async fn xff_header_raw() {
        trace_init();

        let message = "raw";
        let (source, address) = source().await;

        let opts = SendWithOpts {
            channel: Some(Channel::Header("guid")),
            forwarded_for: Some(String::from("10.0.0.1")),
        };

        assert_eq!(
            200,
            send_with(address, "services/collector/raw", message, TOKEN, &opts).await
        );

        let event = collect_n(source, 1).await.remove(0);
        assert_eq!(event.as_log()[log_schema().host_key()], "10.0.0.1".into());
    }

    // Test helps to illustrate that a payload's `host` value should override an x-forwarded-for header
    #[tokio::test]
    async fn xff_header_event_with_host_field() {
        trace_init();

        let message = r#"{"event":"first", "host": "10.1.0.2"}"#;
        let (source, address) = source().await;

        let opts = SendWithOpts {
            channel: Some(Channel::Header("guid")),
            forwarded_for: Some(String::from("10.0.0.1")),
        };

        assert_eq!(
            200,
            send_with(address, "services/collector/event", message, TOKEN, &opts).await
        );

        let event = collect_n(source, 1).await.remove(0);
        assert_eq!(event.as_log()[log_schema().host_key()], "10.1.0.2".into());
    }

    // Test helps to illustrate that a payload's `host` value should override an x-forwarded-for header
    #[tokio::test]
    async fn xff_header_event_without_host_field() {
        trace_init();

        let message = r#"{"event":"first", "color": "blue"}"#;
        let (source, address) = source().await;

        let opts = SendWithOpts {
            channel: Some(Channel::Header("guid")),
            forwarded_for: Some(String::from("10.0.0.1")),
        };

        assert_eq!(
            200,
            send_with(address, "services/collector/event", message, TOKEN, &opts).await
        );

        let event = collect_n(source, 1).await.remove(0);
        assert_eq!(event.as_log()[log_schema().host_key()], "10.0.0.1".into());
    }

    #[tokio::test]
    async fn channel_query_param() {
        trace_init();

        let message = "raw";
        let (source, address) = source().await;

        let opts = SendWithOpts {
            channel: Some(Channel::QueryParam("guid")),
            forwarded_for: None,
        };

        assert_eq!(
            200,
            send_with(address, "services/collector/raw", message, TOKEN, &opts).await
        );

        let event = collect_n(source, 1).await.remove(0);
        assert_eq!(event.as_log()[&super::CHANNEL], "guid".into());
    }

    #[tokio::test]
    async fn no_data() {
        trace_init();

        let (_source, address) = source().await;

        assert_eq!(400, post(address, "services/collector/event", "").await);
    }

    #[tokio::test]
    async fn invalid_token() {
        trace_init();

        let (_source, address) = source().await;
        let opts = SendWithOpts {
            channel: Some(Channel::Header("channel")),
            forwarded_for: None,
        };

        assert_eq!(
            401,
            send_with(address, "services/collector/event", "", "nope", &opts).await
        );
    }

    #[tokio::test]
    async fn no_authorization() {
        trace_init();

        let message = "no_authorization";
        let (source, address) = source_with(None).await;
        let (sink, health) = sink(address, Encoding::Text, Compression::gzip_default()).await;
        assert!(health.await.is_ok());

        let event = channel_n(vec![message], sink, source).await.remove(0);

        assert_eq!(event.as_log()[log_schema().message_key()], message.into());
    }

    #[tokio::test]
    async fn partial() {
        trace_init();

        let message = r#"{"event":"first"}{"event":"second""#;
        let (source, address) = source().await;

        assert_eq!(
            400,
            post(address, "services/collector/event", message).await
        );

        let event = collect_n(source, 1).await.remove(0);
        assert_eq!(event.as_log()[log_schema().message_key()], "first".into());
        assert!(event.as_log().get(log_schema().timestamp_key()).is_some());
        assert_eq!(
            event.as_log()[log_schema().source_type_key()],
            "splunk_hec".into()
        );
    }

    #[tokio::test]
    async fn default() {
        trace_init();

        let message = r#"{"event":"first","source":"main"}{"event":"second"}{"event":"third","source":"secondary"}"#;
        let (source, address) = source().await;

        assert_eq!(
            200,
            post(address, "services/collector/event", message).await
        );

        let events = collect_n(source, 3).await;

        assert_eq!(
            events[0].as_log()[log_schema().message_key()],
            "first".into()
        );
        assert_eq!(events[0].as_log()[&super::SOURCE], "main".into());

        assert_eq!(
            events[1].as_log()[log_schema().message_key()],
            "second".into()
        );
        assert_eq!(events[1].as_log()[&super::SOURCE], "main".into());

        assert_eq!(
            events[2].as_log()[log_schema().message_key()],
            "third".into()
        );
        assert_eq!(events[2].as_log()[&super::SOURCE], "secondary".into());
    }

    #[test]
    fn parse_timestamps() {
        let cases = vec![
            Utc::now(),
            Utc.ymd(1971, 11, 7).and_hms(1, 1, 1),
            Utc.ymd(2011, 8, 5).and_hms(1, 1, 1),
            Utc.ymd(2189, 11, 4).and_hms(2, 2, 2),
        ];

        for case in cases {
            let sec = case.timestamp();
            let millis = case.timestamp_millis();
            let nano = case.timestamp_nanos();

            assert_eq!(
                parse_timestamp(sec as i64).unwrap().timestamp(),
                case.timestamp()
            );
            assert_eq!(
                parse_timestamp(millis as i64).unwrap().timestamp_millis(),
                case.timestamp_millis()
            );
            assert_eq!(
                parse_timestamp(nano as i64).unwrap().timestamp_nanos(),
                case.timestamp_nanos()
            );
        }

        assert!(parse_timestamp(-1).is_none());
    }

    /// This test will fail once `warp` crate fixes support for
    /// custom connection listener, at that point this test can be
    /// modified to pass.
    /// https://github.com/timberio/vector/issues/7097
    /// https://github.com/seanmonstar/warp/issues/830
    /// https://github.com/seanmonstar/warp/pull/713
    #[tokio::test]
    async fn host_test() {
        trace_init();

        let message = "for the host";
        let (sink, source) = start(Encoding::Text, Compression::gzip_default()).await;

        let event = channel_n(vec![message], sink, source).await.remove(0);

        assert_eq!(event.as_log()[log_schema().message_key()], message.into());
        assert!(event.as_log().get(log_schema().host_key()).is_none());
    }
}
