mod retry;

use self::retry::{ElasticSearchRetryLogic, ElasticSearchServiceLogic};
use crate::{
    config::{DataType, SinkConfig, SinkContext, SinkDescription},
    emit,
    http::{Auth, HttpClient, MaybeAuth},
    internal_events::{ElasticSearchEventEncoded, TemplateRenderingFailed},
    rusoto::{self, region_from_endpoint, AwsAuthentication, RegionOrEndpoint},
    sinks::util::{
        encoding::{EncodingConfigWithDefault, EncodingConfiguration},
        http::{BatchedHttpSink, HttpSink, RequestConfig},
        BatchConfig, BatchSettings, Buffer, Compression, EncodedEvent, TowerRequestConfig,
        UriSerde,
    },
    template::{Template, TemplateParseError},
    tls::{TlsOptions, TlsSettings},
    transforms::metric_to_log::{MetricToLog, MetricToLogConfig},
};
use futures::{FutureExt, SinkExt};
use http::{
    header::{HeaderName, HeaderValue},
    uri::InvalidUri,
    Request, StatusCode, Uri,
};
use hyper::Body;
use indexmap::IndexMap;
use lazy_static::lazy_static;
use rusoto_core::Region;
use rusoto_credential::{CredentialsError, ProvideAwsCredentials};
use rusoto_signature::{SignedRequest, SignedRequestPayload};
use serde::{Deserialize, Serialize};
use serde_json::json;
use snafu::{ResultExt, Snafu};
use std::collections::{BTreeMap, HashMap};
use std::convert::TryFrom;
use vector_core::event::{Event, Value};

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct ElasticSearchConfig {
    // Deprecated name
    #[serde(alias = "host")]
    pub endpoint: String,
    // Deprecated, use normal.index instead
    pub index: Option<String>,
    pub doc_type: Option<String>,
    pub id_key: Option<String>,
    pub pipeline: Option<String>,
    #[serde(default)]
    pub mode: ElasticSearchMode,

    #[serde(default)]
    pub compression: Compression,
    #[serde(
        skip_serializing_if = "crate::serde::skip_serializing_if_default",
        default
    )]
    pub encoding: EncodingConfigWithDefault<Encoding>,
    #[serde(default)]
    pub batch: BatchConfig,
    #[serde(default)]
    pub request: RequestConfig,
    pub auth: Option<ElasticSearchAuth>,

    // Deprecated, moved to request.
    pub headers: Option<IndexMap<String, String>>,
    pub query: Option<HashMap<String, String>>,

    pub aws: Option<RegionOrEndpoint>,
    pub tls: Option<TlsOptions>,
    // Deprecated, use normal.bulk_action instead
    pub bulk_action: Option<String>,
    pub normal: Option<NormalConfig>,
    pub data_stream: Option<DataStreamConfig>,
    pub metrics: Option<MetricToLogConfig>,
}

impl ElasticSearchConfig {
    fn bulk_action(&self) -> crate::Result<Option<Template>> {
        Ok(self
            .normal
            .as_ref()
            .and_then(|n| n.bulk_action.as_deref())
            .or_else(|| self.bulk_action.as_deref())
            .map(|value| Template::try_from(value).context(BatchActionTemplate))
            .transpose()?)
    }

    fn index(&self) -> crate::Result<Template> {
        let index = self
            .normal
            .as_ref()
            .and_then(|n| n.index.as_deref())
            .or_else(|| self.index.as_deref())
            .map(String::from)
            .unwrap_or_else(NormalConfig::default_index);
        Ok(Template::try_from(index.as_str()).context(IndexTemplate)?)
    }

