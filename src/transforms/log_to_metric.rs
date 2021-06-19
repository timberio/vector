use crate::{
    config::{
        log_schema, DataType, GenerateConfig, GlobalOptions, TransformConfig, TransformDescription,
    },
    event::metric::{Metric, MetricKind, MetricValue, StatisticKind},
    event::{Event, Value},
    internal_events::{
        LogToMetricFieldNotFound, LogToMetricFieldNull, LogToMetricParseFloatError,
        LogToMetricTemplateParseError, TemplateRenderingFailed,
    },
    template::{Template, TemplateParseError, TemplateRenderingError},
    transforms::{FunctionTransform, Transform},
};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::convert::TryFrom;
use std::num::ParseFloatError;

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct LogToMetricConfig {
    pub metrics: Vec<MetricConfig>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct CounterConfig {
    field: String,
    name: Option<String>,
    namespace: Option<String>,
    #[serde(default = "default_increment_by_value")]
    increment_by_value: bool,
    tags: Option<IndexMap<String, String>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct GaugeConfig {
    pub field: String,
    pub name: Option<String>,
    pub namespace: Option<String>,
    pub tags: Option<IndexMap<String, String>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct SetConfig {
    field: String,
    name: Option<String>,
    namespace: Option<String>,
    tags: Option<IndexMap<String, String>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct HistogramConfig {
    field: String,
    name: Option<String>,
    namespace: Option<String>,
    tags: Option<IndexMap<String, String>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct SummaryConfig {
    field: String,
    name: Option<String>,
    namespace: Option<String>,
    tags: Option<IndexMap<String, String>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MetricConfig {
    Counter(CounterConfig),
    Histogram(HistogramConfig),
    Gauge(GaugeConfig),
    Set(SetConfig),
    Summary(SummaryConfig),
}

impl MetricConfig {
    fn field(&self) -> &str {
        match self {
            MetricConfig::Counter(CounterConfig { field, .. }) => field,
            MetricConfig::Histogram(HistogramConfig { field, .. }) => field,
            MetricConfig::Gauge(GaugeConfig { field, .. }) => field,
            MetricConfig::Set(SetConfig { field, .. }) => field,
            MetricConfig::Summary(SummaryConfig { field, .. }) => field,
        }
    }
}

fn default_increment_by_value() -> bool {
    false
}

#[derive(Debug, Clone)]
pub struct LogToMetric {
    config: LogToMetricConfig,
}

inventory::submit! {
    TransformDescription::new::<LogToMetricConfig>("log_to_metric")
}

impl GenerateConfig for LogToMetricConfig {
    fn generate_config() -> toml::Value {
        toml::Value::try_from(Self {
            metrics: vec![MetricConfig::Counter(CounterConfig {
                field: "field_name".to_string(),
                name: None,
                namespace: None,
                increment_by_value: false,
                tags: None,
            })],
        })
        .unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "log_to_metric")]
impl TransformConfig for LogToMetricConfig {
    async fn build(&self, _globals: &GlobalOptions) -> crate::Result<Transform<Event>> {
        Ok(Transform::function(LogToMetric::new(self.clone())))
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn output_type(&self) -> DataType {
        DataType::Metric
    }

    fn transform_type(&self) -> &'static str {
        "log_to_metric"
    }
}

impl LogToMetric {
    pub fn new(config: LogToMetricConfig) -> Self {
        LogToMetric { config }
    }
}

enum TransformError {
    FieldNotFound {
        field: String,
    },
    FieldNull {
        field: String,
    },
    TemplateParseError(TemplateParseError),
    TemplateRenderingError(TemplateRenderingError),
    ParseFloatError {
        field: String,
        error: ParseFloatError,
    },
}

fn render_template(s: &str, event: &Event) -> Result<String, TransformError> {
    let template = Template::try_from(s).map_err(TransformError::TemplateParseError)?;
    template
        .render_string(event)
        .map_err(TransformError::TemplateRenderingError)
}

fn render_tags(
    tags: &Option<IndexMap<String, String>>,
    event: &Event,
) -> Result<Option<BTreeMap<String, String>>, TransformError> {
    Ok(match tags {
        None => None,
        Some(tags) => {
            let mut map = BTreeMap::new();
            for (name, value) in tags {
                match render_template(value, event) {
                    Ok(tag) => {
                        map.insert(name.to_string(), tag);
                    }
                    Err(TransformError::TemplateRenderingError(error)) => {
                        emit!(TemplateRenderingFailed {
                            error,
                            drop_event: false,
                            field: Some(name.as_str()),
                        });
                    }
                    Err(other) => return Err(other),
                }
            }
            if !map.is_empty() {
                Some(map)
            } else {
                None
            }
        }
    })
}

fn to_metric(config: &MetricConfig, event: &Event) -> Result<Metric, TransformError> {
    let log = event.as_log();

    let timestamp = log
        .get(log_schema().timestamp_key())
        .and_then(Value::as_timestamp)
        .cloned();
    let metadata = event.metadata().clone();

    let field = config.field();

    let value = match log.get(field) {
        None => Err(TransformError::FieldNotFound {
            field: field.to_string(),
        }),
        Some(Value::Null) => Err(TransformError::FieldNull {
            field: field.to_string(),
        }),
        Some(value) => Ok(value),
    }?;

    match config {
        MetricConfig::Counter(counter) => {
            let value = if counter.increment_by_value {
                value.to_string_lossy().parse().map_err(|error| {
                    TransformError::ParseFloatError {
                        field: counter.field.clone(),
                        error,
                    }
                })?
            } else {
                1.0
            };

            let name = counter.name.as_ref().unwrap_or(&counter.field);
            let name = render_template(&name, &event)?;

            let namespace = counter.namespace.as_ref();
            let namespace = namespace
                .map(|namespace| render_template(namespace, &event))
                .transpose()?;

            let tags = render_tags(&counter.tags, &event)?;

            Ok(Metric::new_with_metadata(
                name,
                MetricKind::Incremental,
                MetricValue::Counter { value },
                metadata,
            )
            .with_namespace(namespace)
            .with_tags(tags)
            .with_timestamp(timestamp))
        }
        MetricConfig::Histogram(hist) => {
            let value = value.to_string_lossy().parse().map_err(|error| {
                TransformError::ParseFloatError {
                    field: field.to_string(),
                    error,
                }
            })?;

            let name = hist.name.as_ref().unwrap_or(&hist.field);
            let name = render_template(&name, &event)?;

            let namespace = hist.namespace.as_ref();
            let namespace = namespace
                .map(|namespace| render_template(namespace, &event))
                .transpose()?;

            let tags = render_tags(&hist.tags, &event)?;

            Ok(Metric::new_with_metadata(
                name,
                MetricKind::Incremental,
                MetricValue::Distribution {
                    samples: vector_core::samples![value => 1],
                    statistic: StatisticKind::Histogram,
                },
                metadata,
            )
            .with_namespace(namespace)
            .with_tags(tags)
            .with_timestamp(timestamp))
        }
        MetricConfig::Summary(summary) => {
            let value = value.to_string_lossy().parse().map_err(|error| {
                TransformError::ParseFloatError {
                    field: field.to_string(),
                    error,
                }
            })?;

            let name = summary.name.as_ref().unwrap_or(&summary.field);
            let name = render_template(&name, &event)?;

            let namespace = summary.namespace.as_ref();
            let namespace = namespace
                .map(|namespace| render_template(namespace, &event))
                .transpose()?;

            let tags = render_tags(&summary.tags, &event)?;

            Ok(Metric::new_with_metadata(
                name,
                MetricKind::Incremental,
                MetricValue::Distribution {
                    samples: vector_core::samples![value => 1],
                    statistic: StatisticKind::Summary,
                },
                metadata,
            )
            .with_namespace(namespace)
            .with_tags(tags)
            .with_timestamp(timestamp))
        }
        MetricConfig::Gauge(gauge) => {
            let value = value.to_string_lossy().parse().map_err(|error| {
                TransformError::ParseFloatError {
                    field: field.to_string(),
                    error,
                }
            })?;

            let name = gauge.name.as_ref().unwrap_or(&gauge.field);
            let name = render_template(&name, &event)?;

            let namespace = gauge.namespace.as_ref();
            let namespace = namespace
                .map(|namespace| render_template(namespace, &event))
                .transpose()?;

            let tags = render_tags(&gauge.tags, &event)?;

            Ok(Metric::new_with_metadata(
                name,
                MetricKind::Absolute,
                MetricValue::Gauge { value },
                metadata,
            )
            .with_namespace(namespace)
            .with_tags(tags)
            .with_timestamp(timestamp))
        }
        MetricConfig::Set(set) => {
            let value = value.to_string_lossy();

            let name = set.name.as_ref().unwrap_or(&set.field);
            let name = render_template(&name, &event)?;

            let namespace = set.namespace.as_ref();
            let namespace = namespace
                .map(|namespace| render_template(namespace, &event))
                .transpose()?;

            let tags = render_tags(&set.tags, &event)?;

            Ok(Metric::new_with_metadata(
                name,
                MetricKind::Incremental,
                MetricValue::Set {
                    values: std::iter::once(value).collect(),
                },
                metadata,
            )
            .with_namespace(namespace)
            .with_tags(tags)
            .with_timestamp(timestamp))
        }
    }
}

impl FunctionTransform<Event> for LogToMetric {
    fn transform(&mut self, output: &mut Vec<Event>, event: Event) {
        for config in self.config.metrics.iter() {
            match to_metric(&config, &event) {
                Ok(metric) => {
                    output.push(Event::Metric(metric));
                }
                Err(TransformError::FieldNull { field }) => emit!(LogToMetricFieldNull {
                    field: field.as_ref()
                }),
                Err(TransformError::FieldNotFound { field }) => emit!(LogToMetricFieldNotFound {
                    field: field.as_ref()
                }),
                Err(TransformError::ParseFloatError { field, error }) => {
                    emit!(LogToMetricParseFloatError {
                        field: field.as_ref(),
                        error
                    })
                }
                Err(TransformError::TemplateRenderingError(error)) => {
                    emit!(TemplateRenderingFailed {
                        error,
                        drop_event: false,
                        field: None,
                    })
                }
                Err(TransformError::TemplateParseError(error)) => {
                    emit!(LogToMetricTemplateParseError { error })
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transforms::test::transform_one;
    use crate::{
        config::log_schema,
        event::metric::{Metric, MetricKind, MetricValue, StatisticKind},
        event::Event,
    };
    use chrono::{offset::TimeZone, DateTime, Utc};

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<LogToMetricConfig>();
    }

    fn parse_config(s: &str) -> LogToMetricConfig {
        toml::from_str(s).unwrap()
    }

    fn ts() -> DateTime<Utc> {
        Utc.ymd(2018, 11, 14).and_hms_nano(8, 9, 10, 11)
    }

    fn create_event(key: &str, value: impl Into<Value> + std::fmt::Debug) -> Event {
        let mut log = Event::from("i am a log");
        log.as_mut_log().insert(key, value);
        log.as_mut_log().insert(log_schema().timestamp_key(), ts());
        log
    }

    #[test]
    fn count_http_status_codes() {
        let config = parse_config(
            r#"
            [[metrics]]
            type = "counter"
            field = "status"
            "#,
        );

        let event = create_event("status", "42");
        let metadata = event.metadata().clone();
        let mut transform = LogToMetric::new(config);
        let metric = transform_one(&mut transform, event).unwrap();

        assert_eq!(
            metric.into_metric(),
            Metric::new_with_metadata(
                "status",
                MetricKind::Incremental,
                MetricValue::Counter { value: 1.0 },
                metadata,
            )
            .with_timestamp(Some(ts()))
        );
    }

    #[test]
    fn count_http_requests_with_tags() {
        let config = parse_config(
            r#"
            [[metrics]]
            type = "counter"
            field = "message"
            name = "http_requests_total"
            namespace = "app"
            tags = {method = "{{method}}", code = "{{code}}", missing_tag = "{{unknown}}", host = "localhost"}
            "#,
        );

        let mut event = create_event("message", "i am log");
        event.as_mut_log().insert("method", "post");
        event.as_mut_log().insert("code", "200");
        let metadata = event.metadata().clone();

        let mut transform = LogToMetric::new(config);
        let metric = transform_one(&mut transform, event).unwrap();

        assert_eq!(
            metric.into_metric(),
            Metric::new_with_metadata(
                "http_requests_total",
                MetricKind::Incremental,
                MetricValue::Counter { value: 1.0 },
                metadata,
            )
            .with_namespace(Some("app"))
            .with_tags(Some(
                vec![
                    ("method".to_owned(), "post".to_owned()),
                    ("code".to_owned(), "200".to_owned()),
                    ("host".to_owned(), "localhost".to_owned()),
                ]
                .into_iter()
                .collect(),
            ))
            .with_timestamp(Some(ts()))
        );
    }

    #[test]
    fn count_exceptions() {
        let config = parse_config(
            r#"
            [[metrics]]
            type = "counter"
            field = "backtrace"
            name = "exception_total"
            "#,
        );

        let event = create_event("backtrace", "message");
        let metadata = event.metadata().clone();
        let mut transform = LogToMetric::new(config);
        let metric = transform_one(&mut transform, event).unwrap();

        assert_eq!(
            metric.into_metric(),
            Metric::new_with_metadata(
                "exception_total",
                MetricKind::Incremental,
                MetricValue::Counter { value: 1.0 },
                metadata
            )
            .with_timestamp(Some(ts()))
        );
    }

    #[test]
    fn count_exceptions_no_match() {
        let config = parse_config(
            r#"
            [[metrics]]
            type = "counter"
            field = "backtrace"
            name = "exception_total"
            "#,
        );

        let event = create_event("success", "42");
        let mut transform = LogToMetric::new(config);

        assert_eq!(transform_one(&mut transform, event), None);
    }

    #[test]
    fn sum_order_amounts() {
        let config = parse_config(
            r#"
            [[metrics]]
            type = "counter"
            field = "amount"
            name = "amount_total"
            increment_by_value = true
            "#,
        );

        let event = create_event("amount", "33.99");
        let metadata = event.metadata().clone();
        let mut transform = LogToMetric::new(config);
        let metric = transform_one(&mut transform, event).unwrap();

        assert_eq!(
            metric.into_metric(),
            Metric::new_with_metadata(
                "amount_total",
                MetricKind::Incremental,
                MetricValue::Counter { value: 33.99 },
                metadata,
            )
            .with_timestamp(Some(ts()))
        );
    }

    #[test]
    fn memory_usage_gauge() {
        let config = parse_config(
            r#"
            [[metrics]]
            type = "gauge"
            field = "memory_rss"
            name = "memory_rss_bytes"
            "#,
        );

        let event = create_event("memory_rss", "123");
        let metadata = event.metadata().clone();
        let mut transform = LogToMetric::new(config);
        let metric = transform_one(&mut transform, event).unwrap();

        assert_eq!(
            metric.into_metric(),
            Metric::new_with_metadata(
                "memory_rss_bytes",
                MetricKind::Absolute,
                MetricValue::Gauge { value: 123.0 },
                metadata,
            )
            .with_timestamp(Some(ts()))
        );
    }

    #[test]
    fn parse_failure() {
        let config = parse_config(
            r#"
            [[metrics]]
            type = "counter"
            field = "status"
            name = "status_total"
            increment_by_value = true
            "#,
        );

        let event = create_event("status", "not a number");
        let mut transform = LogToMetric::new(config);

        assert_eq!(transform_one(&mut transform, event), None);
    }

    #[test]
    fn missing_field() {
        let config = parse_config(
            r#"
            [[metrics]]
            type = "counter"
            field = "status"
            name = "status_total"
            "#,
        );

        let event = create_event("not foo", "not a number");
        let mut transform = LogToMetric::new(config);

        assert_eq!(transform_one(&mut transform, event), None);
    }

    #[test]
    fn null_field() {
        let config = parse_config(
            r#"
            [[metrics]]
            type = "counter"
            field = "status"
            name = "status_total"
            "#,
        );

        let event = create_event("status", Value::Null);
        let mut transform = LogToMetric::new(config);

        assert_eq!(transform_one(&mut transform, event), None);
    }

    #[test]
    fn multiple_metrics() {
        let config = parse_config(
            r#"
            [[metrics]]
            type = "counter"
            field = "status"

            [[metrics]]
            type = "counter"
            field = "backtrace"
            name = "exception_total"
            "#,
        );

        let mut event = Event::from("i am a log");
        event
            .as_mut_log()
            .insert(log_schema().timestamp_key(), ts());
        event.as_mut_log().insert("status", "42");
        event.as_mut_log().insert("backtrace", "message");
        let metadata = event.metadata().clone();

        let mut transform = LogToMetric::new(config);

        let mut output = Vec::new();
        transform.transform(&mut output, event);
        assert_eq!(2, output.len());
        assert_eq!(
            output.pop().unwrap().into_metric(),
            Metric::new_with_metadata(
                "exception_total",
                MetricKind::Incremental,
                MetricValue::Counter { value: 1.0 },
                metadata.clone(),
            )
            .with_timestamp(Some(ts()))
        );
        assert_eq!(
            output.pop().unwrap().into_metric(),
            Metric::new_with_metadata(
                "status",
                MetricKind::Incremental,
                MetricValue::Counter { value: 1.0 },
                metadata,
            )
            .with_timestamp(Some(ts()))
        );
    }

    #[test]
    fn multiple_metrics_with_multiple_templates() {
        let config = parse_config(
            r#"
            [[metrics]]
            type = "set"
            field = "status"
            name = "{{host}}_{{worker}}_status_set"

            [[metrics]]
            type = "counter"
            field = "backtrace"
            name = "{{service}}_exception_total"
            namespace = "{{host}}"
            "#,
        );

        let mut event = Event::from("i am a log");
        event
            .as_mut_log()
            .insert(log_schema().timestamp_key(), ts());
        event.as_mut_log().insert("status", "42");
        event.as_mut_log().insert("backtrace", "message");
        event.as_mut_log().insert("host", "local");
        event.as_mut_log().insert("worker", "abc");
        event.as_mut_log().insert("service", "xyz");
        let metadata = event.metadata().clone();

        let mut transform = LogToMetric::new(config);

        let mut output = Vec::new();
        transform.transform(&mut output, event);
        assert_eq!(2, output.len());
        assert_eq!(
            output.pop().unwrap().into_metric(),
            Metric::new_with_metadata(
                "xyz_exception_total",
                MetricKind::Incremental,
                MetricValue::Counter { value: 1.0 },
                metadata.clone(),
            )
            .with_namespace(Some("local"))
            .with_timestamp(Some(ts()))
        );
        assert_eq!(
            output.pop().unwrap().into_metric(),
            Metric::new_with_metadata(
                "local_abc_status_set",
                MetricKind::Incremental,
                MetricValue::Set {
                    values: vec!["42".into()].into_iter().collect()
                },
                metadata,
            )
            .with_timestamp(Some(ts()))
        );
    }

    #[test]
    fn user_ip_set() {
        let config = parse_config(
            r#"
            [[metrics]]
            type = "set"
            field = "user_ip"
            name = "unique_user_ip"
            "#,
        );

        let event = create_event("user_ip", "1.2.3.4");
        let metadata = event.metadata().clone();
        let mut transform = LogToMetric::new(config);
        let metric = transform_one(&mut transform, event).unwrap();

        assert_eq!(
            metric.into_metric(),
            Metric::new_with_metadata(
                "unique_user_ip",
                MetricKind::Incremental,
                MetricValue::Set {
                    values: vec!["1.2.3.4".into()].into_iter().collect()
                },
                metadata,
            )
            .with_timestamp(Some(ts()))
        );
    }

    #[test]
    fn response_time_histogram() {
        let config = parse_config(
            r#"
            [[metrics]]
            type = "histogram"
            field = "response_time"
            "#,
        );

        let event = create_event("response_time", "2.5");
        let metadata = event.metadata().clone();
        let mut transform = LogToMetric::new(config);
        let metric = transform_one(&mut transform, event).unwrap();

        assert_eq!(
            metric.into_metric(),
            Metric::new_with_metadata(
                "response_time",
                MetricKind::Incremental,
                MetricValue::Distribution {
                    samples: vector_core::samples![2.5 => 1],
                    statistic: StatisticKind::Histogram
                },
                metadata
            )
            .with_timestamp(Some(ts()))
        );
    }

    #[test]
    fn response_time_summary() {
        let config = parse_config(
            r#"
            [[metrics]]
            type = "summary"
            field = "response_time"
            "#,
        );

        let event = create_event("response_time", "2.5");
        let metadata = event.metadata().clone();
        let mut transform = LogToMetric::new(config);
        let metric = transform_one(&mut transform, event).unwrap();

        assert_eq!(
            metric.into_metric(),
            Metric::new_with_metadata(
                "response_time",
                MetricKind::Incremental,
                MetricValue::Distribution {
                    samples: vector_core::samples![2.5 => 1],
                    statistic: StatisticKind::Summary
                },
                metadata
            )
            .with_timestamp(Some(ts()))
        );
    }
}
