use crate::{
    buffers::Acker,
    config::{DataType, GenerateConfig, Resource, SinkConfig, SinkContext, SinkDescription},
    event::metric::{Metric, MetricData, MetricKind, MetricValue},
    event::Event,
    internal_events::PrometheusServerRequestComplete,
    sinks::{
        util::{statistic::validate_quantiles, StreamSink},
        Healthcheck, VectorSink,
    },
    tls::{MaybeTlsSettings, TlsConfig},
};
use async_trait::async_trait;
use chrono::Utc;
use futures::{future, stream::BoxStream, FutureExt, StreamExt};
use hyper::{
    header::HeaderValue,
    service::{make_service_fn, service_fn},
    Body, Method, Request, Response, Server, StatusCode,
};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use snafu::Snafu;
use std::{
    convert::Infallible,
    hash::{Hash, Hasher},
    mem::discriminant,
    net::SocketAddr,
    ops::{Deref, DerefMut},
    sync::{Arc, RwLock},
};
use stream_cancel::{Trigger, Tripwire};

use super::collector::{self, MetricCollector as _};

const MIN_FLUSH_PERIOD_SECS: u64 = 1;

#[derive(Debug, Snafu)]
enum BuildError {
    #[snafu(display("Flush period for sets must be greater or equal to {} secs", min))]
    FlushPeriodTooShort { min: u64 },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrometheusExporterConfig {
    #[serde(alias = "namespace")]
    pub default_namespace: Option<String>,
    #[serde(default = "default_address")]
    pub address: SocketAddr,
    pub tls: Option<TlsConfig>,
    #[serde(default = "super::default_histogram_buckets")]
    pub buckets: Vec<f64>,
    #[serde(default = "super::default_summary_quantiles")]
    pub quantiles: Vec<f64>,
    #[serde(default = "default_flush_period_secs")]
    pub flush_period_secs: u64,
}

impl std::default::Default for PrometheusExporterConfig {
    fn default() -> Self {
        Self {
            default_namespace: None,
            address: default_address(),
            tls: None,
            buckets: super::default_histogram_buckets(),
            quantiles: super::default_summary_quantiles(),
            flush_period_secs: default_flush_period_secs(),
        }
    }
}

fn default_address() -> SocketAddr {
    use std::net::{IpAddr, Ipv4Addr};

    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 9598)
}

fn default_flush_period_secs() -> u64 {
    60
}

inventory::submit! {
    SinkDescription::new::<PrometheusExporterConfig>("prometheus")
}

inventory::submit! {
    SinkDescription::new::<PrometheusExporterConfig>("prometheus_exporter")
}

impl GenerateConfig for PrometheusExporterConfig {
    fn generate_config() -> toml::Value {
        toml::Value::try_from(&Self::default()).unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "prometheus_exporter")]
impl SinkConfig for PrometheusExporterConfig {
    async fn build(&self, cx: SinkContext) -> crate::Result<(VectorSink, Healthcheck)> {
        if self.flush_period_secs < MIN_FLUSH_PERIOD_SECS {
            return Err(Box::new(BuildError::FlushPeriodTooShort {
                min: MIN_FLUSH_PERIOD_SECS,
            }));
        }

        validate_quantiles(&self.quantiles)?;

        let sink = PrometheusExporter::new(self.clone(), cx.acker());
        let healthcheck = future::ok(()).boxed();

        Ok((VectorSink::Stream(Box::new(sink)), healthcheck))
    }

    fn input_type(&self) -> DataType {
        DataType::Metric
    }

    fn sink_type(&self) -> &'static str {
        "prometheus_exporter"
    }

    fn resources(&self) -> Vec<Resource> {
        vec![Resource::tcp(self.address)]
    }
}

// Add a compatibility alias to avoid breaking existing configs
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PrometheusCompatConfig {
    #[serde(flatten)]
    config: PrometheusExporterConfig,
}

#[async_trait::async_trait]
#[typetag::serde(name = "prometheus")]
impl SinkConfig for PrometheusCompatConfig {
    async fn build(&self, cx: SinkContext) -> crate::Result<(VectorSink, Healthcheck)> {
        self.config.build(cx).await
    }

    fn input_type(&self) -> DataType {
        self.config.input_type()
    }

    fn sink_type(&self) -> &'static str {
        "prometheus"
    }

    fn resources(&self) -> Vec<Resource> {
        self.config.resources()
    }
}

struct PrometheusExporter {
    server_shutdown_trigger: Option<Trigger>,
    config: PrometheusExporterConfig,
    metrics: Arc<RwLock<ExpiringMetrics>>,
    acker: Acker,
}

