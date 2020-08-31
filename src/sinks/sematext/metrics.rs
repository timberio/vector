use super::Region;
use crate::{
    config::{DataType, SinkConfig, SinkContext, SinkDescription},
    event::metric::{Metric, MetricValue},
    internal_events::SematextMetricsInvalidMetric,
    sinks::influxdb::{encode_timestamp, encode_uri, influx_line_protocol, Field, ProtocolVersion},
    sinks::util::{
        http::{HttpBatchService, HttpClient, HttpRetryLogic},
        BatchConfig, BatchSettings, MetricBuffer, TowerRequestConfig,
    },
    sinks::{Healthcheck, HealthcheckError, RouterSink},
    vector_version,
};
use futures::future::{ready, BoxFuture};
use futures::{FutureExt, TryFutureExt};
use futures01::Sink;
use http::{StatusCode, Uri};
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::task::Poll;
use tower::Service;

#[derive(Clone)]
struct SematextMetricsSvc {
    config: SematextMetricsConfig,
    inner: HttpBatchService<BoxFuture<'static, crate::Result<hyper::Request<Vec<u8>>>>>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
struct SematextMetricsConfig {
    pub region: Option<Region>,
    pub host: Option<String>,
    pub token: String,
    #[serde(default)]
    pub batch: BatchConfig,
    #[serde(default)]
    pub request: TowerRequestConfig,
}

inventory::submit! {
    SinkDescription::new_without_default::<SematextMetricsConfig>("sematext_metrics")
}

async fn healthcheck(endpoint: String, mut client: HttpClient) -> crate::Result<()> {
    // Note, this is not the most ideal endpoint for a healthcheck.
    // Sematext will hopefully be setting up a 'ping' endpoint that we can use in due course.
    let uri = format!(
        "{}/write?db=metrics&v=vector-{}",
        endpoint,
        vector_version()
    );

    let request = hyper::Request::get(uri).body(hyper::Body::empty()).unwrap();

    let response = client.send(request).await?;

    match response.status() {
        StatusCode::OK => Ok(()),
        StatusCode::NO_CONTENT => Ok(()),
        other => Err(HealthcheckError::UnexpectedStatus { status: other }.into()),
    }
}

#[typetag::serde(name = "sematext_metrics")]
impl SinkConfig for SematextMetricsConfig {
    fn build(&self, cx: SinkContext) -> crate::Result<(RouterSink, Healthcheck)> {
        let client = HttpClient::new(cx.resolver(), None)?;

        let host = match (&self.host, &self.region) {
            (Some(host), None) => host.clone(),
            (None, Some(Region::Na)) => "https://spm-receiver.sematext.com".to_owned(),
            (None, Some(Region::Eu)) => "https://spm-receiver.eu.sematext.com".to_owned(),
            (None, None) => {
                return Err("Either `region` or `host` must be set.".into());
            }
            (Some(_), Some(_)) => {
                return Err("Only one of `region` and `host` can be set.".into());
            }
        };

        let healthcheck = healthcheck(host.clone(), client.clone()).boxed().compat();
        let sink = SematextMetricsSvc::new(self.clone(), write_uri(&host)?, cx, client)?;

        Ok((sink, Box::new(healthcheck)))
    }

    fn input_type(&self) -> DataType {
        DataType::Metric
    }

    fn sink_type(&self) -> &'static str {
        "sematext_metrics"
    }
}

lazy_static! {
    static ref REQUEST_DEFAULTS: TowerRequestConfig = TowerRequestConfig {
        retry_attempts: Some(5),
        ..Default::default()
    };
}

fn write_uri(endpoint: &str) -> crate::Result<Uri> {
    encode_uri(
        &endpoint,
        "write",
        &[
            ("db", Some("metrics".into())),
            ("v", Some(format!("vector-{}", vector_version()))),
            ("precision", Some("ns".into())),
        ],
    )
}

