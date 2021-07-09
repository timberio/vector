use crate::{
    config::{
        log_schema, DataType, GenerateConfig, ProxyConfig, SinkConfig, SinkContext, SinkDescription,
    },
    event::{Event, LogEvent, Value},
    http::HttpClient,
    internal_events::{SplunkEventEncodeError, SplunkEventSent, TemplateRenderingFailed},
    sinks::util::{
        encoding::{EncodingConfig, EncodingConfiguration},
        http::{BatchedHttpSink, HttpSink},
        BatchConfig, BatchSettings, Buffer, Compression, Concurrency, EncodedEvent,
        TowerRequestConfig,
    },
    template::Template,
    tls::{TlsOptions, TlsSettings},
};
use futures::{FutureExt, SinkExt};
use http::{Request, StatusCode, Uri};
use hyper::Body;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use serde_json::json;
use snafu::{ResultExt, Snafu};
use std::convert::TryFrom;

#[derive(Debug, Snafu)]
pub enum BuildError {
    #[snafu(display("Host must include a scheme (https:// or http://)"))]
    UriMissingScheme,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct HecSinkConfig {
    pub token: String,
    // Deprecated name
    #[serde(alias = "host")]
    pub endpoint: String,
    #[serde(default = "host_key")]
    pub host_key: String,
    #[serde(default)]
    pub indexed_fields: Vec<String>,
    pub index: Option<Template>,
    pub sourcetype: Option<Template>,
    pub source: Option<Template>,
    pub encoding: EncodingConfig<Encoding>,
    #[serde(default)]
    pub compression: Compression,
    #[serde(default)]
    pub batch: BatchConfig,
    #[serde(default)]
    pub request: TowerRequestConfig,
    pub tls: Option<TlsOptions>,
    #[serde(
        default,
        skip_serializing_if = "crate::serde::skip_serializing_if_default"
    )]
    pub proxy: ProxyConfig,
}

lazy_static! {
    static ref REQUEST_DEFAULTS: TowerRequestConfig = TowerRequestConfig {
        concurrency: Concurrency::Fixed(10),
        rate_limit_num: Some(10),
        ..Default::default()
    };
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq, Derivative)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    Text,
    Json,
}

fn host_key() -> String {
    crate::config::log_schema().host_key().to_string()
}

inventory::submit! {
    SinkDescription::new::<HecSinkConfig>("splunk_hec")
}

impl GenerateConfig for HecSinkConfig {
    fn generate_config() -> toml::Value {
        toml::Value::try_from(Self {
            token: "${VECTOR_SPLUNK_HEC_TOKEN}".to_owned(),
            endpoint: "endpoint".to_owned(),
            host_key: host_key(),
            indexed_fields: vec![],
            index: None,
            sourcetype: None,
            source: None,
            encoding: Encoding::Text.into(),
            compression: Compression::default(),
            batch: BatchConfig::default(),
            request: TowerRequestConfig::default(),
            tls: None,
            proxy: Default::default(),
        })
        .unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "splunk_hec")]
impl SinkConfig for HecSinkConfig {
    async fn build(
        &self,
        cx: SinkContext,
    ) -> crate::Result<(super::VectorSink, super::Healthcheck)> {
        validate_host(&self.endpoint)?;

        let batch = BatchSettings::default()
            .bytes(bytesize::mib(1u64))
            .timeout(1)
            .parse_config(self.batch)?;
        let request = self.request.unwrap_with(&REQUEST_DEFAULTS);
        let tls_settings = TlsSettings::from_options(&self.tls)?;
        let proxy = cx.globals.proxy.build(&self.proxy);
        let client = HttpClient::new(tls_settings, proxy)?;

        let sink = BatchedHttpSink::new(
            self.clone(),
            Buffer::new(batch.size, self.compression),
            request,
            batch.timeout,
            client.clone(),
            cx.acker(),
        )
        .sink_map_err(|error| error!(message = "Fatal splunk_hec sink error.", %error));

        let healthcheck = healthcheck(self.clone(), client).boxed();

        Ok((super::VectorSink::Sink(Box::new(sink)), healthcheck))
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn sink_type(&self) -> &'static str {
        "splunk_hec"
    }
}

