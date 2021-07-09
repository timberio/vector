//! Everything related to building a kubernetes client configuration from a kubeconfig file.
//! Not all features of a kubeconfig are currently supported,
//! e.g. password and OIDC auth as well as inlined certificates and keys.

use super::Config;
use crate::tls::TlsOptions;
use snafu::{OptionExt, ResultExt, Snafu};

use crate::kubernetes::client::config::kubeconfig_types::*;
use std::path::{Path, PathBuf};

impl Config {
    /// Prepares a config by reading a kubeconfig file from defined path
    pub fn kubeconfig(path: &Path) -> Result<Self, Error> {
        kubeconfig_reader(&std::fs::read_to_string(path).context(Io)?)
    }
}

fn kubeconfig_reader(config: &str) -> Result<Config, Error> {
    let kc: Kubeconfig = serde_yaml::from_str(config).context(Parse)?;

    // resolve "current_context"
    let current_context = &kc.current_context;
    let current_context = &kc
        .contexts
        .iter()
        .find(|c| &c.name == current_context)
        .context(MissingRelation {
            from: "current_context",
            missing: "context",
        })?
        .context;
    let current_cluster = &current_context.cluster;
    let current_user = &current_context.user;

    // resolve cluster
    let cluster = &kc
        .clusters
        .iter()
        .find(|c| &c.name == current_cluster)
        .context(MissingRelation {
            from: "context.cluster",
            missing: "cluster",
        })?
        .cluster;
    // resolve user
    let user = &kc
        .auth_infos
        .iter()
        .find(|a| &a.name == current_user)
        .context(MissingRelation {
            from: "context.user",
            missing: "auth_info",
        })?
        .auth_info;

    let base = cluster.server.parse().context(InvalidUrl)?;

    let token = match &user.token {
        Some(t) => Some(t.clone()),
        None => match &user.token_file {
            Some(file) => Some(std::fs::read_to_string(&file).context(Token)?),
            None => None,
        },
    };

    let tls_options = TlsOptions {
        ca_file: cluster.certificate_authority.as_ref().map(PathBuf::from),
        crt_file: user.client_certificate.as_ref().map(PathBuf::from),
        key_file: user.client_key.as_ref().map(PathBuf::from),
        ..Default::default()
    };

    Ok(Config {
        base,
        token,
        tls_options,
        proxy: Default::default(),
    })
}

/// An error returned when building an in-cluster configuration.
#[derive(Debug, Snafu)]
pub enum Error {
    /// The kube_config file does not exist or could not be opened for reading.
    #[snafu(display("unable to load kubernetes configuration (kube_config)"))]
    Io {
        /// The underlying error.
        source: std::io::Error,
    },

    /// The kube_config file could not be parsed as Json or Yaml
    #[snafu(display("unable to parse kubernetes configuration (kube_config)"))]
    Parse {
        /// The underlying error.
        source: serde_yaml::Error,
    },

    /// The token file could not be read successfully.
    #[snafu(display("kube_config data relation could not be resolved"))]
    MissingRelation {
        /// What did we look for?
        missing: &'static str,

        /// Where did we look?
        from: &'static str,
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
        source: http::uri::InvalidUri,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    const KUBECONFIG_JSON: &str = r#"
        {
          "apiVersion": "v1",
          "clusters": [
            {
              "cluster": {
                "certificate-authority": "/certs/public.pem",
                "server": "https://kubernetes.json.dev:4443"
              },
              "name": "public"
            }
          ],
          "contexts": [
            {
              "name": "bar",
              "context": {
                "cluster": "public",
                "user": "alice"
              }
            },
            {
              "name": "foo",
              "context": {
                "cluster": "public",
                "user": "bob"
              }
            }
          ],
          "current-context": "foo",
          "kind": "Config",
          "users": [
            {
              "name": "alice",
              "user": {
                "client-certificate": "/certs/alice.crt",
                "client-key": "/certs/alice.key"
              }
            },
            {
              "name": "bob",
              "user": {
                "client-certificate": "/certs/bob.crt",
                "client-key": "/certs/bob.key"
              }
            }
          ]
        }
    "#;

    const KUBECONFIG_YAML: &str = r#"
        apiVersion: v1
        clusters:
        - cluster:
            certificate-authority: "/certs/public.pem"
            server: https://kubernetes.yaml.dev:4443
          name: public
        contexts:
        - name: bar
          context:
            cluster: public
            user: alice
        - name: foo
          context:
            cluster: public
            user: bob
        current-context: bar
        kind: Config
        users:
        - name: alice
          user:
            token: abcdef654321
        - name: bob
          user:
            client-certificate: "/certs/bob.crt"
            client-key: "/certs/bob.key"
    "#;

    #[test]
    fn test_read_kubeconfig() {
        let kc = kubeconfig_reader(KUBECONFIG_JSON).unwrap();
        assert_eq!(kc.base, "https://kubernetes.json.dev:4443");
        assert_eq!(kc.token, None);
        assert_eq!(
            kc.tls_options.ca_file.unwrap().to_str().unwrap(),
            "/certs/public.pem"
        );
        assert_eq!(
            kc.tls_options.crt_file.unwrap().to_str().unwrap(),
            "/certs/bob.crt"
        );
        assert_eq!(
            kc.tls_options.key_file.unwrap().to_str().unwrap(),
            "/certs/bob.key"
        );

        let kc = kubeconfig_reader(KUBECONFIG_YAML).unwrap();
        assert_eq!(kc.base, "https://kubernetes.yaml.dev:4443");
        assert_eq!(kc.token, Some("abcdef654321".to_string()));
        assert_eq!(
            kc.tls_options.ca_file.unwrap().to_str().unwrap(),
            "/certs/public.pem"
        );
        assert_eq!(kc.tls_options.crt_file, None);
        assert_eq!(kc.tls_options.key_file, None);
    }
}
