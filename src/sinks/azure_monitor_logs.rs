use crate::{
    config::{log_schema, DataType, ProxyConfig, SinkConfig, SinkContext, SinkDescription},
    event::{Event, Value},
    http::HttpClient,
    sinks::{
        util::{
            encoding::{EncodingConfigWithDefault, EncodingConfiguration},
            http::{BatchedHttpSink, HttpSink},
            BatchConfig, BatchSettings, BoxedRawValue, EncodedEvent, JsonArrayBuffer,
            TowerRequestConfig,
        },
        Healthcheck, VectorSink,
    },
    tls::{TlsOptions, TlsSettings},
};
use bytesize::ByteSize;
use futures::{FutureExt, SinkExt};
use http::{
    header, header::HeaderMap, header::HeaderName, header::HeaderValue, Request, StatusCode, Uri,
};
use hyper::Body;
use lazy_static::lazy_static;
use openssl::{base64, hash, pkey, sign};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

fn default_host() -> String {
    "ods.opinsights.azure.com".into()
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct AzureMonitorLogsConfig {
    pub customer_id: String,
    pub shared_key: String,
    pub log_type: String,
    pub azure_resource_id: Option<String>,
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(
        skip_serializing_if = "crate::serde::skip_serializing_if_default",
        default
    )]
    pub encoding: EncodingConfigWithDefault<Encoding>,
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

#[derive(Deserialize, Serialize, Debug, Eq, PartialEq, Clone, Derivative)]
#[serde(rename_all = "snake_case")]
#[derivative(Default)]
pub enum Encoding {
    #[derivative(Default)]
    Default,
}

lazy_static! {
    static ref REQUEST_DEFAULTS: TowerRequestConfig = TowerRequestConfig {
        ..Default::default()
    };
    static ref LOG_TYPE_REGEX: Regex = Regex::new(r"^\w+$").unwrap();
    static ref LOG_TYPE_HEADER: HeaderName = HeaderName::from_static("log-type");
    static ref X_MS_DATE_HEADER: HeaderName = HeaderName::from_static(X_MS_DATE);
    static ref X_MS_AZURE_RESOURCE_HEADER: HeaderName =
        HeaderName::from_static("x-ms-azureresourceid");
    static ref TIME_GENERATED_FIELD_HEADER: HeaderName =
        HeaderName::from_static("time-generated-field");
    static ref CONTENT_TYPE_VALUE: HeaderValue = HeaderValue::from_static(CONTENT_TYPE);
}
inventory::submit! {
    SinkDescription::new::<AzureMonitorLogsConfig>("azure_monitor_logs")
}

impl_generate_config_from_default!(AzureMonitorLogsConfig);

/// Max number of bytes in request body
const MAX_BATCH_SIZE_MB: u64 = 30;
/// API endpoint for submitting logs
const RESOURCE: &str = "/api/logs";
/// JSON content type of logs
const CONTENT_TYPE: &str = "application/json";
/// Custom header used for signing logs
const X_MS_DATE: &str = "x-ms-date";
/// Shared key prefix
const SHARED_KEY: &str = "SharedKey";
/// API version
const API_VERSION: &str = "2016-04-01";

#[async_trait::async_trait]
#[typetag::serde(name = "azure_monitor_logs")]
impl SinkConfig for AzureMonitorLogsConfig {
    async fn build(&self, cx: SinkContext) -> crate::Result<(VectorSink, Healthcheck)> {
        let batch_settings = BatchSettings::default()
            .bytes(bytesize::kib(5000u64))
            .timeout(1)
            .parse_config(self.batch)?;

        let batch_bytes = batch_settings.size.bytes as u64;

        if batch_bytes > bytesize::mb(MAX_BATCH_SIZE_MB) {
            return Err(format!(
                "provided batch size is too big for Azure Monitor: {}, max is {}",
                ByteSize::b(batch_bytes),
                ByteSize::mb(MAX_BATCH_SIZE_MB)
            )
            .into());
        }

        let tls_settings = TlsSettings::from_options(&self.tls)?;
        let proxy = cx.globals.proxy.build(&self.proxy);
        let client = HttpClient::new(Some(tls_settings), proxy)?;

        let sink = AzureMonitorLogsSink::new(self)?;
        let request_settings = self.request.unwrap_with(&REQUEST_DEFAULTS);

        let healthcheck = healthcheck(sink.clone(), client.clone()).boxed();

        let sink = BatchedHttpSink::new(
            sink,
            JsonArrayBuffer::new(batch_settings.size),
            request_settings,
            batch_settings.timeout,
            client,
            cx.acker(),
        )
        .sink_map_err(|error| error!(message = "Fatal azure_monitor_logs sink error.", %error));

        Ok((VectorSink::Sink(Box::new(sink)), healthcheck))
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn sink_type(&self) -> &'static str {
        "azure_monitor_logs"
    }
}

#[derive(Clone)]
struct AzureMonitorLogsSink {
    uri: Uri,
    customer_id: String,
    encoding: EncodingConfigWithDefault<Encoding>,
    shared_key: pkey::PKey<pkey::Private>,
    default_headers: HeaderMap,
}

#[async_trait::async_trait]
impl HttpSink for AzureMonitorLogsSink {
    type Input = serde_json::Value;
    type Output = Vec<BoxedRawValue>;

    fn encode_event(&self, mut event: Event) -> Option<EncodedEvent<Self::Input>> {
        self.encoding.apply_rules(&mut event);

        // it seems like Azure Monitor doesn't support full 9-digit nanosecond precision
        // adjust the timestamp format accordingly, keeping only milliseconds
        let mut log = event.into_log();
        let timestamp_key = log_schema().timestamp_key();

        let timestamp = if let Some(Value::Timestamp(ts)) = log.remove(timestamp_key) {
            ts
        } else {
            chrono::Utc::now()
        };

        let mut entry = serde_json::json!(&log);
        let object_entry = entry.as_object_mut().unwrap();
        object_entry.insert(
            timestamp_key.to_string(),
            JsonValue::String(timestamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
        );

        Some(EncodedEvent::new(entry).with_metadata(log))
    }

    async fn build_request(&self, events: Self::Output) -> crate::Result<Request<Vec<u8>>> {
        self.build_request_sync(events)
    }
}

impl AzureMonitorLogsSink {
    fn new(config: &AzureMonitorLogsConfig) -> crate::Result<AzureMonitorLogsSink> {
        let url = format!(
            "https://{}.{}{}?api-version={}",
            config.customer_id, config.host, RESOURCE, API_VERSION
        );
        let uri: Uri = url.parse()?;

        if config.shared_key.is_empty() {
            return Err("shared_key can't be an empty string".into());
        }

        let shared_key_bytes = base64::decode_block(&config.shared_key)?;
        let shared_key = pkey::PKey::hmac(&shared_key_bytes)?;
        let mut default_headers = HeaderMap::with_capacity(3);

        if config.log_type.len() > 100 || !LOG_TYPE_REGEX.is_match(&config.log_type) {
            return Err(format!(
                "invalid log_type \"{}\": log type can only contain letters, numbers, and underscore (_), and may not exceed 100 characters",
                config.log_type
            ).into());
        }

        let log_type = HeaderValue::from_str(&config.log_type)?;
        default_headers.insert(LOG_TYPE_HEADER.clone(), log_type);

        let timestamp_key = log_schema().timestamp_key();
        default_headers.insert(
            TIME_GENERATED_FIELD_HEADER.clone(),
            HeaderValue::from_str(timestamp_key)?,
        );

        if let Some(azure_resource_id) = &config.azure_resource_id {
            if azure_resource_id.is_empty() {
                return Err("azure_resource_id can't be an empty string".into());
            }

            default_headers.insert(
                X_MS_AZURE_RESOURCE_HEADER.clone(),
                HeaderValue::from_str(azure_resource_id)?,
            );
        }

        default_headers.insert(header::CONTENT_TYPE, CONTENT_TYPE_VALUE.clone());

        Ok(AzureMonitorLogsSink {
            uri,
            encoding: config.encoding.clone(),
            customer_id: config.customer_id.clone(),
            shared_key,
            default_headers,
        })
    }

    fn build_request_sync(&self, events: Vec<BoxedRawValue>) -> crate::Result<Request<Vec<u8>>> {
        let body = serde_json::to_vec(&events)?;
        let len = body.len();

        let mut request = Request::post(self.uri.clone()).body(body)?;
        let rfc1123date = chrono::Utc::now()
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();

        let authorization = self.build_authorization_header_value(&rfc1123date, len)?;

        *request.headers_mut() = self.default_headers.clone();
        request
            .headers_mut()
            .insert(header::AUTHORIZATION, authorization.parse()?);
        request
            .headers_mut()
            .insert(X_MS_DATE_HEADER.clone(), rfc1123date.parse()?);

        Ok(request)
    }

    fn build_authorization_header_value(
        &self,
        rfc1123date: &str,
        len: usize,
    ) -> crate::Result<String> {
        let string_to_hash = format!(
            "POST\n{}\n{}\n{}:{}\n{}",
            len, CONTENT_TYPE, X_MS_DATE, rfc1123date, RESOURCE
        );
        let mut signer = sign::Signer::new(hash::MessageDigest::sha256(), &self.shared_key)?;
        signer.update(string_to_hash.as_bytes())?;

        let signature = signer.sign_to_vec()?;
        let signature_base64 = base64::encode_block(&signature);

        Ok(format!(
            "{} {}:{}",
            SHARED_KEY, self.customer_id, signature_base64
        ))
    }
}

async fn healthcheck(sink: AzureMonitorLogsSink, client: HttpClient) -> crate::Result<()> {
    let request = sink.build_request(vec![]).await?.map(Body::from);

    let res = client.send(request).await?;

    if res.status().is_server_error() {
        return Err("Server returned a server error".into());
    }

    if res.status() == StatusCode::FORBIDDEN {
        return Err("The service failed to authenticate the request. Verify that the workspace ID and connection key are valid".into());
    }

    if res.status() == StatusCode::NOT_FOUND {
        return Err("Either the URL provided is incorrect, or the request is too large".into());
    }

    if res.status() == StatusCode::BAD_REQUEST {
        return Err("The workspace has been closed or the request was invalid".into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::LogEvent;
    use serde_json::value::RawValue;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<AzureMonitorLogsConfig>();
    }

    fn insert_timestamp_kv(log: &mut LogEvent) -> (String, String) {
        let now = chrono::Utc::now();

        let timestamp_key = log_schema().timestamp_key().to_string();
        let timestamp_value = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        log.insert(&timestamp_key, now);

        (timestamp_key, timestamp_value)
    }

    #[test]
    fn encode_valid() {
        let config: AzureMonitorLogsConfig = toml::from_str(
            r#"
            # random GUID and random 64 Base-64 encoded bytes
            customer_id = "97ce69d9-b4be-4241-8dbd-d265edcf06c4"
            shared_key = "SERsIYhgMVlJB6uPsq49gCxNiruf6v0vhMYE+lfzbSGcXjdViZdV/e5pEMTYtw9f8SkVLf4LFlLCc2KxtRZfCA=="
            log_type = "Vector"
        "#,
        )
        .unwrap();

        let sink = AzureMonitorLogsSink::new(&config).unwrap();
        let mut log = [("message", "hello world")]
            .iter()
            .copied()
            .collect::<LogEvent>();
        let (timestamp_key, timestamp_value) = insert_timestamp_kv(&mut log);

        let event = Event::from(log);
        let json = sink.encode_event(event).unwrap().item;
        let expected_json = serde_json::json!({
            timestamp_key: timestamp_value,
            "message": "hello world"
        });
        assert_eq!(json, expected_json);
    }

    #[test]
    fn correct_request() {
        let config: AzureMonitorLogsConfig = toml::from_str(
            r#"
            # random GUID and random 64 Base-64 encoded bytes
            customer_id = "97ce69d9-b4be-4241-8dbd-d265edcf06c4"
            shared_key = "SERsIYhgMVlJB6uPsq49gCxNiruf6v0vhMYE+lfzbSGcXjdViZdV/e5pEMTYtw9f8SkVLf4LFlLCc2KxtRZfCA=="
            log_type = "Vector"
            azure_resource_id = "97ce69d9-b4be-4241-8dbd-d265edcf06c4"
        "#,
        )
        .unwrap();

        let sink = AzureMonitorLogsSink::new(&config).unwrap();

        let mut log1 = [("message", "hello")].iter().copied().collect::<LogEvent>();
        let (timestamp_key1, timestamp_value1) = insert_timestamp_kv(&mut log1);

        let mut log2 = [("message", "world")].iter().copied().collect::<LogEvent>();
        let (timestamp_key2, timestamp_value2) = insert_timestamp_kv(&mut log2);

        let event1 = sink.encode_event(Event::from(log1)).unwrap().item;
        let event2 = sink.encode_event(Event::from(log2)).unwrap().item;

        let json1 = serde_json::to_string(&event1).unwrap();
        let json2 = serde_json::to_string(&event2).unwrap();
        let raw1 = RawValue::from_string(json1).unwrap();
        let raw2 = RawValue::from_string(json2).unwrap();

        let events = vec![raw1, raw2];

        let request = sink.build_request_sync(events);

        let (parts, body) = request.unwrap().into_parts();
        assert_eq!(&parts.method.to_string(), "POST");

        let json: serde_json::Value = serde_json::from_slice(&body[..]).unwrap();
        let expected_json = serde_json::json!([
            {
                timestamp_key1: timestamp_value1,
                "message": "hello"
            },
            {
                timestamp_key2: timestamp_value2,
                "message": "world"
            }
        ]);
        assert_eq!(json, expected_json);

        let headers = parts.headers;
        let rfc1123date = headers.get("x-ms-date").unwrap();

        let auth_expected = sink
            .build_authorization_header_value(rfc1123date.to_str().unwrap(), body.len())
            .unwrap();

        let authorization = headers.get("authorization").unwrap();
        assert_eq!(authorization.to_str().unwrap(), &auth_expected);

        let log_type = headers.get("log-type").unwrap();
        assert_eq!(log_type.to_str().unwrap(), "Vector");

        let time_generated_field = headers.get("time-generated-field").unwrap();
        let timestamp_key = log_schema().timestamp_key();
        assert_eq!(time_generated_field.to_str().unwrap(), timestamp_key);

        let azure_resource_id = headers.get("x-ms-azureresourceid").unwrap();
        assert_eq!(
            azure_resource_id.to_str().unwrap(),
            "97ce69d9-b4be-4241-8dbd-d265edcf06c4"
        );

        assert_eq!(
            &parts.uri.to_string(),
            "https://97ce69d9-b4be-4241-8dbd-d265edcf06c4.ods.opinsights.azure.com/api/logs?api-version=2016-04-01"
        );
    }

    #[tokio::test]
    async fn fails_missing_creds() {
        let config: AzureMonitorLogsConfig = toml::from_str(
            r#"
            customer_id = "97ce69d9-b4be-4241-8dbd-d265edcf06c4"
            shared_key = ""
            log_type = "Vector"
            azure_resource_id = "97ce69d9-b4be-4241-8dbd-d265edcf06c4"
        "#,
        )
        .unwrap();
        if config.build(SinkContext::new_test()).await.is_ok() {
            panic!("config.build failed to error");
        }
    }

    #[test]
    fn correct_host() {
        let config_default = toml::from_str::<AzureMonitorLogsConfig>(
            r#"
            customer_id = "97ce69d9-b4be-4241-8dbd-d265edcf06c4"
            shared_key = "SERsIYhgMVlJB6uPsq49gCxNiruf6v0vhMYE+lfzbSGcXjdViZdV/e5pEMTYtw9f8SkVLf4LFlLCc2KxtRZfCA=="
            log_type = "Vector"
        "#,
        )
        .expect("Config parsing failed without custom host");
        assert_eq!(config_default.host, default_host());

        let config_cn = toml::from_str::<AzureMonitorLogsConfig>(
            r#"
            customer_id = "97ce69d9-b4be-4241-8dbd-d265edcf06c4"
            shared_key = "SERsIYhgMVlJB6uPsq49gCxNiruf6v0vhMYE+lfzbSGcXjdViZdV/e5pEMTYtw9f8SkVLf4LFlLCc2KxtRZfCA=="
            log_type = "Vector"
            host = "ods.opinsights.azure.cn"
        "#,
        )
        .expect("Config parsing failed with .cn custom host");
        assert_eq!(config_cn.host, "ods.opinsights.azure.cn");
    }

    #[tokio::test]
    async fn fails_invalid_base64() {
        let config: AzureMonitorLogsConfig = toml::from_str(
            r#"
            customer_id = "97ce69d9-b4be-4241-8dbd-d265edcf06c4"
            shared_key = "1Qs77Vz40+iDMBBTRmROKJwnEX"
            log_type = "Vector"
            azure_resource_id = "97ce69d9-b4be-4241-8dbd-d265edcf06c4"
        "#,
        )
        .unwrap();
        if config.build(SinkContext::new_test()).await.is_ok() {
            panic!("config.build failed to error");
        }
    }

    #[test]
    fn fails_config_missing_fields() {
        toml::from_str::<AzureMonitorLogsConfig>(
            r#"
            customer_id = "97ce69d9-b4be-4241-8dbd-d265edcf06c4"
            shared_key = "SERsIYhgMVlJB6uPsq49gCxNiruf6v0vhMYE+lfzbSGcXjdViZdV/e5pEMTYtw9f8SkVLf4LFlLCc2KxtRZfCA=="
            azure_resource_id = "97ce69d9-b4be-4241-8dbd-d265edcf06c4"
        "#,
        )
        .expect_err("Config parsing failed to error with missing log_type");

        toml::from_str::<AzureMonitorLogsConfig>(
            r#"
            customer_id = "97ce69d9-b4be-4241-8dbd-d265edcf06c4"
            log_type = "Vector"
            azure_resource_id = "97ce69d9-b4be-4241-8dbd-d265edcf06c4"
        "#,
        )
        .expect_err("Config parsing failed to error with missing shared_key");

        toml::from_str::<AzureMonitorLogsConfig>(
            r#"
            shared_key = "SERsIYhgMVlJB6uPsq49gCxNiruf6v0vhMYE+lfzbSGcXjdViZdV/e5pEMTYtw9f8SkVLf4LFlLCc2KxtRZfCA=="
            log_type = "Vector"
        "#,
        )
        .expect_err("Config parsing failed to error with missing customer_id");
    }
}
