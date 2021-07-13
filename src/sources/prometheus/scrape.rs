use super::parser;
use crate::{
    config::{self, GenerateConfig, ProxyConfig, SourceConfig, SourceContext, SourceDescription},
    http::Auth,
    http::HttpClient,
    internal_events::{
        PrometheusErrorResponse, PrometheusEventReceived, PrometheusHttpError,
        PrometheusParseError, PrometheusRequestCompleted,
    },
    shutdown::ShutdownSignal,
    sources,
    tls::{TlsOptions, TlsSettings},
    Pipeline,
};
use futures::{stream, FutureExt, SinkExt, StreamExt, TryFutureExt};
use hyper::{Body, Request};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use std::{
    future::ready,
    time::{Duration, Instant},
};
use tokio_stream::wrappers::IntervalStream;

#[derive(Debug, Snafu)]
enum ConfigError {
    #[snafu(display("Cannot set both `endpoints` and `hosts`"))]
    BothEndpointsAndHosts,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
struct PrometheusScrapeConfig {
    // Deprecated name
    #[serde(alias = "hosts")]
    endpoints: Vec<String>,
    #[serde(default = "default_scrape_interval_secs")]
    scrape_interval_secs: u64,
    tls: Option<TlsOptions>,
    auth: Option<Auth>,
    #[serde(
        default,
        skip_serializing_if = "crate::serde::skip_serializing_if_default"
    )]
    proxy: ProxyConfig,
}

pub fn default_scrape_interval_secs() -> u64 {
    15
}

inventory::submit! {
    SourceDescription::new::<PrometheusScrapeConfig>("prometheus")
}

inventory::submit! {
    SourceDescription::new::<PrometheusScrapeConfig>("prometheus_scrape")
}

impl GenerateConfig for PrometheusScrapeConfig {
    fn generate_config() -> toml::Value {
        toml::Value::try_from(Self {
            endpoints: vec!["http://localhost:9090/metrics".to_string()],
            scrape_interval_secs: default_scrape_interval_secs(),
            tls: None,
            auth: None,
            proxy: Default::default(),
        })
        .unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "prometheus_scrape")]
impl SourceConfig for PrometheusScrapeConfig {
    async fn build(&self, cx: SourceContext) -> crate::Result<sources::Source> {
        let urls = self
            .endpoints
            .iter()
            .map(|s| s.parse::<http::Uri>().context(sources::UriParseError))
            .collect::<Result<Vec<http::Uri>, sources::BuildError>>()?;
        let tls = TlsSettings::from_options(&self.tls)?;
        Ok(prometheus(
            urls,
            tls,
            self.auth.clone(),
            cx.globals.proxy.build(&self.proxy),
            self.scrape_interval_secs,
            cx.shutdown,
            cx.out,
        ))
    }

    fn output_type(&self) -> config::DataType {
        config::DataType::Metric
    }

    fn source_type(&self) -> &'static str {
        "prometheus_scrape"
    }
}

// Add a compatibility alias to avoid breaking existing configs
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PrometheusCompatConfig {
    // Clone of PrometheusScrapeConfig to work around serde bug
    // https://github.com/serde-rs/serde/issues/1504
    #[serde(alias = "hosts")]
    endpoints: Vec<String>,
    #[serde(default = "default_scrape_interval_secs")]
    scrape_interval_secs: u64,
    tls: Option<TlsOptions>,
    auth: Option<Auth>,
    #[serde(
        default,
        skip_serializing_if = "crate::serde::skip_serializing_if_default"
    )]
    proxy: ProxyConfig,
}

#[async_trait::async_trait]
#[typetag::serde(name = "prometheus")]
impl SourceConfig for PrometheusCompatConfig {
    async fn build(&self, cx: SourceContext) -> crate::Result<sources::Source> {
        // Workaround for serde bug
        // https://github.com/serde-rs/serde/issues/1504
        let config = PrometheusScrapeConfig {
            endpoints: self.endpoints.clone(),
            scrape_interval_secs: self.scrape_interval_secs,
            tls: self.tls.clone(),
            auth: self.auth.clone(),
            proxy: self.proxy.clone(),
        };
        config.build(cx).await
    }