    fn common_mode(&self) -> crate::Result<ElasticSearchCommonMode> {
        match self.mode {
            ElasticSearchMode::Normal => {
                let index = self.index()?;
                let bulk_action = self.bulk_action()?;
                Ok(ElasticSearchCommonMode::Normal { index, bulk_action })
            }
            ElasticSearchMode::DataStream => Ok(ElasticSearchCommonMode::DataStream(
                self.data_stream.clone().unwrap_or_default(),
            )),
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Default, Debug)]
#[serde(rename_all = "snake_case")]
pub struct NormalConfig {
    bulk_action: Option<String>,
    index: Option<String>,
}

impl NormalConfig {
    fn default_index() -> String {
        "vector-%Y.%m.%d".into()
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct DataStreamConfig {
    #[serde(default = "DataStreamConfig::default_timestamp")]
    timestamp: String,
    #[serde(rename = "type", default = "DataStreamConfig::default_type")]
    dtype: Template,
    #[serde(default = "DataStreamConfig::default_dataset")]
    dataset: Template,
    #[serde(default = "DataStreamConfig::default_namespace")]
    namespace: Template,
    #[serde(default = "DataStreamConfig::default_auto_routing")]
    auto_routing: bool,
    #[serde(default = "DataStreamConfig::default_sync_fields")]
    sync_fields: bool,
}

impl Default for DataStreamConfig {
    fn default() -> Self {
        Self {
            timestamp: Self::default_timestamp(),
            dtype: Self::default_type(),
            dataset: Self::default_dataset(),
            namespace: Self::default_namespace(),
            auto_routing: Self::default_auto_routing(),
            sync_fields: Self::default_sync_fields(),
        }
    }
}

impl DataStreamConfig {
    fn default_timestamp() -> String {
        "@timestamp".into()
    }

    fn default_type() -> Template {
        Template::try_from("logs").expect("couldn't build default type template")
    }

    fn default_dataset() -> Template {
        Template::try_from("generic").expect("couldn't build default dataset template")
    }

    fn default_namespace() -> Template {
        Template::try_from("default").expect("couldn't build default namespace template")
    }

    fn default_auto_routing() -> bool {
        true
    }

    fn default_sync_fields() -> bool {
        true
    }

    fn update_timestamp(&self, mut event: Event) -> Event {
        // we keep it if the timestamp field is @timestamp
        if self.timestamp == Self::default_timestamp() {
            return event;
        }
        let log = event.as_mut_log().as_map_mut();
        if let Some(value) = log.remove(&self.timestamp) {
            log.insert(Self::default_timestamp(), value);
        }
        event
    }

    fn dtype(&self, event: &Event) -> Option<String> {
        self.dtype
            .render_string(event)
            .map_err(|error| {
                emit!(TemplateRenderingFailed {
                    error,
                    field: Some("data_stream.type"),
                    drop_event: true,
                });
            })
            .ok()
    }

    fn dataset(&self, event: &Event) -> Option<String> {
        self.dataset
            .render_string(event)
            .map_err(|error| {
                emit!(TemplateRenderingFailed {
                    error,
                    field: Some("data_stream.dataset"),
                    drop_event: true,
                });
            })
            .ok()
    }

    fn namespace(&self, event: &Event) -> Option<String> {
        self.namespace
            .render_string(event)
            .map_err(|error| {
                emit!(TemplateRenderingFailed {
                    error,
                    field: Some("data_stream.namespace"),
                    drop_event: true,
                });
            })
            .ok()
    }

    fn sync_fields(&self, mut event: Event) -> Event {
        if !self.sync_fields {
            return event;
        }
        let dtype = self.dtype(&event);
        let dataset = self.dataset(&event);
        let namespace = self.namespace(&event);

        let existing = event
            .as_mut_log()
            .as_map_mut()
            .entry("data_stream".into())
            .or_insert_with(|| Value::Map(BTreeMap::new()))
            .as_map_mut();
        if let Some(dtype) = dtype {
            existing
                .entry("type".into())
                .or_insert_with(|| dtype.into());
        }
        if let Some(dataset) = dataset {
            existing
                .entry("dataset".into())
                .or_insert_with(|| dataset.into());
        }
        if let Some(namespace) = namespace {
            existing
                .entry("namespace".into())
                .or_insert_with(|| namespace.into());
        }
        event
    }

    fn index(&self, event: &Event) -> Option<String> {
        let (dtype, dataset, namespace) = if !self.auto_routing {
            (
                self.dtype(event)?,
                self.dataset(event)?,
                self.namespace(event)?,
            )
        } else {
            let data_stream = event.as_log().get("data_stream").and_then(|ds| ds.as_map());
            let dtype = data_stream
                .and_then(|ds| ds.get("type"))
                .map(|value| value.to_string_lossy())
                .or_else(|| self.dtype(event))?;
            let dataset = data_stream
                .and_then(|ds| ds.get("dataset"))
                .map(|value| value.to_string_lossy())
                .or_else(|| self.dataset(event))?;
            let namespace = data_stream
                .and_then(|ds| ds.get("namespace"))
                .map(|value| value.to_string_lossy())
                .or_else(|| self.namespace(event))?;
            (dtype, dataset, namespace)
        };
        Some(format!("{}-{}-{}", dtype, dataset, namespace))
    }
}

lazy_static! {
    static ref REQUEST_DEFAULTS: TowerRequestConfig = TowerRequestConfig {
        ..Default::default()
    };
}

#[derive(Deserialize, Serialize, Debug, Eq, PartialEq, Clone, Derivative)]
#[serde(rename_all = "snake_case")]
#[derivative(Default)]
pub enum Encoding {
    #[derivative(Default)]
    Default,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(deny_unknown_fields, rename_all = "snake_case", tag = "strategy")]
pub enum ElasticSearchAuth {
    Basic { user: String, password: String },
    Aws(AwsAuthentication),
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum ElasticSearchMode {
    Normal,
    DataStream,
}

impl Default for ElasticSearchMode {
    fn default() -> Self {
        Self::Normal
    }
}

#[derive(Derivative, Deserialize, Serialize, Clone, Copy, Debug)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum BulkAction {
    Index,
    Create,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
impl BulkAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            BulkAction::Index => "index",
            BulkAction::Create => "create",
        }
    }

    pub fn as_json_pointer(&self) -> &'static str {
        match self {
            BulkAction::Index => "/index",
            BulkAction::Create => "/create",
        }
    }
}

impl TryFrom<&str> for BulkAction {
    type Error = String;

    fn try_from(input: &str) -> Result<Self, Self::Error> {
        match input {
            "index" => Ok(BulkAction::Index),
            "create" => Ok(BulkAction::Create),
            _ => Err(format!("Invalid bulk action: {}", input)),
        }
    }
}

inventory::submit! {
    SinkDescription::new::<ElasticSearchConfig>("elasticsearch")
}

impl_generate_config_from_default!(ElasticSearchConfig);

#[async_trait::async_trait]
#[typetag::serde(name = "elasticsearch")]
impl SinkConfig for ElasticSearchConfig {
    async fn build(
        &self,
        cx: SinkContext,
    ) -> crate::Result<(super::VectorSink, super::Healthcheck)> {
        let common = ElasticSearchCommon::parse_config(self)?;
        let client = HttpClient::new(common.tls_settings.clone())?;

        let healthcheck = common.healthcheck(client.clone()).boxed();

        let common = ElasticSearchCommon::parse_config(self)?;
        let compression = common.compression;
        let batch = BatchSettings::default()
            .bytes(bytesize::mib(10u64))
            .timeout(1)
            .parse_config(self.batch)?;
        let request = self.request.tower.unwrap_with(&REQUEST_DEFAULTS);

        let sink = BatchedHttpSink::with_logic(
            common,
            Buffer::new(batch.size, compression),
            ElasticSearchRetryLogic,
            request,
            batch.timeout,
            client,
            cx.acker(),
            ElasticSearchServiceLogic,
        )
        .sink_map_err(|error| error!(message = "Fatal elasticsearch sink error.", %error));

        Ok((super::VectorSink::Sink(Box::new(sink)), healthcheck))
    }