#[async_trait::async_trait]
impl HttpSink for HecSinkConfig {
    type Input = Vec<u8>;
    type Output = Vec<u8>;

    fn encode_event(&self, event: Event) -> Option<EncodedEvent<Self::Input>> {
        let sourcetype = self.sourcetype.as_ref().and_then(|sourcetype| {
            sourcetype
                .render_string(&event)
                .map_err(|error| {
                    emit!(TemplateRenderingFailed {
                        error,
                        field: Some("sourcetype"),
                        drop_event: false,
                    });
                })
                .ok()
        });

        let source = self.source.as_ref().and_then(|source| {
            source
                .render_string(&event)
                .map_err(|error| {
                    emit!(TemplateRenderingFailed {
                        error,
                        field: Some("source"),
                        drop_event: false,
                    });
                })
                .ok()
        });

        let index = self.index.as_ref().and_then(|index| {
            index
                .render_string(&event)
                .map_err(|error| {
                    emit!(TemplateRenderingFailed {
                        error,
                        field: Some("index"),
                        drop_event: false,
                    });
                })
                .ok()
        });

        let mut event = event.into_log();

        let host = event.get(self.host_key.to_owned()).cloned();

        let timestamp = match event.remove(log_schema().timestamp_key()) {
            Some(Value::Timestamp(ts)) => ts,
            _ => chrono::Utc::now(),
        };
        let timestamp = (timestamp.timestamp_millis() as f64) / 1000f64;

        let fields = self
            .indexed_fields
            .iter()
            .filter_map(|field| event.get(field).map(|value| (field, value.clone())))
            .collect::<LogEvent>();

        let mut event = Event::Log(event);
        self.encoding.apply_rules(&mut event);
        let log = event.into_log();

        let event = match self.encoding.codec() {
            Encoding::Json => json!(&log),
            Encoding::Text => json!(log
                .get(log_schema().message_key())
                .map(|v| v.to_string_lossy())
                .unwrap_or_else(|| "".into())),
        };

        let mut body = json!({
            "event": event,
            "fields": fields,
            "time": timestamp
        });

        if let Some(host) = host {
            let host = host.to_string_lossy();
            body["host"] = json!(host);
        }

        if let Some(index) = index {
            body["index"] = json!(index);
        }

        if let Some(source) = source {
            body["source"] = json!(source);
        }

        if let Some(sourcetype) = &sourcetype {
            body["sourcetype"] = json!(sourcetype);
        }

        match serde_json::to_vec(&body) {
            Ok(value) => {
                emit!(SplunkEventSent {
                    byte_size: value.len()
                });
                Some(EncodedEvent::new(value).with_metadata(log))
            }
            Err(error) => {
                emit!(SplunkEventEncodeError { error });
                None
            }
        }
    }

    async fn build_request(&self, events: Self::Output) -> crate::Result<Request<Vec<u8>>> {
        let uri =
            build_uri(&self.endpoint, "/services/collector/event").expect("Unable to parse URI");

        let mut builder = Request::post(uri)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Splunk {}", self.token));

        if let Some(ce) = self.compression.content_encoding() {
            builder = builder.header("Content-Encoding", ce);
        }

        builder.body(events).map_err(Into::into)
    }
}

#[derive(Debug, Snafu)]
enum HealthcheckError {
    #[snafu(display("Invalid HEC token"))]
    InvalidToken,
    #[snafu(display("Queues are full"))]
    QueuesFull,
}

pub async fn healthcheck(config: HecSinkConfig, client: HttpClient) -> crate::Result<()> {
    let uri = build_uri(&config.endpoint, "/services/collector/health/1.0")
        .context(super::UriParseError)?;

    let request = Request::get(uri)
        .header("Authorization", format!("Splunk {}", config.token))
        .body(Body::empty())
        .unwrap();

    let response = client.send(request).await?;
    match response.status() {
        StatusCode::OK => Ok(()),
        StatusCode::BAD_REQUEST => Err(HealthcheckError::InvalidToken.into()),
        StatusCode::SERVICE_UNAVAILABLE => Err(HealthcheckError::QueuesFull.into()),
        other => Err(super::HealthcheckError::UnexpectedStatus { status: other }.into()),
    }
}

