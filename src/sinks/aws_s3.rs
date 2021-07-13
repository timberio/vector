use crate::{
    config::{log_schema, DataType, GenerateConfig, SinkConfig, SinkContext, SinkDescription},
    event::Event,
    internal_events::TemplateRenderingFailed,
    rusoto::{self, AwsAuthentication, RegionOrEndpoint},
    serde::to_string,
    sinks::util::{
        batch::{BatchConfig, BatchSettings},
        encoding::{EncodingConfig, EncodingConfiguration},
        retries::RetryLogic,
        sink::Response,
        Buffer, Compression, Concurrency, EncodedEvent, PartitionBatchSink, PartitionBuffer,
        PartitionInnerBuffer, ServiceBuilderExt, TowerRequestConfig,
    },
    template::Template,
};
use bytes::Bytes;
use chrono::Utc;
use futures::{future::BoxFuture, stream, FutureExt, SinkExt, StreamExt};
use http::StatusCode;
use lazy_static::lazy_static;
use md5::Digest;
use rusoto_core::RusotoError;
use rusoto_s3::{
    PutObjectError, PutObjectOutput, PutObjectRequest, S3Client, S3,
};
use serde::{Deserialize, Serialize};
use snafu::Snafu;
use std::{
    collections::BTreeMap,
    convert::{TryFrom, TryInto},
    task::{Context, Poll},
};
use tower::{Service, ServiceBuilder};
use tracing_futures::Instrument;
use uuid::Uuid;

