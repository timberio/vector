use crate::{
    config::{DataType, ProxyConfig, SinkConfig, SinkContext, SinkDescription},
    event::{
        metric::{Metric, MetricValue, Sample, StatisticKind},
        Event,
    },
    http::HttpClient,
    sinks::{
        influxdb::{
            encode_timestamp, healthcheck, influx_line_protocol, influxdb_settings, Field,
            InfluxDb1Settings, InfluxDb2Settings, ProtocolVersion,
        },
        util::{
            buffer::metrics::{MetricNormalize, MetricNormalizer, MetricSet, MetricsBuffer},
            encode_namespace,
            http::{HttpBatchService, HttpRetryLogic},
            sink,
            statistic::{validate_quantiles, DistributionStatistic},
            BatchConfig, BatchSettings, EncodedEvent, TowerRequestConfig,
        },
        Healthcheck, VectorSink,
    },
    tls::{TlsOptions, TlsSettings},
};
use bytes::Bytes;
use futures::{future::BoxFuture, stream, SinkExt};
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    future::ready,
    task::Poll,
};
use tower::Service;

#[derive(Clone)]
struct InfluxDbSvc {
    config: InfluxDbConfig,
    protocol_version: ProtocolVersion,
    inner: HttpBatchService<BoxFuture<'static, crate::Result<hyper::Request<Vec<u8>>>>>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct InfluxDbConfig {
    #[serde(alias = "namespace")]
    pub default_namespace: Option<String>,
    pub endpoint: String,
    #[serde(flatten)]
    pub influxdb1_settings: Option<InfluxDb1Settings>,
    #[serde(flatten)]
    pub influxdb2_settings: Option<InfluxDb2Settings>,
    #[serde(default)]
    pub batch: BatchConfig,
    #[serde(default)]
    pub request: TowerRequestConfig,
    pub tags: Option<HashMap<String, String>>,
    pub tls: Option<TlsOptions>,
    #[serde(
        default,
        skip_serializing_if = "crate::serde::skip_serializing_if_default"
    )]
    pub proxy: ProxyConfig,
    #[serde(default = "default_summary_quantiles")]
    pub quantiles: Vec<f64>,
}

pub fn default_summary_quantiles() -> Vec<f64> {
    vec![0.5, 0.75, 0.9, 0.95, 0.99]
}

lazy_static! {
    static ref REQUEST_DEFAULTS: TowerRequestConfig = TowerRequestConfig {
        retry_attempts: Some(5),
        ..Default::default()
    };
}

// https://v2.docs.influxdata.com/v2.0/write-data/#influxdb-api
#[derive(Debug, Clone, PartialEq, Serialize)]
struct InfluxDbRequest {
    series: Vec<String>,
}

inventory::submit! {
    SinkDescription::new::<InfluxDbConfig>("influxdb_metrics")
}

impl_generate_config_from_default!(InfluxDbConfig);

#[async_trait::async_trait]
#[typetag::serde(name = "influxdb_metrics")]
impl SinkConfig for InfluxDbConfig {
    async fn build(&self, cx: SinkContext) -> crate::Result<(VectorSink, Healthcheck)> {
        let tls_settings = TlsSettings::from_options(&self.tls)?;
        let proxy = cx.globals.proxy.build(&self.proxy);
        let client = HttpClient::new(tls_settings, proxy)?;
        let healthcheck = healthcheck(
            self.clone().endpoint,
            self.clone().influxdb1_settings,
            self.clone().influxdb2_settings,
            client.clone(),
        )?;
        validate_quantiles(&self.quantiles)?;
        let sink = InfluxDbSvc::new(self.clone(), cx, client)?;
        Ok((sink, healthcheck))
    }

    fn input_type(&self) -> DataType {
        DataType::Metric
    }

    fn sink_type(&self) -> &'static str {
        "influxdb_metrics"
    }
}

