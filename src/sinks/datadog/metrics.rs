use super::healthcheck;
use crate::{
    config::{DataType, ProxyConfig, SinkConfig, SinkContext, SinkDescription},
    event::metric::{Metric, MetricKind, MetricValue, Sample, StatisticKind},
    event::Event,
    http::HttpClient,
    sinks::{
        util::{
            batch::{BatchConfig, BatchSettings},
            buffer::metrics::{MetricNormalize, MetricNormalizer, MetricSet, MetricsBuffer},
            encode_namespace,
            http::{HttpBatchService, HttpRetryLogic},
            EncodedEvent, PartitionBatchSink, PartitionBuffer, PartitionInnerBuffer,
            TowerRequestConfig,
        },
        Healthcheck, UriParseError, VectorSink,
    },
};
use chrono::{DateTime, Utc};
use futures::{stream, FutureExt, SinkExt};
use http::{uri::InvalidUri, Request, Uri};
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap},
    future::ready,
    sync::atomic::{AtomicI64, Ordering::SeqCst},
};

#[derive(Debug, Snafu)]
enum BuildError {
    #[snafu(display("Invalid host {:?}: {:?}", host, source))]
    InvalidHost { host: String, source: InvalidUri },
}

#[derive(Clone)]
struct DatadogState {
    last_sent_timestamp: i64,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct DatadogConfig {
    #[serde(alias = "namespace")]
    pub default_namespace: Option<String>,
    // Deprecated name
    #[serde(alias = "host")]
    pub endpoint: Option<String>,
    // Deprecated, replaced by the site option
    pub region: Option<super::Region>,
    pub site: Option<String>,
    pub api_key: String,
    #[serde(default)]
    pub batch: BatchConfig,
    #[serde(default)]
    pub request: TowerRequestConfig,
    #[serde(
        default,
        skip_serializing_if = "crate::serde::skip_serializing_if_default"
    )]
    pub proxy: ProxyConfig,
}

struct DatadogSink {
    config: DatadogConfig,
    /// Endpoint -> (uri_path, last_sent_timestamp)
    endpoint_data: HashMap<DatadogEndpoint, (Uri, AtomicI64)>,
}

lazy_static! {
    static ref REQUEST_DEFAULTS: TowerRequestConfig = TowerRequestConfig {
        retry_attempts: Some(5),
        ..Default::default()
    };
}

// https://docs.datadoghq.com/api/?lang=bash#post-timeseries-points
#[derive(Debug, Clone, PartialEq, Serialize)]
struct DatadogRequest<T> {
    series: Vec<T>,
}

impl DatadogConfig {
    fn get_endpoint(&self) -> String {
        self.endpoint.clone().unwrap_or_else(|| {
            // Follow the official Datadog agent convention:
            // <sender-id>.agent.<site>
            let version = str::replace(crate::built_info::PKG_VERSION, ".", "-");
            format!("https://{}-vector.agent.{}", version, &self.get_site())
        })
    }

    // The API endpoint is used for healtcheck/API key validation, it is derived from the `site` or `region` option
    // the same way the offical Datadog Agent does but the `endpoint` option still override both the main and the
    // API endpoint.
    fn get_api_endpoint(&self) -> String {
        self.endpoint
            .clone()
            .unwrap_or_else(|| format!("https://api.{}", &self.get_site()))
    }

    fn get_site(&self) -> &str {
        self.site.as_deref().unwrap_or_else(|| match self.region {
            Some(super::Region::Eu) => "datadoghq.eu",
            None | Some(super::Region::Us) => "datadoghq.com",
        })
    }
}