struct ExpiringMetrics {
    map: IndexMap<MetricEntry, bool>,
    last_flush_timestamp: i64,
}

fn handle(
    req: Request<Body>,
    default_namespace: Option<&str>,
    buckets: &[f64],
    quantiles: &[f64],
    expired: bool,
    metrics: &IndexMap<MetricEntry, bool>,
) -> Response<Body> {
    let mut response = Response::new(Body::empty());

    match (req.method(), req.uri().path()) {
        (&Method::GET, "/metrics") => {
            let mut s = collector::StringCollector::new();

            for (MetricEntry(metric), _) in metrics {
                s.encode_metric(default_namespace, buckets, quantiles, expired, metric);
            }

            *response.body_mut() = s.finish().into();

            response.headers_mut().insert(
                "Content-Type",
                HeaderValue::from_static("text/plain; version=0.0.4"),
            );
        }
        _ => {
            *response.status_mut() = StatusCode::NOT_FOUND;
        }
    }

    response
}

impl PrometheusExporter {
    fn new(config: PrometheusExporterConfig, acker: Acker) -> Self {
        Self {
            server_shutdown_trigger: None,
            config,
            metrics: Arc::new(RwLock::new(ExpiringMetrics {
                map: IndexMap::new(),
                last_flush_timestamp: Utc::now().timestamp(),
            })),
            acker,
        }
    }

    async fn start_server_if_needed(&mut self) {
        if self.server_shutdown_trigger.is_some() {
            return;
        }

        let metrics = Arc::clone(&self.metrics);
        let default_namespace = self.config.default_namespace.clone();
        let buckets = self.config.buckets.clone();
        let quantiles = self.config.quantiles.clone();
        let flush_period_secs = self.config.flush_period_secs;

        let new_service = make_service_fn(move |_| {
            let metrics = Arc::clone(&metrics);
            let default_namespace = default_namespace.clone();
            let buckets = buckets.clone();
            let quantiles = quantiles.clone();
            let flush_period_secs = flush_period_secs;

            async move {
                Ok::<_, Infallible>(service_fn(move |req| {
                    let metrics = metrics.read().unwrap();
                    let interval = (Utc::now().timestamp() - metrics.last_flush_timestamp) as u64;
                    let expired = interval > flush_period_secs;

                    let response = info_span!(
                        "prometheus_server",
                        method = ?req.method(),
                        path = ?req.uri().path(),
                    )
                    .in_scope(|| {
                        handle(
                            req,
                            default_namespace.as_deref(),
                            &buckets,
                            &quantiles,
                            expired,
                            &metrics.map,
                        )
                    });

                    emit!(PrometheusServerRequestComplete {
                        status_code: response.status(),
                    });

                    future::ok::<_, Infallible>(response)
                }))
            }
        });

        let (trigger, tripwire) = Tripwire::new();

        let tls = self.config.tls.clone();
        let address = self.config.address;

        tokio::spawn(async move {
            let tls = MaybeTlsSettings::from_config(&tls, true)
                .map_err(|error| eprintln!("Server TLS error: {}", error))?;
            let listener = tls
                .bind(&address)
                .await
                .map_err(|error| eprintln!("Server bind error: {}", error))?;

            Server::builder(hyper::server::accept::from_stream(listener.accept_stream()))
                .serve(new_service)
                .with_graceful_shutdown(tripwire.then(crate::stream::tripwire_handler))
                .await
                .map_err(|error| eprintln!("Server error: {}", error))?;

            Ok::<(), ()>(())
        });

        self.server_shutdown_trigger = Some(trigger);
    }
}

#[async_trait]
impl StreamSink for PrometheusExporter {
    async fn run(&mut self, mut input: BoxStream<'_, Event>) -> Result<(), ()> {
        self.start_server_if_needed().await;
        while let Some(event) = input.next().await {
            let item = event.into_metric();
            let mut metrics = self.metrics.write().unwrap();

            // sets need to be expired from time to time
            // because otherwise they could grow infinitelly
            let now = Utc::now().timestamp();
            let interval = now - metrics.last_flush_timestamp;
            if interval > self.config.flush_period_secs as i64 {
                metrics.last_flush_timestamp = now;

                metrics.map = metrics
                    .map
                    .drain(..)
                    .map(|(MetricEntry(mut metric), is_incremental_set)| {
                        if is_incremental_set {
                            metric.zero();
                        }
                        (MetricEntry(metric), is_incremental_set)
                    })
                    .collect();
            }

            match item.kind() {
                MetricKind::Incremental => {
                    let mut entry = MetricEntry(item.into_absolute());
                    if let Some((MetricEntry(mut existing), _)) = metrics.map.remove_entry(&entry) {
                        if existing.update(&entry) {
                            entry = MetricEntry(existing);
                        } else {
                            warn!(message = "Metric changed type, dropping old value.", series = %entry.series());
                        }
                    }
                    let is_set = matches!(entry.value(), MetricValue::Set { .. });
                    metrics.map.insert(entry, is_set);
                }
                MetricKind::Absolute => {
                    let new = MetricEntry(item);
                    metrics.map.remove(&new);
                    metrics.map.insert(new, false);
                }
            };

            self.acker.ack(1);
        }
        Ok(())
    }
}

