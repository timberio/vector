use lapin::tcp::{OwnedIdentity, OwnedTLSConfig};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio_amqp::*;

/// Client certificate for rabbit authentication
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClientCertDer {
    /// Certificate embedded in config file as base64 string
    Embedded(String),
    /// Certificate on file system
    Path(PathBuf),
}

/// Tls identity configuration parameters
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub(crate) struct TlsIdentity {
    /// Client certificate and key
    pub client_cert_and_key_der: ClientCertDer,
    /// Password for client cert and key
    #[serde(default)]
    pub client_cert_and_key_password: String,
}

/// Tls configuration parameters
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub(crate) struct AmpqTlsConfig {
    /// Client certificate
    pub identity: Option<TlsIdentity>,
    /// CA certificate to use to establish trust
    pub ca_cert: PathBuf,
}


#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct AmqpConfig {
    /// Format: amqp://user:password@host:port/vhost?timeout=seconds
    pub(crate) connection_string: String,
    pub(crate) tls: Option<AmpqTlsConfig>,
}

impl Default for AmqpConfig {
    fn default() -> Self {
        Self {
            connection_string: "amqp://127.0.0.1/%2f".to_string(),
            tls: None,
        }
    }
}

impl AmqpConfig {
    pub async fn connect(&self) -> Result<(lapin::Connection, lapin::Channel), Box<dyn std::error::Error + Send + Sync>> {
        info!("Connecting to {}", self.connection_string);
        let addr = self.connection_string.clone();
        let conn = match &self.tls {
            Some(tls) => {
                let tls_config = OwnedTLSConfig {
                    identity: match &tls.identity {
                        Some(identity) => {
                            let der = match &identity.client_cert_and_key_der {
                                ClientCertDer::Embedded(cert_bytes) => base64::decode(cert_bytes)?,
                                ClientCertDer::Path(cert_path) => tokio::fs::read(cert_path.to_owned())
                                    .await?,
                            };
                            Some(OwnedIdentity {
                                der: der,
                                password: identity.client_cert_and_key_password.to_string(),
                            })
                        },
                        None => None,
                    },
                    cert_chain: Some(
                        tokio::fs::read_to_string(tls.ca_cert.to_owned()).await?,
                    ),
                };
                lapin::Connection::connect_with_config(&addr, lapin::ConnectionProperties::default().with_tokio(), tls_config.as_ref()).await
            }
            None => {
                lapin::Connection::connect(&addr, lapin::ConnectionProperties::default().with_tokio()).await
            }
        }?;
        let channel = conn.create_channel().await?;
        Ok((conn, channel))
    }
}
