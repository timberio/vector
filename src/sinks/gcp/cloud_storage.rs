use super::{healthcheck_response, GcpAuthConfig, GcpCredentials, Scope};
use crate::{
    dns::Resolver,
    event::{self, Event},
    region::RegionOrEndpoint,
    sinks::{
        util::{
            http::https_client, retries::RetryLogic, tls::TlsSettings, BatchBytesConfig, Buffer,
            PartitionBuffer, PartitionInnerBuffer, ServiceBuilderExt, SinkExt, TowerRequestConfig,
        },
        Healthcheck, RouterSink,
    },
    template::Template,
    topology::config::{DataType, SinkConfig, SinkContext, SinkDescription},
};
use bytes::Bytes;
use chrono::Utc;
use futures::{stream::iter_ok, Future, Poll, Sink};
use http::{Method, Uri};
use hyper::{Body, Request};
use lazy_static::lazy_static;
use rusoto_core::{Region, RusotoError, RusotoFuture};
use rusoto_s3::{PutObjectError, PutObjectOutput, PutObjectRequest, S3Client, S3};
use serde::{Deserialize, Serialize};
use snafu::Snafu;
use std::collections::HashMap;
use std::convert::TryInto;
use tower::{Service, ServiceBuilder};
use tracing::field;
use tracing_futures::{Instrument, Instrumented};
use uuid::Uuid;

const NAME: &str = "gcp_cloud_storage";
const BASE_URL: &str = "https://storage.googleapis.com/";

#[derive(Clone)]
pub struct GcsSink {
    client: S3Client,
}

