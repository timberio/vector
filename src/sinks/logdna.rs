use crate::{
    config::{DataType, GenerateConfig, ProxyConfig, SinkConfig, SinkContext, SinkDescription},
    event::Event,
    http::{Auth, HttpClient},
    internal_events::TemplateRenderingFailed,
    sinks::util::{
        encoding::{EncodingConfigWithDefault, EncodingConfiguration},
        http::{HttpSink, PartitionHttpSink},
        BatchConfig, BatchSettings, BoxedRawValue, EncodedEvent, JsonArrayBuffer, PartitionBuffer,
        PartitionInnerBuffer, TowerRequestConfig, UriSerde,
    },
    template::{Template, TemplateRenderingError},
};
use futures::{FutureExt, SinkExt};
use http::{Request, StatusCode, Uri};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::SystemTime;

lazy_static::lazy_static! {
    static ref HOST: Uri = Uri::from_static("https://logs.logdna.com");
}

const PATH: &str = "/logs/ingest";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogdnaConfig {
    api_key: String,
    // Deprecated name
    #[serde(alias = "host")]
    endpoint: Option<UriSerde>,

    hostname: Template,
    mac: Option<String>,
    ip: Option<String>,
    tags: Option<Vec<Template>>,

    #[serde(
        skip_serializing_if = "crate::serde::skip_serializing_if_default",
        default
    )]
    pub encoding: EncodingConfigWithDefault<Encoding>,

    default_app: Option<String>,
    default_env: Option<String>,

    #[serde(default)]
    batch: BatchConfig,

    #[serde(default)]
    request: TowerRequestConfig,
    #[serde(
        default,
        skip_serializing_if = "crate::serde::skip_serializing_if_default"
    )]
    proxy: ProxyConfig,
}

inventory::submit! {
    SinkDescription::new::<LogdnaConfig>("logdna")
}

impl GenerateConfig for LogdnaConfig {
    fn generate_config() -> toml::Value {
        toml::from_str(
            r#"hostname = "hostname"
            api_key = "${LOGDNA_API_KEY}""#,
        )
        .unwrap()
    }
}

#[derive(Deserialize, Serialize, Debug, Eq, PartialEq, Clone, Derivative)]
#[serde(rename_all = "snake_case")]
#[derivative(Default)]
pub enum Encoding {
    #[derivative(Default)]
    Default,
}

#[async_trait::async_trait]
#[typetag::serde(name = "logdna")]
impl SinkConfig for LogdnaConfig {
    async fn build(
        &self,
        cx: SinkContext,
    ) -> crate::Result<(super::VectorSink, super::Healthcheck)> {
        let request_settings = self.request.unwrap_with(&TowerRequestConfig::default());
        let batch_settings = BatchSettings::default()
            .bytes(bytesize::mib(10u64))
            .timeout(1)
            .parse_config(self.batch)?;
        let proxy = cx.globals.proxy.build(&self.proxy);
        let client = HttpClient::new(None, proxy)?;

        let sink = PartitionHttpSink::new(
            self.clone(),
            PartitionBuffer::new(JsonArrayBuffer::new(batch_settings.size)),
            request_settings,
            batch_settings.timeout,
            client.clone(),
            cx.acker(),
        )
        .sink_map_err(|error| error!(message = "Fatal logdna sink error.", %error));

        let healthcheck = healthcheck(self.clone(), client).boxed();

        Ok((super::VectorSink::Sink(Box::new(sink)), healthcheck))
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn sink_type(&self) -> &'static str {
        "logdna"
    }
}

#[derive(Hash, Eq, PartialEq, Clone)]
pub struct PartitionKey {
    hostname: String,
    tags: Option<Vec<String>>,
}

#[async_trait::async_trait]
impl HttpSink for LogdnaConfig {
    type Input = PartitionInnerBuffer<serde_json::Value, PartitionKey>;
    type Output = PartitionInnerBuffer<Vec<BoxedRawValue>, PartitionKey>;

