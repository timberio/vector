use super::Result;
use crate::{
    config::{
        self,
        provider::{ProviderConfig, ProviderDescription},
        ProxyConfig,
    },
    http::HttpClient,
    signal,
    tls::{TlsOptions, TlsSettings},
};
use async_stream::stream;
use bytes::Buf;
use futures::Stream;
use hyper::Body;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use tokio::time;
use url::Url;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct RequestConfig {
    #[serde(default)]
    pub headers: IndexMap<String, String>,
}

impl Default for RequestConfig {
    fn default() -> Self {
        Self {
            headers: IndexMap::new(),
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields, default)]
pub struct HttpConfig {
    url: Option<Url>,
    request: RequestConfig,
    poll_interval_secs: u64,
    #[serde(flatten)]
    tls_options: Option<TlsOptions>,
    #[serde(
        default,
        skip_serializing_if = "crate::serde::skip_serializing_if_default"
    )]
    proxy: ProxyConfig,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            url: None,
            request: RequestConfig::default(),
            poll_interval_secs: 30,
            tls_options: None,
            proxy: Default::default(),
        }
    }
}

/// Makes an HTTP request to the provided endpoint, returning the String body.
async fn http_request(
    url: &Url,
    tls_options: &Option<TlsOptions>,
    headers: &IndexMap<String, String>,
    proxy: ProxyConfig,
) -> std::result::Result<bytes::Bytes, &'static str> {
    let tls_settings = TlsSettings::from_options(tls_options).map_err(|_| "Invalid TLS options")?;
    let http_client =
        HttpClient::<Body>::new(tls_settings, proxy).map_err(|_| "Invalid TLS settings")?;

    // Build HTTP request.
    let mut builder = http::request::Builder::new().uri(url.to_string());

    // Augment with headers. These may be required e.g. for authentication to
    // private endpoints.
    for (header, value) in headers.iter() {
        builder = builder.header(header.as_str(), value.as_str());
    }

    let request = builder
        .body(Body::empty())
        .map_err(|_| "Couldn't create HTTP request")?;

    info!(
        message = "Attempting to retrieve configuration.",
        url = ?url.as_str()
    );

    let response = http_client.send(request).await.map_err(|err| {
        let message = "HTTP error";
        error!(
            message = ?message,
            error = ?err,
            url = ?url.as_str());
        message
    })?;

    info!(message = "Response received.", url = ?url.as_str());

    hyper::body::to_bytes(response.into_body())
        .await
        .map_err(|err| {
            let message = "Error interpreting response.";
            let cause = err.into_cause();
            error!(
                    message = ?message,
                    error = ?cause);

            message
        })
}

/// Calls `http_request`, serializing the result to a `ConfigBuilder`.
async fn http_request_to_config_builder(
    url: &Url,
    tls_options: &Option<TlsOptions>,
    headers: &IndexMap<String, String>,
    proxy: ProxyConfig,
) -> Result {
    let config_str = http_request(url, tls_options, headers, proxy)
        .await
        .map_err(|e| vec![e.to_owned()])?;

    let (config_builder, warnings) = config::load(config_str.chunk(), None)?;

    for warning in warnings.into_iter() {
        warn!("{}", warning);
    }

    Ok(config_builder)
}

/// Polls the HTTP endpoint after/every `poll_interval_secs`, returning a stream of `ConfigBuilder`.
fn poll_http(
    poll_interval_secs: u64,
    url: Url,
    tls_options: Option<TlsOptions>,
    headers: IndexMap<String, String>,
    proxy: ProxyConfig,
) -> impl Stream<Item = signal::SignalTo> {
    let duration = time::Duration::from_secs(poll_interval_secs);
    let mut interval = time::interval_at(time::Instant::now() + duration, duration);

    stream! {
        loop {
            interval.tick().await;

            match http_request_to_config_builder(&url, &tls_options, &headers, proxy.clone()).await {
                Ok(config_builder) => yield signal::SignalTo::ReloadFromConfigBuilder(config_builder),
                Err(_) => return,
            };

            info!(
                message = "HTTP provider is waiting.",
                poll_interval_secs = ?poll_interval_secs,
                url = ?url.as_str());
        }
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "http")]
impl ProviderConfig for HttpConfig {
    async fn build(&mut self, signal_handler: &mut signal::SignalHandler) -> Result {
        let url = self
            .url
            .take()
            .ok_or_else(|| vec!["URL is required for the `http` provider.".to_owned()])?;

        let tls_options = self.tls_options.take();
        let poll_interval_secs = self.poll_interval_secs;
        let request = self.request.clone();

        let proxy = ProxyConfig::from_env().build(&self.proxy);
        let config_builder =
            http_request_to_config_builder(&url, &tls_options, &request.headers, proxy.clone())
                .await?;

        // Poll for changes to remote configuration.
        signal_handler.add(poll_http(
            poll_interval_secs,
            url,
            tls_options,
            request.headers.clone(),
            proxy,
        ));

        Ok(config_builder)
    }

    fn provider_type(&self) -> &'static str {
        "http"
    }
}

inventory::submit! {
    ProviderDescription::new::<HttpConfig>("http")
}

impl_generate_config_from_default!(HttpConfig);