pub fn validate_host(host: &str) -> crate::Result<()> {
    let uri = Uri::try_from(host).context(super::UriParseError)?;

    match uri.scheme() {
        Some(_) => Ok(()),
        None => Err(Box::new(BuildError::UriMissingScheme)),
    }
}

fn build_uri(host: &str, path: &str) -> Result<Uri, http::uri::InvalidUri> {
    format!("{}{}", host.trim_end_matches('/'), path).parse::<Uri>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Event;
    use crate::sinks::util::{http::HttpSink, test::load_sink};
    use chrono::Utc;
    use serde::Deserialize;
    use std::collections::BTreeMap;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<HecSinkConfig>();
    }

    #[derive(Deserialize, Debug)]
    struct HecEventJson {
        time: f64,
        event: BTreeMap<String, String>,
        fields: BTreeMap<String, String>,
        source: Option<String>,
    }

    #[derive(Deserialize, Debug)]
    struct HecEventText {
        time: f64,
        event: String,
        fields: BTreeMap<String, String>,
    }

    #[test]
    fn splunk_encode_event_json() {
        let mut event = Event::from("hello world");
        event.as_mut_log().insert("key", "value");
        event.as_mut_log().insert("magic", "vector");

        let (config, _cx) = load_sink::<HecSinkConfig>(
            r#"
            host = "test.com"
            token = "alksjdfo"
            host_key = "host"
            indexed_fields = ["key"]
            source = "{{ magic }}"

            [encoding]
            codec = "json"
            except_fields = ["magic"]
        "#,
        )
        .unwrap();

        let bytes = config.encode_event(event).unwrap().item;

        let hec_event = serde_json::from_slice::<HecEventJson>(&bytes[..]).unwrap();

        let event = &hec_event.event;
        let kv = event.get(&"key".to_string()).unwrap();

        assert_eq!(kv, &"value".to_string());
        assert_eq!(
            event[&log_schema().message_key().to_string()],
            "hello world".to_string()
        );
        assert!(event
            .get(&log_schema().timestamp_key().to_string())
            .is_none());

        assert!(!event.contains_key("magic"));
        assert_eq!(hec_event.source, Some("vector".to_string()));

        assert_eq!(
            hec_event.fields.get("key").map(|s| s.as_str()),
            Some("value")
        );

        let now = Utc::now().timestamp_millis() as f64 / 1000f64;
        assert!(
            (hec_event.time - now).abs() < 0.2,
            "hec_event.time = {}, now = {}",
            hec_event.time,
            now
        );
        assert_eq!((hec_event.time * 1000f64).fract(), 0f64);
    }

    #[test]
    fn splunk_encode_event_text() {
        let mut event = Event::from("hello world");
        event.as_mut_log().insert("key", "value");

        let (config, _cx) = load_sink::<HecSinkConfig>(
            r#"
            host = "test.com"
            token = "alksjdfo"
            host_key = "host"
            indexed_fields = ["key"]

            [encoding]
            codec = "text"
        "#,
        )
        .unwrap();

        let bytes = config.encode_event(event).unwrap().item;

        let hec_event = serde_json::from_slice::<HecEventText>(&bytes[..]).unwrap();

        assert_eq!(hec_event.event.as_str(), "hello world");

        assert_eq!(
            hec_event.fields.get("key").map(|s| s.as_str()),
            Some("value")
        );

        let now = Utc::now().timestamp_millis() as f64 / 1000f64;
        assert!(
            (hec_event.time - now).abs() < 0.2,
            "hec_event.time = {}, now = {}",
            hec_event.time,
            now
        );
        assert_eq!((hec_event.time * 1000f64).fract(), 0f64);
    }

    #[test]
    fn splunk_validate_host() {
        let valid = "http://localhost:8888".to_string();
        let invalid_scheme = "localhost:8888".to_string();
        let invalid_uri = "iminvalidohnoes".to_string();

        assert!(validate_host(&valid).is_ok());
        assert!(validate_host(&invalid_scheme).is_err());
        assert!(validate_host(&invalid_uri).is_err());
    }

    #[test]
    fn splunk_build_uri() {
        let uri = build_uri("http://test.com/", "/a");

        assert!(uri.is_ok());
        assert_eq!(format!("{}", uri.unwrap()), "http://test.com/a");
    }
}