    fn encode_event(&self, mut event: Event) -> Option<EncodedEvent<Self::Input>> {
        let key = self
            .render_key(&event)
            .map_err(|(field, error)| {
                emit!(TemplateRenderingFailed {
                    error,
                    field,
                    drop_event: true,
                });
            })
            .ok()?;

        self.encoding.apply_rules(&mut event);
        let mut log = event.into_log();

        let line = log
            .remove(crate::config::log_schema().message_key())
            .unwrap_or_else(|| String::from("").into());
        let timestamp = log
            .remove(crate::config::log_schema().timestamp_key())
            .unwrap_or_else(|| chrono::Utc::now().into());

        let mut map = serde_json::map::Map::new();

        map.insert("line".to_string(), json!(line));
        map.insert("timestamp".to_string(), json!(timestamp));

        if let Some(env) = log.remove("env") {
            map.insert("env".to_string(), json!(env));
        }

        if let Some(app) = log.remove("app") {
            map.insert("app".to_string(), json!(app));
        }

        if let Some(file) = log.remove("file") {
            map.insert("file".to_string(), json!(file));
        }

        if !map.contains_key("env") {
            map.insert(
                "env".to_string(),
                json!(self.default_env.as_deref().unwrap_or("production")),
            );
        }

        if !map.contains_key("app") && !map.contains_key("file") {
            if let Some(default_app) = &self.default_app {
                map.insert("app".to_string(), json!(default_app.as_str()));
            } else {
                map.insert("app".to_string(), json!("vector"));
            }
        }

        if !log.is_empty() {
            map.insert("meta".into(), json!(&log));
        }

        Some(EncodedEvent::new(PartitionInnerBuffer::new(map.into(), key)).with_metadata(log))
    }

    async fn build_request(&self, output: Self::Output) -> crate::Result<http::Request<Vec<u8>>> {
        let (events, key) = output.into_parts();
        let mut query = url::form_urlencoded::Serializer::new(String::new());

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("Time can't drift behind the epoch!")
            .as_millis();

        query.append_pair("hostname", &key.hostname);
        query.append_pair("now", &format!("{}", now));

        if let Some(mac) = &self.mac {
            query.append_pair("mac", mac);
        }

        if let Some(ip) = &self.ip {
            query.append_pair("ip", ip);
        }

        if let Some(tags) = &key.tags {
            let tags = tags.join(",");
            query.append_pair("tags", &tags);
        }

        let query = query.finish();

        let body = serde_json::to_vec(&json!({
            "lines": events,
        }))
        .unwrap();

        let uri = self.build_uri(&query);

        let mut request = Request::builder()
            .uri(uri)
            .method("POST")
            .header("Content-Type", "application/json")
            .body(body)
            .unwrap();

        let auth = Auth::Basic {
            user: self.api_key.clone(),
            password: "".to_string(),
        };

        auth.apply(&mut request);

        Ok(request)
    }
}

impl LogdnaConfig {
    fn build_uri(&self, query: &str) -> Uri {
        let host = self
            .endpoint
            .clone()
            .map(|endpoint| endpoint.uri)
            .unwrap_or_else(|| HOST.clone());

        let uri = format!("{}{}?{}", host, PATH, query);

        uri.parse::<http::Uri>()
            .expect("This should be a valid uri")
    }

    fn render_key(
        &self,
        event: &Event,
    ) -> Result<PartitionKey, (Option<&str>, TemplateRenderingError)> {
        let hostname = self
            .hostname
            .render_string(event)
            .map_err(|e| (Some("hostname"), e))?;
        let tags = self
            .tags
            .as_ref()
            .map(|tags| {
                let mut vec = Vec::with_capacity(tags.len());
                for tag in tags {
                    vec.push(tag.render_string(event).map_err(|e| (None, e))?);
                }
                Ok(Some(vec))
            })
            .unwrap_or(Ok(None))?;
        Ok(PartitionKey { hostname, tags })
    }
}

