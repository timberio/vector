use super::{healthcheck_response, GcpAuthConfig, GcpCredentials, Scope};
use crate::template::TemplateRenderingError;
use crate::{
    config::{log_schema, DataType, ProxyConfig, SinkConfig, SinkContext, SinkDescription},
    event::{Event, Value},
    http::HttpClient,
    internal_events::TemplateRenderingFailed,
    sinks::{
        util::{
            encoding::{EncodingConfigWithDefault, EncodingConfiguration},
            http::{BatchedHttpSink, HttpSink},
            BatchConfig, BatchSettings, BoxedRawValue, EncodedEvent, JsonArrayBuffer,
            TowerRequestConfig,
        },
        Healthcheck, VectorSink,
    },
    template::Template,
    tls::{TlsOptions, TlsSettings},
};
use futures::{FutureExt, SinkExt};
use http::{Request, Uri};
use hyper::Body;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use serde_json::{json, map};
use snafu::Snafu;
use std::collections::HashMap;

#[derive(Debug, Snafu)]
enum HealthcheckError {
    #[snafu(display("Resource not found"))]
    NotFound,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct StackdriverConfig {
    #[serde(flatten)]
    pub log_name: StackdriverLogName,
    pub log_id: Template,

    pub resource: StackdriverResource,
    pub severity_key: Option<String>,

    #[serde(flatten)]
    pub auth: GcpAuthConfig,
    #[serde(
        skip_serializing_if = "crate::serde::skip_serializing_if_default",
        default
    )]
    pub encoding: EncodingConfigWithDefault<Encoding>,

    #[serde(default)]
    pub batch: BatchConfig,
    #[serde(default)]
    pub request: TowerRequestConfig,

    pub tls: Option<TlsOptions>,
    #[serde(
        default,
        skip_serializing_if = "crate::serde::skip_serializing_if_default"
    )]
    pub proxy: ProxyConfig,
}

#[derive(Clone, Debug)]
struct StackdriverSink {
    config: StackdriverConfig,
    creds: Option<GcpCredentials>,
    severity_key: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Eq, PartialEq, Clone, Derivative)]
#[serde(rename_all = "snake_case")]
#[derivative(Default)]
pub enum Encoding {
    #[derivative(Default)]
    Default,
}

#[derive(Clone, Debug, Derivative, Deserialize, Serialize)]
#[derivative(Default)]
pub enum StackdriverLogName {
    #[serde(rename = "billing_account_id")]
    BillingAccount(String),
    #[serde(rename = "folder_id")]
    Folder(String),
    #[serde(rename = "organization_id")]
    Organization(String),
    #[derivative(Default)]
    #[serde(rename = "project_id")]
    Project(String),
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct StackdriverResource {
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(flatten)]
    pub labels: HashMap<String, Template>,
}

inventory::submit! {
    SinkDescription::new::<StackdriverConfig>("gcp_stackdriver_logs")
}

impl_generate_config_from_default!(StackdriverConfig);

lazy_static! {
    static ref REQUEST_DEFAULTS: TowerRequestConfig = TowerRequestConfig {
        rate_limit_num: Some(1000),
        rate_limit_duration_secs: Some(1),
        ..Default::default()
    };
    static ref URI: Uri = "https://logging.googleapis.com/v2/entries:write"
        .parse()
        .unwrap();
}