#[derive(Clone)]
pub struct S3Sink {
    client: S3Client,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct S3SinkConfig {
    pub bucket: String,
    pub key_prefix: Option<String>,
    pub filename_time_format: Option<String>,
    pub filename_append_uuid: Option<bool>,
    pub filename_extension: Option<String>,
    #[serde(flatten)]
    options: S3Options,
    #[serde(flatten)]
    pub region: RegionOrEndpoint,
    pub encoding: EncodingConfig<Encoding>,
    #[serde(default = "Compression::gzip_default")]
    pub compression: Compression,
    #[serde(default)]
    pub batch: BatchConfig,
    #[serde(default)]
    pub request: TowerRequestConfig,
    // Deprecated name. Moved to auth.
    assume_role: Option<String>,
    #[serde(default)]
    pub auth: AwsAuthentication,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct S3Options {
    acl: Option<S3CannedAcl>,
    grant_full_control: Option<String>,
    grant_read: Option<String>,
    grant_read_acp: Option<String>,
    grant_write_acp: Option<String>,
    server_side_encryption: Option<S3ServerSideEncryption>,
    ssekms_key_id: Option<String>,
    storage_class: Option<S3StorageClass>,
    tags: Option<BTreeMap<String, String>>,
    content_encoding: Option<String>, // inherit from compression value
    content_type: Option<String>,     // default `text/x-log`
}

#[derive(Clone, Copy, Debug, Derivative, Deserialize, Serialize)]
#[derivative(Default)]
#[serde(rename_all = "kebab-case")]
enum S3CannedAcl {
    #[derivative(Default)]
    Private,
    PublicRead,
    PublicReadWrite,
    AwsExecRead,
    AuthenticatedRead,
    BucketOwnerRead,
    BucketOwnerFullControl,
    LogDeliveryWrite,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
enum S3ServerSideEncryption {
    #[serde(rename = "AES256")]
    Aes256,
    #[serde(rename = "aws:kms")]
    AwsKms,
}

#[derive(Clone, Copy, Debug, Derivative, Deserialize, PartialEq, Serialize)]
#[derivative(Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum S3StorageClass {
    #[derivative(Default)]
    Standard,
    ReducedRedundancy,
    IntelligentTiering,
    StandardIa,
    OnezoneIa,
    Glacier,
    DeepArchive,
}

lazy_static! {
    static ref REQUEST_DEFAULTS: TowerRequestConfig = TowerRequestConfig {
        concurrency: Concurrency::Fixed(50),
        rate_limit_num: Some(250),
        ..Default::default()
    };
}

#[derive(Deserialize, Serialize, Debug, Eq, PartialEq, Clone, Derivative)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    Text,
    Ndjson,
}

inventory::submit! {
    SinkDescription::new::<S3SinkConfig>("aws_s3")
}

impl GenerateConfig for S3SinkConfig {
    fn generate_config() -> toml::Value {
        toml::Value::try_from(Self {
            bucket: "".to_owned(),
            key_prefix: None,
            filename_time_format: None,
            filename_append_uuid: None,
            filename_extension: None,
            options: S3Options::default(),
            region: RegionOrEndpoint::default(),
            encoding: Encoding::Text.into(),
            compression: Compression::gzip_default(),
            batch: BatchConfig::default(),
            request: TowerRequestConfig::default(),
            assume_role: None,
            auth: AwsAuthentication::default(),
        })
        .unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "aws_s3")]
impl SinkConfig for S3SinkConfig {
    async fn build(
        &self,
        cx: SinkContext,
    ) -> crate::Result<(super::VectorSink, super::Healthcheck)> {
        let client = self.create_client()?;
        let healthcheck = self.clone().healthcheck(client.clone()).boxed();
        let sink = self.new(client, cx)?;
        Ok((sink, healthcheck))
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn sink_type(&self) -> &'static str {
        "aws_s3"
    }
}

#[derive(Debug, Snafu)]
enum HealthcheckError {
    #[snafu(display("Invalid credentials"))]
    InvalidCredentials,
    #[snafu(display("Unknown bucket: {:?}", bucket))]
    UnknownBucket { bucket: String },
    #[snafu(display("Unknown status code: {}", status))]
    UnknownStatus { status: StatusCode },
}

impl S3SinkConfig {
    pub fn new(&self, client: S3Client, cx: SinkContext) -> crate::Result<super::VectorSink> {
        let request = self.request.unwrap_with(&REQUEST_DEFAULTS);
        let encoding = self.encoding.clone();

        let compression = self.compression;
        let filename_time_format = self
            .filename_time_format
            .clone()
            .unwrap_or_else(|| "%s".into());
        let filename_append_uuid = self.filename_append_uuid.unwrap_or(true);
        let batch = BatchSettings::default()
            .bytes(10_000_000)
            .timeout(300)
            .parse_config(self.batch)?;

        let key_prefix = self.key_prefix.as_deref().unwrap_or("date=%F/");
        let key_prefix = Template::try_from(key_prefix)?;

        let s3 = S3Sink { client };

        let filename_extension = self.filename_extension.clone();
        let bucket = self.bucket.clone();
        let options = self.options.clone();

        let svc = ServiceBuilder::new()
            .map(move |req| {
                build_request(
                    req,
                    filename_time_format.clone(),
                    filename_extension.clone(),
                    filename_append_uuid,
                    compression,
                    bucket.clone(),
                    options.clone(),
                )
            })
            .settings(request, S3RetryLogic)
            .service(s3);

        let buffer = PartitionBuffer::new(Buffer::new(batch.size, self.compression));

        let sink = PartitionBatchSink::new(svc, buffer, batch.timeout, cx.acker())
            .with_flat_map(move |e| stream::iter(encode_event(e, &key_prefix, &encoding)).map(Ok))
            .sink_map_err(|error| error!(message = "Sink failed to flush.", %error));

        Ok(super::VectorSink::Sink(Box::new(sink)))
    }

    pub async fn healthcheck(self, client: S3Client) -> crate::Result<()> {
        let req = client.put_object(PutObjectRequest{
            bucket: self.bucket.clone(),
            key: ".healthcheck".to_string(),
            body: None,
            ..Default::default()
        });

        match req.await {
            Ok(_) => Ok(()),
            Err(error) => Err(match error {
                RusotoError::Unknown(resp) => match resp.status {
                    StatusCode::FORBIDDEN => HealthcheckError::InvalidCredentials.into(),
                    StatusCode::NOT_FOUND => HealthcheckError::UnknownBucket {
                        bucket: self.bucket,
                    }
                    .into(),
                    status => HealthcheckError::UnknownStatus { status }.into(),
                },
                error => error.into(),
            }),
        }
    }

