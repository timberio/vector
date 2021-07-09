//! Client configuration.

use crate::config::ProxyConfig;
use crate::tls::TlsOptions;
use http::Uri;

pub mod in_cluster;
pub mod kubeconfig;
mod kubeconfig_types;

/// A k8s client configuration.
///
/// This type is designed to hold all possible variants of the configuration.
/// It also abstracts the client from the various ways to obtain the
/// configuration.
///
/// The implementation is fairly limited, and only covers the use cases we
/// support.
#[derive(Debug, Clone)]
pub struct Config {
    /// The base URL to use when constructing HTTP requests to the k8s API
    /// server.
    pub base: Uri,

    /// The bearer token to use at the `Authorization` header.
    pub token: Option<String>,

    /// The TLS configuration parameters to use at the HTTP client.
    pub tls_options: TlsOptions,

    /// The Proxy configuration at the HTTP client.
    pub proxy: ProxyConfig,
}