    fn output_type(&self) -> config::DataType {
        config::DataType::Metric
    }

    fn source_type(&self) -> &'static str {
        "prometheus_scrape"
    }
}

fn prometheus(
    urls: Vec<http::Uri>,
    tls: TlsSettings,
    auth: Option<Auth>,
    proxy: ProxyConfig,
    interval: u64,
    shutdown: ShutdownSignal,
    out: Pipeline,
) -> sources::Source {
    let out = out.sink_map_err(|error| error!(message = "Error sending metric.", %error));

    Box::pin(IntervalStream::new(tokio::time::interval(Duration::from_secs(interval)))
        .take_until(shutdown)
        .map(move |_| stream::iter(urls.clone()))
        .flatten()
        .map(move |url| {
            let client = HttpClient::new(tls.clone(), proxy.clone()).expect("Building HTTP client failed");

            let mut request = Request::get(&url)
                .body(Body::empty())
                .expect("error creating request");
            if let Some(auth) = &auth {
                auth.apply(&mut request);
            }

            let start = Instant::now();
            client
                .send(request)
                .map_err(crate::Error::from)
                .and_then(|response| async move {
                    let (header, body) = response.into_parts();
                    let body = hyper::body::to_bytes(body).await?;
                    Ok((header, body))
                })
                .into_stream()
                .filter_map(move |response| {
                    ready(match response {
                        Ok((header, body)) if header.status == hyper::StatusCode::OK => {
                            emit!(PrometheusRequestCompleted {
                                start,
                                end: Instant::now()
                            });

                            let byte_size = body.len();
                            let body = String::from_utf8_lossy(&body);

                            match parser::parse_text(&body) {
                                Ok(metrics) => {
                                    emit!(PrometheusEventReceived {
                                        byte_size,
                                        count: metrics.len(),
                                        uri: url.clone()
                                    });
                                    Some(stream::iter(metrics).map(Ok))
                                }
                                Err(error) => {
                                    if url.path() == "/" {
                                        // https://github.com/timberio/vector/pull/3801#issuecomment-700723178
                                        warn!(
                                            message = "No path is set on the endpoint and we got a parse error, did you mean to use /metrics? This behavior changed in version 0.11.",
                                            endpoint = %url
                                        );
                            }
                                    emit!(PrometheusParseError {
                                        error,
                                        url: url.clone(),
                                        body,
                                    });
                                    None
                                }
                            }
                        }
                        Ok((header, _)) => {
                            if header.status == hyper::StatusCode::NOT_FOUND && url.path() == "/" {
                                // https://github.com/timberio/vector/pull/3801#issuecomment-700723178
                                warn!(
                                    message = "No path is set on the endpoint and we got a 404, did you mean to use /metrics? This behavior changed in version 0.11.",
                                    endpoint = %url
                                );
                            }
                            emit!(PrometheusErrorResponse {
                                code: header.status,
                                url: url.clone(),
                            });
                            None
                        }
                        Err(error) => {
                            emit!(PrometheusHttpError {
                                error,
                                url: url.clone(),
                            });
                            None
                        }
                    })
                })
                .flatten()
        })
        .flatten()
        .forward(out)
        .inspect(|_| info!("Finished sending.")))
}

#[cfg(all(test, feature = "sinks-prometheus"))]
mod test {
    use super::*;
    use crate::{
        config,
        sinks::prometheus::exporter::PrometheusExporterConfig,
        test_util::{next_addr, start_topology},
        Error,
    };
    use hyper::{
        service::{make_service_fn, service_fn},
        {Body, Client, Response, Server},
    };
    use pretty_assertions::assert_eq;
    use tokio::time::{sleep, Duration};

    #[test]
    fn genreate_config() {
        crate::test_util::test_generate_config::<PrometheusScrapeConfig>();
    }