#[cfg(test)]
#[cfg(feature = "splunk-integration-tests")]
mod integration_tests {
    use super::*;
    use crate::test_util::retry_until;
    use crate::{
        assert_downcast_matches,
        config::{SinkConfig, SinkContext},
        sinks,
        test_util::{random_lines_with_stream, random_string},
    };
    use futures::stream;
    use serde_json::Value as JsonValue;
    use std::{future::ready, net::SocketAddr};
    use tokio::time::{sleep, Duration};
    use vector_core::event::{BatchNotifier, BatchStatus, Event, LogEvent};
    use warp::Filter;

    const USERNAME: &str = "admin";
    const PASSWORD: &str = "password";

    // It usually takes ~1 second for the event to show up in search, so poll until
    // we see it.
    async fn find_entry(message: &str) -> serde_json::value::Value {
        for _ in 0..20usize {
            match recent_entries(None)
                .await
                .into_iter()
                .find(|entry| entry["_raw"].as_str().unwrap_or("").contains(&message))
            {
                Some(value) => return value,
                None => std::thread::sleep(std::time::Duration::from_millis(100)),
            }
        }
        panic!("Didn't find event in Splunk");
    }

    #[tokio::test]
    async fn splunk_insert_message() {
        let cx = SinkContext::new_test();

        let config = config(Encoding::Text, vec![]).await;
        let (sink, _) = config.build(cx).await.unwrap();

        let message = random_string(100);
        let (batch, mut receiver) = BatchNotifier::new_with_receiver();
        let event = LogEvent::from(message.clone())
            .with_batch_notifier(&batch)
            .into();
        drop(batch);
        sink.run(stream::once(ready(event))).await.unwrap();
        assert_eq!(receiver.try_recv(), Ok(BatchStatus::Delivered));

        let entry = find_entry(message.as_str()).await;

        assert_eq!(message, entry["_raw"].as_str().unwrap());
        assert!(entry.get("message").is_none());
    }

    #[tokio::test]
    async fn splunk_insert_broken_token() {
        let cx = SinkContext::new_test();

        let mut config = config(Encoding::Text, vec![]).await;
        config.token = "BROKEN_TOKEN".into();
        let (sink, _) = config.build(cx).await.unwrap();

        let message = random_string(100);
        let (batch, mut receiver) = BatchNotifier::new_with_receiver();
        let event = LogEvent::from(message.clone())
            .with_batch_notifier(&batch)
            .into();
        drop(batch);
        sink.run(stream::once(ready(event))).await.unwrap();
        assert_eq!(receiver.try_recv(), Ok(BatchStatus::Failed));
    }

    #[tokio::test]
    async fn splunk_insert_source() {
        let cx = SinkContext::new_test();

        let mut config = config(Encoding::Text, vec![]).await;
        config.source = Template::try_from("/var/log/syslog".to_string()).ok();

        let (sink, _) = config.build(cx).await.unwrap();

        let message = random_string(100);
        let event = Event::from(message.clone());
        sink.run(stream::once(ready(event))).await.unwrap();

        let entry = find_entry(message.as_str()).await;

        assert_eq!(entry["source"].as_str(), Some("/var/log/syslog"));
    }

    #[tokio::test]
    async fn splunk_insert_index() {
        let cx = SinkContext::new_test();

        let mut config = config(Encoding::Text, vec![]).await;
        config.index = Template::try_from("custom_index".to_string()).ok();
        let (sink, _) = config.build(cx).await.unwrap();

        let message = random_string(100);
        let event = Event::from(message.clone());
        sink.run(stream::once(ready(event))).await.unwrap();

        let entry = find_entry(message.as_str()).await;

        assert_eq!(entry["index"].as_str().unwrap(), "custom_index");
    }

    #[tokio::test]
    async fn splunk_index_is_interpolated() {
        let cx = SinkContext::new_test();

        let indexed_fields = vec!["asdf".to_string()];
        let mut config = config(Encoding::Json, indexed_fields).await;
        config.index = Template::try_from("{{ index_name }}".to_string()).ok();

        let (sink, _) = config.build(cx).await.unwrap();

        let message = random_string(100);
        let mut event = Event::from(message.clone());
        event.as_mut_log().insert("index_name", "custom_index");
        sink.run(stream::once(ready(event))).await.unwrap();

        let entry = find_entry(message.as_str()).await;

        let index = entry["index"].as_str().unwrap();
        assert_eq!("custom_index", index);
    }