impl InfluxDbSvc {
    pub fn new(
        config: InfluxDbConfig,
        cx: SinkContext,
        client: HttpClient,
    ) -> crate::Result<VectorSink> {
        let settings = influxdb_settings(
            config.influxdb1_settings.clone(),
            config.influxdb2_settings.clone(),
        )?;

        let endpoint = config.endpoint.clone();
        let token = settings.token();
        let protocol_version = settings.protocol_version();

        let batch = BatchSettings::default()
            .events(20)
            .timeout(1)
            .parse_config(config.batch)?;
        let request = config.request.unwrap_with(&REQUEST_DEFAULTS);

        let uri = settings.write_uri(endpoint)?;

        let http_service = HttpBatchService::new(client, create_build_request(uri, token));

        let influxdb_http_service = InfluxDbSvc {
            config,
            protocol_version,
            inner: http_service,
        };
        let mut normalizer = MetricNormalizer::<InfluxMetricNormalize>::default();

        let sink = request
            .batch_sink(
                HttpRetryLogic,
                influxdb_http_service,
                MetricsBuffer::new(batch.size),
                batch.timeout,
                cx.acker(),
                sink::StdServiceLogic::default(),
            )
            .with_flat_map(move |event: Event| {
                stream::iter(
                    normalizer
                        .apply(event)
                        .map(|metric| Ok(EncodedEvent::new(metric))),
                )
            })
            .sink_map_err(|error| error!(message = "Fatal influxdb sink error.", %error));

        Ok(VectorSink::Sink(Box::new(sink)))
    }
}

impl Service<Vec<Metric>> for InfluxDbSvc {
    type Response = http::Response<Bytes>;
    type Error = crate::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut std::task::Context) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, items: Vec<Metric>) -> Self::Future {
        let input = encode_events(
            self.protocol_version,
            items,
            self.config.default_namespace.as_deref(),
            self.config.tags.as_ref(),
            &self.config.quantiles,
        );
        let body: Vec<u8> = input.into_bytes();

        self.inner.call(body)
    }
}

fn create_build_request(
    uri: http::Uri,
    token: String,
) -> impl Fn(Vec<u8>) -> BoxFuture<'static, crate::Result<hyper::Request<Vec<u8>>>> + Sync + Send + 'static
{
    let auth = format!("Token {}", token);
    move |body| {
        Box::pin(ready(
            hyper::Request::post(uri.clone())
                .header("Content-Type", "text/plain")
                .header("Authorization", auth.clone())
                .body(body)
                .map_err(Into::into),
        ))
    }
}