impl SematextMetricsSvc {
    pub fn new(
        config: SematextMetricsConfig,
        host: http::Uri,
        cx: SinkContext,
        client: HttpClient,
    ) -> crate::Result<RouterSink> {
        let batch = BatchSettings::default()
            .events(20)
            .timeout(1)
            .parse_config(config.batch)?;
        let request = config.request.unwrap_with(&REQUEST_DEFAULTS);
        let http_service = HttpBatchService::new(client, create_build_request(host));
        let sematext_service = SematextMetricsSvc {
            config,
            inner: http_service,
        };

        let sink = request
            .batch_sink(
                HttpRetryLogic,
                sematext_service,
                MetricBuffer::new(batch.size),
                batch.timeout,
                cx.acker(),
            )
            .sink_map_err(|e| error!("Fatal sematext metrics sink error: {}", e));

        Ok(Box::new(sink))
    }
}

impl Service<Vec<Metric>> for SematextMetricsSvc {
    type Response = http::Response<bytes::Bytes>;
    type Error = crate::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut std::task::Context) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, items: Vec<Metric>) -> Self::Future {
        let input = encode_events(&self.config.token, items);
        let body: Vec<u8> = input.into_bytes();

        self.inner.call(body)
    }
}

fn create_build_request(
    uri: http::Uri,
) -> impl Fn(Vec<u8>) -> BoxFuture<'static, crate::Result<hyper::Request<Vec<u8>>>> + Sync + Send + 'static
{
    move |body| {
        Box::pin(ready(
            hyper::Request::post(uri.clone())
                .header("Content-Type", "text/plain")
                .body(body)
                .map_err(Into::into),
        ))
    }
}

/// Sematext takes the first part of the name as the namespace for the event.
/// The rest of the name is the value label.
/// If the name is not a '.' separated string, it will take the whole name as the
/// namespace, and set label the value as 'value'.
/// This probably should not happen if you want meaningful metrics in Sematext.
fn split_event_name(name: &str) -> (String, String) {
    match name.find('.') {
        None => (name.into(), "value".into()),
        Some(pos) => (name[0..pos].into(), name[pos + 1..].into()),
    }
}

fn encode_events(token: &str, events: Vec<Metric>) -> String {
    let mut output = String::new();
    for event in events.into_iter() {
        let fullname = event.name;
        let (namespace, label) = split_event_name(&fullname);
        let ts = encode_timestamp(event.timestamp);

        // Authentication in Sematext is by inserting the token as a tag.
        let mut tags = event.tags.clone().unwrap_or_else(BTreeMap::new);
        tags.insert("token".into(), token.into());

        match event.value {
            MetricValue::Counter { value } => {
                let fields = to_fields(label, value);

                influx_line_protocol(
                    ProtocolVersion::V1,
                    namespace,
                    "counter",
                    Some(tags),
                    Some(fields),
                    ts,
                    &mut output,
                );
            }
            MetricValue::Gauge { value } => {
                let fields = to_fields(label, value);

                influx_line_protocol(
                    ProtocolVersion::V1,
                    namespace,
                    "gauge",
                    Some(tags),
                    Some(fields),
                    ts,
                    &mut output,
                );
            }
            _ => {
                emit!(SematextMetricsInvalidMetric {
                    value: event.value,
                    kind: event.kind,
                });
            }
        }
    }

    output.pop();
    output
}