    #[tokio::test]
    async fn splunk_insert_many() {
        let cx = SinkContext::new_test();

        let config = config(Encoding::Text, vec![]).await;
        let (sink, _) = config.build(cx).await.unwrap();

        let (messages, events) = random_lines_with_stream(100, 10, None);
        sink.run(events).await.unwrap();

        let mut found_all = false;
        for _ in 0..20 {
            let entries = recent_entries(None).await;

            found_all = messages.iter().all(|message| {
                entries
                    .iter()
                    .any(|entry| entry["_raw"].as_str().unwrap() == message)
            });

            if found_all {
                break;
            }

            sleep(Duration::from_millis(100)).await;
        }

        assert!(found_all);
    }

    #[tokio::test]
    async fn splunk_custom_fields() {
        let cx = SinkContext::new_test();

        let indexed_fields = vec!["asdf".into()];
        let config = config(Encoding::Json, indexed_fields).await;
        let (sink, _) = config.build(cx).await.unwrap();

        let message = random_string(100);
        let mut event = Event::from(message.clone());
        event.as_mut_log().insert("asdf", "hello");
        sink.run(stream::once(ready(event))).await.unwrap();

        let entry = find_entry(message.as_str()).await;

        assert_eq!(message, entry["message"].as_str().unwrap());
        let asdf = entry["asdf"].as_array().unwrap()[0].as_str().unwrap();
        assert_eq!("hello", asdf);
    }

    #[tokio::test]
    async fn splunk_hostname() {
        let cx = SinkContext::new_test();

        let indexed_fields = vec!["asdf".into()];
        let config = config(Encoding::Json, indexed_fields).await;
        let (sink, _) = config.build(cx).await.unwrap();

        let message = random_string(100);
        let mut event = Event::from(message.clone());
        event.as_mut_log().insert("asdf", "hello");
        event.as_mut_log().insert("host", "example.com:1234");
        sink.run(stream::once(ready(event))).await.unwrap();

        let entry = find_entry(message.as_str()).await;

        assert_eq!(message, entry["message"].as_str().unwrap());
        let asdf = entry["asdf"].as_array().unwrap()[0].as_str().unwrap();
        assert_eq!("hello", asdf);
        let host = entry["host"].as_array().unwrap()[0].as_str().unwrap();
        assert_eq!("example.com:1234", host);
    }

    #[tokio::test]
    async fn splunk_sourcetype() {
        let cx = SinkContext::new_test();

        let indexed_fields = vec!["asdf".to_string()];
        let mut config = config(Encoding::Json, indexed_fields).await;
        config.sourcetype = Template::try_from("_json".to_string()).ok();

        let (sink, _) = config.build(cx).await.unwrap();

        let message = random_string(100);
        let mut event = Event::from(message.clone());
        event.as_mut_log().insert("asdf", "hello");
        sink.run(stream::once(ready(event))).await.unwrap();

        let entry = find_entry(message.as_str()).await;

        assert_eq!(message, entry["message"].as_str().unwrap());
        let asdf = entry["asdf"].as_array().unwrap()[0].as_str().unwrap();
        assert_eq!("hello", asdf);
        let sourcetype = entry["sourcetype"].as_str().unwrap();
        assert_eq!("_json", sourcetype);
    }

    #[tokio::test]
    async fn splunk_configure_hostname() {
        let cx = SinkContext::new_test();

        let config = HecSinkConfig {
            host_key: "roast".into(),
            ..config(Encoding::Json, vec!["asdf".to_string()]).await
        };

        let (sink, _) = config.build(cx).await.unwrap();

        let message = random_string(100);
        let mut event = Event::from(message.clone());
        event.as_mut_log().insert("asdf", "hello");
        event.as_mut_log().insert("host", "example.com:1234");
        event.as_mut_log().insert("roast", "beef.example.com:1234");
        sink.run(stream::once(ready(event))).await.unwrap();

        let entry = find_entry(message.as_str()).await;

        assert_eq!(message, entry["message"].as_str().unwrap());
        let asdf = entry["asdf"].as_array().unwrap()[0].as_str().unwrap();
        assert_eq!("hello", asdf);
        let host = entry["host"].as_array().unwrap()[0].as_str().unwrap();
        assert_eq!("beef.example.com:1234", host);
    }

