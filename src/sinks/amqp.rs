use crate::{
    buffers::Acker,
    config::{log_schema, DataType, GenerateConfig, SinkConfig, SinkContext, SinkDescription},
    event::proto,
    internal_events::{TemplateRenderingFailed, sink::{AmqpAcknowledgementFailed, AmqpDeliveryFailed, AmqpNoAcknowledgement}},
    amqp::AmqpConfig,
    sinks::util::{
        encoding::{EncodingConfig, EncodingConfiguration},
    },
    template::{Template, TemplateParseError},
    event::Event,
};
use futures::{
    future::BoxFuture, ready, FutureExt,
    Sink,
};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use std::{
    convert::TryFrom,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use prost::Message;
use lapin::options::BasicPublishOptions;
use lapin::BasicProperties;

#[derive(Debug, Snafu)]
enum BuildError {
    #[snafu(display("creating amqp producer failed: {}", source))]
    AmqpCreateFailed { source: Box<dyn std::error::Error + Send + Sync> },
    #[snafu(display("invalid exchange template: {}", source))]
    ExchangeTemplate { source: TemplateParseError },
    #[snafu(display("invalid routing key template: {}", source))]
    RoutingKeyTemplate { source: TemplateParseError },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AmqpSinkConfig {
    pub(crate) exchange: String,
    pub(crate) routing_key: Option<String>,
    pub(crate) encoding: EncodingConfig<Encoding>,
    pub(crate) connection: AmqpConfig,
}

impl Default for AmqpSinkConfig {
    fn default() -> Self {
        Self {
            exchange: "vector".to_string(),
            routing_key: None,
            encoding: Encoding::Text.into(),
            connection: AmqpConfig::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Derivative, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    Text,
    Json,
    Protobuf,
}

enum InFlight {
    Sending(BoxFuture<'static, Result<lapin::publisher_confirm::PublisherConfirm, lapin::Error>>),
    Committing(BoxFuture<'static, Result<lapin::publisher_confirm::Confirmation, lapin::Error>>),
}

pub struct AmqpSink {
    channel: Arc<lapin::Channel>,
    exchange: Template,
    routing_key: Option<Template>,
    encoding: EncodingConfig<Encoding>,
    in_flight: Option<InFlight>,
    acker: Acker,
}

inventory::submit! {
    SinkDescription::new::<AmqpSinkConfig>("amqp")
}

impl GenerateConfig for AmqpSinkConfig {
    fn generate_config() -> toml::Value {
        toml::from_str(
            r#"connection_string = "amqp://localhost:5672/%2f"
            routing_key = "user_id"
            exchange = "test"
            encoding.codec = "protobuf""#,
        )
        .unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "amqp")]
impl SinkConfig for AmqpSinkConfig {
    async fn build(
        &self,
        cx: SinkContext,
    ) -> crate::Result<(super::VectorSink, super::Healthcheck)> {
        let sink = AmqpSink::new(self.clone(), cx.acker()).await?;
        let hc = healthcheck(self.clone(), sink.channel.clone()).boxed();
        Ok((super::VectorSink::Sink(Box::new(sink)), hc))
    }

    fn input_type(&self) -> DataType {
        DataType::Any
    }

    fn sink_type(&self) -> &'static str {
        "amqp"
    }
}

impl AmqpSink {
    async fn new(config: AmqpSinkConfig, acker: Acker) -> crate::Result<Self> {
        let (_, channel) = config.connection.connect().await.map_err(|e| BuildError::AmqpCreateFailed { source: e })?;
        Ok(AmqpSink {
            channel: Arc::new(channel),
            exchange: Template::try_from(config.exchange).context(ExchangeTemplate)?,
            routing_key: config.routing_key.map(|k| Template::try_from(k).context(RoutingKeyTemplate)).transpose()?,
            encoding: config.encoding,
            in_flight: None,
            acker: acker,
        })
    }
}

impl Sink<Event> for AmqpSink {
    type Error = ();

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_flush(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Event) -> Result<(), Self::Error> {
        assert!(
            self.in_flight.is_none(),
            "Expected `poll_ready` to be called first."
        );

        let exchange = self.exchange.render_string(&item).map_err(|missing_keys| {
            emit!(TemplateRenderingFailed {
                error: missing_keys,
                field: Some("exchange"),
                drop_event: true,
            });
        })?;

        let routing_key = if let Some(t) = &self.routing_key {
            t.render_string(&item).map_err(|error| {
                emit!(TemplateRenderingFailed {
                    error,
                    field: Some("routing_key"),
                    drop_event: true,
                });
            })?
        } else {
            "".to_string()
        };

        let body = encode_event(item, &self.encoding);

        let channel = self.channel.clone();
        let f = Box::pin(channel
            .basic_publish(
                &exchange,
                &routing_key,
                BasicPublishOptions::default(),
                body,
                BasicProperties::default(),
            )
        );

        self.in_flight = Some(InFlight::Sending(Box::pin(f)));

        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = Pin::into_inner(self);
        while let Some(in_flight) = this.in_flight.as_mut() {
            match in_flight {
                InFlight::Sending(ref mut fut) => {
                    match ready!(fut.as_mut().poll(cx)) {
                        Ok(result) => {
                            this.in_flight = Some(InFlight::Committing(Box::pin(result)));
                        }
                        Err(err) => {
                            emit!(AmqpDeliveryFailed {
                                error: err,
                            });
                            return Poll::Ready(Err(()));
                        }
                    }
                }
                InFlight::Committing(ref mut fut) => {
                    let r = ready!(fut.as_mut().poll(cx));
                    this.in_flight.take();
                    match r {
                        Err(e) => {
                            emit!(AmqpAcknowledgementFailed {
                                error: e,
                            });
                            return Poll::Ready(Err(()));
                        },
                        Ok(confirm) => {
                            if let lapin::publisher_confirm::Confirmation::Nack(_) = confirm {
                                emit!(AmqpNoAcknowledgement::default());
                                return Poll::Ready(Err(()));
                            } else {
                                this.acker.ack(1);
                            }
                        }
                    };
                }
            }
        }

        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_flush(cx)
    }
}

async fn healthcheck(_config: AmqpSinkConfig, channel: Arc<lapin::Channel>) -> crate::Result<()> {
    trace!("Healthcheck started.");

    if !channel.status().connected() {
        return Err(Box::new(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Not Connected")));
    }

    trace!("Healthcheck completed.");
    Ok(())
}

fn encode_event(
    mut event: Event,
    encoding: &EncodingConfig<Encoding>,
) -> Vec<u8> {
    encoding.apply_rules(&mut event);

    let body = match event {
        Event::Log(ref log) => match encoding.codec() {
            Encoding::Json => serde_json::to_vec(log).expect("JSON serialization should not fail"),
            Encoding::Text => log
                .get(log_schema().message_key())
                .map(|v| v.as_bytes().to_vec())
                .unwrap_or_default(),
            Encoding::Protobuf => {
                let event: proto::EventWrapper = event.into();
                let mut payload = Vec::with_capacity(1_000);
                event.encode(&mut payload).expect("Protobuf encoding should not fail");
                payload
            },
        },
        Event::Metric(ref metric) => match encoding.codec() {
            Encoding::Json => serde_json::to_vec(metric).expect("JSON serialization should not fail"),
            Encoding::Text => metric.to_string().into_bytes(),
            Encoding::Protobuf => {
                let event: proto::EventWrapper = event.into();
                let mut payload = Vec::with_capacity(1_000);
                event.encode(&mut payload).expect("Protobuf encoding should not fail");
                payload
            },
        },
    };

    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Metric, MetricKind, MetricValue};
    use std::collections::BTreeMap;

    #[test]
    pub fn generate_config() {
        crate::test_util::test_generate_config::<AmqpSinkConfig>();
    }

    pub fn make_config() -> AmqpSinkConfig {
        let mut config = AmqpSinkConfig::default();
        config.exchange = "it".to_string();
        let user = std::env::var("AMQP_USER").unwrap_or_else(|_| "guest".to_string());
        let pass = std::env::var("AMQP_PASSWORD").unwrap_or_else(|_| "guest".to_string());
        let vhost = std::env::var("AMQP_VHOST").unwrap_or_else(|_| "%2f".to_string());
        config.connection.connection_string = format!("amqp://{}:{}@127.0.0.1:5672/{}", user, pass, vhost);
        config
    }

    #[test]
    fn amqp_encode_event_log_text() {
        crate::test_util::trace_init();
        let message = "hello world".to_string();
        let bytes = encode_event(
            message.clone().into(),
            &EncodingConfig::from(Encoding::Text),
        );

        assert_eq!(&bytes[..], message.as_bytes());
    }

    #[test]
    fn amqp_encode_event_log_json() {
        crate::test_util::trace_init();
        let message = "hello world".to_string();
        let mut event = Event::from(message.clone());
        event.as_mut_log().insert("key", "value");
        event.as_mut_log().insert("foo", "bar");

        let bytes = encode_event(
            event,
            &EncodingConfig::from(Encoding::Json),
        );

        let map: BTreeMap<String, String> = serde_json::from_slice(&bytes[..]).unwrap();

        assert_eq!(map[&log_schema().message_key().to_string()], message);
        assert_eq!(map["key"], "value".to_string());
        assert_eq!(map["foo"], "bar".to_string());
    }

    #[test]
    fn amqp_encode_event_log_protobuf() {
        crate::test_util::trace_init();
        let message = "hello world".to_string();
        let bytes = encode_event(
            message.clone().into(),
            &EncodingConfig::from(Encoding::Text),
        );

        assert_eq!(&bytes[..], message.as_bytes());
    }

    #[test]
    fn amqp_encode_event_metric_text() {
        let metric = Metric::new(
            "amqp-metric",
            MetricKind::Absolute,
            MetricValue::Counter { value: 0.0 },
        );
        let bytes = encode_event(
            metric.clone().into(),
            &EncodingConfig::from(Encoding::Text),
        );

        assert_eq!(metric.to_string(), String::from_utf8_lossy(&bytes));
    }

    #[test]
    fn amqp_encode_event_metric_json() {
        let metric = Metric::new(
            "amqp-metric",
            MetricKind::Absolute,
            MetricValue::Counter { value: 0.0 },
        );
        let bytes = encode_event(
            metric.clone().into(),
            &EncodingConfig::from(Encoding::Json),
        );

        assert_eq!(
            serde_json::to_string(&metric).unwrap(),
            String::from_utf8_lossy(&bytes)
        );
    }

    #[test]
    fn amqp_encode_event_log_apply_rules() {
        crate::test_util::trace_init();
        let mut event = Event::from("hello");
        event.as_mut_log().insert("key", "value");

        let bytes = encode_event(
            event,
            &EncodingConfig {
                codec: Encoding::Json,
                schema: None,
                only_fields: None,
                except_fields: Some(vec!["key".into()]),
                timestamp_format: None,
            },
        );

        let map: BTreeMap<String, String> = serde_json::from_slice(&bytes[..]).unwrap();

        assert!(!map.contains_key("key"));
    }
}

#[cfg(feature = "amqp-integration-tests")]
#[cfg(test)]
mod integration_tests {
    use super::*;
    use super::tests::make_config;
    use crate::{
        buffers::Acker,
        test_util::{random_lines_with_stream, random_string},
        Pipeline,
        shutdown::ShutdownSignal,
    };
    use futures::StreamExt;
    use std::time::Duration;

    #[tokio::test]
    async fn healthcheck() {
        crate::test_util::trace_init();
        let exchange = format!("test-{}-exchange", random_string(10));

        let mut config = make_config();
        config.exchange = exchange;
        let (_conn, channel) = config.connection.connect().await.unwrap();
        super::healthcheck(config, Arc::new(channel)).await.unwrap();
    }

    #[tokio::test]
    async fn amqp_happy_path_plaintext() {
        crate::test_util::trace_init();

        amqp_happy_path().await;
    }

    #[tokio::test]
    async fn amqp_round_trip_plaintext() {
        crate::test_util::trace_init();

        amqp_round_trip().await;
    }

    async fn amqp_happy_path() {
        let mut config = make_config();
        config.exchange = format!("test-{}-exchange", random_string(10));
        let queue = format!("test-{}-queue", random_string(10));

        let (_conn, channel) = config.connection.connect().await.unwrap();
        let mut exchange_opts = lapin::options::ExchangeDeclareOptions::default();
        exchange_opts.auto_delete = true;
        channel.exchange_declare(
            &config.exchange,
            lapin::ExchangeKind::Fanout,
            exchange_opts,
            lapin::types::FieldTable::default(),
        ).await.unwrap();

        let (acker, ack_counter) = Acker::new_for_testing();
        let sink = AmqpSink::new(config.clone(), acker).await.unwrap();

        // prepare consumer
        let mut queue_opts = lapin::options::QueueDeclareOptions::default();
        queue_opts.auto_delete = true;
        channel.queue_declare(
            &queue,
            queue_opts,
            lapin::types::FieldTable::default(),
        ).await.unwrap();

        channel.queue_bind(
            &queue,
            &config.exchange,
            "",
            lapin::options::QueueBindOptions::default(),
            lapin::types::FieldTable::default(),
        ).await.unwrap();

        let consumer = format!("test-{}-consumer", random_string(10));
        let mut consumer = channel
            .basic_consume(
                &queue,
                &consumer,
                lapin::options::BasicConsumeOptions::default(),
                lapin::types::FieldTable::default(),
            )
            .await.unwrap();

        let num_events = 1000;
        let (input, events) = random_lines_with_stream(100, num_events, None);
        events.map(Ok).forward(sink).await.unwrap();

        // loop instead of iter so we can set a timeout
        let mut failures = 0;
        let mut out = Vec::new();
        while failures < 10 && out.len() < input.len() {
            if let Ok(Some(try_msg)) = tokio::time::timeout(Duration::from_secs(10), consumer.next()).await {
                let (_, msg) = try_msg.unwrap();
                let s = String::from_utf8_lossy(msg.data.as_slice()).into_owned();
                out.push(s);
            } else {
                failures += 1;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }

        assert_eq!(out.len(), input.len());
        assert_eq!(out, input);

        assert_eq!(
            ack_counter.load(std::sync::atomic::Ordering::Relaxed),
            num_events
        );
    }

    async fn amqp_round_trip() {
        let mut config = make_config();
        config.exchange = format!("test-{}-exchange", random_string(10));
        let queue = format!("test-{}-queue", random_string(10));

        let (_conn, channel) = config.connection.connect().await.unwrap();
        let mut exchange_opts = lapin::options::ExchangeDeclareOptions::default();
        exchange_opts.auto_delete = true;
        channel.exchange_declare(
            &config.exchange,
            lapin::ExchangeKind::Fanout,
            exchange_opts,
            lapin::types::FieldTable::default(),
        ).await.unwrap();

        let (amqp_acker, amqp_ack_counter) = Acker::new_for_testing();
        let amqp_sink = AmqpSink::new(config.clone(), amqp_acker).await.unwrap();

        let source_cfg = crate::sources::amqp::AmqpSourceConfig {
            connection: config.connection.clone(),
            queue: queue.clone(),
            consumer: format!("test-{}-amqp-source", random_string(10)),
            routing_key: None,
            exchange_key: None,
            offset_key: None,
        };
        let (tx, rx) = Pipeline::new_test();
        let amqp_source = crate::sources::amqp::amqp_source(&source_cfg, ShutdownSignal::noop(), tx).await.unwrap();

        // prepare server
        let mut queue_opts = lapin::options::QueueDeclareOptions::default();
        queue_opts.auto_delete = true;
        channel.queue_declare(
            &queue,
            queue_opts,
            lapin::types::FieldTable::default(),
        ).await.unwrap();

        channel.queue_bind(
            &queue,
            &config.exchange,
            "",
            lapin::options::QueueBindOptions::default(),
            lapin::types::FieldTable::default(),
        ).await.unwrap();

        let _source_fut = tokio::spawn(amqp_source);

        //Have sink publish events
        let events_fut = async move {
            let num_events = 1000;
            let (_, events) = random_lines_with_stream(100, num_events, None);
            events.map(Ok).forward(amqp_sink).await.unwrap();
            num_events
        };
        let nb_events_published = tokio::spawn(events_fut).await.unwrap();
        let output = crate::test_util::collect_n(rx, 1000).await;

        assert_eq!(output.len(), nb_events_published);

        assert_eq!(
            amqp_ack_counter.load(std::sync::atomic::Ordering::Relaxed),
            nb_events_published
        );
    }
}
