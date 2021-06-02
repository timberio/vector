use crate::{
    config::{DataType, SinkConfig, SinkContext, SinkDescription},
    event::{
        metric::{Metric, MetricValue, Sample},
        Event,
    },
    rusoto::{self, AwsAuthentication, RegionOrEndpoint},
    sinks::util::{
        batch::{BatchConfig, BatchSettings},
        buffer::metrics::{MetricNormalize, MetricNormalizer, MetricSet, MetricsBuffer},
        retries::RetryLogic,
        Compression, EncodedEvent, PartitionBatchSink, PartitionBuffer, PartitionInnerBuffer,
        TowerRequestConfig,
    },
};
use chrono::{DateTime, SecondsFormat, Utc};
use futures::{future, future::BoxFuture, stream, FutureExt, SinkExt};
use lazy_static::lazy_static;
use rusoto_cloudwatch::{
    CloudWatch, CloudWatchClient, Dimension, MetricDatum, PutMetricDataError, PutMetricDataInput,
};
use rusoto_core::{Region, RusotoError};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    convert::TryInto,
    task::{Context, Poll},
};
use tower::Service;

#[derive(Clone)]
pub struct CloudWatchMetricsSvc {
    client: CloudWatchClient,
    config: CloudWatchMetricsSinkConfig,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct CloudWatchMetricsSinkConfig {
    #[serde(alias = "namespace")]
    pub default_namespace: String,
    #[serde(flatten)]
    pub region: RegionOrEndpoint,
    #[serde(default)]
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

lazy_static! {
    static ref REQUEST_DEFAULTS: TowerRequestConfig = TowerRequestConfig {
        timeout_secs: Some(30),
        rate_limit_num: Some(150),
        ..Default::default()
    };
}

inventory::submit! {
    SinkDescription::new::<CloudWatchMetricsSinkConfig>("aws_cloudwatch_metrics")
}

impl_generate_config_from_default!(CloudWatchMetricsSinkConfig);

#[async_trait::async_trait]
#[typetag::serde(name = "aws_cloudwatch_metrics")]
impl SinkConfig for CloudWatchMetricsSinkConfig {
    async fn build(
        &self,
        cx: SinkContext,
    ) -> crate::Result<(super::VectorSink, super::Healthcheck)> {
        let client = self.create_client()?;
        let healthcheck = self.clone().healthcheck(client.clone()).boxed();
        let sink = CloudWatchMetricsSvc::new(self.clone(), client, cx)?;
        Ok((sink, healthcheck))
    }

    fn input_type(&self) -> DataType {
        DataType::Metric
    }

    fn sink_type(&self) -> &'static str {
        "aws_cloudwatch_metrics"
    }
}

impl CloudWatchMetricsSinkConfig {
    async fn healthcheck(self, client: CloudWatchClient) -> crate::Result<()> {
        let datum = MetricDatum {
            metric_name: "healthcheck".into(),
            value: Some(1.0),
            ..Default::default()
        };
        let request = PutMetricDataInput {
            namespace: self.default_namespace.clone(),
            metric_data: vec![datum],
        };

        client.put_metric_data(request).await.map_err(Into::into)
    }

    fn create_client(&self) -> crate::Result<CloudWatchClient> {
        let region = (&self.region).try_into()?;
        let region = if cfg!(test) {
            // Moto (used for mocking AWS) doesn't recognize 'custom' as valid region name
            match region {
                Region::Custom { endpoint, .. } => Region::Custom {
                    name: "us-east-1".into(),
                    endpoint,
                },
                _ => panic!("Only Custom regions are supported for CloudWatchClient testing"),
            }
        } else {
            region
        };

        let client = rusoto::client()?;
        let creds = self.auth.build(&region, self.assume_role.clone())?;

        let client = rusoto_core::Client::new_with_encoding(creds, client, self.compression.into());
        Ok(CloudWatchClient::new_with_client(client, region))
    }
}

impl CloudWatchMetricsSvc {
    pub fn new(
        config: CloudWatchMetricsSinkConfig,
        client: CloudWatchClient,
        cx: SinkContext,
    ) -> crate::Result<super::VectorSink> {
        let default_namespace = config.default_namespace.clone();
        let batch = BatchSettings::default()
            .events(20)
            .timeout(1)
            .parse_config(config.batch)?;
        let request = config.request.unwrap_with(&REQUEST_DEFAULTS);

        let cloudwatch_metrics = CloudWatchMetricsSvc { client, config };

        let svc = request.service(CloudWatchMetricsRetryLogic, cloudwatch_metrics);

        let buffer = PartitionBuffer::new(MetricsBuffer::new(batch.size));
        let mut normalizer = MetricNormalizer::<AwsCloudwatchMetricNormalize>::default();

        let sink = PartitionBatchSink::new(svc, buffer, batch.timeout, cx.acker())
            .sink_map_err(|error| error!(message = "Fatal CloudwatchMetrics sink error.", %error))
            .with_flat_map(move |event: Event| {
                stream::iter(normalizer.apply(event).map(|mut metric| {
                    let namespace = metric
                        .series
                        .name
                        .namespace
                        .take()
                        .unwrap_or_else(|| default_namespace.clone());
                    Ok(EncodedEvent::new(PartitionInnerBuffer::new(
                        metric, namespace,
                    )))
                }))
            });

        Ok(super::VectorSink::Sink(Box::new(sink)))
    }