struct MetricEntry(Metric);

impl Deref for MetricEntry {
    type Target = Metric;
    fn deref(&self) -> &Metric {
        &self.0
    }
}

impl DerefMut for MetricEntry {
    fn deref_mut(&mut self) -> &mut Metric {
        &mut self.0
    }
}

impl AsRef<MetricData> for MetricEntry {
    fn as_ref(&self) -> &MetricData {
        self.0.as_ref()
    }
}

impl Eq for MetricEntry {}

impl Hash for MetricEntry {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let metric = &self.0;
        metric.series().hash(state);
        metric.kind().hash(state);
        discriminant(metric.value()).hash(state);

        match metric.value() {
            MetricValue::AggregatedHistogram { buckets, .. } => {
                for bucket in buckets {
                    bucket.upper_limit.to_bits().hash(state);
                }
            }
            MetricValue::AggregatedSummary { quantiles, .. } => {
                for quantile in quantiles {
                    quantile.upper_limit.to_bits().hash(state);
                }
            }
            _ => {}
        }
    }
}

impl PartialEq for MetricEntry {
    fn eq(&self, other: &Self) -> bool {
        // This differs from a straightforward implementation of `eq` by
        // comparing only the "shape" bits (name, tags, and type) while
        // allowing the contained values to be different.
        self.series() == other.series()
            && self.kind() == other.kind()
            && discriminant(self.value()) == discriminant(other.value())
            && match (self.value(), other.value()) {
                (
                    MetricValue::AggregatedHistogram {
                        buckets: buckets1, ..
                    },
                    MetricValue::AggregatedHistogram {
                        buckets: buckets2, ..
                    },
                ) => {
                    buckets1.len() == buckets2.len()
                        && buckets1
                            .iter()
                            .zip(buckets2.iter())
                            .all(|(b1, b2)| b1.upper_limit == b2.upper_limit)
                }
                (
                    MetricValue::AggregatedSummary {
                        quantiles: quantiles1,
                        ..
                    },
                    MetricValue::AggregatedSummary {
                        quantiles: quantiles2,
                        ..
                    },
                ) => {
                    quantiles1.len() == quantiles2.len()
                        && quantiles1
                            .iter()
                            .zip(quantiles2.iter())
                            .all(|(q1, q2)| q1.upper_limit == q2.upper_limit)
                }
                _ => true,
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::ProxyConfig,
        event::metric::{Metric, MetricValue},
        http::HttpClient,
        test_util::{next_addr, random_string, trace_init},
        tls::MaybeTlsSettings,
    };
    use chrono::Duration;
    use indoc::indoc;
    use pretty_assertions::assert_eq;
    use tokio::{sync::mpsc, time};
    use tokio_stream::wrappers::UnboundedReceiverStream;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<PrometheusExporterConfig>();
    }

    #[tokio::test]
    async fn prometheus_notls() {
        export_and_fetch_simple(None).await;
    }

    #[tokio::test]
    async fn prometheus_tls() {
        let mut tls_config = TlsConfig::test_config();
        tls_config.options.verify_hostname = Some(false);
        export_and_fetch_simple(Some(tls_config)).await;
    }