    #[tokio::test]
    async fn splunk_healthcheck() {
        let config_to_healthcheck = move |config: HecSinkConfig| {
            let tls_settings = TlsSettings::from_options(&config.tls).unwrap();
            let client = HttpClient::new(tls_settings).unwrap();
            sinks::splunk_hec::healthcheck(config, client)
        };

        // OK
        {
            let config = config(Encoding::Text, vec![]).await;
            let healthcheck = config_to_healthcheck(config);
            healthcheck.await.unwrap();
        }

        // Server not listening at address
        {
            let config = HecSinkConfig {
                endpoint: "http://localhost:1111".to_string(),
                ..config(Encoding::Text, vec![]).await
            };
            let healthcheck = config_to_healthcheck(config);
            healthcheck.await.unwrap_err();
        }

        // Invalid token
        // The HEC REST docs claim that the healthcheck endpoint will validate the auth token,
        // but my local testing server returns 200 even with a bad token.
        // {
        //     let healthcheck = sinks::splunk::healthcheck(
        //         "wrong".to_string(),
        //         "http://localhost:8088".to_string(),
        //     )
        //     .unwrap();

        //     assert_eq!(rt.block_on(healthcheck).unwrap_err(), "Invalid HEC token");
        // }

        // Unhealthy server
        {
            let config = HecSinkConfig {
                endpoint: "http://localhost:5503".to_string(),
                ..config(Encoding::Text, vec![]).await
            };

            let unhealthy = warp::any()
                .map(|| warp::reply::with_status("i'm sad", StatusCode::SERVICE_UNAVAILABLE));
            let server = warp::serve(unhealthy).bind("0.0.0.0:5503".parse::<SocketAddr>().unwrap());
            tokio::spawn(server);

            let healthcheck = config_to_healthcheck(config);
            assert_downcast_matches!(
                healthcheck.await.unwrap_err(),
                HealthcheckError,
                HealthcheckError::QueuesFull
            );
        }
    }

    async fn recent_entries(index: Option<&str>) -> Vec<JsonValue> {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();

        // https://docs.splunk.com/Documentation/Splunk/7.2.1/RESTREF/RESTsearch#search.2Fjobs
        let search_query = match index {
            Some(index) => format!("search index={}", index),
            None => "search *".into(),
        };
        let res = client
            .post("https://localhost:8089/services/search/jobs?output_mode=json")
            .form(&vec![
                ("search", &search_query[..]),
                ("exec_mode", "oneshot"),
                ("f", "*"),
            ])
            .basic_auth(USERNAME, Some(PASSWORD))
            .send()
            .await
            .unwrap();
        let json: JsonValue = res.json().await.unwrap();

        json["results"].as_array().unwrap().clone()
    }

    async fn config(
        encoding: impl Into<EncodingConfig<Encoding>>,
        indexed_fields: Vec<String>,
    ) -> HecSinkConfig {
        HecSinkConfig {
            token: get_token().await,
            endpoint: "http://localhost:8088/".into(),
            host_key: "host".into(),
            indexed_fields,
            index: None,
            sourcetype: None,
            source: None,
            encoding: encoding.into(),
            compression: Compression::None,
            batch: BatchConfig {
                max_events: Some(1),
                ..Default::default()
            },
            request: TowerRequestConfig::default(),
            tls: None,
        }
    }

    async fn get_token() -> String {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();

        let res = retry_until(
            || {
                client
                    .get("https://localhost:8089/services/data/inputs/http?output_mode=json")
                    .basic_auth(USERNAME, Some(PASSWORD))
                    .send()
            },
            Duration::from_millis(500),
            Duration::from_secs(30),
        )
        .await;

        let json: JsonValue = res.json().await.unwrap();
        let entries = json["entry"].as_array().unwrap().clone();

        if entries.is_empty() {
            // TODO: create one automatically
            panic!("You don't have any HTTP Event Collector inputs set up in Splunk");
        }

        entries[0]["content"]["token"].as_str().unwrap().to_owned()
    }
}
