use super::{healthcheck_response, GcpAuthConfig, GcpCredentials, Scope};
use crate::{
    config::{DataType, GenerateConfig, ProxyConfig, SinkConfig, SinkContext, SinkDescription},
    event::Event,
    http::{HttpClient, HttpClientFuture, HttpError},
    internal_events::TemplateRenderingFailed,
    serde::to_string,
    sinks::{
        util::{
            batch::{BatchConfig, BatchSettings},
            encoding::{EncodingConfig, EncodingConfiguration},
            retries::{RetryAction, RetryLogic},
            Buffer, Compression, Concurrency, EncodedEvent, PartitionBatchSink, PartitionBuffer,
            PartitionInnerBuffer, ServiceBuilderExt, TowerRequestConfig,
        },
        Healthcheck, VectorSink,
    },
    template::{Template, TemplateParseError},
    tls::{TlsOptions, TlsSettings},
};
use bytes::Bytes;
use chrono::Utc;
use futures::{stream, FutureExt, SinkExt, StreamExt};
use http::{StatusCode, Uri};
use hyper::{
    header::{HeaderName, HeaderValue},
    Body, Request, Response,
};
use indoc::indoc;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use std::{collections::HashMap, convert::TryFrom, task::Poll};
use tower::{Service, ServiceBuilder};
use uuid::Uuid;

const NAME: &str = "gcp_cloud_storage";
const BASE_URL: &str = "https://storage.googleapis.com/";

#[derive(Clone)]
struct GcsSink {
    bucket: String,
    client: HttpClient,
    creds: Option<GcpCredentials>,
    base_url: String,
    settings: RequestSettings,
}

#[derive(Debug, Snafu)]
enum GcsError {
    #[snafu(display("Bucket {:?} not found", bucket))]
    BucketNotFound { bucket: String },
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct GcsSinkConfig {
    bucket: String,
    acl: Option<GcsPredefinedAcl>,
    storage_class: Option<GcsStorageClass>,
    metadata: Option<HashMap<String, String>>,
    key_prefix: Option<String>,
    filename_time_format: Option<String>,
    filename_append_uuid: Option<bool>,
    filename_extension: Option<String>,
    encoding: EncodingConfig<Encoding>,
    #[serde(default)]
    compression: Compression,
    #[serde(default)]
    batch: BatchConfig,
    #[serde(default)]
    request: TowerRequestConfig,
    #[serde(flatten)]
    auth: GcpAuthConfig,
    tls: Option<TlsOptions>,
    #[serde(
        default,
        skip_serializing_if = "crate::serde::skip_serializing_if_default"
    )]
    proxy: ProxyConfig,
}

#[cfg(test)]
fn default_config(e: Encoding) -> GcsSinkConfig {
    GcsSinkConfig {
        bucket: Default::default(),
        acl: Default::default(),
        storage_class: Default::default(),
        metadata: Default::default(),
        key_prefix: Default::default(),
        filename_time_format: Default::default(),
        filename_append_uuid: Default::default(),
        filename_extension: Default::default(),
        encoding: e.into(),
        compression: Compression::gzip_default(),
        batch: Default::default(),
        request: Default::default(),
        auth: Default::default(),
        tls: Default::default(),
        proxy: Default::default(),
    }
}

#[derive(Clone, Copy, Debug, Derivative, Deserialize, Serialize)]
#[derivative(Default)]
#[serde(rename_all = "kebab-case")]
enum GcsPredefinedAcl {
    AuthenticatedRead,
    BucketOwnerFullControl,
    BucketOwnerRead,
    Private,
    #[derivative(Default)]
    ProjectPrivate,
    PublicRead,
}

#[derive(Clone, Copy, Debug, Derivative, Deserialize, Serialize)]
#[derivative(Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum GcsStorageClass {
    #[derivative(Default)]
    Standard,
    Nearline,
    Coldline,
    Archive,
}

lazy_static! {
    static ref REQUEST_DEFAULTS: TowerRequestConfig = TowerRequestConfig {
        concurrency: Concurrency::Fixed(25),
        rate_limit_num: Some(1000),
        ..Default::default()
    };
}

#[derive(Deserialize, Serialize, Debug, Eq, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Encoding {
    Text,
    Ndjson,
}