fn merge_tags(
    event: &Metric,
    tags: Option<&HashMap<String, String>>,
) -> Option<BTreeMap<String, String>> {
    match (event.tags().cloned(), tags) {
        (Some(mut event_tags), Some(config_tags)) => {
            event_tags.extend(config_tags.iter().map(|(k, v)| (k.clone(), v.clone())));
            Some(event_tags)
        }
        (Some(event_tags), None) => Some(event_tags),
        (None, Some(config_tags)) => Some(
            config_tags
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
        (None, None) => None,
    }
}

pub struct InfluxMetricNormalize;

impl MetricNormalize for InfluxMetricNormalize {
    fn apply_state(state: &mut MetricSet, metric: Metric) -> Option<Metric> {
        match (metric.kind(), &metric.value()) {
            // Counters are disaggregated. We take the previous value from the state
            // and emit the difference between previous and current as a Counter
            (_, MetricValue::Counter { .. }) => state.make_incremental(metric),
            // Convert incremental gauges into absolute ones
            (_, MetricValue::Gauge { .. }) => state.make_absolute(metric),
            // All others are left as-is
            _ => Some(metric),
        }
    }
}

fn encode_events(
    protocol_version: ProtocolVersion,
    events: Vec<Metric>,
    default_namespace: Option<&str>,
    tags: Option<&HashMap<String, String>>,
    quantiles: &[f64],
) -> String {
    let mut output = String::new();
    for event in events.into_iter() {
        let fullname = encode_namespace(event.namespace().or(default_namespace), '.', event.name());
        let ts = encode_timestamp(event.timestamp());
        let tags = merge_tags(&event, tags);
        let (metric_type, fields) = get_type_and_fields(event.value(), quantiles);

        if let Err(error) = influx_line_protocol(
            protocol_version,
            fullname,
            metric_type,
            tags,
            fields,
            ts,
            &mut output,
        ) {
            warn!(message = "Failed to encode event; dropping event.", %error, internal_log_rate_secs = 30);
        };
    }

    // remove last '\n'
    output.pop();
    output
}

fn get_type_and_fields(
    value: &MetricValue,
    quantiles: &[f64],
) -> (&'static str, Option<HashMap<String, Field>>) {
    match value {
        MetricValue::Counter { value } => ("counter", Some(to_fields(*value))),
        MetricValue::Gauge { value } => ("gauge", Some(to_fields(*value))),
        MetricValue::Set { values } => ("set", Some(to_fields(values.len() as f64))),
        MetricValue::AggregatedHistogram {
            buckets,
            count,
            sum,
        } => {
            let mut fields: HashMap<String, Field> = buckets
                .iter()
                .map(|sample| {
                    (
                        format!("bucket_{}", sample.upper_limit),
                        Field::UnsignedInt(sample.count),
                    )
                })
                .collect();
            fields.insert("count".to_owned(), Field::UnsignedInt(*count));
            fields.insert("sum".to_owned(), Field::Float(*sum));

            ("histogram", Some(fields))
        }
        MetricValue::AggregatedSummary {
            quantiles,
            count,
            sum,
        } => {
            let mut fields: HashMap<String, Field> = quantiles
                .iter()
                .map(|quantile| {
                    (
                        format!("quantile_{}", quantile.upper_limit),
                        Field::Float(quantile.value),
                    )
                })
                .collect();
            fields.insert("count".to_owned(), Field::UnsignedInt(*count));
            fields.insert("sum".to_owned(), Field::Float(*sum));

            ("summary", Some(fields))
        }
        MetricValue::Distribution { samples, statistic } => {
            let quantiles = match statistic {
                StatisticKind::Histogram => &[0.95] as &[_],
                StatisticKind::Summary => quantiles,
            };
            let fields = encode_distribution(samples, quantiles);
            ("distribution", fields)
        }
    }
}

fn encode_distribution(samples: &[Sample], quantiles: &[f64]) -> Option<HashMap<String, Field>> {
    let statistic = DistributionStatistic::from_samples(samples, quantiles)?;

    let fields: HashMap<String, Field> = vec![
        ("min".to_owned(), Field::Float(statistic.min)),
        ("max".to_owned(), Field::Float(statistic.max)),
        ("median".to_owned(), Field::Float(statistic.median)),
        ("avg".to_owned(), Field::Float(statistic.avg)),
        ("sum".to_owned(), Field::Float(statistic.sum)),
        ("count".to_owned(), Field::Float(statistic.count as f64)),
    ]
    .into_iter()
    .chain(
        statistic
            .quantiles
            .iter()
            .map(|&(p, val)| (format!("quantile_{:.2}", p), Field::Float(val))),
    )
    .collect();

    Some(fields)
}

fn to_fields(value: f64) -> HashMap<String, Field> {
    let fields: HashMap<String, Field> = vec![("value".to_owned(), Field::Float(value))]
        .into_iter()
        .collect();
    fields
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::metric::{Metric, MetricKind, MetricValue, StatisticKind};
    use crate::sinks::influxdb::test_util::{assert_fields, split_line_protocol, tags, ts};
    use pretty_assertions::assert_eq;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<InfluxDbConfig>();
    }

    #[test]
    fn test_encode_counter() {
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

        let line_protocols = encode_events(ProtocolVersion::V2, events, Some("vector"), None, &[]);
        assert_eq!(
            line_protocols,
            "ns.total,metric_type=counter value=1.5 1542182950000000011\n\
            ns.check,metric_type=counter,normal_tag=value,true_tag=true value=1 1542182950000000011"
        );
    }

    #[test]
    fn test_encode_gauge() {
        let events = vec![Metric::new(
            "meter",
            MetricKind::Incremental,
            MetricValue::Gauge { value: -1.5 },
        )
        .with_namespace(Some("ns"))
        .with_tags(Some(tags()))
        .with_timestamp(Some(ts()))];

        let line_protocols = encode_events(ProtocolVersion::V2, events, None, None, &[]);
        assert_eq!(
            line_protocols,
            "ns.meter,metric_type=gauge,normal_tag=value,true_tag=true value=-1.5 1542182950000000011"
        );
    }

    #[test]
    fn test_encode_set() {
        let events = vec![Metric::new(
            "users",
            MetricKind::Incremental,
            MetricValue::Set {
                values: vec!["alice".into(), "bob".into()].into_iter().collect(),
            },
        )
        .with_namespace(Some("ns"))
        .with_tags(Some(tags()))
        .with_timestamp(Some(ts()))];

        let line_protocols = encode_events(ProtocolVersion::V2, events, None, None, &[]);
        assert_eq!(
            line_protocols,
            "ns.users,metric_type=set,normal_tag=value,true_tag=true value=2 1542182950000000011"
        );
    }

    #[test]
    fn test_encode_histogram_v1() {
        let events = vec![Metric::new(
            "requests",
            MetricKind::Absolute,
            MetricValue::AggregatedHistogram {
                buckets: vector_core::buckets![1.0 => 1, 2.1 => 2, 3.0 => 3],
                count: 6,
                sum: 12.5,
            },
        )
        .with_namespace(Some("ns"))
        .with_tags(Some(tags()))
        .with_timestamp(Some(ts()))];

        let line_protocols = encode_events(ProtocolVersion::V1, events, None, None, &[]);
        let line_protocols: Vec<&str> = line_protocols.split('\n').collect();
        assert_eq!(line_protocols.len(), 1);

        let line_protocol1 = split_line_protocol(line_protocols[0]);
        assert_eq!("ns.requests", line_protocol1.0);
        assert_eq!(
            "metric_type=histogram,normal_tag=value,true_tag=true",
            line_protocol1.1
        );
        assert_fields(
            line_protocol1.2.to_string(),
            [
                "bucket_1=1i",
                "bucket_2.1=2i",
                "bucket_3=3i",
                "count=6i",
                "sum=12.5",
            ]
            .to_vec(),
        );
        assert_eq!("1542182950000000011", line_protocol1.3);
    }

    #[test]
    fn test_encode_histogram() {
        let events = vec![Metric::new(
            "requests",
            MetricKind::Absolute,
            MetricValue::AggregatedHistogram {
                buckets: vector_core::buckets![1.0 => 1, 2.1 => 2, 3.0 => 3],
                count: 6,
                sum: 12.5,
            },
        )
        .with_namespace(Some("ns"))
        .with_tags(Some(tags()))
        .with_timestamp(Some(ts()))];

        let line_protocols = encode_events(ProtocolVersion::V2, events, None, None, &[]);
        let line_protocols: Vec<&str> = line_protocols.split('\n').collect();
        assert_eq!(line_protocols.len(), 1);

        let line_protocol1 = split_line_protocol(line_protocols[0]);
        assert_eq!("ns.requests", line_protocol1.0);
        assert_eq!(
            "metric_type=histogram,normal_tag=value,true_tag=true",
            line_protocol1.1
        );
        assert_fields(
            line_protocol1.2.to_string(),
            [
                "bucket_1=1u",
                "bucket_2.1=2u",
                "bucket_3=3u",
                "count=6u",
                "sum=12.5",
            ]
            .to_vec(),
        );
        assert_eq!("1542182950000000011", line_protocol1.3);
    }

    #[test]
    fn test_encode_summary_v1() {
        let events = vec![Metric::new(
            "requests_sum",
            MetricKind::Absolute,
            MetricValue::AggregatedSummary {
                quantiles: vector_core::quantiles![0.01 => 1.5, 0.5 => 2.0, 0.99 => 3.0],
                count: 6,
                sum: 12.0,
            },
        )
        .with_namespace(Some("ns"))
        .with_tags(Some(tags()))
        .with_timestamp(Some(ts()))];

        let line_protocols = encode_events(ProtocolVersion::V1, events, None, None, &[]);
        let line_protocols: Vec<&str> = line_protocols.split('\n').collect();
        assert_eq!(line_protocols.len(), 1);

        let line_protocol1 = split_line_protocol(line_protocols[0]);
        assert_eq!("ns.requests_sum", line_protocol1.0);
        assert_eq!(
            "metric_type=summary,normal_tag=value,true_tag=true",
            line_protocol1.1
        );
        assert_fields(
            line_protocol1.2.to_string(),
            [
                "count=6i",
                "quantile_0.01=1.5",
                "quantile_0.5=2",
                "quantile_0.99=3",
                "sum=12",
            ]
            .to_vec(),
        );
        assert_eq!("1542182950000000011", line_protocol1.3);
    }

    #[test]
    fn test_encode_summary() {
        let events = vec![Metric::new(
            "requests_sum",
            MetricKind::Absolute,
            MetricValue::AggregatedSummary {
                quantiles: vector_core::quantiles![0.01 => 1.5, 0.5 => 2.0, 0.99 => 3.0],
                count: 6,
                sum: 12.0,
            },
        )
        .with_namespace(Some("ns"))
        .with_tags(Some(tags()))
        .with_timestamp(Some(ts()))];

        let line_protocols = encode_events(ProtocolVersion::V2, events, None, None, &[]);
        let line_protocols: Vec<&str> = line_protocols.split('\n').collect();
        assert_eq!(line_protocols.len(), 1);

        let line_protocol1 = split_line_protocol(line_protocols[0]);
        assert_eq!("ns.requests_sum", line_protocol1.0);
        assert_eq!(
            "metric_type=summary,normal_tag=value,true_tag=true",
            line_protocol1.1
        );
        assert_fields(
            line_protocol1.2.to_string(),
            [
                "count=6u",
                "quantile_0.01=1.5",
                "quantile_0.5=2",
                "quantile_0.99=3",
                "sum=12",
            ]
            .to_vec(),
        );
        assert_eq!("1542182950000000011", line_protocol1.3);
    }

    #[test]
    fn test_encode_distribution() {
        let events = vec![
            Metric::new(
                "requests",
                MetricKind::Incremental,
                MetricValue::Distribution {
                    samples: vector_core::samples![1.0 => 3, 2.0 => 3, 3.0 => 2],
                    statistic: StatisticKind::Histogram,
                },
            )
            .with_namespace(Some("ns"))
            .with_tags(Some(tags()))
            .with_timestamp(Some(ts())),
            Metric::new(
                "dense_stats",
                MetricKind::Incremental,
                MetricValue::Distribution {
                    samples: (0..20)
                        .map(|v| Sample {
                            value: f64::from(v),
                            rate: 1,
                        })
                        .collect(),
                    statistic: StatisticKind::Histogram,
                },
            )
            .with_namespace(Some("ns"))
            .with_timestamp(Some(ts())),
            Metric::new(
                "sparse_stats",
                MetricKind::Incremental,
                MetricValue::Distribution {
                    samples: (1..5)
                        .map(|v| Sample {
                            value: f64::from(v),
                            rate: v,
                        })
                        .collect(),
                    statistic: StatisticKind::Histogram,
                },
            )
            .with_namespace(Some("ns"))
            .with_timestamp(Some(ts())),
        ];

        let line_protocols = encode_events(ProtocolVersion::V2, events, None, None, &[]);
        let line_protocols: Vec<&str> = line_protocols.split('\n').collect();
        assert_eq!(line_protocols.len(), 3);

        let line_protocol1 = split_line_protocol(line_protocols[0]);
        assert_eq!("ns.requests", line_protocol1.0);
        assert_eq!(
            "metric_type=distribution,normal_tag=value,true_tag=true",
            line_protocol1.1
        );
        assert_fields(
            line_protocol1.2.to_string(),
            [
                "avg=1.875",
                "count=8",
                "max=3",
                "median=2",
                "min=1",
                "quantile_0.95=3",
                "sum=15",
            ]
            .to_vec(),
        );
        assert_eq!("1542182950000000011", line_protocol1.3);

        let line_protocol2 = split_line_protocol(line_protocols[1]);
        assert_eq!("ns.dense_stats", line_protocol2.0);
        assert_eq!("metric_type=distribution", line_protocol2.1);
        assert_fields(
            line_protocol2.2.to_string(),
            [
                "avg=9.5",
                "count=20",
                "max=19",
                "median=9",
                "min=0",
                "quantile_0.95=18",
                "sum=190",
            ]
            .to_vec(),
        );
        assert_eq!("1542182950000000011", line_protocol2.3);

        let line_protocol3 = split_line_protocol(line_protocols[2]);
        assert_eq!("ns.sparse_stats", line_protocol3.0);
        assert_eq!("metric_type=distribution", line_protocol3.1);
        assert_fields(
            line_protocol3.2.to_string(),
            [
                "avg=3",
                "count=10",
                "max=4",
                "median=3",
                "min=1",
                "quantile_0.95=4",
                "sum=30",
            ]
            .to_vec(),
        );
        assert_eq!("1542182950000000011", line_protocol3.3);
    }

    #[test]
    fn test_encode_distribution_empty_stats() {
        let events = vec![Metric::new(
            "requests",
            MetricKind::Incremental,
            MetricValue::Distribution {
                samples: vec![],
                statistic: StatisticKind::Histogram,
            },
        )
        .with_namespace(Some("ns"))
        .with_tags(Some(tags()))
        .with_timestamp(Some(ts()))];

        let line_protocols = encode_events(ProtocolVersion::V2, events, None, None, &[]);
        assert_eq!(line_protocols.len(), 0);
    }

    #[test]
    fn test_encode_distribution_zero_counts_stats() {
        let events = vec![Metric::new(
            "requests",
            MetricKind::Incremental,
            MetricValue::Distribution {
                samples: vector_core::samples![1.0 => 0, 2.0 => 0],
                statistic: StatisticKind::Histogram,
            },
        )
        .with_namespace(Some("ns"))
        .with_tags(Some(tags()))
        .with_timestamp(Some(ts()))];

        let line_protocols = encode_events(ProtocolVersion::V2, events, None, None, &[]);
        assert_eq!(line_protocols.len(), 0);
    }

    #[test]
    fn test_encode_distribution_summary() {
        let events = vec![Metric::new(
            "requests",
            MetricKind::Incremental,
            MetricValue::Distribution {
                samples: vector_core::samples![1.0 => 3, 2.0 => 3, 3.0 => 2],
                statistic: StatisticKind::Summary,
            },
        )
        .with_namespace(Some("ns"))
        .with_tags(Some(tags()))
        .with_timestamp(Some(ts()))];

        let line_protocols = encode_events(
            ProtocolVersion::V2,
            events,
            None,
            None,
            &default_summary_quantiles(),
        );
        let line_protocols: Vec<&str> = line_protocols.split('\n').collect();
        assert_eq!(line_protocols.len(), 1);

        let line_protocol = split_line_protocol(line_protocols[0]);
        assert_eq!("ns.requests", line_protocol.0);
        assert_eq!(
            "metric_type=distribution,normal_tag=value,true_tag=true",
            line_protocol.1
        );
        assert_fields(
            line_protocol.2.to_string(),
            [
                "avg=1.875",
                "count=8",
                "max=3",
                "median=2",
                "min=1",
                "sum=15",
                "quantile_0.50=2",
                "quantile_0.75=2",
                "quantile_0.90=3",
                "quantile_0.95=3",
                "quantile_0.99=3",
            ]
            .to_vec(),
        );
        assert_eq!("1542182950000000011", line_protocol.3);
    }

    #[test]
    fn test_encode_with_some_tags() {
        crate::test_util::trace_init();

        let events = vec![
            Metric::new(
                "cpu",
                MetricKind::Absolute,
                MetricValue::Gauge { value: 2.5 },
            )
            .with_namespace(Some("vector"))
            .with_timestamp(Some(ts())),
            Metric::new(
                "mem",
                MetricKind::Absolute,
                MetricValue::Gauge { value: 1000.0 },
            )
            .with_namespace(Some("vector"))
            .with_tags(Some(tags()))
            .with_timestamp(Some(ts())),
        ];

        let mut tags = HashMap::new();
        tags.insert("host".to_owned(), "local".to_owned());
        tags.insert("datacenter".to_owned(), "us-east".to_owned());

        let line_protocols = encode_events(
            ProtocolVersion::V1,
            events,
            Some("ns"),
            Some(tags).as_ref(),
            &[],
        );
        let line_protocols: Vec<&str> = line_protocols.split('\n').collect();
        assert_eq!(line_protocols.len(), 2);
        assert_eq!(
            line_protocols[0],
            "vector.cpu,datacenter=us-east,host=local,metric_type=gauge value=2.5 1542182950000000011"
        );
        assert_eq!(
            line_protocols[1],
            "vector.mem,datacenter=us-east,host=local,metric_type=gauge,normal_tag=value,true_tag=true value=1000 1542182950000000011"
        );
    }
}