    fn input_type(&self) -> DataType {
        DataType::Any
    }

    fn sink_type(&self) -> &'static str {
        "elasticsearch"
    }
}

#[derive(Debug)]
enum ElasticSearchCommonMode {
    Normal {
        index: Template,
        bulk_action: Option<Template>,
    },
    DataStream(DataStreamConfig),
}

impl ElasticSearchCommonMode {
    fn index(&self, event: &Event) -> Option<String> {
        match self {
            Self::Normal { index, .. } => index
                .render_string(event)
                .map_err(|error| {
                    emit!(TemplateRenderingFailed {
                        error,
                        field: Some("index"),
                        drop_event: true,
                    });
                })
                .ok(),
            Self::DataStream(ds) => ds.index(event),
        }
    }

    fn bulk_action(&self, event: &Event) -> Option<BulkAction> {
        match self {
            ElasticSearchCommonMode::Normal { bulk_action, .. } => match bulk_action {
                Some(template) => template
                    .render_string(event)
                    .map_err(|error| {
                        emit!(TemplateRenderingFailed {
                            error,
                            field: Some("bulk_action"),
                            drop_event: true,
                        });
                    })
                    .ok()
                    .and_then(|value| BulkAction::try_from(value.as_str()).ok()),
                None => Some(BulkAction::Index),
            },
            // avoid the interpolation
            ElasticSearchCommonMode::DataStream(_) => Some(BulkAction::Create),
        }
    }

