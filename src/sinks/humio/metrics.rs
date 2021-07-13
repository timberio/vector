use super::{host_key, logs::HumioLogsConfig, Encoding};
use crate::{
    config::{
        DataType, GenerateConfig, ProxyConfig, SinkConfig, SinkContext, SinkDescription,
        TransformConfig,
    },
    sinks::util::{encoding::EncodingConfig, BatchConfig, Compression, TowerRequestConfig},
    sinks::{Healthcheck, VectorSink},
    template::Template,
    tls::TlsOptions,
    transforms::metric_to_log::MetricToLogConfig,
};
use futures::{stream, SinkExt, StreamExt};
use indoc::indoc;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HumioMetricsConfig {
    #[serde(flatten)]
    transform: MetricToLogConfig,

    token: String,
    // Deprecated name
    #[serde(alias = "host")]
    pub(in crate::sinks::humio) endpoint: Option<String>,
    source: Option<Template>,
    encoding: EncodingConfig<Encoding>,

    event_type: Option<Template>,

    #[serde(default = "host_key")]
    host_key: String,

    #[serde(default)]
    compression: Compression,

    #[serde(default)]
    request: TowerRequestConfig,

    #[serde(default)]
    batch: BatchConfig,

    tls: Option<TlsOptions>,

    #[serde(
        default,
        skip_serializing_if = "crate::serde::skip_serializing_if_default"
    )]
    proxy: ProxyConfig,
    // The obove settings are copied from HumioLogsConfig. In theory we should do below:
    //
    // #[serde(flatten)]
    // sink: HumioLogsConfig,
    //
    // However there is an issue in serde (https://github.com/serde-rs/serde/issues/1504) with aliased
    // fields in flattened structs which interferes with the host field alias.
    // Until that issue is fixed, we will have to just copy the fields instead.
}

inventory::submit! {
    SinkDescription::new::<HumioMetricsConfig>("humio_metrics")
}

impl GenerateConfig for HumioMetricsConfig {
    fn generate_config() -> toml::Value {
        toml::from_str(indoc! {r#"
                host_key = "hostname"
                token = "${HUMIO_TOKEN}"
                encoding.codec = "json"
            "#})
        .unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "humio_metrics")]
impl SinkConfig for HumioMetricsConfig {
    async fn build(&self, cx: SinkContext) -> crate::Result<(VectorSink, Healthcheck)> {
        let mut transform = self.transform.clone().build(&cx.globals).await?;
        let sink = HumioLogsConfig {
            token: self.token.clone(),
            endpoint: self.endpoint.clone(),
            source: self.source.clone(),
            encoding: self.encoding.clone(),
            event_type: self.event_type.clone(),
            host_key: self.host_key.clone(),
            compression: self.compression,
            request: self.request,
            batch: self.batch,
            tls: self.tls.clone(),
            proxy: self.proxy.clone(),
        };

        let (sink, healthcheck) = sink.clone().build(cx).await?;

        let sink = Box::new(sink.into_sink().with_flat_map(move |e| {
            let mut buf = Vec::with_capacity(1);
            transform.as_function().transform(&mut buf, e);
            stream::iter(buf.into_iter()).map(Ok)
        }));

        Ok((VectorSink::Sink(sink), healthcheck))
    }

    fn input_type(&self) -> DataType {
        DataType::Metric
    }

    fn sink_type(&self) -> &'static str {
        "humio_metrics"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        event::{
            metric::{MetricKind, MetricValue, StatisticKind},
            Event, Metric,
        },
        sinks::util::test::{build_test_server, load_sink},
        test_util,
    };
    use chrono::{offset::TimeZone, Utc};
    use indoc::indoc;
    use pretty_assertions::assert_eq;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<HumioMetricsConfig>();
    }

    #[test]
    fn test_endpoint_field() {
        let (config, _) = load_sink::<HumioMetricsConfig>(indoc! {r#"
            token = "atoken"
            batch.max_events = 1
            endpoint = "https://localhost:9200/"
            encoding = "json"
        "#})
        .unwrap();

        assert_eq!(Some("https://localhost:9200/".to_string()), config.endpoint);
        let (config, _) = load_sink::<HumioMetricsConfig>(indoc! {r#"
            token = "atoken"
            batch.max_events = 1
            host = "https://localhost:9200/"
            encoding = "json"
        "#})
        .unwrap();

        assert_eq!(Some("https://localhost:9200/".to_string()), config.endpoint);
    }

    #[tokio::test]
    async fn smoke_json() {
        let (mut config, cx) = load_sink::<HumioMetricsConfig>(indoc! {r#"
            token = "atoken"
            batch.max_events = 1
            encoding = "json"
        "#})
        .unwrap();

        let addr = test_util::next_addr();
        // Swap out the endpoint so we can force send it
        // to our local server
        let endpoint = format!("http://{}", addr);
        config.endpoint = Some(endpoint.clone());

        let (sink, _) = config.build(cx).await.unwrap();

        let (rx, _trigger, server) = build_test_server(addr);
        tokio::spawn(server);

        // Make our test metrics.
        let metrics = vec![
            Event::from(
                Metric::new(
                    "metric1",
                    MetricKind::Incremental,
                    MetricValue::Counter { value: 42.0 },
                )
                .with_tags(Some(
                    vec![("os.host".to_string(), "somehost".to_string())]
                        .into_iter()
                        .collect(),
                ))
                .with_timestamp(Some(Utc.ymd(2020, 8, 18).and_hms(21, 0, 1))),
            ),
            Event::from(
                Metric::new(
                    "metric2",
                    MetricKind::Absolute,
                    MetricValue::Distribution {
                        samples: vector_core::samples![1.0 => 100, 2.0 => 200, 3.0 => 300],
                        statistic: StatisticKind::Histogram,
                    },
                )
                .with_tags(Some(
                    vec![("os.host".to_string(), "somehost".to_string())]
                        .into_iter()
                        .collect(),
                ))
                .with_timestamp(Some(Utc.ymd(2020, 8, 18).and_hms(21, 0, 2))),
            ),
        ];

        let len = metrics.len();
        let _ = sink.run(stream::iter(metrics)).await.unwrap();

        let output = rx.take(len).collect::<Vec<_>>().await;
        assert_eq!(
            r#"{"event":{"counter":{"value":42.0},"kind":"incremental","name":"metric1","tags":{"os.host":"somehost"}},"fields":{},"time":1597784401.0}"#,
            output[0].1
        );
        assert_eq!(
            r#"{"event":{"distribution":{"samples":[{"rate":100,"value":1.0},{"rate":200,"value":2.0},{"rate":300,"value":3.0}],"statistic":"histogram"},"kind":"absolute","name":"metric2","tags":{"os.host":"somehost"}},"fields":{},"time":1597784402.0}"#,
            output[1].1
        );
    }
}