#[cfg(feature = "influxdb-integration-tests")]
#[cfg(test)]
mod integration_tests {
    use crate::{
        config::{SinkConfig, SinkContext},
        event::metric::{Metric, MetricKind, MetricValue},
        event::Event,
        http::HttpClient,
        sinks::influxdb::{
            metrics::{default_summary_quantiles, InfluxDbConfig, InfluxDbSvc},
            test_util::{
                cleanup_v1, format_timestamp, onboarding_v1, onboarding_v2, query_v1, BUCKET, ORG,
                TOKEN,
            },
            InfluxDb1Settings, InfluxDb2Settings,
        },
        tls::{self, TlsOptions},
    };
    use chrono::{SecondsFormat, Utc};
    use futures::stream;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn inserts_metrics_v1_over_https() {
        insert_metrics_v1(
            "https://localhost:8087",
            Some(TlsOptions {
                ca_file: Some(tls::TEST_PEM_CA_PATH.into()),
                ..Default::default()
            }),
        )
        .await
    }

    #[tokio::test]
    async fn inserts_metrics_v1_over_http() {
        insert_metrics_v1("http://localhost:8086", None).await
    }

    async fn insert_metrics_v1(url: &str, tls: Option<TlsOptions>) {
        crate::test_util::trace_init();
        let database = onboarding_v1(url).await;

        let cx = SinkContext::new_test();

        let config = InfluxDbConfig {
            endpoint: url.to_string(),
            influxdb1_settings: Some(InfluxDb1Settings {
                consistency: None,
                database: database.clone(),
                retention_policy_name: Some("autogen".to_string()),
                username: None,
                password: None,
            }),
            influxdb2_settings: None,
            batch: Default::default(),
            request: Default::default(),
            tls,
            quantiles: default_summary_quantiles(),
            tags: None,
            default_namespace: None,
        };

        let events: Vec<_> = (0..10).map(create_event).collect();
        let (sink, _) = config.build(cx).await.expect("error when building config");
        sink.run(stream::iter(events.clone())).await.unwrap();

        let res = query_v1_json(url, &format!("show series on {}", database)).await;

        //
        // {"results":[{"statement_id":0,"series":[{"columns":["key"],"values":
        //  [
        //    ["ns.counter-0,metric_type=counter,production=true,region=us-west-1"],
        //    ["ns.counter-1,metric_type=counter,production=true,region=us-west-1"],
        //    ["ns.counter-2,metric_type=counter,production=true,region=us-west-1"],
        //    ["ns.counter-3,metric_type=counter,production=true,region=us-west-1"],
        //    ["ns.counter-4,metric_type=counter,production=true,region=us-west-1"],
        //    ["ns.counter-5,metric_type=counter,production=true,region=us-west-1"],
        //    ["ns.counter-6,metric_type=counter,production=true,region=us-west-1"],
        //    ["ns.counter-7,metric_type=counter,production=true,region=us-west-1"],
        //    ["ns.counter-8,metric_type=counter,production=true,region=us-west-1"],
        //    ["ns.counter-9,metric_type=counter,production=true,region=us-west-1"]
        //  ]}]}]}\n
        //

        assert_eq!(
            res["results"][0]["series"][0]["values"]
                .as_array()
                .unwrap()
                .len(),
            events.len()
        );

        for event in events {
            let metric = event.into_metric();
            let name = format!("{}.{}", metric.namespace().unwrap(), metric.name());
            let value = match metric.value() {
                MetricValue::Counter { value } => *value,
                _ => unreachable!(),
            };
            let timestamp = format_timestamp(metric.timestamp().unwrap(), SecondsFormat::Nanos);
            let res =
                query_v1_json(url, &format!("select * from {}..\"{}\"", database, name)).await;

            assert_eq!(
                res,
                serde_json::json! {
                    {"results": [{
                        "statement_id": 0,
                        "series": [{
                            "name": name,
                            "columns": ["time", "metric_type", "production", "region", "value"],
                            "values": [[timestamp, "counter", "true", "us-west-1", value as isize]]
                        }]
                    }]}
                }
            );
        }

        cleanup_v1(url, &database).await;
    }

