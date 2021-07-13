use super::collector::{self, MetricCollector as _};
use crate::{
    config::{self, ProxyConfig, SinkConfig, SinkDescription},
    event::{Event, Metric},
    http::{Auth, HttpClient},
    internal_events::TemplateRenderingFailed,
    sinks::{
        self,
        util::{
            batch::{BatchConfig, BatchSettings},
            buffer::metrics::{MetricNormalize, MetricNormalizer, MetricSet, MetricsBuffer},
            http::HttpRetryLogic,
            EncodedEvent, PartitionBatchSink, PartitionBuffer, PartitionInnerBuffer,
            TowerRequestConfig,
        },
    },
    template::Template,
    tls::{TlsOptions, TlsSettings},
};
use bytes::{Bytes, BytesMut};
use futures::{future::BoxFuture, stream, FutureExt, SinkExt};
use http::Uri;
use prost::Message;
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use std::task;
use tower::ServiceBuilder;

#[derive(Debug, Snafu)]
enum Errors {
    #[snafu(display(r#"Prometheus remote_write sink cannot accept "set" metrics"#))]
    SetMetricInvalid,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteWriteConfig {
    pub endpoint: String,

    pub default_namespace: Option<String>,

    #[serde(default = "super::default_histogram_buckets")]
    pub buckets: Vec<f64>,
    #[serde(default = "super::default_summary_quantiles")]
    pub quantiles: Vec<f64>,

    #[serde(default)]
    pub batch: BatchConfig,
    #[serde(default)]
    pub request: TowerRequestConfig,

    #[serde(default)]
    pub tenant_id: Option<Template>,

    pub tls: Option<TlsOptions>,

    pub auth: Option<Auth>,
    #[serde(
        default,
        skip_serializing_if = "crate::serde::skip_serializing_if_default"
    )]
    pub proxy: ProxyConfig,
}

inventory::submit! {
    SinkDescription::new::<RemoteWriteConfig>("prometheus_remote_write")
}

lazy_static::lazy_static! {
    static ref REQUEST_DEFAULTS: TowerRequestConfig = Default::default();
}

impl_generate_config_from_default!(RemoteWriteConfig);

#[async_trait::async_trait]
#[typetag::serde(name = "prometheus_remote_write")]
impl SinkConfig for RemoteWriteConfig {
    async fn build(
        &self,
        cx: config::SinkContext,
    ) -> crate::Result<(sinks::VectorSink, sinks::Healthcheck)> {
        let endpoint = self.endpoint.parse::<Uri>().context(sinks::UriParseError)?;
        let tls_settings = TlsSettings::from_options(&self.tls)?;
        let batch = BatchSettings::default()
            .events(1_000)
            .timeout(1)
            .parse_config(self.batch)?;
        let request = self.request.unwrap_with(&REQUEST_DEFAULTS);
        let buckets = self.buckets.clone();
        let quantiles = self.quantiles.clone();

        let proxy = cx.globals.proxy.build(&self.proxy);
        let client = HttpClient::new(tls_settings, proxy)?;
        let tenant_id = self.tenant_id.clone();
        let auth = self.auth.clone();

        let healthcheck = healthcheck(endpoint.clone(), client.clone()).boxed();
        let service = RemoteWriteService {
            endpoint,
            default_namespace: self.default_namespace.clone(),
            client,
            buckets,
            quantiles,
            auth,
        };

        let sink = {
            let service = request.service(HttpRetryLogic, service);
            let service = ServiceBuilder::new().service(service);
            let buffer = PartitionBuffer::new(MetricsBuffer::new(batch.size));
            let mut normalizer = MetricNormalizer::<PrometheusMetricNormalize>::default();

            PartitionBatchSink::new(service, buffer, batch.timeout, cx.acker())
                .with_flat_map(move |event: Event| {
                    stream::iter(normalizer.apply(event).map(|event| {
                        let tenant_id = tenant_id.as_ref().and_then(|template| {
                            template
                                .render_string(&event)
                                .map_err(|error| {
                                    emit!(TemplateRenderingFailed {
                                        error,
                                        field: Some("tenant_id"),
                                        drop_event: false,
                                    })
                                })
                                .ok()
                        });
                        let key = PartitionKey { tenant_id };
                        Ok(EncodedEvent::new(PartitionInnerBuffer::new(event, key)))
                    }))
                })
                .sink_map_err(
                    |error| error!(message = "Prometheus remote_write sink error.", %error),
                )
        };

        Ok((sinks::VectorSink::Sink(Box::new(sink)), healthcheck))
    }