    fn encode_events(&mut self, events: Vec<Metric>) -> Vec<MetricDatum> {
        events
            .into_iter()
            .map(|event| {
                let metric_name = event.name().to_string();
                let timestamp = event.data.timestamp.map(timestamp_to_string);
                let dimensions = event.series.tags.clone().map(tags_to_dimensions);
                // AwsCloudwatchMetricNormalize converts these to the right MetricKind
                let (value, values, counts) = match event.data.value {
                    MetricValue::Counter { value } => (Some(value), None, None),
                    MetricValue::Distribution {
                        samples,
                        statistic: _,
                    } => {
                        let samples = normalize_distribution(samples);
                        (
                            None,
                            Some(samples.iter().map(|s| s.value).collect()),
                            Some(samples.iter().map(|s| f64::from(s.rate)).collect()),
                        )
                    }
                    MetricValue::Set { values } => (Some(values.len() as f64), None, None),
                    MetricValue::Gauge { value } => (Some(value), None, None),
                    _ => unreachable!(),
                };
                MetricDatum {
                    metric_name,
                    value,
                    values,
                    counts,
                    timestamp,
                    dimensions,
                    ..Default::default()
                }
            })
            .collect()
    }
}

fn median_value(samples: &[Sample]) -> f64 {
    let count = samples.len();
    let midpt = count / 2;
    if count % 2 == 1 {
        samples[midpt].value
    } else {
        (samples[midpt - 1].value + samples[midpt].value) / 2.0
    }
}

/// Normalize a distribution to reduce over-sized sample sets. AWS
/// CloudWatch metrics only accepts distributions with 150 or less
/// samples, so check the length and downsample the distribution if it's
/// over 150.  See
/// https://docs.aws.amazon.com/AmazonCloudWatch/latest/APIReference/API_PutMetricData.html
fn normalize_distribution(samples: Vec<Sample>) -> Vec<Sample> {
    if samples.len() > 150 {
        downsample_distribution(samples, 150)
    } else {
        samples
    }
}

/// Reduce a distribution to a maximum number of elements by chunking
/// together sets of values and summing their rates.
fn downsample_distribution(mut samples: Vec<Sample>, maxsize: usize) -> Vec<Sample> {
    use std::cmp::Ordering;
    // Sort the samples so that they are grouped together with their closest values.
    samples.sort_unstable_by(|a, b| {
        if a.value < b.value {
            Ordering::Less
        } else if a.value > b.value {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    });
    // Figure out the required chunk size, rounding up to ensure the
    // result is at most `maxsize` long.
    let chunk_size = (samples.len() + maxsize - 1) / maxsize;
    samples
        .chunks(chunk_size)
        .map(|chunk| Sample {
            value: median_value(chunk),
            rate: chunk.iter().map(|s| s.rate).sum(),
        })
        .collect()
}

struct AwsCloudwatchMetricNormalize;

impl MetricNormalize for AwsCloudwatchMetricNormalize {
    fn apply_state(state: &mut MetricSet, metric: Metric) -> Option<Metric> {
        match &metric.data.value {
            MetricValue::Gauge { .. } => state.make_absolute(metric),
            MetricValue::Counter { .. }
            | MetricValue::Set { .. }
            | MetricValue::Distribution { .. } => state.make_incremental(metric),
            // Drop all other types, Cloudwatch can't handle them
            _ => None,
        }
    }
}

impl Service<PartitionInnerBuffer<Vec<Metric>, String>> for CloudWatchMetricsSvc {
    type Response = ();
    type Error = RusotoError<PutMetricDataError>;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, items: PartitionInnerBuffer<Vec<Metric>, String>) -> Self::Future {
        let (items, namespace) = items.into_parts();
        let metric_data = self.encode_events(items);
        if metric_data.is_empty() {
            return future::ok(()).boxed();
        }

        let input = PutMetricDataInput {
            metric_data,
            namespace,
        };

        debug!(message = "Sending data.", input = ?input);
        let client = self.client.clone();
        Box::pin(async move { client.put_metric_data(input).await })
    }
}

#[derive(Debug, Clone)]
struct CloudWatchMetricsRetryLogic;

impl RetryLogic for CloudWatchMetricsRetryLogic {
    type Error = RusotoError<PutMetricDataError>;
    type Response = ();