#[async_trait::async_trait]
#[typetag::serde(name = "gcp_stackdriver_logs")]
impl SinkConfig for StackdriverConfig {
    async fn build(&self, cx: SinkContext) -> crate::Result<(VectorSink, Healthcheck)> {
        let creds = self.auth.make_credentials(Scope::LoggingWrite).await?;

        let batch = BatchSettings::default()
            .bytes(bytesize::kib(5000u64))
            .timeout(1)
            .parse_config(self.batch)?;
        let request = self.request.unwrap_with(&REQUEST_DEFAULTS);
        let tls_settings = TlsSettings::from_options(&self.tls)?;
        let proxy = cx.globals.proxy.build(&self.proxy);
        let client = HttpClient::new(tls_settings, proxy)?;

        let sink = StackdriverSink {
            config: self.clone(),
            creds,
            severity_key: self.severity_key.clone(),
        };

        let healthcheck = healthcheck(client.clone(), sink.clone()).boxed();

        let sink = BatchedHttpSink::new(
            sink,
            JsonArrayBuffer::new(batch.size),
            request,
            batch.timeout,
            client,
            cx.acker(),
        )
        .sink_map_err(|error| error!(message = "Fatal gcp_stackdriver_logs sink error.", %error));

        Ok((VectorSink::Sink(Box::new(sink)), healthcheck))
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn sink_type(&self) -> &'static str {
        "gcp_stackdriver_logs"
    }
}

#[async_trait::async_trait]
impl HttpSink for StackdriverSink {
    type Input = serde_json::Value;
    type Output = Vec<BoxedRawValue>;

    fn encode_event(&self, event: Event) -> Option<EncodedEvent<Self::Input>> {
        let mut labels = HashMap::with_capacity(self.config.resource.labels.len());
        for (key, template) in &self.config.resource.labels {
            let value = template
                .render_string(&event)
                .map_err(|error| {
                    emit!(TemplateRenderingFailed {
                        error,
                        field: Some("resource.labels"),
                        drop_event: true,
                    });
                })
                .ok()?;
            labels.insert(key.clone(), value);
        }
        let log_name = self
            .config
            .log_name(&event)
            .map_err(|error| {
                emit!(TemplateRenderingFailed {
                    error,
                    field: Some("log_id"),
                    drop_event: true,
                });
            })
            .ok()?;

        let mut log = event.into_log();
        let severity = self
            .severity_key
            .as_ref()
            .and_then(|key| log.remove(key))
            .map(remap_severity)
            .unwrap_or_else(|| 0.into());

        let mut event = Event::Log(log);
        self.config.encoding.apply_rules(&mut event);

        let log = event.into_log();

        let mut entry = map::Map::with_capacity(5);
        entry.insert("logName".into(), json!(log_name));
        entry.insert("jsonPayload".into(), json!(log));
        entry.insert("severity".into(), json!(severity));
        entry.insert(
            "resource".into(),
            json!({
                "type": self.config.resource.type_,
                "labels": labels,
            }),
        );

        // If the event contains a timestamp, send it in the main message so gcp can pick it up.
        if let Some(timestamp) = log.get(log_schema().timestamp_key()) {
            entry.insert("timestamp".into(), json!(timestamp));
        }

        Some(EncodedEvent::new(json!(entry)).with_metadata(log))
    }

    async fn build_request(&self, events: Self::Output) -> crate::Result<Request<Vec<u8>>> {
        let events = serde_json::json!({ "entries": events });

        let body = serde_json::to_vec(&events).unwrap();

        let mut request = Request::post(URI.clone())
            .header("Content-Type", "application/json")
            .body(body)
            .unwrap();

        if let Some(creds) = &self.creds {
            creds.apply(&mut request);
        }

        Ok(request)
    }
}

fn remap_severity(severity: Value) -> Value {
    let n = match severity {
        Value::Integer(n) => n - n % 100,
        Value::Bytes(s) => {
            let s = String::from_utf8_lossy(&s);
            match s.parse::<usize>() {
                Ok(n) => (n - n % 100) as i64,
                Err(_) => match s.to_uppercase() {
                    s if s.starts_with("EMERG") || s.starts_with("FATAL") => 800,
                    s if s.starts_with("ALERT") => 700,
                    s if s.starts_with("CRIT") => 600,
                    s if s.starts_with("ERR") => 500,
                    s if s.starts_with("WARN") => 400,
                    s if s.starts_with("NOTICE") => 300,
                    s if s.starts_with("INFO") => 200,
                    s if s.starts_with("DEBUG") || s.starts_with("TRACE") => 100,
                    s if s.starts_with("DEFAULT") => 0,
                    _ => {
                        warn!(
                            message = "Unknown severity value string, using DEFAULT.",
                            value = %s,
                            internal_log_rate_secs = 10
                        );
                        0
                    }
                },
            }
        }
        value => {
            warn!(
                message = "Unknown severity value type, using DEFAULT.",
                ?value,
                internal_log_rate_secs = 10
            );
            0
        }
    };
    Value::Integer(n)
}

