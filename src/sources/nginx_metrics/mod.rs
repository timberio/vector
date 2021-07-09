use crate::{
    config::{DataType, ProxyConfig, SourceConfig, SourceContext, SourceDescription},
    event::metric::{Metric, MetricKind, MetricValue},
    event::Event,
    http::{Auth, HttpClient},
    internal_events::{
        NginxMetricsCollectCompleted, NginxMetricsEventsReceived, NginxMetricsRequestError,
        NginxMetricsStubStatusParseError,
    },
    tls::{TlsOptions, TlsSettings},
};
use bytes::Bytes;
use chrono::Utc;
use futures::{future::join_all, stream, SinkExt, StreamExt, TryFutureExt};
use http::{Request, StatusCode};
use hyper::{body::to_bytes as body_to_bytes, Body, Uri};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use std::{collections::BTreeMap, convert::TryFrom, time::Instant};
use tokio::time;
use tokio_stream::wrappers::IntervalStream;

pub mod parser;
use parser::NginxStubStatus;

macro_rules! counter {
    ($value:expr) => {
        MetricValue::Counter {
            value: $value as f64,
        }
    };
}

macro_rules! gauge {
    ($value:expr) => {
        MetricValue::Gauge {
            value: $value as f64,
        }
    };
}

#[derive(Debug, Snafu)]
enum NginxBuildError {
    #[snafu(display("Failed to parse endpoint: {}", source))]
    HostInvalidUri { source: http::uri::InvalidUri },
}

#[derive(Debug, Snafu)]
enum NginxError {
    #[snafu(display("Invalid response status: {}", status))]
    InvalidResponseStatus { status: StatusCode },
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
struct NginxMetricsConfig {
    endpoints: Vec<String>,
    #[serde(default = "default_scrape_interval_secs")]
    scrape_interval_secs: u64,
    #[serde(default = "default_namespace")]
    namespace: String,
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

pub fn default_namespace() -> String {
    "nginx".to_string()
}

inventory::submit! {
    SourceDescription::new::<NginxMetricsConfig>("nginx_metrics")
}

impl_generate_config_from_default!(NginxMetricsConfig);

#[async_trait::async_trait]
#[typetag::serde(name = "nginx_metrics")]
impl SourceConfig for NginxMetricsConfig {
    async fn build(&self, cx: SourceContext) -> crate::Result<super::Source> {
        let tls = TlsSettings::from_options(&self.tls)?;
        let proxy = cx.globals.proxy.build(&self.proxy);
        let http_client = HttpClient::new(tls, proxy)?;

        let namespace = Some(self.namespace.clone()).filter(|namespace| !namespace.is_empty());
        let mut sources = Vec::with_capacity(self.endpoints.len());
        for endpoint in self.endpoints.iter() {
            sources.push(NginxMetrics::new(
                http_client.clone(),
                endpoint.clone(),
                self.auth.clone(),
                namespace.clone(),
            )?);
        }

        let mut out = cx
            .out
            .sink_map_err(|error| error!(message = "Error sending mongodb metrics.", %error));

        let duration = time::Duration::from_secs(self.scrape_interval_secs);
        let shutdown = cx.shutdown;
        Ok(Box::pin(async move {
            let mut interval = IntervalStream::new(time::interval(duration)).take_until(shutdown);
            while interval.next().await.is_some() {
                let start = Instant::now();
                let metrics = join_all(sources.iter().map(|nginx| nginx.collect())).await;
                emit!(NginxMetricsCollectCompleted {
                    start,
                    end: Instant::now()
                });

                let mut stream = stream::iter(metrics)
                    .map(stream::iter)
                    .flatten()
                    .map(Event::Metric)
                    .map(Ok);
                out.send_all(&mut stream).await?;
            }

            Ok(())
        }))
    }

    fn output_type(&self) -> DataType {
        DataType::Metric
    }

    fn source_type(&self) -> &'static str {
        "nginx_metrics"
    }
}

#[derive(Debug)]
struct NginxMetrics {
    http_client: HttpClient,
    endpoint: String,
    auth: Option<Auth>,
    namespace: Option<String>,
    tags: BTreeMap<String, String>,
}

impl NginxMetrics {
    fn new(
        http_client: HttpClient,
        endpoint: String,
        auth: Option<Auth>,
        namespace: Option<String>,
    ) -> crate::Result<Self> {
        let mut tags = BTreeMap::new();
        tags.insert("endpoint".into(), endpoint.clone());
        tags.insert("host".into(), Self::get_endpoint_host(&endpoint)?);

        Ok(Self {
            http_client,
            endpoint,
            auth,
            namespace,
            tags,
        })
    }