    pub fn create_client(&self) -> crate::Result<S3Client> {
        let region = (&self.region).try_into()?;
        let client = rusoto::client()?;

        let creds = self.auth.build(&region, self.assume_role.clone())?;

        Ok(S3Client::new_with(client, creds, region))
    }
}

impl Service<Request> for S3Sink {
    type Response = PutObjectOutput;
    type Error = RusotoError<PutObjectError>;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let options = request.options;

        let content_encoding = request.content_encoding;
        let content_encoding = options
            .content_encoding
            .or_else(|| content_encoding.map(|ce| ce.to_string()));
        let content_type = options
            .content_type
            .or_else(|| Some("text/x-log".to_owned()));

        let content_md5 = base64::encode(md5::Md5::digest(&request.body));

        let mut tagging = url::form_urlencoded::Serializer::new(String::new());
        if let Some(tags) = options.tags {
            for (p, v) in tags {
                tagging.append_pair(&p, &v);
            }
        }
        let tagging = tagging.finish();

        let client = self.client.clone();
        let request = PutObjectRequest {
            body: Some(request.body.into()),
            bucket: request.bucket,
            key: request.key,
            content_encoding,
            content_type,
            acl: options.acl.map(to_string),
            grant_full_control: options.grant_full_control,
            grant_read: options.grant_read,
            grant_read_acp: options.grant_read_acp,
            grant_write_acp: options.grant_write_acp,
            server_side_encryption: options.server_side_encryption.map(to_string),
            ssekms_key_id: options.ssekms_key_id,
            storage_class: options.storage_class.map(to_string),
            tagging: Some(tagging),
            content_md5: Some(content_md5),
            ..Default::default()
        };

        Box::pin(async move {
            client
                .put_object(request)
                .instrument(info_span!("request"))
                .await
        })
    }
}

fn build_request(
    req: PartitionInnerBuffer<Vec<u8>, Bytes>,
    time_format: String,
    extension: Option<String>,
    uuid: bool,
    compression: Compression,
    bucket: String,
    options: S3Options,
) -> Request {
    let (inner, key) = req.into_parts();

    // TODO: pull the seconds from the last event
    let filename = {
        let seconds = Utc::now().format(&time_format);

        if uuid {
            let uuid = Uuid::new_v4();
            format!("{}-{}", seconds, uuid.to_hyphenated())
        } else {
            seconds.to_string()
        }
    };

    let extension = extension.unwrap_or_else(|| compression.extension().into());
    let key = String::from_utf8_lossy(&key[..]).into_owned();
    let key = format!("{}{}.{}", key, filename, extension);

    debug!(
        message = "Sending events.",
        bytes = ?inner.len(),
        bucket = ?bucket,
        key = ?key
    );

    Request {
        body: inner,
        bucket,
        key,
        content_encoding: compression.content_encoding(),
        options,
    }
}

#[derive(Debug, Clone)]
struct Request {
    body: Vec<u8>,
    bucket: String,
    key: String,
    content_encoding: Option<&'static str>,
    options: S3Options,
}

impl Response for PutObjectOutput {}

#[derive(Debug, Clone)]
struct S3RetryLogic;

impl RetryLogic for S3RetryLogic {
    type Error = RusotoError<PutObjectError>;
    type Response = PutObjectOutput;