    fn is_retriable_error(&self, error: &Self::Error) -> bool {
        match error {
            RusotoError::Service(PutMetricDataError::InternalServiceFault(_)) => true,
            error => rusoto::is_retriable_error(error),
        }
    }
}

fn timestamp_to_string(timestamp: DateTime<Utc>) -> String {
    timestamp.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn tags_to_dimensions(tags: BTreeMap<String, String>) -> Vec<Dimension> {
    // according to the API, up to 10 dimensions per metric can be provided
    tags.iter()
        .take(10)
        .map(|(k, v)| Dimension {
            name: k.to_string(),
            value: v.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::metric::{Metric, MetricKind, MetricValue, StatisticKind};
    use chrono::offset::TimeZone;
    use pretty_assertions::assert_eq;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<CloudWatchMetricsSinkConfig>();
    }

    fn config() -> CloudWatchMetricsSinkConfig {
        CloudWatchMetricsSinkConfig {
            default_namespace: "vector".into(),
            region: RegionOrEndpoint::with_endpoint("local".to_owned()),
            ..Default::default()
        }
    }

    fn svc() -> CloudWatchMetricsSvc {
        let config = config();
        let client = config.create_client().unwrap();
        CloudWatchMetricsSvc { client, config }
    }

    #[test]
    fn encode_events_basic_counter() {
        let events = vec![
            Metric::new(
                "exception_total",
                MetricKind::Incremental,
                MetricValue::Counter { value: 1.0 },
            ),
            Metric::new(
                "bytes_out",
                MetricKind::Incremental,
                MetricValue::Counter { value: 2.5 },
            )
            .with_timestamp(Some(
                Utc.ymd(2018, 11, 14).and_hms_nano(8, 9, 10, 123456789),
            )),
            Metric::new(
                "healthcheck",
                MetricKind::Incremental,
                MetricValue::Counter { value: 1.0 },
            )
            .with_tags(Some(
                vec![("region".to_owned(), "local".to_owned())]
                    .into_iter()
                    .collect(),
            ))
            .with_timestamp(Some(
                Utc.ymd(2018, 11, 14).and_hms_nano(8, 9, 10, 123456789),
            )),
        ];

        assert_eq!(
            svc().encode_events(events),
            vec![
                MetricDatum {
                    metric_name: "exception_total".into(),
                    value: Some(1.0),
                    ..Default::default()
                },
                MetricDatum {
                    metric_name: "bytes_out".into(),
                    value: Some(2.5),
                    timestamp: Some("2018-11-14T08:09:10.123Z".into()),
                    ..Default::default()
                },
                MetricDatum {
                    metric_name: "healthcheck".into(),
                    value: Some(1.0),
                    timestamp: Some("2018-11-14T08:09:10.123Z".into()),
                    dimensions: Some(vec![Dimension {
                        name: "region".into(),
                        value: "local".into()
                    }]),
                    ..Default::default()
                },
            ]
        );
    }

    #[test]
    fn encode_events_absolute_gauge() {
        let events = vec![Metric::new(
            "temperature",
            MetricKind::Absolute,
            MetricValue::Gauge { value: 10.0 },
        )];

        assert_eq!(
            svc().encode_events(events),
            vec![MetricDatum {
                metric_name: "temperature".into(),
                value: Some(10.0),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn encode_events_distribution() {
        let events = vec![Metric::new(
            "latency",
            MetricKind::Incremental,
            MetricValue::Distribution {
                samples: vector_core::samples![11.0 => 100, 12.0 => 50],
                statistic: StatisticKind::Histogram,
            },
        )];

        assert_eq!(
            svc().encode_events(events),
            vec![MetricDatum {
                metric_name: "latency".into(),
                values: Some(vec![11.0, 12.0]),
                counts: Some(vec![100.0, 50.0]),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn encode_events_set() {
        let events = vec![Metric::new(
            "users",
            MetricKind::Incremental,
            MetricValue::Set {
                values: vec!["alice".into(), "bob".into()].into_iter().collect(),
            },
        )];

        assert_eq!(
            svc().encode_events(events),
            vec![MetricDatum {
                metric_name: "users".into(),
                value: Some(2.0),
                ..Default::default()
            }]
        );
    }
}

#[cfg(feature = "aws-cloudwatch-metrics-integration-tests")]
#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::{
        event::metric::StatisticKind,
        event::{Event, MetricKind},
        test_util::random_string,
    };
    use chrono::offset::TimeZone;
    use rand::seq::SliceRandom;

    fn config() -> CloudWatchMetricsSinkConfig {
        CloudWatchMetricsSinkConfig {
            default_namespace: "vector".into(),
            region: RegionOrEndpoint::with_endpoint("http://localhost:4566".to_owned()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn cloudwatch_metrics_healthchecks() {
        let config = config();
        let client = config.create_client().unwrap();
        config.healthcheck(client).await.unwrap();
    }

    #[tokio::test]
    async fn cloudwatch_metrics_put_data() {
        let cx = SinkContext::new_test();
        let config = config();
        let client = config.create_client().unwrap();
        let sink = CloudWatchMetricsSvc::new(config, client, cx).unwrap();

        let mut events = Vec::new();

        for i in 0..100 {
            let event = Event::Metric(
                Metric::new(
                    format!("counter-{}", 0),
                    MetricKind::Incremental,
                    MetricValue::Counter { value: i as f64 },
                )
                .with_tags(Some(
                    vec![
                        ("region".to_owned(), "us-west-1".to_owned()),
                        ("production".to_owned(), "true".to_owned()),
                        ("e".to_owned(), "".to_owned()),
                    ]
                    .into_iter()
                    .collect(),
                )),
            );
            events.push(event);
        }

        let gauge_name = random_string(10);
        for i in 0..10 {
            let event = Event::Metric(Metric::new(
                format!("gauge-{}", gauge_name),
                MetricKind::Absolute,
                MetricValue::Gauge { value: i as f64 },
            ));
            events.push(event);
        }

        let distribution_name = random_string(10);
        for i in 0..10 {
            let event = Event::Metric(
                Metric::new(
                    format!("distribution-{}", distribution_name),
                    MetricKind::Incremental,
                    MetricValue::Distribution {
                        samples: vector_core::samples![i as f64 => 100],
                        statistic: StatisticKind::Histogram,
                    },
                )
                .with_timestamp(Some(
                    Utc.ymd(2018, 11, 14).and_hms_nano(8, 9, 10, 123456789),
                )),
            );
            events.push(event);
        }

        let stream = stream::iter(events);
        sink.run(stream).await.unwrap();
    }

    #[tokio::test]
    async fn cloudwatch_metrics_namespace_partitioning() {
        let cx = SinkContext::new_test();
        let config = config();
        let client = config.create_client().unwrap();
        let sink = CloudWatchMetricsSvc::new(config, client, cx).unwrap();

        let mut events = Vec::new();

        for namespace in ["ns1", "ns2", "ns3", "ns4"].iter() {
            for _ in 0..100 {
                let event = Event::Metric(
                    Metric::new(
                        "counter",
                        MetricKind::Incremental,
                        MetricValue::Counter { value: 1.0 },
                    )
                    .with_namespace(Some(*namespace)),
                );
                events.push(event);
            }
        }

        events.shuffle(&mut rand::thread_rng());

        let stream = stream::iter(events);
        sink.run(stream).await.unwrap();
    }
}