// https://github.com/DataDog/datadogpy/blob/1f143ab875e5994a94345ed373ac308c9f69b0ec/datadog/api/distributions.py#L9-L11
#[derive(Debug, Clone, PartialEq, Serialize)]
struct DatadogDistributionMetric {
    metric: String,
    interval: Option<i64>,
    points: Vec<DatadogPoint<Vec<f64>>>,
    tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct DatadogMetric {
    metric: String,
    r#type: DatadogMetricType,
    interval: Option<i64>,
    points: Vec<DatadogPoint<f64>>,
    tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DatadogMetricType {
    Gauge,
    Count,
    Rate,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct DatadogPoint<T>(i64, T);

#[derive(Debug, Clone, PartialEq)]
struct DatadogStats {
    min: f64,
    max: f64,
    median: f64,
    avg: f64,
    sum: f64,
    count: f64,
    quantiles: Vec<(f64, f64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum DatadogEndpoint {
    Series,
    Distribution,
}

impl DatadogEndpoint {
    fn build_uri(host: &str) -> crate::Result<Vec<(Self, Uri)>> {
        Ok(vec![
            (DatadogEndpoint::Series, build_uri(host, "/api/v1/series")?),
            (
                DatadogEndpoint::Distribution,
                build_uri(host, "/api/v1/distribution_points")?,
            ),
        ])
    }

    fn from_metric(metric: &Metric) -> Self {
        match metric.value() {
            MetricValue::Distribution {
                statistic: StatisticKind::Summary,
                ..
            } => Self::Distribution,
            _ => Self::Series,
        }
    }
}

inventory::submit! {
    SinkDescription::new::<DatadogConfig>("datadog_metrics")
}

impl_generate_config_from_default!(DatadogConfig);

#[async_trait::async_trait]
#[typetag::serde(name = "datadog_metrics")]
impl SinkConfig for DatadogConfig {
    async fn build(&self, cx: SinkContext) -> crate::Result<(VectorSink, Healthcheck)> {
        let proxy = cx.globals.proxy.build(&self.proxy);
        let client = HttpClient::new(None, proxy)?;
        let healthcheck = healthcheck(
            self.get_api_endpoint(),
            self.api_key.clone(),
            client.clone(),
        )
        .boxed();

        let batch = BatchSettings::default()
            .events(20)
            .timeout(1)
            .parse_config(self.batch)?;
        let request = self.request.unwrap_with(&REQUEST_DEFAULTS);

        let uri = DatadogEndpoint::build_uri(&self.get_endpoint())?;
        let timestamp = Utc::now().timestamp();

        let sink = DatadogSink {
            config: self.clone(),
            endpoint_data: uri
                .into_iter()
                .map(|(endpoint, uri)| (endpoint, (uri, AtomicI64::new(timestamp))))
                .collect(),
        };

        let svc = request.service(
            HttpRetryLogic,
            HttpBatchService::new(client, move |request| ready(sink.build_request(request))),
        );

        let buffer = PartitionBuffer::new(MetricsBuffer::new(batch.size));
        let mut normalizer = MetricNormalizer::<DatadogMetricNormalize>::default();

        let svc_sink = PartitionBatchSink::new(svc, buffer, batch.timeout, cx.acker())
            .sink_map_err(|error| error!(message = "Fatal datadog metric sink error.", %error))
            .with_flat_map(move |event: Event| {
                stream::iter(normalizer.apply(event).map(encode_metric))
            });

        Ok((VectorSink::Sink(Box::new(svc_sink)), healthcheck))
    }

    fn input_type(&self) -> DataType {
        DataType::Metric
    }

    fn sink_type(&self) -> &'static str {
        "datadog_metrics"
    }
}

fn encode_metric(
    metric: Metric,
) -> Result<EncodedEvent<PartitionInnerBuffer<Metric, DatadogEndpoint>>, ()> {
    let endpoint = DatadogEndpoint::from_metric(&metric);
    // TODO: Avoiding this clone requires rewriting MetricsBuffer to
    // accept separated MetricSeries and MetricData values, which in
    // turn requires rewriting all metrics sinks. See Issue #6045
    let metadata = metric.metadata().clone();
    Ok(EncodedEvent {
        item: PartitionInnerBuffer::new(metric, endpoint),
        metadata: Some(metadata),
    })
}

impl DatadogSink {
    fn build_request(
        &self,
        events: PartitionInnerBuffer<Vec<Metric>, DatadogEndpoint>,
    ) -> crate::Result<Request<Vec<u8>>> {
        let (events, endpoint) = events.into_parts();
        let endpoint_data = self
            .endpoint_data
            .get(&endpoint)
            .expect("The endpoint doesn't have data.");

        let now = Utc::now().timestamp();
        let interval = now - endpoint_data.1.load(SeqCst);
        endpoint_data.1.store(now, SeqCst);

        let body = match endpoint {
            DatadogEndpoint::Series => {
                let input =
                    encode_events(events, self.config.default_namespace.as_deref(), interval);
                serde_json::to_vec(&input).unwrap()
            }
            DatadogEndpoint::Distribution => {
                let input = encode_distribution_events(
                    events,
                    self.config.default_namespace.as_deref(),
                    interval,
                );
                serde_json::to_vec(&input).unwrap()
            }
        };

        Request::post(endpoint_data.0.clone())
            .header("Content-Type", "application/json")
            .header("DD-API-KEY", self.config.api_key.clone())
            .body(body)
            .map_err(Into::into)
    }
}

fn build_uri(host: &str, endpoint: &'static str) -> crate::Result<Uri> {
    let uri = format!("{}{}", host, endpoint)
        .parse::<Uri>()
        .context(UriParseError)?;

    Ok(uri)
}

fn encode_tags(tags: &BTreeMap<String, String>) -> Vec<String> {
    let mut pairs: Vec<_> = tags
        .iter()
        .map(|(name, value)| format!("{}:{}", name, value))
        .collect();
    pairs.sort();
    pairs
}

fn encode_timestamp(timestamp: Option<DateTime<Utc>>) -> i64 {
    if let Some(ts) = timestamp {
        ts.timestamp()
    } else {
        Utc::now().timestamp()
    }
}

fn stats(source: &[Sample]) -> Option<DatadogStats> {
    let mut samples = Vec::new();
    for sample in source {
        for _ in 0..sample.rate {
            samples.push(sample.value);
        }
    }

    if samples.is_empty() {
        return None;
    }

    if samples.len() == 1 {
        let val = samples[0];
        return Some(DatadogStats {
            min: val,
            max: val,
            median: val,
            avg: val,
            sum: val,
            count: 1.0,
            quantiles: vec![(0.95, val)],
        });
    }

    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));

    let length = samples.len() as f64;
    let min = samples.first().unwrap();
    let max = samples.last().unwrap();

    let p50 = samples[(0.50 * length - 1.0).round() as usize];
    let p95 = samples[(0.95 * length - 1.0).round() as usize];

    let sum = samples.iter().sum();
    let avg = sum / length;

    Some(DatadogStats {
        min: *min,
        max: *max,
        median: p50,
        avg,
        sum,
        count: length,
        quantiles: vec![(0.95, p95)],
    })
}

struct DatadogMetricNormalize;

impl MetricNormalize for DatadogMetricNormalize {
    fn apply_state(state: &mut MetricSet, metric: Metric) -> Option<Metric> {
        match &metric.value() {
            MetricValue::Gauge { .. } => state.make_absolute(metric),
            _ => state.make_incremental(metric),
        }
    }
}

fn encode_events(
    events: Vec<Metric>,
    default_namespace: Option<&str>,
    interval: i64,
) -> DatadogRequest<DatadogMetric> {
    debug!(message = "Series.", count = events.len());
    let series = events
        .into_iter()
        .filter_map(|event| {
            let fullname =
                encode_namespace(event.namespace().or(default_namespace), '.', event.name());
            let ts = encode_timestamp(event.timestamp());
            let tags = event.tags().map(encode_tags);
            // DatadogMetricNormalize converts these to the right MetricKind
            match event.value() {
                MetricValue::Counter { value } => Some(vec![DatadogMetric {
                    metric: fullname,
                    r#type: DatadogMetricType::Count,
                    interval: Some(interval),
                    points: vec![DatadogPoint(ts, *value)],
                    tags,
                }]),
                MetricValue::Distribution {
                    samples,
                    statistic: StatisticKind::Histogram,
                } => {
                    // https://docs.datadoghq.com/developers/metrics/metrics_type/?tab=histogram#metric-type-definition
                    if let Some(s) = stats(samples) {
                        let mut result = vec![
                            DatadogMetric {
                                metric: format!("{}.min", &fullname),
                                r#type: DatadogMetricType::Gauge,
                                interval: Some(interval),
                                points: vec![DatadogPoint(ts, s.min)],
                                tags: tags.clone(),
                            },
                            DatadogMetric {
                                metric: format!("{}.avg", &fullname),
                                r#type: DatadogMetricType::Gauge,
                                interval: Some(interval),
                                points: vec![DatadogPoint(ts, s.avg)],
                                tags: tags.clone(),
                            },
                            DatadogMetric {
                                metric: format!("{}.count", &fullname),
                                r#type: DatadogMetricType::Rate,
                                interval: Some(interval),
                                points: vec![DatadogPoint(ts, s.count)],
                                tags: tags.clone(),
                            },
                            DatadogMetric {
                                metric: format!("{}.median", &fullname),
                                r#type: DatadogMetricType::Gauge,
                                interval: Some(interval),
                                points: vec![DatadogPoint(ts, s.median)],
                                tags: tags.clone(),
                            },
                            DatadogMetric {
                                metric: format!("{}.max", &fullname),
                                r#type: DatadogMetricType::Gauge,
                                interval: Some(interval),
                                points: vec![DatadogPoint(ts, s.max)],
                                tags: tags.clone(),
                            },
                        ];
                        for (q, v) in s.quantiles {
                            result.push(DatadogMetric {
                                metric: format!("{}.{}percentile", &fullname, (q * 100.0) as u32),
                                r#type: DatadogMetricType::Gauge,
                                interval: Some(interval),
                                points: vec![DatadogPoint(ts, v)],
                                tags: tags.clone(),
                            })
                        }
                        Some(result)
                    } else {
                        None
                    }
                }
                MetricValue::Set { values } => Some(vec![DatadogMetric {
                    metric: fullname,
                    r#type: DatadogMetricType::Gauge,
                    interval: None,
                    points: vec![DatadogPoint(ts, values.len() as f64)],
                    tags,
                }]),
                MetricValue::Gauge { value } => Some(vec![DatadogMetric {
                    metric: fullname,
                    r#type: DatadogMetricType::Gauge,
                    interval: None,
                    points: vec![DatadogPoint(ts, *value)],
                    tags,
                }]),
                _ => None,
            }
        })
        .flatten()
        .collect::<Vec<_>>();

    DatadogRequest { series }
}

fn encode_distribution_events(
    events: Vec<Metric>,
    default_namespace: Option<&str>,
    interval: i64,
) -> DatadogRequest<DatadogDistributionMetric> {
    debug!(message = "Distribution.", count = events.len());
    let series = events
        .into_iter()
        .filter_map(|event| {
            let fullname =
                encode_namespace(event.namespace().or(default_namespace), '.', event.name());
            let ts = encode_timestamp(event.timestamp());
            let tags = event.tags().map(encode_tags);
            match event.kind() {
                MetricKind::Incremental => match event.value() {
                    MetricValue::Distribution {
                        samples,
                        statistic: StatisticKind::Summary,
                    } => {
                        let samples = samples
                            .iter()
                            .map(|sample| (0..sample.rate).map(move |_| sample.value))
                            .flatten()
                            .collect::<Vec<_>>();

                        if samples.is_empty() {
                            None
                        } else {
                            Some(DatadogDistributionMetric {
                                metric: fullname,
                                interval: Some(interval),
                                points: vec![DatadogPoint(ts, samples)],
                                tags,
                            })
                        }
                    }
                    _ => None,
                },
                _ => None,
            }
        })
        .collect();

    DatadogRequest { series }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{event::metric::Sample, sinks::util::test::load_sink};
    use chrono::offset::TimeZone;
    use http::Method;
    use pretty_assertions::assert_eq;
    use regex::Regex;
    use std::sync::atomic::AtomicI64;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<DatadogConfig>();
    }

    fn ts() -> DateTime<Utc> {
        Utc.ymd(2018, 11, 14).and_hms_nano(8, 9, 10, 11)
    }

    fn tags() -> BTreeMap<String, String> {
        vec![
            ("normal_tag".to_owned(), "value".to_owned()),
            ("true_tag".to_owned(), "true".to_owned()),
            ("empty_tag".to_owned(), "".to_owned()),
        ]
        .into_iter()
        .collect()
    }

    #[tokio::test]
    async fn test_request() {
        let (sink, _cx) = load_sink::<DatadogConfig>(
            r#"
            site = "us3.datadoghq.com"
            api_key = "test"
        "#,
        )
        .unwrap();

        let timestamp = Utc::now().timestamp();
        let uri = DatadogEndpoint::build_uri(&sink.get_endpoint()).unwrap();
        let sink = DatadogSink {
            config: sink,
            endpoint_data: uri
                .into_iter()
                .map(|(endpoint, uri)| (endpoint, (uri, AtomicI64::new(timestamp))))
                .collect(),
        };

        let events = vec![
            Metric::new(
                "total",
                MetricKind::Incremental,
                MetricValue::Counter { value: 1.5 },
            )
            .with_namespace(Some("test")),
            Metric::new(
                "check",
                MetricKind::Incremental,
                MetricValue::Counter { value: 1.0 },
            )
            .with_namespace(Some("test"))
            .with_tags(Some(tags()))
            .with_timestamp(Some(ts())),
            Metric::new(
                "unsupported",
                MetricKind::Absolute,
                MetricValue::Counter { value: 1.0 },
            )
            .with_namespace(Some("test"))
            .with_tags(Some(tags()))
            .with_timestamp(Some(ts())),
        ];
        let req = sink
            .build_request(PartitionInnerBuffer::new(events, DatadogEndpoint::Series))
            .unwrap();

        assert_eq!(req.method(), Method::POST);
        let uri_validator =
            Regex::new(r"^https://\d+-\d+-\d+-vector.agent.us3.datadoghq.com/api/v1/series$")
                .unwrap();
        assert!(uri_validator.is_match(&req.uri().to_string()));
    }

    #[test]
    fn test_encode_tags() {
        assert_eq!(
            encode_tags(&tags()),
            vec!["empty_tag:", "normal_tag:value", "true_tag:true"]
        );
    }

    #[test]
    fn test_encode_timestamp() {
        assert_eq!(encode_timestamp(None), Utc::now().timestamp());
        assert_eq!(encode_timestamp(Some(ts())), 1542182950);
    }

    #[test]
    fn encode_counter() {
        let interval = 60;
        let events = vec![
            Metric::new(
                "total",
                MetricKind::Incremental,
                MetricValue::Counter { value: 1.5 },
            )
            .with_namespace(Some("ns"))
            .with_timestamp(Some(ts())),
            Metric::new(
                "check",
                MetricKind::Incremental,
                MetricValue::Counter { value: 1.0 },
            )
            .with_namespace(Some("ns"))
            .with_tags(Some(tags()))
            .with_timestamp(Some(ts())),
        ];
        let input = encode_events(events, None, interval);
        let json = serde_json::to_string(&input).unwrap();

        assert_eq!(
            json,
            r#"{"series":[{"metric":"ns.total","type":"count","interval":60,"points":[[1542182950,1.5]],"tags":null},{"metric":"ns.check","type":"count","interval":60,"points":[[1542182950,1.0]],"tags":["empty_tag:","normal_tag:value","true_tag:true"]}]}"#
        );
    }

    #[test]
    fn encode_gauge() {
        let events = vec![Metric::new(
            "volume",
            MetricKind::Absolute,
            MetricValue::Gauge { value: -1.1 },
        )
        .with_timestamp(Some(ts()))];
        let input = encode_events(events, None, 60);
        let json = serde_json::to_string(&input).unwrap();

        assert_eq!(
            json,
            r#"{"series":[{"metric":"volume","type":"gauge","interval":null,"points":[[1542182950,-1.1]],"tags":null}]}"#
        );
    }

    #[test]
    fn encode_set() {
        let events = vec![Metric::new(
            "users",
            MetricKind::Incremental,
            MetricValue::Set {
                values: vec!["alice".into(), "bob".into()].into_iter().collect(),
            },
        )
        .with_timestamp(Some(ts()))];
        let input = encode_events(events, Some("ns"), 60);
        let json = serde_json::to_string(&input).unwrap();

        assert_eq!(
            json,
            r#"{"series":[{"metric":"ns.users","type":"gauge","interval":null,"points":[[1542182950,2.0]],"tags":null}]}"#
        );
    }

    #[test]
    fn test_dense_stats() {
        // https://github.com/DataDog/dd-agent/blob/master/tests/core/test_histogram.py
        let samples: Vec<_> = (0..20)
            .map(|v| Sample {
                value: f64::from(v),
                rate: 1,
            })
            .collect();

        assert_eq!(
            stats(&samples),
            Some(DatadogStats {
                min: 0.0,
                max: 19.0,
                median: 9.0,
                avg: 9.5,
                sum: 190.0,
                count: 20.0,
                quantiles: vec![(0.95, 18.0)],
            })
        );
    }

    #[test]
    fn test_sparse_stats() {
        let samples: Vec<_> = (1..5)
            .map(|v| Sample {
                value: f64::from(v),
                rate: v,
            })
            .collect();

        assert_eq!(
            stats(&samples),
            Some(DatadogStats {
                min: 1.0,
                max: 4.0,
                median: 3.0,
                avg: 3.0,
                sum: 30.0,
                count: 10.0,
                quantiles: vec![(0.95, 4.0)],
            })
        );
    }

    #[test]
    fn test_single_value_stats() {
        let samples = vector_core::samples![10.0 => 1];

        assert_eq!(
            stats(&samples),
            Some(DatadogStats {
                min: 10.0,
                max: 10.0,
                median: 10.0,
                avg: 10.0,
                sum: 10.0,
                count: 1.0,
                quantiles: vec![(0.95, 10.0)],
            })
        );
    }
    #[test]
    fn test_nan_stats() {
        let samples = vector_core::samples![1.0 => 1, std::f64::NAN => 1];
        assert!(stats(&samples).is_some());
    }

    #[test]
    fn test_empty_stats() {
        let samples = vec![];
        assert!(stats(&samples).is_none());
    }

    #[test]
    fn test_zero_counts_stats() {
        let samples = vector_core::samples![1.0 => 0, 2.0 => 0];
        assert!(stats(&samples).is_none());
    }

    #[test]
    fn encode_distribution() {
        // https://docs.datadoghq.com/developers/metrics/metrics_type/?tab=histogram#metric-type-definition
        let events = vec![Metric::new(
            "requests",
            MetricKind::Incremental,
            MetricValue::Distribution {
                samples: vector_core::samples![1.0 => 3, 2.0 => 3, 3.0 => 2],
                statistic: StatisticKind::Histogram,
            },
        )
        .with_timestamp(Some(ts()))];
        let input = encode_events(events, None, 60);
        let json = serde_json::to_string(&input).unwrap();

        assert_eq!(
            json,
            r#"{"series":[{"metric":"requests.min","type":"gauge","interval":60,"points":[[1542182950,1.0]],"tags":null},{"metric":"requests.avg","type":"gauge","interval":60,"points":[[1542182950,1.875]],"tags":null},{"metric":"requests.count","type":"rate","interval":60,"points":[[1542182950,8.0]],"tags":null},{"metric":"requests.median","type":"gauge","interval":60,"points":[[1542182950,2.0]],"tags":null},{"metric":"requests.max","type":"gauge","interval":60,"points":[[1542182950,3.0]],"tags":null},{"metric":"requests.95percentile","type":"gauge","interval":60,"points":[[1542182950,3.0]],"tags":null}]}"#
        );
    }

    #[test]
    fn encode_datadog_distribution() {
        // https://docs.datadoghq.com/developers/metrics/types/?tab=distribution#definition
        let events = vec![Metric::new(
            "requests",
            MetricKind::Incremental,
            MetricValue::Distribution {
                samples: vector_core::samples![1.0 => 3, 2.0 => 3, 3.0 => 2],
                statistic: StatisticKind::Summary,
            },
        )
        .with_timestamp(Some(ts()))];
        let input = encode_distribution_events(events, None, 60);
        let json = serde_json::to_string(&input).unwrap();

        assert_eq!(
            json,
            r#"{"series":[{"metric":"requests","interval":60,"points":[[1542182950,[1.0,1.0,1.0,2.0,2.0,2.0,3.0,3.0]]],"tags":null}]}"#
        );
    }
}
