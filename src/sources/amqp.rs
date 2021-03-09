use crate::{
    config::{log_schema, DataType, SourceConfig, SourceDescription},
    event::{Event, Value},
    internal_events::source::{AmqpEventFailed, AmqpEventReceived, AmqpDeliveryFailed, AmqpCommitFailed, AmqpConsumerFailed},
    amqp::AmqpConfig,
    shutdown::ShutdownSignal,
    Pipeline,
};
use bytes::Bytes;
use chrono::{TimeZone, Utc};
use futures::{FutureExt, SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use snafu::Snafu;
use lapin::{Connection, Channel};
use crate::config::SourceContext;

#[derive(Debug, Snafu)]
enum BuildError {
    #[snafu(display("Could not create Amqp consumer: {}", source))]
    AmqpCreateError { source: Box<dyn std::error::Error + Send + Sync> },
    #[snafu(display("Could not subscribe to Amqp queue: {}", source))]
    AmqpSubscribeError { source: lapin::Error },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AmqpSourceConfig {
    pub(crate) queue: String,
    pub(crate) consumer: String,
    pub(crate) routing_key: Option<String>,
    pub(crate) exchange_key: Option<String>,
    pub(crate) offset_key: Option<String>,
    pub(crate) connection: AmqpConfig,
}

impl Default for AmqpSourceConfig {
    fn default() -> Self {
        Self {
            queue: "vector".to_string(),
            consumer: "vector".to_string(),
            routing_key: None,
            exchange_key: None,
            offset_key: None,
            connection: AmqpConfig::default(),
        }
    }
}

inventory::submit! {
    SourceDescription::new::<AmqpSourceConfig>("amqp")
}

impl_generate_config_from_default!(AmqpSourceConfig);

#[async_trait::async_trait]
#[typetag::serde(name = "amqp")]
impl SourceConfig for AmqpSourceConfig {
    async fn build(
        &self,
        cx: SourceContext,
    ) -> crate::Result<super::Source> {
        amqp_source(self, cx.shutdown, cx.out).await
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn source_type(&self) -> &'static str {
        "amqp"
    }
}

enum ShutdownOrMessage {
    Shutdown,
    Message(Option<Result<(lapin::Channel, lapin::message::Delivery), lapin::Error>>),
}

pub(crate) async fn amqp_source(
    config: &AmqpSourceConfig,
    shutdown: ShutdownSignal,
    out: Pipeline,
) -> crate::Result<super::Source> {
    let config = config.clone();
    let (conn, channel) = config.connection.connect().await.map_err(|e| BuildError::AmqpCreateError {
        source: e,
    })?;

    Ok(Box::pin(run_amqp_source(config, shutdown, out, conn, channel)))
}

async fn run_amqp_source(config: AmqpSourceConfig, shutdown: ShutdownSignal, mut out: Pipeline, _conn: Connection, channel: Channel) -> Result<(), ()> {
    let ack_options = lapin::options::BasicAckOptions::default();
    debug!("Starting amqp source, listening to queue {}", config.queue);
    let mut consumer = channel
        .basic_consume(
            &config.queue,
            &config.consumer,
            lapin::options::BasicConsumeOptions::default(),
            lapin::types::FieldTable::default(),
        )
        .await.map_err(|error| {
        emit!(AmqpConsumerFailed { error });
        ()
    })?.fuse();
    let mut shutdown = shutdown.fuse();
    loop {
        let msg = futures::select! {
            _ = shutdown => ShutdownOrMessage::Shutdown,
            opt_m = consumer.next() => ShutdownOrMessage::Message(opt_m),
        };
        if let ShutdownOrMessage::Message(Some(try_m)) = msg {
            match try_m {
                Err(error) => {
                    emit!(AmqpEventFailed { error });
                    return Err(());
                }
                Ok((_, msg)) => {
                    emit!(AmqpEventReceived {
                        byte_size: msg.data.len()
                    });

                    if msg.data.is_empty() {
                        return Err(());
                    }

                    let mut event = Event::new_empty_log();
                    let log = event.as_mut_log();

                    log.insert(
                        log_schema().message_key(),
                        Value::from(Bytes::from(msg.data)),
                    );

                    // Extract timestamp from amqp message
                    let timestamp = msg
                        .properties
                        .timestamp()
                        .and_then(|millis| {
                            Utc.timestamp_millis_opt(millis as _).latest()
                        })
                        .unwrap_or_else(Utc::now);
                    log.insert(log_schema().timestamp_key(), timestamp);

                    // Add source type
                    log.insert(log_schema().source_type_key(), Bytes::from("amqp"));

                    if let Some(key_field) = &config.routing_key {
                        log.insert(
                            key_field,
                            Value::from(msg.routing_key.to_string()),
                        );
                    }

                    if let Some(exchange_key) = &config.exchange_key {
                        log.insert(exchange_key, Value::from(msg.exchange.to_string()));
                    }

                    if let Some(offset_key) = &config.offset_key {
                        log.insert(offset_key, Value::from(msg.delivery_tag as i64));
                    }

                    if let Err(error) = out.send(event).await {
                        emit!(AmqpDeliveryFailed { error });
                    }
                    if let Err(error) = msg.acker.ack(ack_options).await {
                        emit!(AmqpCommitFailed { error });
                    }
                }
            }
        } else {
            break;
        }
    }

    Ok(())
}

#[cfg(test)]
pub mod test {
    use super::{amqp_source, AmqpSourceConfig};
    use crate::{shutdown::ShutdownSignal, Pipeline};

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<AmqpSourceConfig>();
    }

    pub fn make_config() -> AmqpSourceConfig {
        let mut config = AmqpSourceConfig::default();
        config.queue = "it".to_string();
        let user = std::env::var("AMQP_USER").unwrap_or_else(|_| "guest".to_string());
        let pass = std::env::var("AMQP_PASSWORD").unwrap_or_else(|_| "guest".to_string());
        let vhost = std::env::var("AMQP_VHOST").unwrap_or_else(|_| "%2f".to_string());
        config.connection.connection_string = format!("amqp://{}:{}@127.0.0.1:5672/{}", user, pass, vhost);
        config
    }

    #[tokio::test]
    async fn amqp_source_create_ok() {
        let config = make_config();
        assert!(amqp_source(&config, ShutdownSignal::noop(), Pipeline::new_test().0).await.is_ok());
    }
}

#[cfg(feature = "amqp-integration-tests")]
#[cfg(test)]
mod integration_test {
    use super::*;
    use super::test::*;
    use crate::{
        shutdown::ShutdownSignal,
        test_util::{collect_n, random_string},
        Pipeline,
    };
    use chrono::Utc;
    use lapin::options::*;
    use lapin::BasicProperties;

    async fn send_event(channel: &lapin::Channel, exchange: &str, routing_key: &str, text: &str, _timestamp: i64) {
        let payload = text.as_bytes().to_vec();
        let payload_len = payload.len();
        trace!(
            "Sending message of length {} to {}",
            payload_len,
            exchange,
        );

        channel
            .basic_publish(
                exchange,
                routing_key,
                BasicPublishOptions::default(),
                payload,
                BasicProperties::default(),
            )
            .await.unwrap()
            .await.unwrap();
    }

    #[tokio::test]
    async fn amqp_source_consume_event() {
        let exchange = format!("test-{}-exchange", random_string(10));
        let queue = format!("test-{}-queue", random_string(10));
        let routing_key = "my_key";
        println!("Test exchange name: {}", exchange);
        let consumer = format!("test-consumer-{}", random_string(10));

        let mut config = make_config();
        config.consumer = consumer;
        config.queue = queue;
        config.routing_key = Some("message_key".to_string());
        config.exchange_key = Some("exchange".to_string());
        let (_conn, channel) = config.connection.connect().await.unwrap();

        let mut exchange_opts = lapin::options::ExchangeDeclareOptions::default();
        exchange_opts.auto_delete = true;
        channel.exchange_declare(
            &exchange,
            lapin::ExchangeKind::Fanout,
            exchange_opts,
            lapin::types::FieldTable::default(),
        ).await.unwrap();

        let mut queue_opts = QueueDeclareOptions::default();
        queue_opts.auto_delete = true;
        channel.queue_declare(
            &config.queue,
            queue_opts,
            lapin::types::FieldTable::default(),
        ).await.unwrap();

        channel.queue_bind(
            &config.queue,
            &exchange,
            "",
            lapin::options::QueueBindOptions::default(),
            lapin::types::FieldTable::default(),
        ).await.unwrap();

        println!("Sending event...");
        let now = Utc::now();
        send_event(
            &channel,
            &exchange,
            routing_key,
            "my message",
            now.timestamp_millis(),
        )
        .await;

        println!("Receiving event...");
        let (tx, rx) = Pipeline::new_test();
        tokio::spawn(amqp_source(&config, ShutdownSignal::noop(), tx).await.unwrap());
        let events = collect_n(rx, 1).await;

        assert!(!events.is_empty());

        let log = events[0].as_log();
        println!("{:?}", log);
        assert_eq!(
            log[log_schema().message_key()],
            "my message".into()
        );
        assert_eq!(log["message_key"], routing_key.into());
        assert_eq!(
            log[log_schema().source_type_key()],
            "amqp".into()
        );
        let log_ts = log[log_schema().timestamp_key()].as_timestamp().unwrap();
        assert!(log_ts.signed_duration_since(now) < chrono::Duration::seconds(1));
        assert_eq!(log["exchange"], exchange.into());
    }
}