impl Encoding {
    fn content_type(self) -> &'static str {
        match self {
            Self::Text => "text/plain",
            Self::Ndjson => "application/x-ndjson",
        }
    }
}

inventory::submit! {
    SinkDescription::new::<GcsSinkConfig>(NAME)
}

impl GenerateConfig for GcsSinkConfig {
    fn generate_config() -> toml::Value {
        toml::from_str(indoc! {r#"
            bucket = "my-bucket"
            credentials_path = "/path/to/credentials.json"
            encoding.codec = "ndjson"
        "#})
        .unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "gcp_cloud_storage")]
impl SinkConfig for GcsSinkConfig {
    async fn build(&self, cx: SinkContext) -> crate::Result<(VectorSink, Healthcheck)> {
        let sink = GcsSink::new(self, &cx).await?;
        let healthcheck = sink.clone().healthcheck().boxed();
        let service = sink.service(self, &cx)?;

        Ok((service, healthcheck))
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn sink_type(&self) -> &'static str {
        NAME
    }
}

#[derive(Debug, Snafu)]
enum HealthcheckError {
    #[snafu(display("Invalid credentials"))]
    InvalidCredentials,
    #[snafu(display("Unknown bucket: {:?}", bucket))]
    UnknownBucket { bucket: String },
    #[snafu(display("key_prefix template parse error: {}", source))]
    KeyPrefixTemplate { source: TemplateParseError },
}

impl GcsSink {
    async fn new(config: &GcsSinkConfig, cx: &SinkContext) -> crate::Result<Self> {
        let creds = config
            .auth
            .make_credentials(Scope::DevStorageReadWrite)
            .await?;
        let settings = RequestSettings::new(config)?;
        let tls = TlsSettings::from_options(&config.tls)?;
        let proxy = cx.globals.proxy.build(&config.proxy);
        let client = HttpClient::new(tls, proxy)?;
        let base_url = format!("{}{}/", BASE_URL, config.bucket);
        let bucket = config.bucket.clone();
        Ok(GcsSink {
            bucket,
            client,
            creds,
            base_url,
            settings,
        })
    }

    fn service(self, config: &GcsSinkConfig, cx: &SinkContext) -> crate::Result<VectorSink> {
        let request = config.request.unwrap_with(&REQUEST_DEFAULTS);
        let encoding = config.encoding.clone();

        let batch = BatchSettings::default()
            .bytes(bytesize::mib(10u64))
            .timeout(300)
            .parse_config(config.batch)?;

        let key_prefix = config.key_prefix.as_deref().unwrap_or("date=%F/");
        let key_prefix = Template::try_from(key_prefix).context(KeyPrefixTemplate)?;

        let settings = self.settings.clone();

        let svc = ServiceBuilder::new()
            .map(move |req| RequestWrapper::new(req, settings.clone()))
            .settings(request, GcsRetryLogic)
            .service(self);

        let buffer = PartitionBuffer::new(Buffer::new(batch.size, config.compression));

        let sink = PartitionBatchSink::new(svc, buffer, batch.timeout, cx.acker())
            .sink_map_err(|error| error!(message = "Fatal gcp_cloud_storage error.", %error))
            .with_flat_map(move |event| {
                stream::iter(encode_event(event, &key_prefix, &encoding)).map(Ok)
            });

        Ok(VectorSink::Sink(Box::new(sink)))
    }

    async fn healthcheck(self) -> crate::Result<()> {
        let uri = self.base_url.parse::<Uri>()?;
        let mut request = http::Request::head(uri).body(Body::empty())?;

        if let Some(creds) = self.creds.as_ref() {
            creds.apply(&mut request);
        }

        let bucket = self.bucket;
        let not_found_error = GcsError::BucketNotFound { bucket }.into();

        let response = self.client.send(request).await?;
        healthcheck_response(self.creds, not_found_error)(response)
    }
}

impl Service<RequestWrapper> for GcsSink {
    type Response = Response<Body>;
    type Error = HttpError;
    type Future = HttpClientFuture;

    fn poll_ready(&mut self, _: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: RequestWrapper) -> Self::Future {
        let settings = request.settings;

        let uri = format!("{}{}", self.base_url, request.key)
            .parse::<Uri>()
            .unwrap();
        let mut builder = Request::put(uri);
        let headers = builder.headers_mut().unwrap();
        headers.insert("content-type", settings.content_type);
        headers.insert(
            "content-length",
            HeaderValue::from_str(&format!("{}", request.body.len())).unwrap(),
        );
        settings
            .content_encoding
            .map(|ce| headers.insert("content-encoding", ce));
        settings.acl.map(|acl| headers.insert("x-goog-acl", acl));
        headers.insert("x-goog-storage-class", settings.storage_class);
        for (p, v) in settings.metadata {
            headers.insert(p, v);
        }

        let mut request = builder.body(Body::from(request.body)).unwrap();
        if let Some(creds) = &self.creds {
            creds.apply(&mut request);
        }

        self.client.call(request)
    }
}

#[derive(Clone, Debug)]
struct RequestWrapper {
    body: Vec<u8>,
    key: String,
    settings: RequestSettings,
}

impl RequestWrapper {
    fn new(req: PartitionInnerBuffer<Vec<u8>, Bytes>, settings: RequestSettings) -> Self {
        let (body, key) = req.into_parts();

        // TODO: pull the seconds from the last event
        let filename = {
            let seconds = Utc::now().format(&settings.time_format);

            if settings.append_uuid {
                let uuid = Uuid::new_v4();
                format!("{}-{}", seconds, uuid.to_hyphenated())
            } else {
                seconds.to_string()
            }
        };

        let key = format!(
            "{}{}.{}",
            String::from_utf8_lossy(&key[..]),
            filename,
            settings.extension
        );

        debug!(message = "Sending events.", bytes = ?body.len(), key = ?key);

        Self {
            body,
            key,
            settings,
        }
    }
}

// Settings required to produce a request that do not change per
// request. All possible values are pre-computed for direct use in
// producing a request.
#[derive(Clone, Debug)]
struct RequestSettings {
    acl: Option<HeaderValue>,
    content_type: HeaderValue,
    content_encoding: Option<HeaderValue>,
    storage_class: HeaderValue,
    metadata: Vec<(HeaderName, HeaderValue)>,
    extension: String,
    time_format: String,
    append_uuid: bool,
}

impl RequestSettings {
    fn new(config: &GcsSinkConfig) -> crate::Result<Self> {
        let acl = config
            .acl
            .map(|acl| HeaderValue::from_str(&to_string(acl)).unwrap());
        let content_type = HeaderValue::from_str(config.encoding.codec().content_type()).unwrap();
        let content_encoding = config
            .compression
            .content_encoding()
            .map(|ce| HeaderValue::from_str(&to_string(ce)).unwrap());
        let storage_class = config.storage_class.unwrap_or_default();
        let storage_class = HeaderValue::from_str(&to_string(storage_class)).unwrap();
        let metadata = config
            .metadata
            .as_ref()
            .map(|metadata| {
                metadata
                    .iter()
                    .map(make_header)
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap_or_else(|| Ok(vec![]))?;
        let extension = config
            .filename_extension
            .clone()
            .unwrap_or_else(|| config.compression.extension().into());
        let time_format = config
            .filename_time_format
            .clone()
            .unwrap_or_else(|| "%s".into());
        let append_uuid = config.filename_append_uuid.unwrap_or(true);
        Ok(Self {
            acl,
            content_type,
            content_encoding,
            storage_class,
            metadata,
            extension,
            time_format,
            append_uuid,
        })
    }
}

// Make a header pair from a key-value string pair
fn make_header((name, value): (&String, &String)) -> crate::Result<(HeaderName, HeaderValue)> {
    Ok((
        HeaderName::from_bytes(name.as_bytes())?,
        HeaderValue::from_str(value)?,
    ))
}

fn encode_event(
    mut event: Event,
    key_prefix: &Template,
    encoding: &EncodingConfig<Encoding>,
) -> Option<EncodedEvent<PartitionInnerBuffer<Vec<u8>, Bytes>>> {
    let key = key_prefix
        .render_string(&event)
        .map_err(|error| {
            emit!(TemplateRenderingFailed {
                error,
                field: Some("key_prefix"),
                drop_event: true,
            });
        })
        .ok()?;
    encoding.apply_rules(&mut event);
    let log = event.into_log();
    let bytes = match encoding.codec() {
        Encoding::Ndjson => serde_json::to_vec(&log)
            .map(|mut b| {
                b.push(b'\n');
                b
            })
            .expect("Failed to encode event as json, this is a bug!"),
        Encoding::Text => {
            let mut bytes = log
                .get(crate::config::log_schema().message_key())
                .map(|v| v.as_bytes().to_vec())
                .unwrap_or_default();
            bytes.push(b'\n');
            bytes
        }
    };

    Some(EncodedEvent::new(PartitionInnerBuffer::new(
        bytes,
        key.into(),
    )))
}

#[derive(Clone)]
struct GcsRetryLogic;

// This is a clone of HttpRetryLogic for the Body type, should get merged
impl RetryLogic for GcsRetryLogic {
    type Error = hyper::Error;
    type Response = Response<Body>;

    fn is_retriable_error(&self, _error: &Self::Error) -> bool {
        true
    }

    fn should_retry_response(&self, response: &Self::Response) -> RetryAction {
        let status = response.status();

        match status {
            StatusCode::TOO_MANY_REQUESTS => RetryAction::Retry("too many requests".into()),
            StatusCode::NOT_IMPLEMENTED => {
                RetryAction::DontRetry("endpoint not implemented".into())
            }
            _ if status.is_server_error() => RetryAction::Retry(format!("{}", status)),
            _ if status.is_success() => RetryAction::Successful,
            _ => RetryAction::DontRetry(format!("response status: {}", status)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<GcsSinkConfig>();
    }

    #[test]
    fn gcs_encode_event_text() {
        let message = "hello world".to_string();
        let batch_time_format = Template::try_from("date=%F").unwrap();
        let encoded = encode_event(
            message.clone().into(),
            &batch_time_format,
            &Encoding::Text.into(),
        )
        .unwrap();

        let encoded_message = message + "\n";
        let (bytes, _) = encoded.item.into_parts();
        assert_eq!(&bytes[..], encoded_message.as_bytes());
    }

    #[test]
    fn gcs_encode_event_ndjson() {
        let message = "hello world".to_string();
        let mut event = Event::from(message.clone());
        event.as_mut_log().insert("key", "value");

        let batch_time_format = Template::try_from("date=%F").unwrap();
        let encoded = encode_event(event, &batch_time_format, &Encoding::Ndjson.into()).unwrap();

        let (bytes, _) = encoded.item.into_parts();
        let map: HashMap<String, String> = serde_json::from_slice(&bytes[..]).unwrap();

        assert_eq!(
            map.get(&crate::config::log_schema().message_key().to_string()),
            Some(&message)
        );
        assert_eq!(map["key"], "value".to_string());
    }

    #[test]
    fn gcs_encode_event_apply_rules() {
        crate::test_util::trace_init();

        let message = "hello world".to_string();
        let mut event = Event::from(message);
        event.as_mut_log().insert("key", "value");

        let key_format = Template::try_from("key: {{ key }}").unwrap();
        let encoded = encode_event(event, &key_format, &Encoding::Text.into()).unwrap();

        let (_, key) = encoded.item.into_parts();
        assert_eq!(key, "key: value");
    }

    fn request_settings(
        extension: Option<&str>,
        uuid: bool,
        compression: Compression,
    ) -> RequestSettings {
        RequestSettings::new(&GcsSinkConfig {
            key_prefix: Some("key/".into()),
            filename_time_format: Some("date".into()),
            filename_extension: extension.map(Into::into),
            filename_append_uuid: Some(uuid),
            compression,
            ..default_config(Encoding::Ndjson)
        })
        .expect("Could not create request settings")
    }

    #[test]
    fn gcs_build_request() {
        let buf = PartitionInnerBuffer::new(vec![0u8; 10], Bytes::from("key/"));

        let req = RequestWrapper::new(
            buf.clone(),
            request_settings(Some("ext"), false, Compression::None),
        );
        assert_eq!(req.key, "key/date.ext".to_string());

        let req = RequestWrapper::new(
            buf.clone(),
            request_settings(None, false, Compression::None),
        );
        assert_eq!(req.key, "key/date.log".to_string());

        let req = RequestWrapper::new(
            buf.clone(),
            request_settings(None, false, Compression::gzip_default()),
        );
        assert_eq!(req.key, "key/date.log.gz".to_string());

        let req = RequestWrapper::new(
            buf,
            request_settings(None, true, Compression::gzip_default()),
        );
        assert_ne!(req.key, "key/date.log.gz".to_string());
    }
}