    fn as_data_stream_config(&self) -> Option<&DataStreamConfig> {
        match self {
            Self::DataStream(value) => Some(value),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct ElasticSearchCommon {
    pub base_url: String,
    id_key: Option<String>,
    bulk_uri: Uri,
    authorization: Option<Auth>,
    credentials: Option<rusoto::AwsCredentialsProvider>,
    encoding: EncodingConfigWithDefault<Encoding>,
    mode: ElasticSearchCommonMode,
    doc_type: String,
    tls_settings: TlsSettings,
    compression: Compression,
    region: Region,
    request: RequestConfig,
    query_params: HashMap<String, String>,
    metric_to_log: MetricToLog,
}

#[derive(Debug, Snafu)]
enum ParseError {
    #[snafu(display("Invalid host {:?}: {:?}", host, source))]
    InvalidHost { host: String, source: InvalidUri },
    #[snafu(display("Host {:?} must include hostname", host))]
    HostMustIncludeHostname { host: String },
    #[snafu(display("Could not generate AWS credentials: {:?}", source))]
    AwsCredentialsGenerateFailed { source: CredentialsError },
    #[snafu(display("Index template parse error: {}", source))]
    IndexTemplate { source: TemplateParseError },
    #[snafu(display("Batch action template parse error: {}", source))]
    BatchActionTemplate { source: TemplateParseError },
}

impl ElasticSearchCommon {
    fn encode_log(&self, event: Event) -> Option<EncodedEvent<Vec<u8>>> {
        let index = self.mode.index(&event)?;

        let mut event = if let Some(cfg) = self.mode.as_data_stream_config() {
            cfg.update_timestamp(cfg.sync_fields(event))
        } else {
            event
        };

        let bulk_action = self.mode.bulk_action(&event)?;

        let mut action = json!({
            bulk_action.as_str(): {
                "_index": index,
                "_type": self.doc_type,
            }
        });

        maybe_set_id(
            self.id_key.as_ref(),
            action.pointer_mut(bulk_action.as_json_pointer()).unwrap(),
            &mut event,
        );

        let mut body = serde_json::to_vec(&action).unwrap();
        body.push(b'\n');

        self.encoding.apply_rules(&mut event);

        let log = event.into_log();
        serde_json::to_writer(&mut body, &log).unwrap();
        body.push(b'\n');

        emit!(ElasticSearchEventEncoded {
            byte_size: body.len(),
            index,
        });

        Some(EncodedEvent::new(body).with_metadata(log))
    }
}

#[async_trait::async_trait]
impl HttpSink for ElasticSearchCommon {
    type Input = Vec<u8>;
    type Output = Vec<u8>;

    fn encode_event(&self, event: Event) -> Option<EncodedEvent<Self::Input>> {
        let log = match event {
            Event::Log(log) => Some(log),
            Event::Metric(metric) => self.metric_to_log.transform_one(metric),
        };
        log.and_then(|log| self.encode_log(log.into()))
    }

    async fn build_request(&self, events: Self::Output) -> crate::Result<http::Request<Vec<u8>>> {
        let mut builder = Request::post(&self.bulk_uri);

        if let Some(credentials_provider) = &self.credentials {
            let mut request = self.signed_request("POST", &self.bulk_uri, true);

            request.add_header("Content-Type", "application/x-ndjson");

            if let Some(ce) = self.compression.content_encoding() {
                request.add_header("Content-Encoding", ce);
            }

            for (header, value) in &self.request.headers {
                request.add_header(header, value);
            }

            request.set_payload(Some(events));

            // mut builder?
            builder = finish_signer(&mut request, credentials_provider, builder).await?;

            // The SignedRequest ends up owning the body, so we have
            // to play games here
            let body = request.payload.take().unwrap();
            match body {
                SignedRequestPayload::Buffer(body) => {
                    builder.body(body.to_vec()).map_err(Into::into)
                }
                _ => unreachable!(),
            }
        } else {
            builder = builder.header("Content-Type", "application/x-ndjson");

            if let Some(ce) = self.compression.content_encoding() {
                builder = builder.header("Content-Encoding", ce);
            }

            for (header, value) in &self.request.headers {
                builder = builder.header(&header[..], &value[..]);
            }

            if let Some(auth) = &self.authorization {
                builder = auth.apply_builder(builder);
            }

            builder.body(events).map_err(Into::into)
        }
    }
}

impl ElasticSearchCommon {
    pub fn parse_config(config: &ElasticSearchConfig) -> crate::Result<Self> {
        // Test the configured host, but ignore the result
        let uri = format!("{}/_test", &config.endpoint);
        let uri = uri.parse::<Uri>().with_context(|| InvalidHost {
            host: &config.endpoint,
        })?;
        if uri.host().is_none() {
            return Err(ParseError::HostMustIncludeHostname {
                host: config.endpoint.clone(),
            }
            .into());
        }

        let authorization = match &config.auth {
            Some(ElasticSearchAuth::Basic { user, password }) => Some(Auth::Basic {
                user: user.clone(),
                password: password.clone(),
            }),
            _ => None,
        };
        let uri = config.endpoint.parse::<UriSerde>()?;
        let authorization = authorization.choose_one(&uri.auth)?;
        let base_url = uri.uri.to_string().trim_end_matches('/').to_owned();

        let region = match &config.aws {
            Some(region) => Region::try_from(region)?,
            None => region_from_endpoint(&base_url)?,
        };

        let credentials = match &config.auth {
            Some(ElasticSearchAuth::Basic { .. }) | None => None,
            Some(ElasticSearchAuth::Aws(aws)) => Some(aws.build(&region, None)?),
        };

        let compression = config.compression;
        let mode = config.common_mode()?;

        let doc_type = config.doc_type.clone().unwrap_or_else(|| "_doc".into());

        let tower_request = config.request.tower.unwrap_with(&REQUEST_DEFAULTS);

        let mut query_params = config.query.clone().unwrap_or_default();
        query_params.insert(
            "timeout".into(),
            format!("{}s", tower_request.timeout.as_secs()),
        );

        if let Some(pipeline) = &config.pipeline {
            query_params.insert("pipeline".into(), pipeline.into());
        }

        let mut query = url::form_urlencoded::Serializer::new(String::new());
        for (p, v) in &query_params {
            query.append_pair(&p[..], &v[..]);
        }
        let bulk_url = format!("{}/_bulk?{}", base_url, query.finish());
        let bulk_uri = bulk_url.parse::<Uri>().unwrap();

        let tls_settings = TlsSettings::from_options(&config.tls)?;
        let mut config = config.clone();
        let mut request = config.request;
        request.add_old_option(config.headers.take());

        let metric_config = config.metrics.clone().unwrap_or_default();
        let metric_to_log = MetricToLog::new(
            metric_config.host_tag,
            metric_config.timezone.unwrap_or_default(),
        );

        Ok(Self {
            authorization,
            base_url,
            bulk_uri,
            compression,
            credentials,
            doc_type,
            encoding: config.encoding,
            id_key: config.id_key,
            mode,
            query_params,
            request,
            region,
            tls_settings,
            metric_to_log,
        })
    }

    fn signed_request(&self, method: &str, uri: &Uri, use_params: bool) -> SignedRequest {
        let mut request = SignedRequest::new(method, "es", &self.region, uri.path());
        request.set_hostname(uri.host().map(|host| host.into()));
        if use_params {
            for (key, value) in &self.query_params {
                request.add_param(key, value);
            }
        }
        request
    }

    async fn healthcheck(self, client: HttpClient) -> crate::Result<()> {
        let mut builder = Request::get(format!("{}/_cluster/health", self.base_url));

        match &self.credentials {
            None => {
                if let Some(authorization) = &self.authorization {
                    builder = authorization.apply_builder(builder);
                }
            }
            Some(credentials_provider) => {
                let mut signer = self.signed_request("GET", builder.uri_ref().unwrap(), false);
                builder = finish_signer(&mut signer, credentials_provider, builder).await?;
            }
        }
        let request = builder.body(Body::empty())?;
        let response = client.send(request).await?;

        match response.status() {
            StatusCode::OK => Ok(()),
            status => Err(super::HealthcheckError::UnexpectedStatus { status }.into()),
        }
    }
}

async fn finish_signer(
    signer: &mut SignedRequest,
    credentials_provider: &rusoto::AwsCredentialsProvider,
    mut builder: http::request::Builder,
) -> crate::Result<http::request::Builder> {
    let credentials = credentials_provider
        .credentials()
        .await
        .context(AwsCredentialsGenerateFailed)?;

    signer.sign(&credentials);

    for (name, values) in signer.headers() {
        let header_name = name
            .parse::<HeaderName>()
            .expect("Could not parse header name.");
        for value in values {
            let header_value =
                HeaderValue::from_bytes(value).expect("Could not parse header value.");
            builder = builder.header(&header_name, header_value);
        }
    }

    Ok(builder)
}

fn maybe_set_id(key: Option<impl AsRef<str>>, doc: &mut serde_json::Value, event: &mut Event) {
    if let Event::Log(_) = event {
        if let Some(val) = key.and_then(|k| event.as_mut_log().remove(k)) {
            let val = val.to_string_lossy();

            doc.as_object_mut()
                .unwrap()
                .insert("_id".into(), json!(val));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        event::{Event, Metric, MetricKind, MetricValue, Value},
        sinks::util::retries::{RetryAction, RetryLogic},
    };
    use bytes::Bytes;
    use http::{Response, StatusCode};
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<ElasticSearchConfig>();
    }

    #[test]
    fn parse_aws_auth() {
        toml::from_str::<ElasticSearchConfig>(
            r#"
            endpoint = ""
            auth.strategy = "aws"
            auth.assume_role = "role"
        "#,
        )
        .unwrap();

        toml::from_str::<ElasticSearchConfig>(
            r#"
            endpoint = ""
            auth.strategy = "aws"
        "#,
        )
        .unwrap();
    }

    #[test]
    fn parse_mode() {
        let config = toml::from_str::<ElasticSearchConfig>(
            r#"
            endpoint = ""
            mode = "data_stream"
            data_stream.type = "synthetics"
        "#,
        )
        .unwrap();
        assert!(matches!(config.mode, ElasticSearchMode::DataStream));
        assert!(config.data_stream.is_some());
    }

    #[test]
    fn removes_and_sets_id_from_custom_field() {
        let id_key = Some("foo");
        let mut event = Event::from("butts");
        event.as_mut_log().insert("foo", "bar");
        let mut action = json!({});

        maybe_set_id(id_key, &mut action, &mut event);

        assert_eq!(json!({"_id": "bar"}), action);
        assert_eq!(None, event.as_log().get("foo"));
    }

    #[test]
    fn doesnt_set_id_when_field_missing() {
        let id_key = Some("foo");
        let mut event = Event::from("butts");
        event.as_mut_log().insert("not_foo", "bar");
        let mut action = json!({});

        maybe_set_id(id_key, &mut action, &mut event);

        assert_eq!(json!({}), action);
    }

    #[test]
    fn doesnt_set_id_when_not_configured() {
        let id_key: Option<&str> = None;
        let mut event = Event::from("butts");
        event.as_mut_log().insert("foo", "bar");
        let mut action = json!({});

        maybe_set_id(id_key, &mut action, &mut event);

        assert_eq!(json!({}), action);
    }

    #[test]
    fn sets_create_action_when_configured() {
        use crate::config::log_schema;
        use chrono::{TimeZone, Utc};

        let config = ElasticSearchConfig {
            bulk_action: Some(String::from("{{ action }}te")),
            index: Some(String::from("vector")),
            endpoint: String::from("https://example.com"),
            ..Default::default()
        };
        let es = ElasticSearchCommon::parse_config(&config).unwrap();

        let mut event = Event::from("hello there");
        event.as_mut_log().insert(
            log_schema().timestamp_key(),
            Utc.ymd(2020, 12, 1).and_hms(1, 2, 3),
        );
        event.as_mut_log().insert("action", "crea");
        let encoded = es.encode_event(event).unwrap().item;
        let expected = r#"{"create":{"_index":"vector","_type":"_doc"}}
{"action":"crea","message":"hello there","timestamp":"2020-12-01T01:02:03Z"}
"#;
        assert_eq!(std::str::from_utf8(&encoded).unwrap(), expected);
    }

    fn data_stream_body() -> BTreeMap<String, Value> {
        let mut ds = BTreeMap::<String, Value>::new();
        ds.insert("type".into(), Value::from("synthetics"));
        ds.insert("dataset".into(), Value::from("testing"));
        ds
    }

    #[test]
    fn encode_datastream_mode() {
        use crate::config::log_schema;
        use chrono::{TimeZone, Utc};

        let config = ElasticSearchConfig {
            index: Some(String::from("vector")),
            endpoint: String::from("https://example.com"),
            mode: ElasticSearchMode::DataStream,
            ..Default::default()
        };
        let es = ElasticSearchCommon::parse_config(&config).unwrap();

        let mut event = Event::from("hello there");
        event.as_mut_log().insert(
            log_schema().timestamp_key(),
            Utc.ymd(2020, 12, 1).and_hms(1, 2, 3),
        );
        event.as_mut_log().insert("data_stream", data_stream_body());
        let encoded = es.encode_event(event).unwrap().item;
        let expected = r#"{"create":{"_index":"synthetics-testing-default","_type":"_doc"}}
{"data_stream":{"dataset":"testing","namespace":"default","type":"synthetics"},"message":"hello there","timestamp":"2020-12-01T01:02:03Z"}
"#;
        assert_eq!(std::str::from_utf8(&encoded).unwrap(), expected);
    }

    #[test]
    fn encode_datastream_mode_no_routing() {
        use crate::config::log_schema;
        use chrono::{TimeZone, Utc};

        let config = ElasticSearchConfig {
            index: Some(String::from("vector")),
            endpoint: String::from("https://example.com"),
            mode: ElasticSearchMode::DataStream,
            data_stream: Some(DataStreamConfig {
                auto_routing: false,
                namespace: Template::try_from("something").unwrap(),
                timestamp: log_schema().timestamp_key().into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let es = ElasticSearchCommon::parse_config(&config).unwrap();

        let mut event = Event::from("hello there");
        event.as_mut_log().insert("data_stream", data_stream_body());
        event.as_mut_log().insert(
            log_schema().timestamp_key(),
            Utc.ymd(2020, 12, 1).and_hms(1, 2, 3),
        );
        let encoded = es.encode_event(event).unwrap().item;
        let expected = r#"{"create":{"_index":"logs-generic-something","_type":"_doc"}}
{"@timestamp":"2020-12-01T01:02:03Z","data_stream":{"dataset":"testing","namespace":"something","type":"synthetics"},"message":"hello there"}
"#;
        assert_eq!(std::str::from_utf8(&encoded).unwrap(), expected);
    }

    #[test]
    fn handle_metrics() {
        let config = ElasticSearchConfig {
            bulk_action: Some(String::from("create")),
            index: Some(String::from("vector")),
            endpoint: String::from("https://example.com"),
            ..Default::default()
        };
        let es = ElasticSearchCommon::parse_config(&config).unwrap();

        let metric = Metric::new(
            "cpu",
            MetricKind::Absolute,
            MetricValue::Gauge { value: 42.0 },
        );
        let event = Event::from(metric);

        let encoded = es.encode_event(event).unwrap().item;
        let encoded = std::str::from_utf8(&encoded).unwrap();
        let encoded_lines = encoded.split('\n').map(String::from).collect::<Vec<_>>();
        assert_eq!(encoded_lines.len(), 3); // there's an empty line at the end
        assert_eq!(
            encoded_lines.get(0).unwrap(),
            r#"{"create":{"_index":"vector","_type":"_doc"}}"#
        );
        println!("line: {}", encoded_lines.get(1).unwrap());
        assert!(encoded_lines
            .get(1)
            .unwrap()
            .starts_with(r#"{"gauge":{"value":42.0},"kind":"absolute","name":"cpu","timestamp""#));
    }

    #[test]
    fn decode_bulk_action_error() {
        let config = ElasticSearchConfig {
            bulk_action: Some(String::from("{{ action }}")),
            index: Some(String::from("vector")),
            endpoint: String::from("https://example.com"),
            ..Default::default()
        };
        let es = ElasticSearchCommon::parse_config(&config).unwrap();

        let mut event = Event::from("hello world");
        event.as_mut_log().insert("foo", "bar");
        event.as_mut_log().insert("idx", "purple");
        let action = es.mode.bulk_action(&event);
        assert!(action.is_none());
    }

    #[test]
    fn decode_bulk_action() {
        let config = ElasticSearchConfig {
            bulk_action: Some(String::from("create")),
            index: Some(String::from("vector")),
            endpoint: String::from("https://example.com"),
            ..Default::default()
        };
        let es = ElasticSearchCommon::parse_config(&config).unwrap();

        let event = Event::from("hello there");
        let action = es.mode.bulk_action(&event).unwrap();
        assert!(matches!(action, BulkAction::Create));
    }

    #[test]
    fn encode_datastream_mode_no_sync() {
        use crate::config::log_schema;
        use chrono::{TimeZone, Utc};

        let config = ElasticSearchConfig {
            index: Some(String::from("vector")),
            endpoint: String::from("https://example.com"),
            mode: ElasticSearchMode::DataStream,
            data_stream: Some(DataStreamConfig {
                namespace: Template::try_from("something").unwrap(),
                sync_fields: false,
                ..Default::default()
            }),
            ..Default::default()
        };

        let es = ElasticSearchCommon::parse_config(&config).unwrap();

        let mut event = Event::from("hello there");
        event.as_mut_log().insert("data_stream", data_stream_body());
        event.as_mut_log().insert(
            log_schema().timestamp_key(),
            Utc.ymd(2020, 12, 1).and_hms(1, 2, 3),
        );
        let encoded = es.encode_event(event).unwrap().item;
        let expected = r#"{"create":{"_index":"synthetics-testing-something","_type":"_doc"}}
{"data_stream":{"dataset":"testing","type":"synthetics"},"message":"hello there","timestamp":"2020-12-01T01:02:03Z"}
"#;
        assert_eq!(std::str::from_utf8(&encoded).unwrap(), expected);
    }

    #[test]
    fn handles_error_response() {
        let json = "{\"took\":185,\"errors\":true,\"items\":[{\"index\":{\"_index\":\"test-hgw28jv10u\",\"_type\":\"log_lines\",\"_id\":\"3GhQLXEBE62DvOOUKdFH\",\"status\":400,\"error\":{\"type\":\"illegal_argument_exception\",\"reason\":\"mapper [message] of different type, current_type [long], merged_type [text]\"}}}]}";
        let response = Response::builder()
            .status(StatusCode::OK)
            .body(Bytes::from(json))
            .unwrap();
        let logic = ElasticSearchRetryLogic;
        assert!(matches!(
            logic.should_retry_response(&response),
            RetryAction::DontRetry(_)
        ));
    }

    #[test]
    fn allows_using_excepted_fields() {
        let config = ElasticSearchConfig {
            index: Some(String::from("{{ idx }}")),
            encoding: EncodingConfigWithDefault {
                except_fields: Some(vec!["idx".to_string(), "timestamp".to_string()]),
                ..Default::default()
            },
            endpoint: String::from("https://example.com"),
            ..Default::default()
        };
        let es = ElasticSearchCommon::parse_config(&config).unwrap();

        let mut event = Event::from("hello there");
        event.as_mut_log().insert("foo", "bar");
        event.as_mut_log().insert("idx", "purple");

        let encoded = es.encode_event(event).unwrap().item;
        let expected = r#"{"index":{"_index":"purple","_type":"_doc"}}
{"foo":"bar","message":"hello there"}
"#;
        assert_eq!(std::str::from_utf8(&encoded).unwrap(), expected);
    }

    #[test]
    fn validate_host_header_on_aws_requests() {
        let config = ElasticSearchConfig {
            auth: Some(ElasticSearchAuth::Aws(AwsAuthentication::Default {})),
            endpoint: "http://abc-123.us-east-1.es.amazonaws.com".into(),
            batch: BatchConfig {
                max_events: Some(1),
                ..Default::default()
            },
            ..Default::default()
        };

        let common = ElasticSearchCommon::parse_config(&config).expect("Config error");

        let signed_request = common.signed_request(
            "POST",
            &"http://abc-123.us-east-1.es.amazonaws.com"
                .parse::<Uri>()
                .unwrap(),
            true,
        );

        assert_eq!(
            signed_request.hostname(),
            "abc-123.us-east-1.es.amazonaws.com".to_string()
        );
    }
}

#[cfg(test)]
#[cfg(feature = "es-integration-tests")]
mod integration_tests {
    use super::*;
    use crate::{
        config::{SinkConfig, SinkContext},
        http::HttpClient,
        sinks::HealthcheckError,
        test_util::{random_events_with_stream, random_string, trace_init},
        tls::{self, TlsOptions},
    };
    use chrono::Utc;
    use futures::{stream, StreamExt};
    use http::{Request, StatusCode};
    use hyper::Body;
    use serde_json::{json, Value};
    use std::{fs::File, future::ready, io::Read};
    use vector_core::event::{BatchNotifier, BatchStatus, LogEvent};

    impl ElasticSearchCommon {
        async fn flush_request(&self) -> crate::Result<()> {
            let url = format!("{}/_flush", self.base_url)
                .parse::<hyper::Uri>()
                .unwrap();
            let mut builder = Request::post(&url);

            if let Some(credentials_provider) = &self.credentials {
                let mut request = self.signed_request("POST", &url, true);

                if let Some(ce) = self.compression.content_encoding() {
                    request.add_header("Content-Encoding", ce);
                }

                for (header, value) in &self.request.headers {
                    request.add_header(header, value);
                }

                builder = finish_signer(&mut request, &credentials_provider, builder).await?;
            } else {
                if let Some(ce) = self.compression.content_encoding() {
                    builder = builder.header("Content-Encoding", ce);
                }

                for (header, value) in &self.request.headers {
                    builder = builder.header(&header[..], &value[..]);
                }

                if let Some(auth) = &self.authorization {
                    builder = auth.apply_builder(builder);
                }
            }

            let request = builder.body(Body::empty())?;
            let client = HttpClient::new(self.tls_settings.clone())
                .expect("Could not build client to flush");
            let response = client.send(request).await?;

            match response.status() {
                StatusCode::OK => Ok(()),
                status => Err(HealthcheckError::UnexpectedStatus { status }.into()),
            }
        }
    }

    async fn flush(common: ElasticSearchCommon) -> crate::Result<()> {
        use tokio::time::{sleep, Duration};
        sleep(Duration::from_secs(2)).await;
        common.flush_request().await?;
        sleep(Duration::from_secs(2)).await;

        Ok(())
    }

    async fn create_template_index(common: &ElasticSearchCommon, name: &str) -> crate::Result<()> {
        let client = create_http_client();
        let uri = format!("{}/_index_template/{}", common.base_url, name);
        let response = client
            .put(uri)
            .json(&json!({
                "index_patterns": ["my-*-*"],
                "data_stream": {},
            }))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        Ok(())
    }

    #[test]
    fn ensure_pipeline_in_params() {
        let index = gen_index();
        let pipeline = String::from("test-pipeline");

        let config = ElasticSearchConfig {
            endpoint: "http://localhost:9200".into(),
            index: Some(index),
            pipeline: Some(pipeline.clone()),
            ..config()
        };
        let common = ElasticSearchCommon::parse_config(&config).expect("Config error");

        assert_eq!(common.query_params["pipeline"], pipeline);
    }

    #[tokio::test]
    async fn structures_events_correctly() {
        let index = gen_index();
        let config = ElasticSearchConfig {
            endpoint: "http://localhost:9200".into(),
            index: Some(index.clone()),
            doc_type: Some("log_lines".into()),
            id_key: Some("my_id".into()),
            compression: Compression::None,
            ..config()
        };
        let common = ElasticSearchCommon::parse_config(&config).expect("Config error");
        let base_url = common.base_url.clone();

        let cx = SinkContext::new_test();
        let (sink, _hc) = config.build(cx.clone()).await.unwrap();

        let (batch, mut receiver) = BatchNotifier::new_with_receiver();
        let mut input_event = LogEvent::from("raw log line").with_batch_notifier(&batch);
        input_event.insert("my_id", "42");
        input_event.insert("foo", "bar");
        drop(batch);

        let timestamp = input_event[crate::config::log_schema().timestamp_key()].clone();

        sink.run(stream::once(ready(input_event.into())))
            .await
            .unwrap();

        assert_eq!(receiver.try_recv(), Ok(BatchStatus::Delivered));

        // make sure writes all all visible
        flush(common).await.unwrap();

        let response = reqwest::Client::new()
            .get(&format!("{}/{}/_search", base_url, index))
            .json(&json!({
                "query": { "query_string": { "query": "*" } }
            }))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();

        let total = response["hits"]["total"]
            .as_u64()
            .or_else(|| response["hits"]["total"]["value"].as_u64())
            .expect("Elasticsearch response does not include hits->total nor hits->total->value");
        assert_eq!(1, total);

        let hits = response["hits"]["hits"]
            .as_array()
            .expect("Elasticsearch response does not include hits->hits");

        let hit = hits.iter().next().unwrap();
        assert_eq!("42", hit["_id"]);

        let value = hit
            .get("_source")
            .expect("Elasticsearch hit missing _source");
        assert_eq!(None, value["my_id"].as_str());

        let expected = json!({
            "message": "raw log line",
            "foo": "bar",
            "timestamp": timestamp,
        });
        assert_eq!(&expected, value);
    }

    #[tokio::test]
    async fn insert_events_over_http() {
        trace_init();

        run_insert_tests(
            ElasticSearchConfig {
                endpoint: "http://localhost:9200".into(),
                doc_type: Some("log_lines".into()),
                compression: Compression::None,
                ..config()
            },
            false,
            BatchStatus::Delivered,
        )
        .await;
    }

    #[tokio::test]
    async fn insert_events_over_https() {
        trace_init();

        run_insert_tests(
            ElasticSearchConfig {
                auth: Some(ElasticSearchAuth::Basic {
                    user: "elastic".into(),
                    password: "vector".into(),
                }),
                endpoint: "https://localhost:9201".into(),
                doc_type: Some("log_lines".into()),
                compression: Compression::None,
                tls: Some(TlsOptions {
                    ca_file: Some(tls::TEST_PEM_CA_PATH.into()),
                    ..Default::default()
                }),
                ..config()
            },
            false,
            BatchStatus::Delivered,
        )
        .await;
    }

    #[tokio::test]
    async fn insert_events_on_aws() {
        trace_init();

        run_insert_tests(
            ElasticSearchConfig {
                auth: Some(ElasticSearchAuth::Aws(AwsAuthentication::Default {})),
                endpoint: "http://localhost:4571".into(),
                ..config()
            },
            false,
            BatchStatus::Delivered,
        )
        .await;
    }

    #[tokio::test]
    async fn insert_events_on_aws_with_compression() {
        trace_init();

        run_insert_tests(
            ElasticSearchConfig {
                auth: Some(ElasticSearchAuth::Aws(AwsAuthentication::Default {})),
                endpoint: "http://localhost:4571".into(),
                compression: Compression::gzip_default(),
                ..config()
            },
            false,
            BatchStatus::Delivered,
        )
        .await;
    }

    #[tokio::test]
    async fn insert_events_with_failure() {
        trace_init();

        run_insert_tests(
            ElasticSearchConfig {
                endpoint: "http://localhost:9200".into(),
                doc_type: Some("log_lines".into()),
                compression: Compression::None,
                ..config()
            },
            true,
            BatchStatus::Failed,
        )
        .await;
    }

    #[tokio::test]
    async fn insert_events_in_data_stream() {
        trace_init();
        let template_index = format!("my-template-{}", gen_index());
        let stream_index = format!("my-stream-{}", gen_index());

        let cfg = ElasticSearchConfig {
            endpoint: "http://localhost:9200".into(),
            mode: ElasticSearchMode::DataStream,
            index: Some(stream_index.clone()),
            ..config()
        };
        let common = ElasticSearchCommon::parse_config(&cfg).expect("Config error");

        create_template_index(&common, &template_index)
            .await
            .expect("Template index creation error");

        create_data_stream(&common, &stream_index)
            .await
            .expect("Data stream creation error");

        run_insert_tests_with_config(&cfg, false, BatchStatus::Delivered).await;
    }

    async fn run_insert_tests(
        mut config: ElasticSearchConfig,
        break_events: bool,
        status: BatchStatus,
    ) {
        config.index = Some(gen_index());
        run_insert_tests_with_config(&config, break_events, status).await;
    }

    fn create_http_client() -> reqwest::Client {
        let mut test_ca = Vec::<u8>::new();
        File::open(tls::TEST_PEM_CA_PATH)
            .unwrap()
            .read_to_end(&mut test_ca)
            .unwrap();
        let test_ca = reqwest::Certificate::from_pem(&test_ca).unwrap();

        reqwest::Client::builder()
            .add_root_certificate(test_ca)
            .danger_accept_invalid_certs(true)
            .build()
            .expect("Could not build HTTP client")
    }

    async fn run_insert_tests_with_config(
        config: &ElasticSearchConfig,
        break_events: bool,
        batch_status: BatchStatus,
    ) {
        let common = ElasticSearchCommon::parse_config(&config).expect("Config error");
        let index = match config.mode {
            // Data stream mode uses an index name generated from the event.
            ElasticSearchMode::DataStream => format!(
                "{}",
                Utc::now().format(".ds-logs-generic-default-%Y.%m.%d-000001")
            ),
            ElasticSearchMode::Normal => config.index.clone().unwrap(),
        };
        let base_url = common.base_url.clone();

        let cx = SinkContext::new_test();
        let (sink, healthcheck) = config
            .build(cx.clone())
            .await
            .expect("Building config failed");

        healthcheck.await.expect("Health check failed");

        let (batch, mut receiver) = BatchNotifier::new_with_receiver();
        let (input, events) = random_events_with_stream(100, 100, Some(batch));
        let events = events.map(|mut event| {
            event.as_mut_log().insert("@timestamp", chrono::Utc::now());
            event
        });
        if break_events {
            // Break all but the first event to simulate some kind of partial failure
            let mut doit = false;
            sink.run(events.map(move |mut event| {
                if doit {
                    event.as_mut_log().insert("_type", 1);
                }
                doit = true;
                event
            }))
            .await
            .expect("Sending events failed");
        } else {
            sink.run(events).await.expect("Sending events failed");
        }

        assert_eq!(receiver.try_recv(), Ok(batch_status));

        // make sure writes all all visible
        flush(common).await.expect("Flushing writes failed");

        let client = create_http_client();
        let mut response = client
            .get(&format!("{}/{}/_search", base_url, index))
            .basic_auth("elastic", Some("vector"))
            .json(&json!({
                "query": { "query_string": { "query": "*" } }
            }))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();

        let total = response["hits"]["total"]["value"]
            .as_u64()
            .or_else(|| response["hits"]["total"].as_u64())
            .expect("Elasticsearch response does not include hits->total nor hits->total->value");

        if break_events {
            assert_ne!(input.len() as u64, total);
        } else {
            assert_eq!(input.len() as u64, total);

            let hits = response["hits"]["hits"]
                .as_array_mut()
                .expect("Elasticsearch response does not include hits->hits");
            let input = input
                .into_iter()
                .map(|rec| serde_json::to_value(&rec.into_log()).unwrap())
                .collect::<Vec<_>>();
            for hit in hits {
                let hit = hit
                    .get_mut("_source")
                    .expect("Elasticsearch hit missing _source");
                hit.as_object_mut().unwrap().remove("data_stream");
                hit.as_object_mut().unwrap().remove("@timestamp");
                assert!(input.contains(&hit));
            }
        }
    }

    fn gen_index() -> String {
        format!("test-{}", random_string(10).to_lowercase())
    }

    async fn create_data_stream(common: &ElasticSearchCommon, name: &str) -> crate::Result<()> {
        let client = create_http_client();
        let uri = format!("{}/_data_stream/{}", common.base_url, name);
        let response = client
            .put(uri)
            .header("Content-Type", "application/json")
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        Ok(())
    }

    fn config() -> ElasticSearchConfig {
        ElasticSearchConfig {
            batch: BatchConfig {
                max_events: Some(1),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}