    #[tokio::test]
    async fn updates_timestamps() {
        let timestamp1 = Utc::now();
        let (name, event1) = create_metric_gauge(None, 123.4);
        let event1 = Event::from(event1.into_metric().with_timestamp(Some(timestamp1)));
        let (_, event2) = create_metric_gauge(Some(name.clone()), 12.0);
        let timestamp2 = timestamp1 + Duration::seconds(1);
        let event2 = Event::from(event2.into_metric().with_timestamp(Some(timestamp2)));
        let events = vec![event1, event2];

        let body = export_and_fetch(None, events).await;
        let timestamp = timestamp2.timestamp_millis();
        assert_eq!(
            body,
            format!(
                indoc! {r#"
                    # HELP {name} {name}
                    # TYPE {name} gauge
                    {name}{{some_tag="some_value"}} 135.4 {timestamp}
                "#},
                name = name,
                timestamp = timestamp
            )
        );
    }

    async fn export_and_fetch(tls_config: Option<TlsConfig>, events: Vec<Event>) -> String {
        trace_init();

        let client_settings = MaybeTlsSettings::from_config(&tls_config, false).unwrap();
        let proto = match &tls_config {
            Some(_) => "https",
            None => "http",
        };

        let address = next_addr();
        let config = PrometheusExporterConfig {
            address,
            tls: tls_config,
            ..Default::default()
        };
        let (sink, _) = config.build(SinkContext::new_test()).await.unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(sink.run(Box::pin(UnboundedReceiverStream::new(rx))));

        for event in events {
            tx.send(event).expect("Failed to send event.");
        }

        time::sleep(time::Duration::from_millis(100)).await;

        let request = Request::get(format!("{}://{}/metrics", proto, address))
            .body(Body::empty())
            .expect("Error creating request.");
        let result = HttpClient::new(client_settings, ProxyConfig::default())
            .unwrap()
            .send(request)
            .await
            .expect("Could not fetch query");

        assert!(result.status().is_success());

        let body = result.into_body();
        let bytes = hyper::body::to_bytes(body)
            .await
            .expect("Reading body failed");
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    async fn export_and_fetch_simple(tls_config: Option<TlsConfig>) {
        let (name1, event1) = create_metric_gauge(None, 123.4);
        let (name2, event2) = tests::create_metric_set(None, vec!["0", "1", "2"]);
        let events = vec![event1, event2];

        let body = export_and_fetch(tls_config, events).await;

        assert!(body.contains(&format!(
            indoc! {r#"
               # HELP {name} {name}
               # TYPE {name} gauge
               {name}{{some_tag="some_value"}} 123.4
            "#},
            name = name1
        )));
        assert!(body.contains(&format!(
            indoc! {r#"
               # HELP {name} {name}
               # TYPE {name} gauge
               {name}{{some_tag="some_value"}} 3
            "#},
            name = name2
        )));
    }

    pub fn create_metric_gauge(name: Option<String>, value: f64) -> (String, Event) {
        create_metric(name, MetricValue::Gauge { value })
    }

    pub fn create_metric_set(name: Option<String>, values: Vec<&'static str>) -> (String, Event) {
        create_metric(
            name,
            MetricValue::Set {
                values: values.into_iter().map(Into::into).collect(),
            },
        )
    }

    pub fn create_metric(name: Option<String>, value: MetricValue) -> (String, Event) {
        let name = name.unwrap_or_else(|| format!("vector_set_{}", random_string(16)));
        let event = Metric::new(name.clone(), MetricKind::Incremental, value)
            .with_tags(Some(
                vec![("some_tag".to_owned(), "some_value".to_owned())]
                    .into_iter()
                    .collect(),
            ))
            .into();
        (name, event)
    }

    #[tokio::test]
    async fn sink_absolute() {
        let config = PrometheusExporterConfig {
            address: next_addr(), // Not actually bound, just needed to fill config
            tls: None,
            ..Default::default()
        };
        let cx = SinkContext::new_test();

        let mut sink = PrometheusExporter::new(config, cx.acker());

        let m1 = Metric::new(
            "absolute",
            MetricKind::Absolute,
            MetricValue::Counter { value: 32. },
        )
        .with_tags(Some(
            vec![("tag1".to_owned(), "value1".to_owned())]
                .into_iter()
                .collect(),
        ));

        let m2 = m1.clone().with_tags(Some(
            vec![("tag1".to_owned(), "value2".to_owned())]
                .into_iter()
                .collect(),
        ));

        let metrics = vec![
            Event::Metric(m1.clone().with_value(MetricValue::Counter { value: 32. })),
            Event::Metric(m2.clone().with_value(MetricValue::Counter { value: 33. })),
            Event::Metric(m1.clone().with_value(MetricValue::Counter { value: 40. })),
        ];

        sink.run(Box::pin(futures::stream::iter(metrics)))
            .await
            .unwrap();

        let map = &sink.metrics.read().unwrap().map;

        assert_eq!(
            map.get_full(&MetricEntry(m1)).unwrap().1.value(),
            &MetricValue::Counter { value: 40. }
        );

        assert_eq!(
            map.get_full(&MetricEntry(m2)).unwrap().1.value(),
            &MetricValue::Counter { value: 33. }
        );
    }
}

#[cfg(all(test, feature = "prometheus-integration-tests"))]
mod integration_tests {
    use super::*;
    use crate::{http::HttpClient, test_util::trace_init};
    use chrono::Utc;
    use serde_json::Value;
    use tokio::{sync::mpsc, time};
    use tokio_stream::wrappers::UnboundedReceiverStream;

    const PROMETHEUS_ADDRESS: &str = "127.0.0.1:9101";

    #[tokio::test]
    async fn prometheus_metrics() {
        trace_init();

        prometheus_scrapes_metrics().await;
        time::sleep(time::Duration::from_millis(500)).await;
        reset_on_flush_period().await;
    }

    async fn prometheus_scrapes_metrics() {
        let start = Utc::now().timestamp();

        let config = PrometheusExporterConfig {
            address: PROMETHEUS_ADDRESS.parse().unwrap(),
            ..Default::default()
        };
        let (sink, _) = config.build(SinkContext::new_test()).await.unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(sink.run(Box::pin(UnboundedReceiverStream::new(rx))));

        let (name, event) = tests::create_metric_gauge(None, 123.4);
        tx.send(event).expect("Failed to send.");

        // Wait a bit for the prometheus server to scrape the metrics
        time::sleep(time::Duration::from_secs(2)).await;

        // Now try to download them from prometheus
        let result = prometheus_query(&name).await;

        let data = &result["data"]["result"][0];
        assert_eq!(data["metric"]["__name__"], Value::String(name));
        assert_eq!(
            data["metric"]["instance"],
            Value::String(PROMETHEUS_ADDRESS.into())
        );
        assert_eq!(
            data["metric"]["some_tag"],
            Value::String("some_value".into())
        );
        assert!(data["value"][0].as_f64().unwrap() >= start as f64);
        assert_eq!(data["value"][1], Value::String("123.4".into()));
    }

    async fn reset_on_flush_period() {
        let config = PrometheusExporterConfig {
            address: PROMETHEUS_ADDRESS.parse().unwrap(),
            flush_period_secs: 3,
            ..Default::default()
        };
        let (sink, _) = config.build(SinkContext::new_test()).await.unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(sink.run(Box::pin(UnboundedReceiverStream::new(rx))));

        let (name1, event) = tests::create_metric_set(None, vec!["0", "1", "2"]);
        tx.send(event).expect("Failed to send.");
        let (name2, event) = tests::create_metric_set(None, vec!["3", "4", "5"]);
        tx.send(event).expect("Failed to send.");

        // Wait a bit for the prometheus server to scrape the metrics
        time::sleep(time::Duration::from_secs(2)).await;

        // Now try to download them from prometheus
        let result = prometheus_query(&name1).await;
        assert_eq!(
            result["data"]["result"][0]["value"][1],
            Value::String("3".into())
        );
        let result = prometheus_query(&name2).await;
        assert_eq!(
            result["data"]["result"][0]["value"][1],
            Value::String("3".into())
        );

        // Wait a bit for expired metrics
        time::sleep(time::Duration::from_secs(3)).await;

        let (name1, event) = tests::create_metric_set(Some(name1), vec!["6", "7"]);
        tx.send(event).expect("Failed to send.");
        let (name2, event) = tests::create_metric_set(Some(name2), vec!["8", "9"]);
        tx.send(event).expect("Failed to send.");

        // Wait a bit for the prometheus server to scrape the metrics
        time::sleep(time::Duration::from_secs(2)).await;

        // Now try to download them from prometheus
        let result = prometheus_query(&name1).await;
        assert_eq!(
            result["data"]["result"][0]["value"][1],
            Value::String("2".into())
        );
        let result = prometheus_query(&name2).await;
        assert_eq!(
            result["data"]["result"][0]["value"][1],
            Value::String("2".into())
        );
    }

    async fn prometheus_query(query: &str) -> Value {
        let url = format!("http://127.0.0.1:9090/api/v1/query?query={}", query);
        let request = Request::post(url)
            .body(Body::empty())
            .expect("Error creating request.");
        let result = HttpClient::new(None)
            .unwrap()
            .send(request)
            .await
            .expect("Could not fetch query");
        let result = hyper::body::to_bytes(result.into_body())
            .await
            .expect("Error fetching body");
        let result = String::from_utf8_lossy(&result);
        serde_json::from_str(result.as_ref()).expect("Invalid JSON from prometheus")
    }
}
