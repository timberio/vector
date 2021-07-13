use super::ApiKey;
use crate::{
    config::{
        log_schema, DataType, GenerateConfig, ProxyConfig, SinkConfig, SinkContext, SinkDescription,
    },
    event::Event,
    http::HttpClient,
    internal_events::DatadogLogEventProcessed,
    sinks::{
        util::{
            batch::{Batch, BatchError},
            encode_event,
            encoding::{EncodingConfig, EncodingConfiguration},
            http::{HttpSink, PartitionHttpSink},
            BatchConfig, BatchSettings, BoxedRawValue, Compression, EncodedEvent, Encoding,
            JsonArrayBuffer, PartitionBuffer, PartitionInnerBuffer, TowerRequestConfig, VecBuffer,
        },
        Healthcheck, VectorSink,
    },
    tls::{MaybeTlsSettings, TlsConfig},
};
use bytes::Bytes;
use flate2::write::GzEncoder;
use futures::{FutureExt, SinkExt};
use http::{Request, StatusCode};
use hyper::body::Body;
use indoc::indoc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{io::Write, sync::Arc, time::Duration};

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct DatadogLogsConfig {
    endpoint: Option<String>,
    // Deprecated, replaced by the site option
    region: Option<super::Region>,
    site: Option<String>,
    // Deprecated name
    #[serde(alias = "api_key")]
    default_api_key: String,
    encoding: EncodingConfig<Encoding>,
    tls: Option<TlsConfig>,

    #[serde(default)]
    compression: Option<Compression>,

    #[serde(default)]
    batch: BatchConfig,

    #[serde(default)]
    request: TowerRequestConfig,
    #[serde(
        default,
        skip_serializing_if = "crate::serde::skip_serializing_if_default"
    )]
    proxy: ProxyConfig,
}

#[derive(Clone)]
struct DatadogLogsJsonService {
    config: DatadogLogsConfig,
    // Used to store the complete URI and avoid calling `get_uri` for each request
    uri: String,
    default_api_key: ApiKey,
}

#[derive(Clone)]
struct DatadogLogsTextService {
    config: DatadogLogsConfig,
    // Used to store the complete URI and avoid calling `get_uri` for each request
    uri: String,
    default_api_key: ApiKey,
}

inventory::submit! {
    SinkDescription::new::<DatadogLogsConfig>("datadog_logs")
}