    fn input_type(&self) -> config::DataType {
        config::DataType::Metric
    }

    fn sink_type(&self) -> &'static str {
        "prometheus_remote_write"
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct PartitionKey {
    tenant_id: Option<String>,
}

async fn healthcheck(endpoint: Uri, client: HttpClient) -> crate::Result<()> {
    let request = http::Request::get(endpoint)
        .body(hyper::Body::empty())
        .unwrap();

    let response = client.send(request).await?;

    match response.status() {
        http::StatusCode::OK => Ok(()),
        other => Err(sinks::HealthcheckError::UnexpectedStatus { status: other }.into()),
    }
}
pub struct PrometheusMetricNormalize;

impl MetricNormalize for PrometheusMetricNormalize {
    fn apply_state(state: &mut MetricSet, metric: Metric) -> Option<Metric> {
        state.make_absolute(metric)
    }
}

#[derive(Clone)]
struct RemoteWriteService {
    endpoint: Uri,
    default_namespace: Option<String>,
    client: HttpClient,
    buckets: Vec<f64>,
    quantiles: Vec<f64>,
    auth: Option<Auth>,
}

impl RemoteWriteService {
    fn encode_events(&self, metrics: Vec<Metric>) -> Bytes {
        let mut time_series = collector::TimeSeries::new();
        for metric in metrics {
            time_series.encode_metric(
                self.default_namespace.as_deref(),
                &self.buckets,
                &self.quantiles,
                false,
                &metric,
            );
        }
        let request = time_series.finish();

        let mut out = BytesMut::with_capacity(request.encoded_len());
        request.encode(&mut out).expect("Out of memory");
        out.freeze()
    }
}

impl tower::Service<PartitionInnerBuffer<Vec<Metric>, PartitionKey>> for RemoteWriteService {
    type Response = http::Response<Bytes>;
    type Error = crate::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _task: &mut task::Context<'_>) -> task::Poll<Result<(), Self::Error>> {
        task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, buffer: PartitionInnerBuffer<Vec<Metric>, PartitionKey>) -> Self::Future {
        let (events, key) = buffer.into_parts();
        let body = self.encode_events(events);
        let body = snap_block(body);

        let mut builder = http::Request::post(self.endpoint.clone())
            .header("X-Prometheus-Remote-Write-Version", "0.1.0")
            .header("Content-Encoding", "snappy")
            .header("Content-Type", "application/x-protobuf");
        if let Some(tenant_id) = key.tenant_id {
            builder = builder.header("X-Scope-OrgID", tenant_id);
        }

        let mut request = builder.body(body.into()).unwrap();
        if let Some(auth) = &self.auth {
            auth.apply(&mut request);
        }
        let client = self.client.clone();

        Box::pin(async move {
            let response = client.send(request).await?;
            let (parts, body) = response.into_parts();
            let body = hyper::body::to_bytes(body).await?;
            Ok(hyper::Response::from_parts(parts, body))
        })
    }
}