    fn is_retriable_error(&self, error: &Self::Error) -> bool {
        rusoto::is_retriable_error(error)
    }
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
                .get(log_schema().message_key())
                .map(|v| v.as_bytes().to_vec())
                .unwrap_or_default();
            bytes.push(b'\n');
            bytes
        }
    };

    let (_fields, metadata) = log.into_parts();
    Some(EncodedEvent {
        item: PartitionInnerBuffer::new(bytes, key.into()),
        metadata: Some(metadata),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<S3SinkConfig>();
    }

    #[test]
    fn s3_encode_event_text() {
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
    fn s3_encode_event_ndjson() {
        let message = "hello world".to_string();
        let mut event = Event::from(message.clone());
        event.as_mut_log().insert("key", "value");

        let batch_time_format = Template::try_from("date=%F").unwrap();
        let encoded = encode_event(event, &batch_time_format, &Encoding::Ndjson.into()).unwrap();

        let (bytes, _) = encoded.item.into_parts();
        let map: BTreeMap<String, String> = serde_json::from_slice(&bytes[..]).unwrap();

        assert_eq!(map[&log_schema().message_key().to_string()], message);
        assert_eq!(map["key"], "value".to_string());
    }

    #[test]
    fn s3_encode_event_with_removed_key() {
        let message = "hello world".to_string();
        let mut event = Event::from(message.clone());
        event.as_mut_log().insert("key", "value");

        let key_prefix = Template::try_from("{{ key }}").unwrap();

        let encoding_config = EncodingConfig {
            codec: Encoding::Ndjson,
            schema: None,
            only_fields: None,
            except_fields: Some(vec!["key".into()]),
            timestamp_format: None,
        };

        let encoded = encode_event(event, &key_prefix, &encoding_config).unwrap();

        let (bytes, _) = encoded.item.into_parts();
        let map: BTreeMap<String, String> = serde_json::from_slice(&bytes[..]).unwrap();

        assert_eq!(map[&log_schema().message_key().to_string()], message);
        // assert_eq!(map["key"], "value".to_string());
    }

    #[test]
    fn s3_build_request() {
        let buf = PartitionInnerBuffer::new(vec![0u8; 10], Bytes::from("key/"));

        let req = build_request(
            buf.clone(),
            "date".into(),
            Some("ext".into()),
            false,
            Compression::None,
            "bucket".into(),
            S3Options::default(),
        );
        assert_eq!(req.key, "key/date.ext".to_string());

        let req = build_request(
            buf.clone(),
            "date".into(),
            None,
            false,
            Compression::None,
            "bucket".into(),
            S3Options::default(),
        );
        assert_eq!(req.key, "key/date.log".to_string());

        let req = build_request(
            buf.clone(),
            "date".into(),
            None,
            false,
            Compression::gzip_default(),
            "bucket".into(),
            S3Options::default(),
        );
        assert_eq!(req.key, "key/date.log.gz".to_string());

        let req = build_request(
            buf,
            "date".into(),
            None,
            true,
            Compression::gzip_default(),
            "bucket".into(),
            S3Options::default(),
        );
        assert_ne!(req.key, "key/date.log.gz".to_string());
    }

    #[test]
    fn storage_class_names() {
        for &(name, storage_class) in &[
            ("DEEP_ARCHIVE", S3StorageClass::DeepArchive),
            ("GLACIER", S3StorageClass::Glacier),
            ("INTELLIGENT_TIERING", S3StorageClass::IntelligentTiering),
            ("ONEZONE_IA", S3StorageClass::OnezoneIa),
            ("REDUCED_REDUNDANCY", S3StorageClass::ReducedRedundancy),
            ("STANDARD", S3StorageClass::Standard),
            ("STANDARD_IA", S3StorageClass::StandardIa),
        ] {
            assert_eq!(name, to_string(storage_class));
            let result: S3StorageClass = serde_json::from_str(&format!("{:?}", name))
                .unwrap_or_else(|error| {
                    panic!("Unparsable storage class name {:?}: {}", name, error)
                });
            assert_eq!(result, storage_class);
        }
    }
}

#[cfg(feature = "aws-s3-integration-tests")]
#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::{
        assert_downcast_matches,
        test_util::{random_lines_with_stream, random_string},
    };
    use bytes::{Buf, BytesMut};
    use flate2::read::MultiGzDecoder;
    use futures::Stream;
    use pretty_assertions::assert_eq;
    use rusoto_core::region::Region;
    use std::io::{BufRead, BufReader};
    use vector_core::event::{BatchNotifier, BatchStatus, BatchStatusReceiver, LogEvent};

    #[tokio::test]
    async fn s3_insert_message_into() {
        let cx = SinkContext::new_test();

        let bucket = uuid::Uuid::new_v4().to_string();

        create_bucket(&bucket, false).await;

        let config = config(&bucket, 1000000);
        let prefix = config.key_prefix.clone();
        let client = config.create_client().unwrap();
        let sink = config.new(client, cx).unwrap();

        let (lines, events, mut receiver) = make_events_batch(100, 10);
        sink.run(events).await.unwrap();
        assert_eq!(receiver.try_recv(), Ok(BatchStatus::Delivered));

        let keys = get_keys(&bucket, prefix.unwrap()).await;
        assert_eq!(keys.len(), 1);

        let key = keys[0].clone();
        assert!(key.ends_with(".log"));

        let obj = get_object(&bucket, key).await;
        assert_eq!(obj.content_encoding, Some("identity".to_string()));

        let response_lines = get_lines(obj).await;
        assert_eq!(lines, response_lines);
    }

    #[tokio::test]
    async fn s3_rotate_files_after_the_buffer_size_is_reached() {
        let cx = SinkContext::new_test();

        let bucket = uuid::Uuid::new_v4().to_string();

        create_bucket(&bucket, false).await;

        let config = S3SinkConfig {
            key_prefix: Some(format!("{}/{}", random_string(10), "{{i}}")),
            filename_time_format: Some("waitsforfullbatch".into()),
            filename_append_uuid: Some(false),
            ..config(&bucket, 1010)
        };
        let prefix = config.key_prefix.clone();
        let client = config.create_client().unwrap();
        let sink = config.new(client, cx).unwrap();

        let (lines, _events) = random_lines_with_stream(100, 30, None);

        let events = lines.clone().into_iter().enumerate().map(|(i, line)| {
            let mut e = LogEvent::from(line);
            let i = if i < 10 {
                1
            } else if i < 20 {
                2
            } else {
                3
            };
            e.insert("i", format!("{}", i));
            Event::from(e)
        });

        sink.run(stream::iter(events)).await.unwrap();

        let keys = get_keys(&bucket, prefix.unwrap()).await;
        assert_eq!(keys.len(), 3);

        let response_lines = stream::iter(keys)
            .fold(Vec::new(), |mut acc, key| async {
                acc.push(get_lines(get_object(&bucket, key).await).await);
                acc
            })
            .await;

        assert_eq!(&lines[00..10], response_lines[0].as_slice());
        assert_eq!(&lines[10..20], response_lines[1].as_slice());
        assert_eq!(&lines[20..30], response_lines[2].as_slice());
    }

    #[tokio::test]
    async fn s3_gzip() {
        let cx = SinkContext::new_test();

        let bucket = uuid::Uuid::new_v4().to_string();

        create_bucket(&bucket, false).await;

        let config = S3SinkConfig {
            compression: Compression::gzip_default(),
            filename_time_format: Some("%s%f".into()),
            ..config(&bucket, 10000)
        };

        let prefix = config.key_prefix.clone();
        let client = config.create_client().unwrap();
        let sink = config.new(client, cx).unwrap();

        let (lines, events, mut receiver) = make_events_batch(100, 500);
        sink.run(events).await.unwrap();
        assert_eq!(receiver.try_recv(), Ok(BatchStatus::Delivered));

        let keys = get_keys(&bucket, prefix.unwrap()).await;
        assert_eq!(keys.len(), 6);

        let response_lines = stream::iter(keys).fold(Vec::new(), |mut acc, key| async {
            assert!(key.ends_with(".log.gz"));

            let obj = get_object(&bucket, key).await;
            assert_eq!(obj.content_encoding, Some("gzip".to_string()));

            acc.append(&mut get_gzipped_lines(obj).await);
            acc
        });

        assert_eq!(lines, response_lines.await);
    }

    // NOTE: this test doesn't actually validate anything because localstack doesn't enforce the
    // required Content-MD5 header on the request for buckets with object lock enabled
    // https://github.com/localstack/localstack/issues/4166
    #[tokio::test]
    async fn s3_insert_message_into_object_lock() {
        let cx = SinkContext::new_test();

        let bucket = uuid::Uuid::new_v4().to_string();

        create_bucket(&bucket, true).await;

        client()
            .put_object_lock_configuration(rusoto_s3::PutObjectLockConfigurationRequest {
                bucket: bucket.to_string(),
                object_lock_configuration: Some(rusoto_s3::ObjectLockConfiguration {
                    object_lock_enabled: Some(String::from("Enabled")),
                    rule: Some(rusoto_s3::ObjectLockRule {
                        default_retention: Some(rusoto_s3::DefaultRetention {
                            days: Some(1),
                            mode: Some(String::from("GOVERNANCE")),
                            years: None,
                        }),
                    }),
                }),
                ..Default::default()
            })
            .await
            .unwrap();

        let config = config(&bucket, 1000000);
        let prefix = config.key_prefix.clone();
        let client = config.create_client().unwrap();
        let sink = config.new(client, cx).unwrap();

        let (lines, events, mut receiver) = make_events_batch(100, 10);
        sink.run(events).await.unwrap();
        assert_eq!(receiver.try_recv(), Ok(BatchStatus::Delivered));

        let keys = get_keys(&bucket, prefix.unwrap()).await;
        assert_eq!(keys.len(), 1);

        let key = keys[0].clone();
        assert!(key.ends_with(".log"));

        let obj = get_object(&bucket, key).await;
        assert_eq!(obj.content_encoding, Some("identity".to_string()));

        let response_lines = get_lines(obj).await;
        assert_eq!(lines, response_lines);
    }

    #[tokio::test]
    async fn acknowledges_failures() {
        let cx = SinkContext::new_test();

        let bucket = uuid::Uuid::new_v4().to_string();

        create_bucket(&bucket, false).await;

        let mut config = config(&bucket, 1);
        // Break the bucket name
        config.bucket = format!("BREAK{}IT", config.bucket);
        let prefix = config.key_prefix.clone();
        let client = config.create_client().unwrap();
        let sink = config.new(client, cx).unwrap();

        let (_lines, events, mut receiver) = make_events_batch(1, 1);
        sink.run(events).await.unwrap();
        assert_eq!(receiver.try_recv(), Ok(BatchStatus::Errored));

        let objects = list_objects(&bucket, prefix.unwrap()).await;
        assert_eq!(objects, None);
    }

    #[tokio::test]
    async fn s3_healthchecks() {
        let bucket = uuid::Uuid::new_v4().to_string();

        create_bucket(&bucket, false).await;

        let config = config(&bucket, 1);
        let client = config.create_client().unwrap();
        config.healthcheck(client).await.unwrap();
    }

    #[tokio::test]
    async fn s3_healthchecks_invalid_bucket() {
        let config = config("s3_healthchecks_invalid_bucket", 1);

        let client = config.create_client().unwrap();
        assert_downcast_matches!(
            config.healthcheck(client).await.unwrap_err(),
            HealthcheckError,
            HealthcheckError::UnknownBucket { .. }
        );
    }

    fn client() -> S3Client {
        let region = Region::Custom {
            name: "minio".to_owned(),
            endpoint: "http://localhost:4566".to_owned(),
        };

        use rusoto_core::HttpClient;
        use rusoto_credential::StaticProvider;

        let p = StaticProvider::new_minimal("test-access-key".into(), "test-secret-key".into());
        let d = HttpClient::new().unwrap();

        S3Client::new_with(d, p, region)
    }

    fn config(bucket: &str, batch_size: usize) -> S3SinkConfig {
        S3SinkConfig {
            bucket: bucket.to_string(),
            key_prefix: Some(random_string(10) + "/date=%F/"),
            filename_time_format: None,
            filename_append_uuid: None,
            filename_extension: None,
            options: S3Options::default(),
            region: RegionOrEndpoint::with_endpoint("http://localhost:4566".to_owned()),
            encoding: Encoding::Text.into(),
            compression: Compression::None,
            batch: BatchConfig {
                max_bytes: Some(batch_size),
                timeout_secs: Some(5),
                ..Default::default()
            },
            request: TowerRequestConfig::default(),
            assume_role: None,
            auth: Default::default(),
        }
    }

    fn make_events_batch(
        len: usize,
        count: usize,
    ) -> (Vec<String>, impl Stream<Item = Event>, BatchStatusReceiver) {
        let (lines, events) = random_lines_with_stream(len, count, None);

        let (batch, receiver) = BatchNotifier::new_with_receiver();
        let events = events.map(move |event| event.into_log().with_batch_notifier(&batch).into());

        (lines, events, receiver)
    }

    async fn create_bucket(bucket: &str, object_lock_enabled: bool) {
        use rusoto_s3::{CreateBucketError, CreateBucketRequest};

        let req = CreateBucketRequest {
            bucket: bucket.to_string(),
            object_lock_enabled_for_bucket: Some(object_lock_enabled),
            ..Default::default()
        };

        match client().create_bucket(req).await {
            Ok(_) | Err(RusotoError::Service(CreateBucketError::BucketAlreadyOwnedByYou(_))) => {}
            Err(e) => match e {
                RusotoError::Unknown(resp) => {
                    let body = String::from_utf8_lossy(&resp.body[..]);
                    panic!("Couldn't create bucket: {:?}; Body {}", resp, body);
                }
                _ => panic!("Couldn't create bucket: {}", e),
            },
        }
    }

    async fn list_objects(bucket: &str, prefix: String) -> Option<Vec<rusoto_s3::Object>> {
        let prefix = prefix.split('/').next().unwrap().to_string();

        client()
            .list_objects_v2(rusoto_s3::ListObjectsV2Request {
                bucket: bucket.to_string(),
                prefix: Some(prefix),
                ..Default::default()
            })
            .await
            .unwrap()
            .contents
    }

    async fn get_keys(bucket: &str, prefix: String) -> Vec<String> {
        list_objects(bucket, prefix)
            .await
            .unwrap()
            .into_iter()
            .map(|obj| obj.key.unwrap())
            .collect()
    }

    async fn get_object(bucket: &str, key: String) -> rusoto_s3::GetObjectOutput {
        client()
            .get_object(rusoto_s3::GetObjectRequest {
                bucket: bucket.to_string(),
                key,
                ..Default::default()
            })
            .await
            .unwrap()
    }

    async fn get_lines(obj: rusoto_s3::GetObjectOutput) -> Vec<String> {
        let body = get_object_output_body(obj).await;
        let buf_read = BufReader::new(body);
        buf_read.lines().map(|l| l.unwrap()).collect()
    }

    async fn get_gzipped_lines(obj: rusoto_s3::GetObjectOutput) -> Vec<String> {
        let body = get_object_output_body(obj).await;
        let buf_read = BufReader::new(MultiGzDecoder::new(body));
        buf_read.lines().map(|l| l.unwrap()).collect()
    }

    async fn get_object_output_body(obj: rusoto_s3::GetObjectOutput) -> impl std::io::Read {
        let bytes = obj
            .body
            .unwrap()
            .fold(BytesMut::new(), |mut store, bytes| async move {
                store.extend_from_slice(&bytes.unwrap());
                store
            })
            .await;
        bytes.freeze().reader()
    }
}