    async fn query_v1_json(url: &str, query: &str) -> serde_json::Value {
        let string = query_v1(url, query)
            .await
            .text()
            .await
            .expect("Fetching text from InfluxDB query failed");
        serde_json::from_str(&string).expect("Error when parsing InfluxDB response JSON")
    }

    #[tokio::test]
    async fn influxdb2_metrics_put_data() {
        crate::test_util::trace_init();
        onboarding_v2().await;

        let cx = SinkContext::new_test();

        let config = InfluxDbConfig {
            endpoint: "http://localhost:9999".to_string(),
            influxdb1_settings: None,
            influxdb2_settings: Some(InfluxDb2Settings {
                org: ORG.to_string(),
                bucket: BUCKET.to_string(),
                token: TOKEN.to_string(),
            }),
            quantiles: default_summary_quantiles(),
            batch: Default::default(),
            request: Default::default(),
            tags: None,
            tls: None,
            default_namespace: None,
        };

        let metric = format!("counter-{}", Utc::now().timestamp_nanos());
        let mut events = Vec::new();
        for i in 0..10 {
            let event = Event::Metric(
                Metric::new(
                    metric.clone(),
                    MetricKind::Incremental,
                    MetricValue::Counter { value: i as f64 },
                )
                .with_namespace(Some("ns"))
                .with_tags(Some(
                    vec![
                        ("region".to_owned(), "us-west-1".to_owned()),
                        ("production".to_owned(), "true".to_owned()),
                    ]
                    .into_iter()
                    .collect(),
                )),
            );
            events.push(event);
        }

        let client = HttpClient::new(None).unwrap();
        let sink = InfluxDbSvc::new(config, cx, client).unwrap();
        sink.run(stream::iter(events)).await.unwrap();

        let mut body = std::collections::HashMap::new();
        body.insert("query", format!("from(bucket:\"my-bucket\") |> range(start: 0) |> filter(fn: (r) => r._measurement == \"ns.{}\")", metric));
        body.insert("type", "flux".to_owned());

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();

        let res = client
            .post("http://localhost:9999/api/v2/query?org=my-org")
            .json(&body)
            .header("accept", "application/json")
            .header("Authorization", "Token my-token")
            .send()
            .await
            .unwrap();
        let string = res.text().await.unwrap();

        let lines = string.split('\n').collect::<Vec<&str>>();
        let header = lines[0].split(',').collect::<Vec<&str>>();
        let record = lines[1].split(',').collect::<Vec<&str>>();

        assert_eq!(
            record[header
                .iter()
                .position(|&r| r.trim() == "metric_type")
                .unwrap()]
            .trim(),
            "counter"
        );
        assert_eq!(
            record[header
                .iter()
                .position(|&r| r.trim() == "production")
                .unwrap()]
            .trim(),
            "true"
        );
        assert_eq!(
            record[header.iter().position(|&r| r.trim() == "region").unwrap()].trim(),
            "us-west-1"
        );
        assert_eq!(
            record[header
                .iter()
                .position(|&r| r.trim() == "_measurement")
                .unwrap()]
            .trim(),
            format!("ns.{}", metric)
        );
        assert_eq!(
            record[header.iter().position(|&r| r.trim() == "_field").unwrap()].trim(),
            "value"
        );
        assert_eq!(
            record[header.iter().position(|&r| r.trim() == "_value").unwrap()].trim(),
            "45"
        );
    }

    fn create_event(i: i32) -> Event {
        Event::Metric(
            Metric::new(
                format!("counter-{}", i),
                MetricKind::Incremental,
                MetricValue::Counter { value: i as f64 },
            )
            .with_namespace(Some("ns"))
            .with_tags(Some(
                vec![
                    ("region".to_owned(), "us-west-1".to_owned()),
                    ("production".to_owned(), "true".to_owned()),
                ]
                .into_iter()
                .collect(),
            ))
            .with_timestamp(Some(Utc::now())),
        )
    }
}