    #[tokio::test]
    async fn test_prometheus_routing() {
        let in_addr = next_addr();
        let out_addr = next_addr();

        let make_svc = make_service_fn(|_| async {
            Ok::<_, Error>(service_fn(|_| async {
                Ok::<_, Error>(Response::new(Body::from(
                    r##"
                    # HELP promhttp_metric_handler_requests_total Total number of scrapes by HTTP status code.
                    # TYPE promhttp_metric_handler_requests_total counter
                    promhttp_metric_handler_requests_total{code="200"} 100 1612411516789
                    promhttp_metric_handler_requests_total{code="404"} 7 1612411516789
                    prometheus_remote_storage_samples_in_total 57011636 1612411516789
                    # A histogram, which has a pretty complex representation in the text format:
                    # HELP http_request_duration_seconds A histogram of the request duration.
                    # TYPE http_request_duration_seconds histogram
                    http_request_duration_seconds_bucket{le="0.05"} 24054 1612411516789
                    http_request_duration_seconds_bucket{le="0.1"} 33444 1612411516789
                    http_request_duration_seconds_bucket{le="0.2"} 100392 1612411516789
                    http_request_duration_seconds_bucket{le="0.5"} 129389 1612411516789
                    http_request_duration_seconds_bucket{le="1"} 133988 1612411516789
                    http_request_duration_seconds_bucket{le="+Inf"} 144320 1612411516789
                    http_request_duration_seconds_sum 53423 1612411516789
                    http_request_duration_seconds_count 144320 1612411516789
                    # Finally a summary, which has a complex representation, too:
                    # HELP rpc_duration_seconds A summary of the RPC duration in seconds.
                    # TYPE rpc_duration_seconds summary
                    rpc_duration_seconds{code="200",quantile="0.01"} 3102 1612411516789
                    rpc_duration_seconds{code="200",quantile="0.05"} 3272 1612411516789
                    rpc_duration_seconds{code="200",quantile="0.5"} 4773 1612411516789
                    rpc_duration_seconds{code="200",quantile="0.9"} 9001 1612411516789
                    rpc_duration_seconds{code="200",quantile="0.99"} 76656 1612411516789
                    rpc_duration_seconds_sum{code="200"} 1.7560473e+07 1612411516789
                    rpc_duration_seconds_count{code="200"} 2693 1612411516789
                    "##,
                )))
            }))
        });

        tokio::spawn(async move {
            if let Err(error) = Server::bind(&in_addr).serve(make_svc).await {
                error!(message = "Server error.", %error);
            }
        });

        let mut config = config::Config::builder();
        config.add_source(
            "in",
            PrometheusScrapeConfig {
                endpoints: vec![format!("http://{}", in_addr)],
                scrape_interval_secs: 1,
                tls: None,
                auth: None,
                proxy: Default::default(),
            },
        );
        config.add_sink(
            "out",
            &["in"],
            PrometheusExporterConfig {
                address: out_addr,
                tls: None,
                default_namespace: Some("vector".into()),
                buckets: vec![1.0, 2.0, 4.0],
                quantiles: vec![],
                flush_period_secs: 1,
            },
        );

        let (topology, _crash) = start_topology(config.build().unwrap(), false).await;
        sleep(Duration::from_secs(1)).await;

        let response = Client::new()
            .get(format!("http://{}/metrics", out_addr).parse().unwrap())
            .await
            .unwrap();
        assert!(response.status().is_success());

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let lines = std::str::from_utf8(&body)
            .unwrap()
            .lines()
            .collect::<Vec<_>>();

        assert_eq!(lines, vec![
            "# HELP vector_http_request_duration_seconds http_request_duration_seconds",
            "# TYPE vector_http_request_duration_seconds histogram",
            "vector_http_request_duration_seconds_bucket{le=\"0.05\"} 24054 1612411516789",
            "vector_http_request_duration_seconds_bucket{le=\"0.1\"} 33444 1612411516789",
            "vector_http_request_duration_seconds_bucket{le=\"0.2\"} 100392 1612411516789",
            "vector_http_request_duration_seconds_bucket{le=\"0.5\"} 129389 1612411516789",
            "vector_http_request_duration_seconds_bucket{le=\"1\"} 133988 1612411516789",
            "vector_http_request_duration_seconds_bucket{le=\"+Inf\"} 144320 1612411516789",
            "vector_http_request_duration_seconds_sum 53423 1612411516789",
            "vector_http_request_duration_seconds_count 144320 1612411516789",
            "# HELP vector_prometheus_remote_storage_samples_in_total prometheus_remote_storage_samples_in_total",
            "# TYPE vector_prometheus_remote_storage_samples_in_total gauge",
            "vector_prometheus_remote_storage_samples_in_total 57011636 1612411516789",
            "# HELP vector_promhttp_metric_handler_requests_total promhttp_metric_handler_requests_total",
            "# TYPE vector_promhttp_metric_handler_requests_total counter",
            "vector_promhttp_metric_handler_requests_total{code=\"200\"} 100 1612411516789",
            "vector_promhttp_metric_handler_requests_total{code=\"404\"} 7 1612411516789",
            "# HELP vector_rpc_duration_seconds rpc_duration_seconds",
            "# TYPE vector_rpc_duration_seconds summary",
            "vector_rpc_duration_seconds{code=\"200\",quantile=\"0.01\"} 3102 1612411516789",
            "vector_rpc_duration_seconds{code=\"200\",quantile=\"0.05\"} 3272 1612411516789",
            "vector_rpc_duration_seconds{code=\"200\",quantile=\"0.5\"} 4773 1612411516789",
            "vector_rpc_duration_seconds{code=\"200\",quantile=\"0.9\"} 9001 1612411516789",
            "vector_rpc_duration_seconds{code=\"200\",quantile=\"0.99\"} 76656 1612411516789",
            "vector_rpc_duration_seconds_sum{code=\"200\"} 17560473 1612411516789",
            "vector_rpc_duration_seconds_count{code=\"200\"} 2693 1612411516789",
            ],
        );

        topology.stop().await;
    }
}

