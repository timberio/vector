//! A Kubernetes API client built using Vector interfaces to the system
//! resources as building blocks.
//!
//! Here are a few pointers to the resources that were used as an inspiration
//! for this mod:
//!
//! - https://github.com/kubernetes/client-go/blob/master/tools/clientcmd/api/types.go
//!
//!   A part of the official Kubernetes client library (in Go) that contains
//!   the structure for KUBECONFIG files. Used for reference on naming things.
//!
//! - https://github.com/kubernetes/apimachinery/blob/master/pkg/watch/watch.go
//!
//!   The reference design of the watchers composition and interfaces that's
//!   known to work.
//!
//! - https://github.com/kubernetes/client-go/blob/master/rest/config.go
//!
//!   The reference implementation on preparing the in-cluster config.
//!

use crate::{
    config::ProxyConfig,
    http::{HttpClient, HttpError},
    tls::TlsSettings,
};
use http::{
    header::{self, HeaderValue},
    uri, Request, Response, Uri,
};
use hyper::body::Body;

pub mod config;

pub use config::Config;

/// A client to the k8s API.
///
/// Wraps our in-house [`HttpClient`].
#[derive(Debug, Clone)]
pub struct Client {
    inner: HttpClient,
    uri_scheme: uri::Scheme,
    uri_authority: uri::Authority,
    auth_header: Option<HeaderValue>,
}

impl Client {
    /// Create a new [`Client`].
    ///
    /// Takes the common kubernetes API cluster configuration [`Config`].
    ///
    /// Consumes the configuration to populate the internal state.
    /// Returns an error if the configuration is not valid.
    // TODO: add a proper error type.
    pub fn new(config: Config) -> crate::Result<Self> {
        let Config {
            base,
            tls_options,
            token,
            proxy,
        } = config;

        let tls_settings = TlsSettings::from_options(&Some(tls_options))?;
        let inner = HttpClient::new(tls_settings, ProxyConfig::from_env().build(&proxy))?;

        let uri::Parts {
            scheme, authority, ..
        } = base.into_parts();

        let uri_scheme = scheme.ok_or("no scheme")?;
        let uri_authority = authority.ok_or("no authority")?;

        let auth_header = match &token {
            Some(t) => Some(HeaderValue::from_str(format!("Bearer {}", t).as_str())?),
            None => None,
        };

        Ok(Self {
            inner,
            uri_scheme,
            uri_authority,
            auth_header,
        })
    }

    /// Alters a request according to the client configuration and sends it.
    pub async fn send<B: Into<Body>>(
        &mut self,
        req: Request<B>,
    ) -> Result<Response<Body>, HttpError> {
        let req = self.prepare_request(req);
        self.inner.send(req).await
    }

    fn prepare_request<B: Into<Body>>(&self, req: Request<B>) -> Request<Body> {
        let (mut parts, body) = req.into_parts();
        let body = body.into();

        parts.uri = self.adjust_uri(parts.uri);
        if let Some(ah) = self.auth_header.as_ref() {
            parts.headers.insert(header::AUTHORIZATION, ah.clone());
        }

        Request::from_parts(parts, body)
    }

    fn adjust_uri(&self, uri: Uri) -> Uri {
        let mut parts = uri.into_parts();
        parts.scheme = Some(self.uri_scheme.clone());
        parts.authority = Some(self.uri_authority.clone());
        Uri::from_parts(parts).unwrap()
    }
}
