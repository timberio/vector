//! Everything related to building in-cluster configuration.

use super::Config;
use crate::tls::TlsOptions;
use http::Uri;
use snafu::{ResultExt, Snafu};

impl Config {
    /// Prepares a config suitable for use when running in k8s cluster.
    pub fn in_cluster() -> Result<Self, Error> {
        let host = std::env::var("KUBERNETES_SERVICE_HOST").context(NotInCluster {
            missing: "KUBERNETES_SERVICE_HOST",
        })?;
        let port = std::env::var("KUBERNETES_SERVICE_PORT").context(NotInCluster {
            missing: "KUBERNETES_SERVICE_PORT",
        })?;

        let base = Uri::builder()
            .scheme("https")
            .authority(join_host_port(host.as_str(), port.as_str()).as_str())
            .path_and_query("/")
            .build()
            .context(InvalidUrl)?;

        let token_file = "/var/run/secrets/kubernetes.io/serviceaccount/token";
        let root_ca_file = "/var/run/secrets/kubernetes.io/serviceaccount/ca.crt";

        let token = Some(std::fs::read_to_string(token_file).context(Token)?);

        let tls_options = TlsOptions {
            ca_file: Some(root_ca_file.into()),
            ..Default::default()
        };

        Ok(Self {
            base,
            token,
            tls_options,
            proxy: Default::default(),
        })
    }
}

/// An error returned when building an in-cluster configuration.
#[derive(Debug, Snafu)]
pub enum Error {
    /// The in-cluster configuration requested while executing not in a cluster
    /// environment.
    #[snafu(display("unable to load in-cluster configuration, KUBERNETES_SERVICE_HOST and KUBERNETES_SERVICE_PORT must be defined"))]
    NotInCluster {
        /// The underlying error.
        source: std::env::VarError,

        /// The field that's missing.
        missing: &'static str,
    },

    /// The token file could not be read successfully.
    #[snafu(display("unable to read the token file"))]
    Token {
        /// The underlying error.
        source: std::io::Error,
    },

    /// The configuration resulted in an invalid URL.
    #[snafu(display("unable to construct a proper API server URL"))]
    InvalidUrl {
        /// The underlying error.
        source: http::Error,
    },
}

/// This function implements the exact same logic that Go's `net.JoinHostPort`
/// has.
/// Rust doesn't have anything like this out of the box, yet the reference
/// kubernetes client in-cluster config implementation uses it:
/// https://github.com/kubernetes/client-go/blob/3d5c80942cce510064da1ab62c579e190a0230fd/rest/config.go#L484
///
/// To avoid needlessly complicating the logic here, we simply implement the
/// `net.JoinHostPort` as it is in Go: https://golang.org/pkg/net/#JoinHostPort
fn join_host_port(host: &str, port: &str) -> String {
    if host.contains(':') {
        // If IPv6 address is used, use a special notation.
        return format!("[{}]:{}", host, port);
    }
    // Use traditional notation for domain names and IPv4 addresses.
    format!("{}:{}", host, port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_join_host_port() {
        // IPv4
        assert_eq!(join_host_port("0.0.0.0", "1234"), "0.0.0.0:1234");
        assert_eq!(join_host_port("127.0.0.1", "443"), "127.0.0.1:443");
        // IPv6
        assert_eq!(join_host_port("::", "1234"), "[::]:1234");
        assert_eq!(
            join_host_port("2001:0db8:0000:0000:0000:8a2e:0370:7334", "1234"),
            "[2001:0db8:0000:0000:0000:8a2e:0370:7334]:1234"
        );
        assert_eq!(
            join_host_port("2001:db8::8a2e:370:7334", "1234"),
            "[2001:db8::8a2e:370:7334]:1234"
        );
        // DNS
        assert_eq!(join_host_port("example.com", "1234"), "example.com:1234");
    }
}