impl GenerateConfig for DatadogLogsConfig {
    fn generate_config() -> toml::Value {
        toml::from_str(indoc! {r#"
            default_api_key = "${DATADOG_API_KEY_ENV_VAR}"
            encoding.codec = "json"
        "#})
        .unwrap()
    }
}

impl DatadogLogsConfig {
    fn get_uri(&self) -> String {
        self.endpoint
            .clone()
            .or_else(|| {
                self.site
                    .as_ref()
                    .map(|s| format!("https://http-intake.logs.{}/v1/input", s))
            })
            .unwrap_or_else(|| match self.region {
                Some(super::Region::Eu) => {
                    "https://http-intake.logs.datadoghq.eu/v1/input".to_string()
                }
                None | Some(super::Region::Us) => {
                    "https://http-intake.logs.datadoghq.com/v1/input".to_string()
                }
            })
    }

    fn batch_settings<T: Batch>(&self) -> Result<BatchSettings<T>, BatchError> {
        BatchSettings::default()
            .bytes(bytesize::kib(100u64))
            .events(20)
            .timeout(1)
            .parse_config(self.batch)
    }

    /// Builds the required BatchedHttpSink.
    /// Since the DataDog sink can create one of two different sinks, this
    /// extracts most of the shared functionality required to create either sink.
    fn build_sink<T, B, O>(
        &self,
        cx: SinkContext,
        service: T,
        batch: B,
        timeout: Duration,
    ) -> crate::Result<(VectorSink, Healthcheck)>
    where
        O: 'static,
        B: Batch<Output = Vec<O>> + std::marker::Send + 'static,
        B::Output: std::marker::Send + Clone,
        B::Input: std::marker::Send,
        T: HttpSink<
                Input = PartitionInnerBuffer<B::Input, ApiKey>,
                Output = PartitionInnerBuffer<B::Output, ApiKey>,
            > + Clone,
    {
        let request_settings = self.request.unwrap_with(&TowerRequestConfig::default());

        let tls_settings = MaybeTlsSettings::from_config(
            &Some(self.tls.clone().unwrap_or_else(TlsConfig::enabled)),
            false,
        )?;

        let proxy = cx.globals.proxy.build(&self.proxy);
        let client = HttpClient::new(tls_settings, proxy)?;
        let healthcheck = healthcheck(
            service.clone(),
            client.clone(),
            self.default_api_key.clone(),
        )
        .boxed();
        let sink = PartitionHttpSink::new(
            service,
            PartitionBuffer::new(batch),
            request_settings,
            timeout,
            client,
            cx.acker(),
        )
        .sink_map_err(|error| error!(message = "Fatal datadog_logs text sink error.", %error));

        Ok((VectorSink::Sink(Box::new(sink)), healthcheck))
    }

    /// Build the request, GZipping the contents if the config specifies.
    fn build_request(
        &self,
        uri: &str,
        api_key: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> crate::Result<http::Request<Vec<u8>>> {
        let request = Request::post(uri)
            .header("Content-Type", content_type)
            .header("DD-API-KEY", api_key);

        let compression = self.compression.unwrap_or(Compression::Gzip(None));

        let (request, body) = match compression {
            Compression::None => (request, body),
            Compression::Gzip(level) => {
                // Default the compression level to 6, which is similar to datadog agent.
                // https://docs.datadoghq.com/agent/logs/log_transport/?tab=https#log-compression
                let level = level.unwrap_or(6);
                let mut encoder =
                    GzEncoder::new(Vec::new(), flate2::Compression::new(level as u32));

                encoder.write_all(&body)?;
                (
                    request.header("Content-Encoding", "gzip"),
                    encoder.finish()?,
                )
            }
        };

        request
            .header("Content-Length", body.len())
            .body(body)
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "datadog_logs")]
impl SinkConfig for DatadogLogsConfig {
    async fn build(&self, cx: SinkContext) -> crate::Result<(VectorSink, Healthcheck)> {
        // Create a different sink depending on which encoding we have chosen.
        // Json and Text have different batching strategies and so each needs to be
        // handled differently.
        match self.encoding.codec {
            Encoding::Json => {
                let batch_settings = self.batch_settings()?;
                self.build_sink(
                    cx,
                    DatadogLogsJsonService {
                        config: self.clone(),
                        uri: self.get_uri(),
                        default_api_key: Arc::from(self.default_api_key.clone()),
                    },
                    JsonArrayBuffer::new(batch_settings.size),
                    batch_settings.timeout,
                )
            }
            Encoding::Text => {
                let batch_settings = self.batch_settings()?;
                self.build_sink(
                    cx,
                    DatadogLogsTextService {
                        config: self.clone(),
                        uri: self.get_uri(),
                        default_api_key: Arc::from(self.default_api_key.clone()),
                    },
                    VecBuffer::new(batch_settings.size),
                    batch_settings.timeout,
                )
            }
        }
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn sink_type(&self) -> &'static str {
        "datadog_logs"
    }
}

#[async_trait::async_trait]
impl HttpSink for DatadogLogsJsonService {
    type Input = PartitionInnerBuffer<serde_json::Value, ApiKey>;
    type Output = PartitionInnerBuffer<Vec<BoxedRawValue>, ApiKey>;

    fn encode_event(&self, mut event: Event) -> Option<EncodedEvent<Self::Input>> {
        let log = event.as_mut_log();

        if let Some(message) = log.remove(log_schema().message_key()) {
            log.insert("message", message);
        }

        if let Some(timestamp) = log.remove(log_schema().timestamp_key()) {
            log.insert("date", timestamp);
        }

        if let Some(host) = log.remove(log_schema().host_key()) {
            log.insert("host", host);
        }

        self.config.encoding.apply_rules(&mut event);

        let (fields, metadata) = event.into_log().into_parts();
        let json_event = json!(fields);
        let api_key = metadata
            .datadog_api_key()
            .as_ref()
            .unwrap_or(&self.default_api_key);

        Some(EncodedEvent {
            item: PartitionInnerBuffer::new(json_event, Arc::clone(api_key)),
            metadata: Some(metadata),
        })
    }

    async fn build_request(&self, events: Self::Output) -> crate::Result<Request<Vec<u8>>> {
        let (events, api_key) = events.into_parts();

        let body = serde_json::to_vec(&events)?;
        // check the number of events to ignore health-check requests
        if !events.is_empty() {
            emit!(DatadogLogEventProcessed {
                byte_size: body.len(),
                count: events.len(),
            });
        }
        self.config
            .build_request(self.uri.as_str(), &api_key[..], "application/json", body)
    }
}

#[async_trait::async_trait]
impl HttpSink for DatadogLogsTextService {
    type Input = PartitionInnerBuffer<Bytes, ApiKey>;
    type Output = PartitionInnerBuffer<Vec<Bytes>, ApiKey>;

    fn encode_event(&self, event: Event) -> Option<EncodedEvent<Self::Input>> {
        let api_key = Arc::clone(
            event
                .metadata()
                .datadog_api_key()
                .as_ref()
                .unwrap_or(&self.default_api_key),
        );

        encode_event(event, &self.config.encoding).map(|e| {
            emit!(DatadogLogEventProcessed {
                byte_size: e.item.len(),
                count: 1,
            });

            EncodedEvent {
                item: PartitionInnerBuffer::new(e.item, api_key),
                metadata: e.metadata,
            }
        })
    }

    async fn build_request(&self, events: Self::Output) -> crate::Result<Request<Vec<u8>>> {
        let (events, api_key) = events.into_parts();
        let body: Vec<u8> = events.into_iter().flat_map(Bytes::into_iter).collect();

        self.config
            .build_request(self.uri.as_str(), &api_key[..], "text/plain", body)
    }
}

/// The healthcheck is performed by sending an empty request to Datadog and checking
/// the return.
async fn healthcheck<T, O>(sink: T, client: HttpClient, api_key: String) -> crate::Result<()>
where
    T: HttpSink<Output = PartitionInnerBuffer<Vec<O>, ApiKey>>,
{
    let req = sink
        .build_request(PartitionInnerBuffer::new(
            Vec::with_capacity(0),
            Arc::from(api_key),
        ))
        .await?
        .map(Body::from);

    let res = client.send(req).await?;

    let status = res.status();
    let body = hyper::body::to_bytes(res.into_body()).await?;

    match status {
        StatusCode::OK => Ok(()),
        StatusCode::UNAUTHORIZED => {
            let json: serde_json::Value = serde_json::from_slice(&body[..])?;

            Err(json
                .as_object()
                .and_then(|o| o.get("error"))
                .and_then(|s| s.as_str())
                .unwrap_or("Token is not valid, 401 returned.")
                .to_string()
                .into())
        }
        _ => {
            let body = String::from_utf8_lossy(&body[..]);

            Err(format!(
                "Server returned unexpected error status: {} body: {}",
                status, body
            )
            .into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::SinkConfig,
        sinks::util::test::{build_test_server_status, load_sink},
        test_util::{next_addr, random_lines_with_stream},
    };
    use futures::{
        channel::mpsc::{Receiver, TryRecvError},
        stream, StreamExt,
    };
    use hyper::StatusCode;
    use indoc::indoc;
    use pretty_assertions::assert_eq;
    use vector_core::event::{BatchNotifier, BatchStatus};

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<DatadogLogsConfig>();
    }

    fn event_with_api_key(msg: &str, key: &str) -> Event {
        let mut e = Event::from(msg);
        e.as_mut_log()
            .metadata_mut()
            .set_datadog_api_key(Some(Arc::from(key)));
        e
    }

    #[tokio::test]
    async fn smoke_text() {
        let (expected, output) = smoke_test("text").await;

        for (i, val) in output.iter().enumerate() {
            assert_eq!(val.0.headers.get("Content-Type").unwrap(), "text/plain");
            assert_eq!(val.1, format!("{}\n", expected[i]));
        }
    }

    #[tokio::test]
    async fn smoke_json() {
        let (expected, output) = smoke_test("json").await;

        for (i, val) in output.iter().enumerate() {
            assert_eq!(
                val.0.headers.get("Content-Type").unwrap(),
                "application/json"
            );

            let mut json = serde_json::Deserializer::from_slice(&val.1[..])
                .into_iter::<serde_json::Value>()
                .map(|v| v.expect("decoding json"));

            let json = json.next().unwrap();

            // The json we send to Datadog is an array of events.
            // As we have set batch.max_events to 1, each entry will be
            // an array containing a single record.
            let message = json
                .get(0)
                .unwrap()
                .get("message")
                .unwrap()
                .as_str()
                .unwrap();
            assert_eq!(message, expected[i]);
        }
    }

    async fn smoke_test(encoding: &str) -> (Vec<String>, Vec<(http::request::Parts, Bytes)>) {
        let (expected, rx) = start_test(encoding, StatusCode::OK, BatchStatus::Delivered).await;

        let output = rx.take(expected.len()).collect::<Vec<_>>().await;

        (expected, output)
    }

    #[tokio::test]
    async fn handles_failure_text() {
        handles_failure("text").await;
    }

    #[tokio::test]
    async fn handles_failure_json() {
        handles_failure("json").await;
    }

    async fn handles_failure(encoding: &str) {
        let (_expected, mut rx) =
            start_test(encoding, StatusCode::FORBIDDEN, BatchStatus::Failed).await;

        assert!(matches!(rx.try_next(), Err(TryRecvError { .. })));
    }

    async fn start_test(
        encoding: &str,
        http_status: StatusCode,
        batch_status: BatchStatus,
    ) -> (Vec<String>, Receiver<(http::request::Parts, Bytes)>) {
        let config = format!(
            indoc! {r#"
            default_api_key = "atoken"
            encoding = "{}"
            compression = "none"
            batch.max_events = 1
        "#},
            encoding
        );
        let (mut config, cx) = load_sink::<DatadogLogsConfig>(&config).unwrap();

        let addr = next_addr();
        // Swap out the endpoint so we can force send it
        // to our local server
        let endpoint = format!("http://{}", addr);
        config.endpoint = Some(endpoint.clone());

        let (sink, _) = config.build(cx).await.unwrap();

        let (rx, _trigger, server) = build_test_server_status(addr, http_status);
        tokio::spawn(server);

        let (batch, mut receiver) = BatchNotifier::new_with_receiver();
        let (expected, events) = random_lines_with_stream(100, 10, Some(batch));

        let _ = sink.run(events).await.unwrap();

        assert_eq!(receiver.try_recv(), Ok(batch_status));

        (expected, rx)
    }

    #[tokio::test]
    async fn api_key_in_metadata() {
        let (mut config, cx) = load_sink::<DatadogLogsConfig>(indoc! {r#"
            default_api_key = "atoken"
            encoding = "json"
            compression = "none"
            batch.max_events = 1
        "#})
        .unwrap();

        let addr = next_addr();
        // Swap out the endpoint so we can force send it
        // to our local server
        let endpoint = format!("http://{}", addr);
        config.endpoint = Some(endpoint.clone());

        let (sink, _) = config.build(cx).await.unwrap();

        let (rx, _trigger, server) = build_test_server_status(addr, StatusCode::OK);
        tokio::spawn(server);

        let (expected, events) = random_lines_with_stream(100, 10, None);

        let mut events = events.map(|mut e| {
            e.as_mut_log()
                .metadata_mut()
                .set_datadog_api_key(Some(Arc::from("from_metadata")));
            Ok(e)
        });

        let _ = sink.into_sink().send_all(&mut events).await.unwrap();
        let output = rx.take(expected.len()).collect::<Vec<_>>().await;

        for (i, val) in output.iter().enumerate() {
            assert_eq!(val.0.headers.get("DD-API-KEY").unwrap(), "from_metadata");

            assert_eq!(
                val.0.headers.get("Content-Type").unwrap(),
                "application/json"
            );

            let mut json = serde_json::Deserializer::from_slice(&val.1[..])
                .into_iter::<serde_json::Value>()
                .map(|v| v.expect("decoding json"));

            let json = json.next().unwrap();

            // The json we send to Datadog is an array of events.
            // As we have set batch.max_events to 1, each entry will be
            // an array containing a single record.
            let message = json
                .get(0)
                .unwrap()
                .get("message")
                .unwrap()
                .as_str()
                .unwrap();
            assert_eq!(message, expected[i]);
        }
    }

    #[tokio::test]
    async fn multiple_api_keys() {
        let (mut config, cx) = load_sink::<DatadogLogsConfig>(indoc! {r#"
            default_api_key = "atoken"
            encoding = "json"
            compression = "none"
            batch.max_events = 1
        "#})
        .unwrap();

        let addr = next_addr();
        // Swap out the endpoint so we can force send it
        // to our local server
        let endpoint = format!("http://{}", addr);
        config.endpoint = Some(endpoint.clone());

        let (sink, _) = config.build(cx).await.unwrap();

        let (rx, _trigger, server) = build_test_server_status(addr, StatusCode::OK);
        tokio::spawn(server);

        let events = vec![
            event_with_api_key("mow", "pkc"),
            event_with_api_key("pnh", "vvo"),
            Event::from("no API key in metadata"),
        ];

        let _ = sink.run(stream::iter(events)).await.unwrap();

        let mut keys = rx
            .take(3)
            .map(|r| r.0.headers.get("DD-API-KEY").unwrap().clone())
            .collect::<Vec<_>>()
            .await;

        keys.sort();
        assert_eq!(keys, vec!["atoken", "pkc", "vvo"])
    }
}