async fn healthcheck(config: LogdnaConfig, client: HttpClient) -> crate::Result<()> {
    let uri = config.build_uri("");

    let req = Request::post(uri).body(hyper::Body::empty()).unwrap();

    let res = client.send(req).await?;

    if res.status().is_server_error() {
        return Err("Server returned a server error".into());
    }

    if res.status() == StatusCode::FORBIDDEN {
        return Err("Token is not valid, 403 returned.".into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::SinkConfig,
        sinks::util::test::{build_test_server_status, load_sink},
        test_util::{next_addr, random_lines, trace_init},
    };
    use futures::{channel::mpsc, stream, StreamExt};
    use http::{request::Parts, StatusCode};
    use serde_json::json;
    use vector_core::event::{BatchNotifier, BatchStatus, Event, LogEvent};

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<LogdnaConfig>();
    }

    #[test]
    fn encode_event() {
        let (config, _cx) = load_sink::<LogdnaConfig>(
            r#"
            api_key = "mylogtoken"
            hostname = "vector"
            default_env = "acceptance"
            codec.except_fields = ["magic"]
        "#,
        )
        .unwrap();

        let mut event1 = Event::from("hello world");
        event1.as_mut_log().insert("app", "notvector");
        event1.as_mut_log().insert("magic", "vector");

        let mut event2 = Event::from("hello world");
        event2.as_mut_log().insert("file", "log.txt");

        let event3 = Event::from("hello world");

        let mut event4 = Event::from("hello world");
        event4.as_mut_log().insert("env", "staging");

        let event1_out = config.encode_event(event1).unwrap().item.into_parts().0;
        let event1_out = event1_out.as_object().unwrap();
        let event2_out = config.encode_event(event2).unwrap().item.into_parts().0;
        let event2_out = event2_out.as_object().unwrap();
        let event3_out = config.encode_event(event3).unwrap().item.into_parts().0;
        let event3_out = event3_out.as_object().unwrap();
        let event4_out = config.encode_event(event4).unwrap().item.into_parts().0;
        let event4_out = event4_out.as_object().unwrap();

        assert_eq!(event1_out.get("app").unwrap(), &json!("notvector"));
        assert_eq!(event2_out.get("file").unwrap(), &json!("log.txt"));
        assert_eq!(event3_out.get("app").unwrap(), &json!("vector"));
        assert_eq!(event3_out.get("env").unwrap(), &json!("acceptance"));
        assert_eq!(event4_out.get("env").unwrap(), &json!("staging"));
    }

    async fn smoke_start(
        status_code: StatusCode,
        batch_status: BatchStatus,
    ) -> (
        Vec<&'static str>,
        Vec<Vec<String>>,
        mpsc::Receiver<(Parts, bytes::Bytes)>,
    ) {
        trace_init();

        let (mut config, cx) = load_sink::<LogdnaConfig>(
            r#"
            api_key = "mylogtoken"
            ip = "127.0.0.1"
            mac = "some-mac-addr"
            hostname = "{{ hostname }}"
            tags = ["test","maybeanothertest"]
        "#,
        )
        .unwrap();

        // Make sure we can build the config
        let _ = config.build(cx.clone()).await.unwrap();

        let addr = next_addr();
        // Swap out the host so we can force send it
        // to our local server
        let endpoint = format!("http://{}", addr).parse::<http::Uri>().unwrap();
        config.endpoint = Some(endpoint.into());

        let (sink, _) = config.build(cx).await.unwrap();

        let (rx, _trigger, server) = build_test_server_status(addr, status_code);
        tokio::spawn(server);

        let lines = random_lines(100).take(10).collect::<Vec<_>>();
        let mut events = Vec::new();
        let hosts = vec!["host0", "host1"];

        let (batch, mut receiver) = BatchNotifier::new_with_receiver();
        let mut partitions = vec![Vec::new(), Vec::new()];
        // Create 10 events where the first one contains custom
        // fields that are not just `message`.
        for (i, line) in lines.iter().enumerate() {
            let mut event = LogEvent::from(line.as_str()).with_batch_notifier(&batch);
            let p = i % 2;
            event.insert("hostname", hosts[p]);

            partitions[p].push(line.into());
            events.push(Event::Log(event));
        }
        drop(batch);

        sink.run(stream::iter(events)).await.unwrap();

        assert_eq!(receiver.try_recv(), Ok(batch_status));

        (hosts, partitions, rx)
    }

    #[tokio::test]
    async fn smoke_fails() {
        let (_hosts, _partitions, mut rx) =
            smoke_start(StatusCode::FORBIDDEN, BatchStatus::Failed).await;
        assert!(matches!(rx.try_next(), Err(mpsc::TryRecvError { .. })));
    }

    #[tokio::test]
    async fn smoke() {
        let (hosts, partitions, mut rx) = smoke_start(StatusCode::OK, BatchStatus::Delivered).await;

        for _ in 0..partitions.len() {
            let output = rx.next().await.unwrap();

            let request = &output.0;
            let body: serde_json::Value = serde_json::from_slice(&output.1[..]).unwrap();

            let query = request.uri.query().unwrap();

            let (p, host) = hosts
                .iter()
                .enumerate()
                .find(|(_, host)| query.contains(&format!("hostname={}", host)))
                .expect("invalid hostname");
            let lines = &partitions[p];

            assert!(query.contains("ip=127.0.0.1"));
            assert!(query.contains("mac=some-mac-addr"));
            assert!(query.contains("tags=test%2Cmaybeanothertest"));

            let output = body
                .as_object()
                .unwrap()
                .get("lines")
                .unwrap()
                .as_array()
                .unwrap();

            for (i, line) in output.iter().enumerate() {
                // All lines are json objects
                let line = line.as_object().unwrap();

                assert_eq!(line.get("app").unwrap(), &json!("vector"));
                assert_eq!(line.get("env").unwrap(), &json!("production"));
                assert_eq!(line.get("line").unwrap(), &json!(lines[i]));

                assert_eq!(
                    line.get("meta").unwrap(),
                    &json!({
                        "hostname": host,
                    })
                );
            }
        }
    }
}