#[derive(Debug, Snafu)]
enum GcsHealthcheckError {
    #[snafu(display("Bucket {:?} not found", bucket))]
    BucketNotFound { bucket: String },
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct GcsSinkConfig {
    pub bucket: String,
    pub key_prefix: Option<String>,
    pub filename_time_format: Option<String>,
    pub filename_append_uuid: Option<bool>,
    pub filename_extension: Option<String>,
    #[serde(flatten)]
    options: GcsOptions,
    #[serde(flatten)]
    pub region: RegionOrEndpoint,
    pub encoding: Encoding,
    pub compression: Compression,
    #[serde(default)]
    pub batch: BatchBytesConfig,
    #[serde(default)]
    pub request: TowerRequestConfig,
    #[serde(flatten)]
    auth: GcpAuthConfig,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct GcsOptions {
    acl: Option<GcsPredefinedAcl>,
    grant_full_control: Option<String>,
    grant_read: Option<String>,
    grant_read_acp: Option<String>,
    grant_write_acp: Option<String>,
    server_side_encryption: Option<GcsServerSideEncryption>,
    ssekms_key_id: Option<String>,
    storage_class: Option<GcsStorageClass>,
    tags: Option<HashMap<String, String>>,
}

#[derive(Clone, Copy, Debug, Derivative, Deserialize, Serialize)]
#[derivative(Default)]
#[serde(rename_all = "kebab-case")]
enum GcsPredefinedAcl {
    AuthenticatedRead,
    BucketOwnerFullControl,
    BucketOwnerRead,
    #[derivative(Default)]
    Private,
    PublicRead,
    ProjectPrivate,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
enum GcsServerSideEncryption {
    #[serde(rename = "AES256")]
    AES256,
    #[serde(rename = "aws:kms")]
    AwsKms,
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
        in_flight_limit: Some(25),
        rate_limit_num: Some(25),
        ..Default::default()
    };
}

#[derive(Deserialize, Serialize, Debug, Eq, PartialEq, Clone, Derivative)]
#[serde(rename_all = "snake_case")]
#[derivative(Default)]
pub enum Encoding {
    #[derivative(Default)]
    Text,
    Ndjson,
}

#[derive(Deserialize, Serialize, Debug, Clone, Derivative)]
#[serde(rename_all = "snake_case")]
#[derivative(Default)]
pub enum Compression {
    #[derivative(Default)]
    Gzip,
    None,
}

inventory::submit! {
    SinkDescription::new::<GcsSinkConfig>(NAME)
}

#[typetag::serde(name = "gcp_cloud_storage")]
impl SinkConfig for GcsSinkConfig {
    fn build(&self, cx: SinkContext) -> crate::Result<(RouterSink, Healthcheck)> {
        let creds = self.auth.make_credentials(Scope::DevStorageReadWrite)?;
        let healthcheck = GcsSink::healthcheck(self, &cx, &creds)?;
        let sink = GcsSink::new(self, cx, &creds)?;

        Ok((sink, healthcheck))
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
    #[snafu(display("Unknown status code: {}", status))]
    UnknownStatus { status: http::StatusCode },
}

impl GcsSink {
    pub fn new(
        config: &GcsSinkConfig,
        cx: SinkContext,
        _creds: &Option<GcpCredentials>,
    ) -> crate::Result<RouterSink> {
        let request = config.request.unwrap_with(&REQUEST_DEFAULTS);
        let encoding = config.encoding.clone();

        let compression = match config.compression {
            Compression::Gzip => true,
            Compression::None => false,
        };
        let filename_time_format = config.filename_time_format.clone().unwrap_or("%s".into());
        let filename_append_uuid = config.filename_append_uuid.unwrap_or(true);
        let batch = config.batch.unwrap_or(bytesize::mib(10u64), 300);

        let key_prefix = if let Some(kp) = &config.key_prefix {
            Template::from(kp.as_str())
        } else {
            Template::from("date=%F/")
        };

        let region = config.region.clone().try_into()?;

        let s3 = GcsSink {
            client: Self::create_client(cx.resolver(), region)?,
        };

        let tls = TlsSettings::from_options(&config.tls)?;
        let _client = https_client(cx.resolver(), tls)?;

        let filename_extension = config.filename_extension.clone();
        let bucket = config.bucket.clone();
        let options = config.options.clone();

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
            .settings(request, GcsRetryLogic)
            .service(s3);

        let sink = crate::sinks::util::BatchServiceSink::new(svc, cx.acker())
            .partitioned_batched_with_min(PartitionBuffer::new(Buffer::new(compression)), &batch)
            .with_flat_map(move |e| iter_ok(encode_event(e, &key_prefix, &encoding)));

        Ok(Box::new(sink))
    }

    pub fn healthcheck(
        config: &GcsSinkConfig,
        cx: &SinkContext,
        creds: &Option<GcpCredentials>,
    ) -> crate::Result<Healthcheck> {
        let mut builder = Request::builder();
        builder.method(Method::HEAD);
        builder.uri(format!("{}{}/", BASE_URL, config.bucket).parse::<Uri>()?);

        let mut request = builder.body(Body::empty()).unwrap();
        if let Some(creds) = &creds {
            creds.apply(&mut request);
        }

        let client = https_client(
            cx.resolver(),
            TlsSettings::from_options(&Default::default())?,
        )?;

        let healthcheck =
            client
                .request(request)
                .map_err(Into::into)
                .and_then(healthcheck_response(
                    creds.clone(),
                    GcsHealthcheckError::BucketNotFound {
                        bucket: config.bucket.clone(),
                    }
                    .into(),
                ));

        Ok(Box::new(healthcheck))
    }

    pub fn create_client(resolver: Resolver, region: Region) -> crate::Result<S3Client> {
        // Hack around the fact that rusoto will not pick up runtime
        // env vars. This is designed to only for test purposes use
        // static credentials.
        #[cfg(not(test))]
        {
            use rusoto_credential::DefaultCredentialsProvider;

            let p = DefaultCredentialsProvider::new()?;
            let d = crate::sinks::util::rusoto::client(resolver)?;

            Ok(S3Client::new_with(d, p, region))
        }

        #[cfg(test)]
        {
            use rusoto_credential::StaticProvider;

            let p = StaticProvider::new_minimal("test-access-key".into(), "test-secret-key".into());
            let d = crate::sinks::util::rusoto::client(resolver)?;

            Ok(S3Client::new_with(d, p, region))
        }
    }
}

impl Service<InsertRequest> for GcsSink {
    type Response = PutObjectOutput;
    type Error = RusotoError<PutObjectError>;
    type Future = Instrumented<RusotoFuture<PutObjectOutput, PutObjectError>>;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        Ok(().into())
    }

    fn call(&mut self, request: InsertRequest) -> Self::Future {
        let options = request.options;
        let mut tagging = url::form_urlencoded::Serializer::new(String::new());
        if let Some(tags) = options.tags {
            for (p, v) in tags {
                tagging.append_pair(&p, &v);
            }
        }
        let tagging = tagging.finish();
        self.client
            .put_object(PutObjectRequest {
                body: Some(request.body.into()),
                bucket: request.bucket,
                key: request.key,
                content_encoding: request.content_encoding,
                acl: options.acl.map(to_string),
                grant_full_control: options.grant_full_control,
                grant_read: options.grant_read,
                grant_read_acp: options.grant_read_acp,
                grant_write_acp: options.grant_write_acp,
                server_side_encryption: options.server_side_encryption.map(to_string),
                ssekms_key_id: options.ssekms_key_id,
                storage_class: options.storage_class.map(to_string),
                tagging: Some(tagging),
                ..Default::default()
            })
            .instrument(info_span!("request"))
    }
}

fn to_string(value: impl Serialize) -> String {
    serde_json::to_value(&value).unwrap().to_string()
}

fn build_request(
    req: PartitionInnerBuffer<Vec<u8>, Bytes>,
    time_format: String,
    extension: Option<String>,
    uuid: bool,
    gzip: bool,
    bucket: String,
    options: GcsOptions,
) -> InsertRequest {
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

    let extension = extension.unwrap_or_else(|| if gzip { "log.gz".into() } else { "log".into() });

    let key = String::from_utf8_lossy(&key[..]).into_owned();

    let key = format!("{}{}.{}", key, filename, extension);

    debug!(
        message = "sending events.",
        bytes = &field::debug(inner.len()),
        bucket = &field::debug(&bucket),
        key = &field::debug(&key)
    );

    InsertRequest {
        body: inner,
        bucket,
        key,
        content_encoding: if gzip { Some("gzip".to_string()) } else { None },
        options,
    }
}

#[derive(Debug, Clone)]
struct InsertRequest {
    body: Vec<u8>,
    bucket: String,
    key: String,
    content_encoding: Option<String>,
    options: GcsOptions,
}

#[derive(Debug, Clone)]
struct GcsRetryLogic;

impl RetryLogic for GcsRetryLogic {
    type Error = RusotoError<PutObjectError>;
    type Response = PutObjectOutput;

    fn is_retriable_error(&self, error: &Self::Error) -> bool {
        match error {
            RusotoError::HttpDispatch(_) => true,
            RusotoError::Unknown(res) if res.status.is_server_error() => true,
            _ => false,
        }
    }
}

fn encode_event(
    event: Event,
    key_prefix: &Template,
    encoding: &Encoding,
) -> Option<PartitionInnerBuffer<Vec<u8>, Bytes>> {
    let key = key_prefix
        .render_string(&event)
        .map_err(|missing_keys| {
            warn!(
                message = "Keys do not exist on the event. Dropping event.",
                ?missing_keys,
                rate_limit_secs = 30,
            );
        })
        .ok()?;

    let log = event.into_log();
    let bytes = match encoding {
        Encoding::Ndjson => serde_json::to_vec(&log.unflatten())
            .map(|mut b| {
                b.push(b'\n');
                b
            })
            .expect("Failed to encode event as json, this is a bug!"),
        &Encoding::Text => {
            let mut bytes = log
                .get(&event::MESSAGE)
                .map(|v| v.as_bytes().to_vec())
                .unwrap_or_default();
            bytes.push(b'\n');
            bytes
        }
    };

    Some(PartitionInnerBuffer::new(bytes, key.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{self, Event};

    use std::collections::HashMap;

    #[test]
    fn gcs_encode_event_text() {
        let message = "hello world".to_string();
        let batch_time_format = Template::from("date=%F");
        let bytes =
            encode_event(message.clone().into(), &batch_time_format, &Encoding::Text).unwrap();

        let encoded_message = message + "\n";
        let (bytes, _) = bytes.into_parts();
        assert_eq!(&bytes[..], encoded_message.as_bytes());
    }

    #[test]
    fn gcs_encode_event_ndjson() {
        let message = "hello world".to_string();
        let mut event = Event::from(message.clone());
        event.as_mut_log().insert("key", "value");

        let batch_time_format = Template::from("date=%F");
        let bytes = encode_event(event, &batch_time_format, &Encoding::Ndjson).unwrap();

        let (bytes, _) = bytes.into_parts();
        let map: HashMap<String, String> = serde_json::from_slice(&bytes[..]).unwrap();

        assert_eq!(map[&event::MESSAGE.to_string()], message);
        assert_eq!(map["key"], "value".to_string());
    }

    #[test]
    fn gcs_build_request() {
        let buf = PartitionInnerBuffer::new(vec![0u8; 10], Bytes::from("key/"));

        let req = build_request(
            buf.clone(),
            "date".into(),
            Some("ext".into()),
            false,
            false,
            "bucket".into(),
            GcsOptions::default(),
        );
        assert_eq!(req.key, "key/date.ext".to_string());

        let req = build_request(
            buf.clone(),
            "date".into(),
            None,
            false,
            false,
            "bucket".into(),
            GcsOptions::default(),
        );
        assert_eq!(req.key, "key/date.log".to_string());

        let req = build_request(
            buf.clone(),
            "date".into(),
            None,
            false,
            true,
            "bucket".into(),
            GcsOptions::default(),
        );
        assert_eq!(req.key, "key/date.log.gz".to_string());

        let req = build_request(
            buf.clone(),
            "date".into(),
            None,
            true,
            true,
            "bucket".into(),
            GcsOptions::default(),
        );
        assert_ne!(req.key, "key/date.log.gz".to_string());
    }
}

#[cfg(feature = "gcs-integration-tests")]
#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::{
        assert_downcast_matches,
        event::Event,
        region::RegionOrEndpoint,
        runtime::Runtime,
        test_util::{random_lines_with_stream, random_string, runtime},
        topology::config::SinkContext,
    };
    use flate2::read::GzDecoder;
    use futures::{Future, Sink};
    use pretty_assertions::assert_eq;
    use rusoto_core::region::Region;
    use rusoto_s3::{S3Client, S3};
    use std::io::{BufRead, BufReader};

    const BUCKET: &str = "router-tests";

    #[test]
    fn gcs_insert_message_into() {
        let mut rt = runtime();
        let cx = SinkContext::new_test(rt.executor());

        let config = config(1000000);
        let prefix = config.key_prefix.clone();
        let sink = GcsSink::new(&config, cx, &None).unwrap();

        let (lines, events) = random_lines_with_stream(100, 10);

        let pump = sink.send_all(events);
        let _ = rt.block_on(pump).unwrap();

        let keys = get_keys(prefix.unwrap());
        assert_eq!(keys.len(), 1);

        let key = keys[0].clone();
        assert!(key.ends_with(".log"));

        let obj = get_object(key);
        assert_eq!(obj.content_encoding, None);

        let response_lines = get_lines(obj);
        assert_eq!(lines, response_lines);
    }

    #[test]
    fn gcs_rotate_files_after_the_buffer_size_is_reached() {
        let mut rt = runtime();
        let cx = SinkContext::new_test(rt.executor());

        ensure_bucket(&client());

        let config = GcsSinkConfig {
            key_prefix: Some(format!("{}/{}", random_string(10), "{{i}}")),
            filename_time_format: Some("waitsforfullbatch".into()),
            filename_append_uuid: Some(false),
            ..config(1000)
        };
        let prefix = config.key_prefix.clone();
        let sink = GcsSink::new(&config, cx, &None).unwrap();

        let (lines, _events) = random_lines_with_stream(100, 30);

        let events = lines.clone().into_iter().enumerate().map(|(i, line)| {
            let mut e = Event::from(line);
            let i = if i < 10 {
                1
            } else if i < 20 {
                2
            } else {
                3
            };
            e.as_mut_log().insert("i", format!("{}", i));
            e
        });

        let pump = sink.send_all(futures::stream::iter_ok(events));
        let _ = rt.block_on(pump).unwrap();

        let keys = get_keys(prefix.unwrap());
        assert_eq!(keys.len(), 3);

        let response_lines = keys
            .into_iter()
            .map(|key| get_lines(get_object(key)))
            .collect::<Vec<_>>();

        assert_eq!(&lines[00..10], response_lines[0].as_slice());
        assert_eq!(&lines[10..20], response_lines[1].as_slice());
        assert_eq!(&lines[20..30], response_lines[2].as_slice());
    }

    #[test]
    fn gcs_waits_for_full_batch_or_timeout_before_sending() {
        let rt = runtime();
        let cx = SinkContext::new_test(rt.executor());

        ensure_bucket(&client());

        let config = GcsSinkConfig {
            key_prefix: Some(format!("{}/{}", random_string(10), "{{i}}")),
            filename_time_format: Some("waitsforfullbatch".into()),
            filename_append_uuid: Some(false),
            ..config(1000)
        };

        let prefix = config.key_prefix.clone();
        let sink = GcsSink::new(&config, cx, &None).unwrap();

        let (lines, _) = random_lines_with_stream(100, 30);

        let (tx, rx) = futures::sync::mpsc::channel(1);
        let pump = sink.send_all(rx).map(|_| ()).map_err(|_| ());

        let mut rt = Runtime::new().unwrap();
        rt.spawn(pump);

        let mut tx = tx.wait();

        for (i, line) in lines.iter().enumerate().take(15) {
            let mut event = Event::from(line.as_str());

            let i = if i < 10 { 1 } else { 2 };

            event.as_mut_log().insert("i", format!("{}", i));
            tx.send(event).unwrap();
        }

        std::thread::sleep(std::time::Duration::from_millis(100));

        for (i, line) in lines.iter().skip(15).enumerate() {
            let mut event = Event::from(line.as_str());

            let i = if i < 5 { 2 } else { 3 };

            event.as_mut_log().insert("i", format!("{}", i));
            tx.send(event).unwrap();
        }

        drop(tx);

        crate::test_util::shutdown_on_idle(rt);

        let keys = get_keys(prefix.unwrap());
        assert_eq!(keys.len(), 3);

        let response_lines = keys
            .into_iter()
            .map(|key| get_lines(get_object(key)))
            .collect::<Vec<_>>();

        assert_eq!(&lines[00..10], response_lines[0].as_slice());
        assert_eq!(&lines[10..20], response_lines[1].as_slice());
        assert_eq!(&lines[20..30], response_lines[2].as_slice());
    }

    #[test]
    fn gcs_gzip() {
        let mut rt = runtime();
        let cx = SinkContext::new_test(rt.executor());

        ensure_bucket(&client());

        let config = GcsSinkConfig {
            compression: Compression::Gzip,
            filename_time_format: Some("%S%f".into()),
            ..config(1000)
        };

        let prefix = config.key_prefix.clone();
        let sink = GcsSink::new(&config, cx, &None).unwrap();

        let (lines, events) = random_lines_with_stream(100, 500);

        let pump = sink.send_all(events);
        let _ = rt.block_on(pump).unwrap();

        let keys = get_keys(prefix.unwrap());
        assert_eq!(keys.len(), 2);

        let response_lines = keys
            .into_iter()
            .map(|key| {
                assert!(key.ends_with(".log.gz"));

                let obj = get_object(key);
                assert_eq!(obj.content_encoding, Some("gzip".to_string()));

                get_gzipped_lines(obj)
            })
            .flatten()
            .collect::<Vec<_>>();

        assert_eq!(lines, response_lines);
    }

    #[test]
    fn gcs_healthchecks() {
        let mut rt = Runtime::new().unwrap();
        let context = SinkContext::new_test(rt.executor());

        let healthcheck = GcsSink::healthcheck(&config(1), &context, &None).unwrap();
        rt.block_on(healthcheck).unwrap();
    }

    #[test]
    fn gcs_healthchecks_invalid_bucket() {
        let mut rt = Runtime::new().unwrap();
        let context = SinkContext::new_test(rt.executor());

        let config = GcsSinkConfig {
            bucket: "asdflkjadskdaadsfadf".to_string(),
            ..config(1)
        };
        let healthcheck = GcsSink::healthcheck(&config, &context, &None).unwrap();
        assert_downcast_matches!(
            rt.block_on(healthcheck).unwrap_err(),
            HealthcheckError,
            HealthcheckError::UnknownBucket{ .. }
        );
    }

    fn client() -> S3Client {
        let region = Region::Custom {
            name: "minio".to_owned(),
            endpoint: "http://localhost:9000".to_owned(),
        };

        use rusoto_core::HttpClient;
        use rusoto_credential::StaticProvider;

        let p = StaticProvider::new_minimal("test-access-key".into(), "test-secret-key".into());
        let d = HttpClient::new().unwrap();

        S3Client::new_with(d, p, region)
    }

    fn config(batch_size: usize) -> GcsSinkConfig {
        ensure_bucket(&client());

        GcsSinkConfig {
            key_prefix: Some(random_string(10) + "/date=%F/"),
            bucket: BUCKET.to_string(),
            compression: Compression::None,
            batch: BatchBytesConfig {
                max_size: Some(batch_size),
                timeout_secs: Some(5),
            },
            region: RegionOrEndpoint::with_endpoint("http://localhost:9000".to_owned()),
            ..Default::default()
        }
    }

    fn ensure_bucket(client: &S3Client) {
        use rusoto_s3::{CreateBucketError, CreateBucketRequest};

        let req = CreateBucketRequest {
            bucket: BUCKET.to_string(),
            ..Default::default()
        };

        let res = client.create_bucket(req);

        match res.sync() {
            Ok(_) | Err(RusotoError::Service(CreateBucketError::BucketAlreadyOwnedByYou(_))) => {}
            Err(e) => match e {
                RusotoError::Unknown(b) => {
                    let body = String::from_utf8_lossy(&b.body[..]);
                    panic!("Couldn't create bucket: {:?}; Body {}", b, body);
                }
                _ => panic!("Couldn't create bucket: {}", e),
            },
        }
    }

    fn get_keys(prefix: String) -> Vec<String> {
        let prefix = prefix.split("/").into_iter().next().unwrap().to_string();

        let list_res = client()
            .list_objects_v2(rusoto_s3::ListObjectsV2Request {
                bucket: BUCKET.to_string(),
                prefix: Some(prefix),
                ..Default::default()
            })
            .sync()
            .unwrap();

        list_res
            .contents
            .unwrap()
            .into_iter()
            .map(|obj| obj.key.unwrap())
            .collect()
    }

    fn get_object(key: String) -> rusoto_s3::GetObjectOutput {
        client()
            .get_object(rusoto_s3::GetObjectRequest {
                bucket: BUCKET.to_string(),
                key,
                ..Default::default()
            })
            .sync()
            .unwrap()
    }

    fn get_lines(obj: rusoto_s3::GetObjectOutput) -> Vec<String> {
        let buf_read = BufReader::new(obj.body.unwrap().into_blocking_read());
        buf_read.lines().map(|l| l.unwrap()).collect()
    }

    fn get_gzipped_lines(obj: rusoto_s3::GetObjectOutput) -> Vec<String> {
        let buf_read = BufReader::new(GzDecoder::new(obj.body.unwrap().into_blocking_read()));
        buf_read.lines().map(|l| l.unwrap()).collect()
    }
}
