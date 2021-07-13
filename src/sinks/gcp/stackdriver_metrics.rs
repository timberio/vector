use crate::config::{DataType, ProxyConfig, SinkConfig, SinkContext, SinkDescription};
use crate::event::{Event, Metric, MetricValue};
use crate::http::HttpClient;
use crate::sinks::gcp;
use crate::sinks::util::buffer::metrics::MetricsBuffer;
use crate::sinks::util::http::{BatchedHttpSink, HttpSink};
use crate::sinks::util::{BatchConfig, BatchSettings, EncodedEvent, TowerRequestConfig};
use crate::sinks::{Healthcheck, VectorSink};
use crate::tls::{TlsOptions, TlsSettings};
use chrono::{DateTime, Utc};
use futures::{sink::SinkExt, FutureExt};
use http::header::AUTHORIZATION;
use http::{HeaderValue, Uri};
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct StackdriverConfig {
    pub project_id: String,
    pub resource: gcp::GcpTypedResource,
    pub credentials_path: Option<String>,
    #[serde(default = "default_metric_namespace_value")]
    pub default_namespace: String,
    #[serde(default)]
    pub request: TowerRequestConfig,
    #[serde(default)]
    pub batch: BatchConfig,
    pub tls: Option<TlsOptions>,
    #[serde(
        default,
        skip_serializing_if = "crate::serde::skip_serializing_if_default"
    )]
    pub proxy: ProxyConfig,
}

fn default_metric_namespace_value() -> String {
    "namespace".to_string()
}

impl_generate_config_from_default!(StackdriverConfig);

inventory::submit! {
    SinkDescription::new::<StackdriverConfig>("gcp_stackdriver_metrics")
}

#[async_trait::async_trait]
#[typetag::serde(name = "gcp_stackdriver_metrics")]
impl SinkConfig for StackdriverConfig {
    async fn build(&self, cx: SinkContext) -> crate::Result<(VectorSink, Healthcheck)> {
        let mut token = gouth::Builder::new().scopes(&[
            "https://www.googleapis.com/auth/cloud-platform",
            "https://www.googleapis.com/auth/monitoring",
            "https://www.googleapis.com/auth/monitoring.write",
        ]);

        if let Some(credentials_path) = self.credentials_path.as_ref() {
            token = token.file(credentials_path);
        }

        let token = token.build()?;
        let healthcheck = healthcheck().boxed();
        let started = chrono::Utc::now();
        let request = self.request.unwrap_with(&REQUEST_DEFAULTS);
        let tls_settings = TlsSettings::from_options(&self.tls)?;
        let proxy = cx.globals.proxy.build(&self.proxy);
        let client = HttpClient::new(tls_settings, proxy)?;
        let batch = BatchSettings::default()
            .events(1)
            .parse_config(self.batch)?;

        let sink = HttpEventSink {
            config: self.clone(),
            started,
            token,
        };

        let sink = BatchedHttpSink::new(
            sink,
            MetricsBuffer::new(batch.size),
            request,
            batch.timeout,
            client,
            cx.acker(),
        )
        .sink_map_err(
            |error| error!(message = "Fatal gcp_stackdriver_metrics sink error.", %error),
        );

        Ok((VectorSink::Sink(Box::new(sink)), healthcheck))
    }

    fn input_type(&self) -> DataType {
        DataType::Metric
    }

    fn sink_type(&self) -> &'static str {
        "gcp_stackdriver_metrics"
    }
}

struct HttpEventSink {
    config: StackdriverConfig,
    started: DateTime<Utc>,
    token: gouth::Token,
}

#[async_trait::async_trait]
impl HttpSink for HttpEventSink {
    type Input = Metric;
    type Output = Vec<Metric>;

    fn encode_event(&self, event: Event) -> Option<EncodedEvent<Self::Input>> {
        let metric = event.into_metric();

        match metric.value() {
            &MetricValue::Counter { .. } => Some(metric),
            &MetricValue::Gauge { .. } => Some(metric),
            not_supported => {
                warn!("Unsupported metric type: {:?}.", not_supported);
                None
            }
        }
        .map(EncodedEvent::new)
    }

    async fn build_request(
        &self,
        mut metrics: Self::Output,
    ) -> crate::Result<hyper::Request<Vec<u8>>> {
        let metric = metrics.pop().expect("only one metric");
        let (series, data, _metadata) = metric.into_parts();
        let namespace = series
            .name
            .namespace
            .unwrap_or_else(|| self.config.default_namespace.clone());
        let metric_type = format!(
            "custom.googleapis.com/{}/metrics/{}",
            namespace, series.name.name
        );

        let end_time = data.timestamp.unwrap_or_else(chrono::Utc::now);

        let (point_value, interval, metric_kind) = match &data.value {
            MetricValue::Counter { value } => {
                let interval = gcp::GcpInterval {
                    start_time: Some(self.started),
                    end_time,
                };

                (*value, interval, gcp::GcpMetricKind::Cumulative)
            }
            MetricValue::Gauge { value } => {
                let interval = gcp::GcpInterval {
                    start_time: None,
                    end_time,
                };

                (*value, interval, gcp::GcpMetricKind::Gauge)
            }
            _ => unreachable!(),
        };

        let metric_labels = series
            .tags
            .unwrap_or_default()
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();

        let series = gcp::GcpSeries {
            time_series: &[gcp::GcpSerie {
                metric: gcp::GcpTypedResource {
                    r#type: metric_type,
                    labels: metric_labels,
                },
                resource: gcp::GcpTypedResource {
                    r#type: self.config.resource.r#type.clone(),
                    labels: self.config.resource.labels.clone(),
                },
                metric_kind,
                value_type: gcp::GcpValueType::Int64,
                points: &[gcp::GcpPoint {
                    interval,
                    value: gcp::GcpPointValue {
                        int64_value: Some(point_value as i64),
                    },
                }],
            }],
        };

        let body = serde_json::to_vec(&series).unwrap();
        let uri: Uri = format!(
            "https://monitoring.googleapis.com/v3/projects/{}/timeSeries",
            self.config.project_id
        )
        .parse()?;

        let request = hyper::Request::post(uri)
            .header("content-type", "application/json")
            .header(
                AUTHORIZATION,
                self.token.header_value()?.parse::<HeaderValue>()?,
            )
            .body(body)?;

        Ok(request)
    }
}

lazy_static! {
    static ref REQUEST_DEFAULTS: TowerRequestConfig = TowerRequestConfig {
        rate_limit_num: Some(1000),
        rate_limit_duration_secs: Some(1),
        ..Default::default()
    };
}

async fn healthcheck() -> crate::Result<()> {
    Ok(())
}