fn snap_block(data: Bytes) -> Vec<u8> {
    snap::raw::Encoder::new()
        .compress_vec(&data)
        .expect("Out of memory")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::SinkContext,
        event::{MetricKind, MetricValue},
        sinks::util::test::build_test_server,
        test_util,
    };
    use futures::StreamExt;
    use http::HeaderMap;
    use indoc::indoc;
    use prometheus_parser::proto;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<RemoteWriteConfig>();
    }

    macro_rules! labels {
        ( $( $name:expr => $value:expr ),* ) => {
            vec![ $( proto::Label {
                name: $name.to_string(),
                value: $value.to_string()
            }, )* ]
        }
    }

    #[tokio::test]
    async fn sends_request() {
        let outputs = send_request("", vec![create_event("gauge-2".into(), 32.0)]).await;
        assert_eq!(outputs.len(), 1);
        let (headers, req) = &outputs[0];

        assert!(!headers.contains_key("x-scope-orgid"));

        assert_eq!(req.timeseries.len(), 1);
        assert_eq!(
            req.timeseries[0].labels,
            labels!("__name__" => "gauge-2", "production" => "true", "region" => "us-west-1")
        );
        assert_eq!(req.timeseries[0].samples.len(), 1);
        assert_eq!(req.timeseries[0].samples[0].value, 32.0);
        assert_eq!(req.metadata.len(), 1);
        assert_eq!(req.metadata[0].r#type, proto::MetricType::Gauge as i32);
        assert_eq!(req.metadata[0].metric_family_name, "gauge-2");
    }

    #[tokio::test]
    async fn sends_authenticated_request() {
        let outputs = send_request(
            indoc! {r#"
                tenant_id = "tenant-%Y"
                [auth]
                strategy = "basic"
                user = "user"
                password = "password"
            "#},
            vec![create_event("gauge-2".into(), 32.0)],
        )
        .await;

        assert_eq!(outputs.len(), 1);
        let (_headers, req) = &outputs[0];

        assert_eq!(req.timeseries.len(), 1);
        assert_eq!(
            req.timeseries[0].labels,
            labels!("__name__" => "gauge-2", "production" => "true", "region" => "us-west-1")
        );
        assert_eq!(req.timeseries[0].samples.len(), 1);
        assert_eq!(req.timeseries[0].samples[0].value, 32.0);
        assert_eq!(req.metadata.len(), 1);
        assert_eq!(req.metadata[0].r#type, proto::MetricType::Gauge as i32);
        assert_eq!(req.metadata[0].metric_family_name, "gauge-2");
    }

    #[tokio::test]
    async fn sends_x_scope_orgid_header() {
        let outputs = send_request(
            r#"tenant_id = "tenant""#,
            vec![create_event("gauge-3".into(), 12.0)],
        )
        .await;

        assert_eq!(outputs.len(), 1);
        let (headers, _) = &outputs[0];
        assert_eq!(headers["x-scope-orgid"], "tenant");
    }

    #[tokio::test]
    async fn sends_templated_x_scope_orgid_header() {
        let outputs = send_request(
            r#"tenant_id = "tenant-%Y""#,
            vec![create_event("gauge-3".into(), 12.0)],
        )
        .await;

        assert_eq!(outputs.len(), 1);
        let (headers, _) = &outputs[0];
        let orgid = headers["x-scope-orgid"]
            .to_str()
            .expect("Missing x-scope-orgid header");
        assert!(orgid.starts_with("tenant-20"));
        assert_eq!(orgid.len(), 11);
    }

    #[tokio::test]
    async fn retains_state_between_requests() {
        // This sink converts all incremental events to absolute, and
        // should accumulate their totals between batches.
        let outputs = send_request(
            r#"batch.max_events = 1"#,
            vec![
                create_inc_event("counter-1".into(), 12.0),
                create_inc_event("counter-2".into(), 13.0),
                create_inc_event("counter-1".into(), 14.0),
            ],
        )
        .await;

        assert_eq!(outputs.len(), 3);

        let check_output = |index: usize, name: &str, value: f64| {
            let (_, req) = &outputs[index];
            assert_eq!(req.timeseries.len(), 1);
            assert_eq!(req.timeseries[0].labels, labels!("__name__" => name));
            assert_eq!(req.timeseries[0].samples.len(), 1);
            assert_eq!(req.timeseries[0].samples[0].value, value);
        };
        check_output(0, "counter-1", 12.0);
        check_output(1, "counter-2", 13.0);
        check_output(2, "counter-1", 26.0);
    }

    async fn send_request(
        config: &str,
        events: Vec<Event>,
    ) -> Vec<(HeaderMap, proto::WriteRequest)> {
        let addr = test_util::next_addr();
        let (rx, trigger, server) = build_test_server(addr);
        tokio::spawn(server);

        let config = format!("endpoint = \"http://{}/write\"\n{}", addr, config);
        let config: RemoteWriteConfig = toml::from_str(&config).unwrap();
        let cx = SinkContext::new_test();

        let (sink, _) = config.build(cx).await.unwrap();
        sink.run(stream::iter(events)).await.unwrap();

        drop(trigger);

        rx.map(|(parts, body)| {
            assert_eq!(parts.method, "POST");
            assert_eq!(parts.uri.path(), "/write");
            let headers = parts.headers;
            assert_eq!(headers["x-prometheus-remote-write-version"], "0.1.0");
            assert_eq!(headers["content-encoding"], "snappy");
            assert_eq!(headers["content-type"], "application/x-protobuf");

            if config.auth.is_some() {
                assert!(headers.contains_key("authorization"));
            }

            let decoded = snap::raw::Decoder::new()
                .decompress_vec(&body)
                .expect("Invalid snappy compressed data");
            let request =
                proto::WriteRequest::decode(Bytes::from(decoded)).expect("Invalid protobuf");
            (headers, request)
        })
        .collect::<Vec<_>>()
        .await
    }

    pub(super) fn create_event(name: String, value: f64) -> Event {
        Metric::new(name, MetricKind::Absolute, MetricValue::Gauge { value })
            .with_tags(Some(
                vec![
                    ("region".to_owned(), "us-west-1".to_owned()),
                    ("production".to_owned(), "true".to_owned()),
                ]
                .into_iter()
                .collect(),
            ))
            .with_timestamp(Some(chrono::Utc::now()))
            .into()
    }

    fn create_inc_event(name: String, value: f64) -> Event {
        Metric::new(
            name,
            MetricKind::Incremental,
            MetricValue::Counter { value },
        )
        .with_timestamp(Some(chrono::Utc::now()))
        .into()
    }
}

#[cfg(all(test, feature = "prometheus-integration-tests"))]
mod integration_tests {
    use super::tests::*;
    use super::*;
    use crate::{
        config::{SinkConfig, SinkContext},
        event::metric::MetricValue,
        event::Event,
        sinks::influxdb::test_util::{cleanup_v1, format_timestamp, onboarding_v1, query_v1},
        tls::{self, TlsOptions},
    };
    use futures::stream;
    use serde_json::Value;
    use std::{collections::HashMap, ops::Range};

    const HTTP_URL: &str = "http://localhost:8086";
    const HTTPS_URL: &str = "https://localhost:8087";

    #[tokio::test]
    async fn insert_metrics_over_http() {
        insert_metrics(HTTP_URL).await;
    }

    #[tokio::test]
    async fn insert_metrics_over_https() {
        insert_metrics(HTTPS_URL).await;
    }

    async fn insert_metrics(url: &str) {
        crate::test_util::trace_init();

        let database = onboarding_v1(url).await;

        let cx = SinkContext::new_test();

        let config = RemoteWriteConfig {
            endpoint: format!("{}/api/v1/prom/write?db={}", url, database),
            tls: Some(TlsOptions {
                ca_file: Some(tls::TEST_PEM_CA_PATH.into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let events = create_events(0..5, |n| n * 11.0);

        let (sink, _) = config.build(cx).await.expect("error building config");
        sink.run(stream::iter(events.clone())).await.unwrap();

        let result = query(url, &format!("show series on {}", database)).await;

        let values = &result["results"][0]["series"][0]["values"];
        assert_eq!(values.as_array().unwrap().len(), 5);

        for event in events {
            let metric = event.into_metric();
            let result = query(
                url,
                &format!(r#"SELECT * FROM "{}".."{}""#, database, metric.name()),
            )
            .await;

            let metrics = decode_metrics(&result["results"][0]["series"][0]);
            assert_eq!(metrics.len(), 1);
            let output = &metrics[0];

            match metric.value() {
                MetricValue::Gauge { value } => {
                    assert_eq!(output["value"], Value::Number((*value as u32).into()))
                }
                _ => panic!("Unhandled metric value, fix the test"),
            }
            for (tag, value) in metric.tags().unwrap() {
                assert_eq!(output[&tag[..]], Value::String(value.to_string()));
            }
            let timestamp =
                format_timestamp(metric.timestamp().unwrap(), chrono::SecondsFormat::Millis);
            assert_eq!(output["time"], Value::String(timestamp));
        }

        cleanup_v1(url, &database).await;
    }

    async fn query(url: &str, query: &str) -> Value {
        let result = query_v1(url, query).await;
        let text = result.text().await.unwrap();
        serde_json::from_str(&text).expect("error when parsing InfluxDB response JSON")
    }

    fn decode_metrics(data: &Value) -> Vec<HashMap<String, Value>> {
        let data = data.as_object().expect("Data is not an object");
        let columns = data["columns"].as_array().expect("Columns is not an array");
        data["values"]
            .as_array()
            .expect("Values is not an array")
            .iter()
            .map(|values| {
                columns
                    .iter()
                    .zip(values.as_array().unwrap().iter())
                    .map(|(column, value)| (column.as_str().unwrap().to_owned(), value.clone()))
                    .collect()
            })
            .collect()
    }

    fn create_events(name_range: Range<i32>, value: impl Fn(f64) -> f64) -> Vec<Event> {
        name_range
            .map(move |num| create_event(format!("metric_{}", num), value(num as f64)))
            .collect()
    }
}
