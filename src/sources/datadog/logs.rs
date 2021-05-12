use crate::{
    config::{
        log_schema, DataType, GenerateConfig, Resource, SourceConfig, SourceContext,
        SourceDescription,
    },
    event::Event,
    sources::{
        self,
        util::{decode_body, Encoding, ErrorMessage, HttpSource, HttpSourceAuthConfig},
    },
    tls::TlsConfig,
};
use bytes::Bytes;
use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, net::SocketAddr};

use warp::http::HeaderMap;

lazy_static! {
    static ref API_KEY_MATCHER: Regex =
        Regex::new(r"^/v1/input/(?P<api_key>[[:alnum:]]{32})/??").unwrap();
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct DatadogLogsConfig {
    address: SocketAddr,
    tls: Option<TlsConfig>,
    auth: Option<HttpSourceAuthConfig>,
    drop_invalid_api_key: bool,
    valid_api_keys: Option<Vec<String>>,
}

inventory::submit! {
    SourceDescription::new::<DatadogLogsConfig>("datadog_logs")
}

impl GenerateConfig for DatadogLogsConfig {
    fn generate_config() -> toml::Value {
        toml::Value::try_from(Self {
            address: "0.0.0.0:8080".parse().unwrap(),
            tls: None,
            auth: None,
            drop_invalid_api_key: false,
            valid_api_keys: None,
        })
        .unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "datadog_logs")]
impl SourceConfig for DatadogLogsConfig {
    async fn build(&self, cx: SourceContext) -> crate::Result<sources::Source> {
        let source = DatadogLogsSource {
            drop_invalid_api_key: self.drop_invalid_api_key,
            valid_api_keys: self.valid_api_keys.to_owned().unwrap_or(Vec::new())
        };
        // We accept /v1/input & /v1/input/<API_KEY>
        source.run(
            self.address,
            "/v1/input",
            false,
            &self.tls,
            &self.auth,
            cx.out,
            cx.shutdown,
        )
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn source_type(&self) -> &'static str {
        "datadog_logs"
    }

    fn resources(&self) -> Vec<Resource> {
        vec![Resource::tcp(self.address)]
    }
}

#[derive(Clone, Default)]
struct DatadogLogsSource {
    drop_invalid_api_key: bool,
    valid_api_keys: Vec<String>,
}

impl HttpSource for DatadogLogsSource {
    fn build_event(
        &self,
        body: Bytes,
        header_map: HeaderMap,
        _query_parameters: HashMap<String, String>,
        request_path: &str,
    ) -> Result<Vec<Event>, ErrorMessage> {
        if body.is_empty() {
            // The datadog agent may sent empty payload as keep alive
            debug!(
                message = "Empty payload ignored.",
                internal_log_rate_secs = 30
            );
            return Ok(Vec::new());
        }

        let api_key = filter_api_key(
            extract_api_key(&header_map, request_path),
            self.drop_invalid_api_key,
            &self.valid_api_keys,
        );

        decode_body(body, Encoding::Json).map(|mut events| {
            // Add source type & Datadog API key
            let key = log_schema().source_type_key();
            for event in events.iter_mut() {
                let log = event.as_mut_log();
                log.try_insert(key, Bytes::from("datadog_logs"));
                if let Some(k) = &api_key {
                    log.insert("dd_api_key", k.clone());
                }
            }
            events
        })
    }
}

fn extract_api_key<'a>(headers: &'a HeaderMap, path: &'a str) -> Option<String> {
    // Grab from URL first
    API_KEY_MATCHER
        .captures(path)
        .and_then(|cap| cap.name("api_key").map(|key| key.as_str()))
        // Try from header next
        .or_else(|| headers.get("dd-api-key").and_then(|key| key.to_str().ok()))
        .map(str::to_owned)
}

fn filter_api_key(api_key: Option<String>, drop_invalid_api_keys: bool, valid_api_keys: &Vec<String>) -> Option<String> {
    if let Some(ref key) = api_key {
        if drop_invalid_api_keys && !valid_api_keys.is_empty() && !valid_api_keys.contains(key) {
            return None;
        }
    }
    api_key
}

#[cfg(test)]
mod tests {
    use super::DatadogLogsConfig;