#[cfg(all(test, feature = "prometheus-integration-tests"))]
mod integration_tests {
    use super::*;
    use crate::{
        config::SourceContext,
        event::{MetricKind, MetricValue},
        test_util, Pipeline,
    };
    use tokio::time::Duration;

    #[tokio::test]
    async fn scrapes_metrics() {
        let config = PrometheusScrapeConfig {
            endpoints: vec!["http://localhost:9090/metrics".into()],
            scrape_interval_secs: 1,
            auth: None,
            tls: None,
        };

        let (tx, rx) = Pipeline::new_test();
        let source = config.build(SourceContext::new_test(tx)).await.unwrap();

        tokio::spawn(source);
        tokio::time::sleep(Duration::from_secs(1)).await;

        let events = test_util::collect_ready(rx).await;
        assert!(!events.is_empty());

        let metrics: Vec<_> = events
            .into_iter()
            .map(|event| event.into_metric())
            .collect();

        let find_metric = |name: &str| {
            metrics
                .iter()
                .find(|metric| metric.name() == name)
                .unwrap_or_else(|| panic!("Missing metric {:?}", name))
        };

        // Sample some well-known metrics
        let build = find_metric("prometheus_build_info");
        assert!(matches!(build.kind(), MetricKind::Absolute));
        assert!(matches!(build.value(), &MetricValue::Gauge { .. }));
        assert!(build.tags().unwrap().contains_key("branch"));
        assert!(build.tags().unwrap().contains_key("version"));

        let queries = find_metric("prometheus_engine_queries");
        assert!(matches!(queries.kind(), MetricKind::Absolute));
        assert!(matches!(queries.value(), &MetricValue::Gauge { .. }));

        let go_info = find_metric("go_info");
        assert!(matches!(go_info.kind(), MetricKind::Absolute));
        assert!(matches!(go_info.value(), &MetricValue::Gauge { .. }));
        assert!(go_info.tags().unwrap().contains_key("version"));
    }
}