    fn get_endpoint_host(endpoint: &str) -> crate::Result<String> {
        let uri: Uri = endpoint.parse().context(HostInvalidUri)?;
        Ok(match (uri.host().unwrap_or(""), uri.port()) {
            (host, None) => host.to_owned(),
            (host, Some(port)) => format!("{}:{}", host, port),
        })
    }

    async fn collect(&self) -> Vec<Metric> {
        let (up_value, mut metrics) = match self.collect_metrics().await {
            Ok(metrics) => (1.0, metrics),
            Err(()) => (0.0, vec![]),
        };

        metrics.push(self.create_metric("up", gauge!(up_value)));

        emit!(NginxMetricsEventsReceived {
            count: metrics.len(),
            uri: &self.endpoint
        });

        metrics
    }

    async fn collect_metrics(&self) -> Result<Vec<Metric>, ()> {
        let response = self.get_nginx_response().await.map_err(|error| {
            emit!(NginxMetricsRequestError {
                error,
                endpoint: &self.endpoint,
            })
        })?;

        let status = NginxStubStatus::try_from(String::from_utf8_lossy(&response).as_ref())
            .map_err(|error| {
                emit!(NginxMetricsStubStatusParseError {
                    error,
                    endpoint: &self.endpoint,
                })
            })?;

        Ok(vec![
            self.create_metric("connections_active", gauge!(status.active)),
            self.create_metric("connections_accepted_total", counter!(status.accepts)),
            self.create_metric("connections_handled_total", counter!(status.handled)),
            self.create_metric("http_requests_total", counter!(status.requests)),
            self.create_metric("connections_reading", gauge!(status.reading)),
            self.create_metric("connections_writing", gauge!(status.writing)),
            self.create_metric("connections_waiting", gauge!(status.waiting)),
        ])
    }

    async fn get_nginx_response(&self) -> crate::Result<Bytes> {
        let mut request = Request::get(&self.endpoint).body(Body::empty())?;
        if let Some(auth) = &self.auth {
            auth.apply(&mut request);
        }

        let response = self.http_client.send(request).await?;
        let (parts, body) = response.into_parts();
        match parts.status {
            StatusCode::OK => body_to_bytes(body).err_into().await,
            status => Err(Box::new(NginxError::InvalidResponseStatus { status })),
        }
    }

    fn create_metric(&self, name: &str, value: MetricValue) -> Metric {
        Metric::new(name, MetricKind::Absolute, value)
            .with_namespace(self.namespace.clone())
            .with_tags(Some(self.tags.clone()))
            .with_timestamp(Some(Utc::now()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<NginxMetricsConfig>();
    }
}

#[cfg(all(test, feature = "nginx-integration-tests"))]
mod integration_tests {
    use super::*;
    use crate::{test_util::trace_init, Pipeline};

    async fn test_nginx(endpoint: &'static str, auth: Option<Auth>) {
        trace_init();

        let (sender, mut recv) = Pipeline::new_test();

        tokio::spawn(async move {
            NginxMetricsConfig {
                endpoints: vec![endpoint.to_owned()],
                scrape_interval_secs: 15,
                namespace: "vector_nginx".to_owned(),
                tls: None,
                auth,
            }
            .build(SourceContext::new_test(sender))
            .await
            .unwrap()
            .await
            .unwrap()
        });

        let event = time::timeout(time::Duration::from_secs(3), recv.next())
            .await
            .expect("fetch metrics timeout")
            .expect("failed to get metrics from a stream");
        let mut events = vec![event];
        loop {
            match time::timeout(time::Duration::from_millis(10), recv.next()).await {
                Ok(Some(event)) => events.push(event),
                Ok(None) => break,
                Err(_) => break,
            }
        }

        assert_eq!(events.len(), 8);
    }

    #[tokio::test]
    async fn test_stub_status() {
        test_nginx("http://localhost:8010/basic_status", None).await
    }

    #[tokio::test]
    async fn test_stub_status_auth() {
        test_nginx(
            "http://localhost:8010/basic_status_auth",
            Some(Auth::Basic {
                user: "vector".to_owned(),
                password: "vector".to_owned(),
            }),
        )
        .await
    }
}