    use crate::{
        config::{log_schema, SourceConfig, SourceContext},
        event::Event,
        test_util::{collect_n, next_addr, trace_init, wait_for_tcp},
        Pipeline,
    };
    use futures::channel::mpsc;
    use http::HeaderMap;
    use pretty_assertions::assert_eq;
    use std::net::SocketAddr;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<DatadogLogsConfig>();
    }

    async fn source(drop_invalid_api_key: bool, valid_api_keys: Option<Vec<String>>) -> (mpsc::Receiver<Event>, SocketAddr) {
        let (sender, recv) = Pipeline::new_test();
        let address = next_addr();
        tokio::spawn(async move {
            DatadogLogsConfig {
                address,
                tls: None,
                auth: None,
                drop_invalid_api_key,
                valid_api_keys,
            }
            .build(SourceContext::new_test(sender))
            .await
            .unwrap()
            .await
            .unwrap();
        });
        wait_for_tcp(address).await;
        (recv, address)
    }

    async fn send_with_path(
        address: SocketAddr,
        body: &str,
        headers: HeaderMap,
        path: &str,
    ) -> u16 {
        reqwest::Client::new()
            .post(&format!("http://{}{}", address, path))
            .headers(headers)
            .body(body.to_owned())
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    #[tokio::test]
    async fn no_api_key() {
        trace_init();
        let (rx, addr) = source(true, None).await;

        assert_eq!(
            200,
            send_with_path(
                addr,
                r#"[{"message":"foo", "timestamp": 123}]"#,
                HeaderMap::new(),
                "/v1/input/"
            )
            .await
        );

        let mut events = collect_n(rx, 1).await;
        {
            let event = events.remove(0);
            let log = event.as_log();
            assert_eq!(log["message"], "foo".into());
            assert_eq!(log["timestamp"], 123.into());
            assert!(log.get("dd_api_key").is_none());
            assert_eq!(log[log_schema().source_type_key()], "datadog_logs".into());
        }
    }

    #[tokio::test]
    async fn api_key_in_url() {
        trace_init();
        let (rx, addr) = source(true, None).await;

        assert_eq!(
            200,
            send_with_path(
                addr,
                r#"[{"message":"bar", "timestamp": 456}]"#,
                HeaderMap::new(),
                "/v1/input/12345678abcdefgh12345678abcdefgh"
            )
            .await
        );

        let mut events = collect_n(rx, 1).await;
        {
            let event = events.remove(0);
            let log = event.as_log();
            assert_eq!(log["message"], "bar".into());
            assert_eq!(log["timestamp"], 456.into());
            assert_eq!(log["dd_api_key"], "12345678abcdefgh12345678abcdefgh".into());
            assert_eq!(log[log_schema().source_type_key()], "datadog_logs".into());
        }
    }

    #[tokio::test]
    async fn api_key_in_header() {
        trace_init();
        let (rx, addr) = source(true, None).await;

        let mut headers = HeaderMap::new();
        headers.insert(
            "dd-api-key",
            "12345678abcdefgh12345678abcdefgh".parse().unwrap(),
        );

        assert_eq!(
            200,
            send_with_path(
                addr,
                r#"[{"message":"baz", "timestamp": 789}]"#,
                headers,
                "/v1/input/"
            )
            .await
        );

        let mut events = collect_n(rx, 1).await;
        {
            let event = events.remove(0);
            let log = event.as_log();
            assert_eq!(log["message"], "baz".into());
            assert_eq!(log["timestamp"], 789.into());
            assert_eq!(log["dd_api_key"], "12345678abcdefgh12345678abcdefgh".into());
            assert_eq!(log[log_schema().source_type_key()], "datadog_logs".into());
        }
    }

    #[tokio::test]
    async fn invalid_api_key_in_url_to_drop() {
        trace_init();
        let (rx, addr) = source(true, Some(vec!["val".to_owned()])).await;

        assert_eq!(
            200,
            send_with_path(
                addr,
                r#"[{"message":"bar", "timestamp": 456}]"#,
                HeaderMap::new(),
                "/v1/input/12345678abcdefgh12345678abcdefgh"
            )
                .await
        );

        let mut events = collect_n(rx, 1).await;
        {
            let event = events.remove(0);
            let log = event.as_log();
            assert!(log.get("dd_api_key").is_none());
        }
    }

    #[tokio::test]
    async fn invalid_api_key_in_url_to_keep() {
        trace_init();
        let (rx, addr) = source(false, Some(vec!["val".to_owned()])).await;

        assert_eq!(
            200,
            send_with_path(
                addr,
                r#"[{"message":"bar", "timestamp": 456}]"#,
                HeaderMap::new(),
                "/v1/input/12345678abcdefgh12345678abcdefgh"
            )
                .await
        );

        let mut events = collect_n(rx, 1).await;
        {
            let event = events.remove(0);
            let log = event.as_log();
            assert_eq!(log["dd_api_key"], "12345678abcdefgh12345678abcdefgh".into());
        }
    }

    #[tokio::test]
    async fn valid_api_key_in_header() {
        trace_init();
        let (rx, addr) = source(true, Some(vec!["12345678abcdefgh12345678abcdefgh".to_owned()])).await;

        let mut headers = HeaderMap::new();
        headers.insert(
            "dd-api-key",
            "12345678abcdefgh12345678abcdefgh".parse().unwrap(),
        );

        assert_eq!(
            200,
            send_with_path(
                addr,
                r#"[{"message":"baz", "timestamp": 789}]"#,
                headers,
                "/v1/input/"
            )
                .await
        );

        let mut events = collect_n(rx, 1).await;
        {
            let event = events.remove(0);
            let log = event.as_log();
            assert_eq!(log["dd_api_key"], "12345678abcdefgh12345678abcdefgh".into());
        }
    }
}