async fn healthcheck(client: HttpClient, sink: StackdriverSink) -> crate::Result<()> {
    let request = sink.build_request(vec![]).await?.map(Body::from);

    let response = client.send(request).await?;
    healthcheck_response(sink.creds.clone(), HealthcheckError::NotFound.into())(response)
}

impl StackdriverConfig {
    fn log_name(&self, event: &Event) -> Result<String, TemplateRenderingError> {
        use StackdriverLogName::*;

        let log_id = self.log_id.render_string(event)?;

        Ok(match &self.log_name {
            BillingAccount(acct) => format!("billingAccounts/{}/logs/{}", acct, log_id),
            Folder(folder) => format!("folders/{}/logs/{}", folder, log_id),
            Organization(org) => format!("organizations/{}/logs/{}", org, log_id),
            Project(project) => format!("projects/{}/logs/{}", project, log_id),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{LogEvent, Value};
    use chrono::{TimeZone, Utc};
    use indoc::indoc;
    use serde_json::value::RawValue;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<StackdriverConfig>();
    }

    #[test]
    fn encode_valid() {
        let config: StackdriverConfig = toml::from_str(indoc! {r#"
            project_id = "project"
            log_id = "{{ log_id }}"
            resource.type = "generic_node"
            resource.namespace = "office"
            resource.node_id = "{{ node_id }}"
            encoding.except_fields = ["anumber", "node_id", "log_id"]
        "#})
        .unwrap();

        let sink = StackdriverSink {
            config,
            creds: None,
            severity_key: Some("anumber".into()),
        };

        let log = [
            ("message", "hello world"),
            ("anumber", "100"),
            ("node_id", "10.10.10.1"),
            ("log_id", "testlogs"),
        ]
        .iter()
        .copied()
        .collect::<LogEvent>();
        let json = sink.encode_event(Event::from(log)).unwrap().item;
        assert_eq!(
            json,
            serde_json::json!({
                "logName":"projects/project/logs/testlogs",
                "jsonPayload":{"message":"hello world"},
                "severity":100,
                "resource":{
                    "type":"generic_node",
                    "labels":{"namespace":"office","node_id":"10.10.10.1"}
                }
            })
        );
    }

    #[test]
    fn encode_inserts_timestamp() {
        let config: StackdriverConfig = toml::from_str(indoc! {r#"
            project_id = "project"
            log_id = "testlogs"
            resource.type = "generic_node"
            resource.namespace = "office"
        "#})
        .unwrap();

        let sink = StackdriverSink {
            config,
            creds: None,
            severity_key: Some("anumber".into()),
        };

        let mut log = LogEvent::default();
        log.insert("message", Value::Bytes("hello world".into()));
        log.insert("anumber", Value::Bytes("100".into()));
        log.insert(
            "timestamp",
            Value::Timestamp(Utc.ymd(2020, 1, 1).and_hms(12, 30, 0)),
        );

        let json = sink.encode_event(Event::from(log)).unwrap().item;
        assert_eq!(
            json,
            serde_json::json!({
                "logName":"projects/project/logs/testlogs",
                "jsonPayload":{"message":"hello world","timestamp":"2020-01-01T12:30:00Z"},
                "severity":100,
                "resource":{
                    "type":"generic_node",
                    "labels":{"namespace":"office"}},
                "timestamp":"2020-01-01T12:30:00Z"
            })
        );
    }

    #[test]
    fn severity_remaps_strings() {
        for &(s, n) in &[
            ("EMERGENCY", 800), // Handles full upper case
            ("EMERG", 800),     // Handles abbreviations
            ("FATAL", 800),     // Handles highest alternate
            ("alert", 700),     // Handles lower case
            ("CrIt1c", 600),    // Handles mixed case and suffixes
            ("err404", 500),    // Handles lower case and suffixes
            ("warnings", 400),
            ("notice", 300),
            ("info", 200),
            ("DEBUG2", 100), // Handles upper case and suffixes
            ("trace", 100),  // Handles lowest alternate
            ("nothing", 0),  // Maps unknown terms to DEFAULT
            ("123", 100),    // Handles numbers in strings
            ("-100", 0),     // Maps negatives to DEFAULT
        ] {
            assert_eq!(
                remap_severity(s.into()),
                Value::Integer(n),
                "remap_severity({:?}) != {}",
                s,
                n
            );
        }
    }

    #[tokio::test]
    async fn correct_request() {
        let config: StackdriverConfig = toml::from_str(indoc! {r#"
            project_id = "project"
            log_id = "testlogs"
            resource.type = "generic_node"
            resource.namespace = "office"
        "#})
        .unwrap();

        let sink = StackdriverSink {
            config,
            creds: None,
            severity_key: None,
        };

        let log1 = [("message", "hello")].iter().copied().collect::<LogEvent>();
        let log2 = [("message", "world")].iter().copied().collect::<LogEvent>();
        let event1 = sink.encode_event(Event::from(log1)).unwrap().item;
        let event2 = sink.encode_event(Event::from(log2)).unwrap().item;

        let json1 = serde_json::to_string(&event1).unwrap();
        let json2 = serde_json::to_string(&event2).unwrap();
        let raw1 = RawValue::from_string(json1).unwrap();
        let raw2 = RawValue::from_string(json2).unwrap();

        let events = vec![raw1, raw2];

        let request = sink.build_request(events).await.unwrap();

        let (parts, body) = request.into_parts();

        let json: serde_json::Value = serde_json::from_slice(&body[..]).unwrap();

        assert_eq!(
            &parts.uri.to_string(),
            "https://logging.googleapis.com/v2/entries:write"
        );
        assert_eq!(
            json,
            serde_json::json!({
                "entries": [
                    {
                        "logName": "projects/project/logs/testlogs",
                        "severity": 0,
                        "jsonPayload": {
                            "message": "hello"
                        },
                        "resource": {
                            "type": "generic_node",
                            "labels": {
                                "namespace": "office"
                            }
                        }
                    },
                    {
                        "logName": "projects/project/logs/testlogs",
                        "severity": 0,
                        "jsonPayload": {
                            "message": "world"
                        },
                        "resource": {
                            "type": "generic_node",
                            "labels": {
                                "namespace": "office"
                            }
                        }
                    }
                ]
            })
        );
    }

    #[tokio::test]
    async fn fails_missing_creds() {
        let config: StackdriverConfig = toml::from_str(indoc! {r#"
            project_id = "project"
            log_id = "testlogs"
            resource.type = "generic_node"
            resource.namespace = "office"
        "#})
        .unwrap();
        if config.build(SinkContext::new_test()).await.is_ok() {
            panic!("config.build failed to error");
        }
    }

    #[test]
    fn fails_invalid_log_names() {
        toml::from_str::<StackdriverConfig>(indoc! {r#"
            log_id = "testlogs"
            resource.type = "generic_node"
            resource.namespace = "office"
        "#})
        .expect_err("Config parsing failed to error with missing ids");

        toml::from_str::<StackdriverConfig>(indoc! {r#"
            project_id = "project"
            folder_id = "folder"
            log_id = "testlogs"
            resource.type = "generic_node"
            resource.namespace = "office"
        "#})
        .expect_err("Config parsing failed to error with extraneous ids");
    }
}