pub fn to_fields(label: String, value: f64) -> HashMap<String, Field> {
    let mut result = HashMap::new();
    result.insert(label, Field::Float(value));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{
        metric::{MetricKind, MetricValue},
        Event, Metric,
    };
    use crate::sinks::util::test::{build_test_server, load_sink};
    use crate::test_util;
    use chrono::{offset::TimeZone, Utc};
    use futures::{compat::Future01CompatExt, StreamExt};

    #[test]
    fn test_encode_counter_event() {
        let events = vec![Metric {
            name: "jvm.pool.used".into(),
            timestamp: Some(Utc.ymd(2020, 08, 18).and_hms_nano(21, 0, 0, 0)),
            tags: None,
            kind: MetricKind::Incremental,
            value: MetricValue::Counter { value: 42.0 },
        }];

        assert_eq!(
            "jvm,metric_type=counter,token=aaa pool.used=42 1597784400000000000",
            encode_events("aaa", events)
        );
    }

    #[test]
    fn test_encode_counter_event_no_namespace() {
        let events = vec![Metric {
            name: "used".into(),
            timestamp: Some(Utc.ymd(2020, 08, 18).and_hms_nano(21, 0, 0, 0)),
            tags: None,
            kind: MetricKind::Incremental,
            value: MetricValue::Counter { value: 42.0 },
        }];

        assert_eq!(
            "used,metric_type=counter,token=aaa value=42 1597784400000000000",
            encode_events("aaa", events)
        );
    }

    #[test]
    fn test_encode_counter_multiple_events() {
        let events = vec![
            Metric {
                name: "jvm.pool.used".into(),
                timestamp: Some(Utc.ymd(2020, 08, 18).and_hms_nano(21, 0, 0, 0)),
                tags: None,
                kind: MetricKind::Incremental,
                value: MetricValue::Counter { value: 42.0 },
            },
            Metric {
                name: "jvm.pool.committed".into(),
                timestamp: Some(Utc.ymd(2020, 08, 18).and_hms_nano(21, 0, 0, 1)),
                tags: None,
                kind: MetricKind::Incremental,
                value: MetricValue::Counter { value: 18874368.0 },
            },
        ];

        assert_eq!(
            "jvm,metric_type=counter,token=aaa pool.used=42 1597784400000000000\n\
             jvm,metric_type=counter,token=aaa pool.committed=18874368 1597784400000000001",
            encode_events("aaa", events)
        );
    }

    #[tokio::test]
    async fn smoke() {
        let (mut config, cx) = load_sink::<SematextMetricsConfig>(
            r#"
            region = "eu"
            token = "atoken"
            batch.max_events = 1
            "#,
        )
        .unwrap();

        let _ = config.build(cx.clone()).unwrap();

        let addr = test_util::next_addr();
        // Swap out the host so we can force send it
        // to our local server
        let host = format!("http://{}", addr);
        config.host = Some(host.clone());
        config.region = None;

        let (sink, _) = config.build(cx).unwrap();

        let (rx, _trigger, server) = build_test_server(addr);
        tokio::spawn(server);

        // Make our test metrics.
        let metrics = vec![
            ("os.swap.size", 324292.0),
            ("os.network.tx", 42000.0),
            ("os.network.rx", 54293.0),
            ("process.count", 12.0),
            ("process.uptime", 32423.0),
            ("process.rss", 2342333.0),
            ("jvm.pool.used", 18874368.0),
            ("jvm.pool.committed", 18868584.0),
            ("jvm.pool.max", 18874368.0),
        ];

        let mut events = Vec::new();
        for i in 0..metrics.len() {
            let (metric, val) = metrics[i];
            let event = Event::from(Metric {
                name: metric.to_string(),
                timestamp: Some(Utc.ymd(2020, 08, 18).and_hms_nano(21, 0, 0, i as u32)),
                tags: Some(
                    vec![("os.host".to_owned(), "somehost".to_owned())]
                        .into_iter()
                        .collect(),
                ),
                kind: MetricKind::Incremental,
                value: MetricValue::Counter { value: val as f64 },
            });
            events.push(event);
        }

        let _ = sink
            .send_all(futures01::stream::iter_ok(events.clone()))
            .compat()
            .await
            .unwrap();

        let output = rx.take(metrics.len()).collect::<Vec<_>>().await;
        assert_eq!("os,metric_type=counter,os.host=somehost,token=atoken swap.size=324292 1597784400000000000", output[0].1);
        assert_eq!("os,metric_type=counter,os.host=somehost,token=atoken network.tx=42000 1597784400000000001", output[1].1);
        assert_eq!("os,metric_type=counter,os.host=somehost,token=atoken network.rx=54293 1597784400000000002", output[2].1);
        assert_eq!("process,metric_type=counter,os.host=somehost,token=atoken count=12 1597784400000000003", output[3].1);
        assert_eq!("process,metric_type=counter,os.host=somehost,token=atoken uptime=32423 1597784400000000004", output[4].1);
        assert_eq!("process,metric_type=counter,os.host=somehost,token=atoken rss=2342333 1597784400000000005", output[5].1);
        assert_eq!("jvm,metric_type=counter,os.host=somehost,token=atoken pool.used=18874368 1597784400000000006", output[6].1);
        assert_eq!("jvm,metric_type=counter,os.host=somehost,token=atoken pool.committed=18868584 1597784400000000007", output[7].1);
        assert_eq!("jvm,metric_type=counter,os.host=somehost,token=atoken pool.max=18874368 1597784400000000008", output[8].1);
    }
}
