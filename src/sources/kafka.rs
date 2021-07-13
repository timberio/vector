use super::util::{
    decoding::{DecodingBuilder, DecodingConfig},
    finalizer::OrderedFinalizer,
};
use crate::{
    config::{log_schema, DataType, SourceConfig, SourceContext, SourceDescription},
    event::{BatchNotifier, LogEvent, Value},
    internal_events::{
        KafkaDecodeMessageFailed, KafkaEventFailed, KafkaEventReceived, KafkaOffsetUpdateFailed,
    },
    kafka::{KafkaAuthConfig, KafkaStatisticsContext},
    shutdown::ShutdownSignal,
    Pipeline,
};
use bytes::Bytes;
use chrono::{TimeZone, Utc};
use futures::{FutureExt, SinkExt, StreamExt};
use rdkafka::{
    config::ClientConfig,
    consumer::{Consumer, StreamConsumer},
    message::{BorrowedMessage, Headers, Message},
    Offset, TopicPartitionList,
};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

#[derive(Debug, Snafu)]
enum BuildError {
    #[snafu(display("Could not create Kafka consumer: {}", source))]
    KafkaCreateError { source: rdkafka::error::KafkaError },
    #[snafu(display("Could not subscribe to Kafka topics: {}", source))]
    KafkaSubscribeError { source: rdkafka::error::KafkaError },
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KafkaSourceConfig {
    bootstrap_servers: String,
    topics: Vec<String>,
    group_id: String,
    #[serde(default = "default_auto_offset_reset")]
    auto_offset_reset: String,
    #[serde(default = "default_session_timeout_ms")]
    session_timeout_ms: u64,
    #[serde(default = "default_socket_timeout_ms")]
    socket_timeout_ms: u64,
    #[serde(default = "default_fetch_wait_max_ms")]
    fetch_wait_max_ms: u64,
    #[serde(default = "default_commit_interval_ms")]
    commit_interval_ms: u64,
    #[serde(default = "default_key_field")]
    key_field: String,
    #[serde(default = "default_topic_key")]
    topic_key: String,
    #[serde(default = "default_partition_key")]
    partition_key: String,
    #[serde(default = "default_offset_key")]
    offset_key: String,
    #[serde(default = "default_headers_key")]
    headers_key: String,
    librdkafka_options: Option<HashMap<String, String>>,
    #[serde(flatten)]
    auth: KafkaAuthConfig,
    decoding: Option<DecodingConfig>,
}

fn default_session_timeout_ms() -> u64 {
    10000 // default in librdkafka
}

fn default_socket_timeout_ms() -> u64 {
    60000 // default in librdkafka
}

fn default_fetch_wait_max_ms() -> u64 {
    100 // default in librdkafka
}

fn default_commit_interval_ms() -> u64 {
    5000 // default in librdkafka
}

fn default_auto_offset_reset() -> String {
    "largest".into() // default in librdkafka
}

fn default_key_field() -> String {
    "message_key".into()
}

fn default_topic_key() -> String {
    "topic".into()
}

fn default_partition_key() -> String {
    "partition".into()
}

fn default_offset_key() -> String {
    "offset".into()
}

fn default_headers_key() -> String {
    "headers".into()
}

inventory::submit! {
    SourceDescription::new::<KafkaSourceConfig>("kafka")
}

impl_generate_config_from_default!(KafkaSourceConfig);

#[async_trait::async_trait]
#[typetag::serde(name = "kafka")]
impl SourceConfig for KafkaSourceConfig {
    async fn build(&self, cx: SourceContext) -> crate::Result<super::Source> {
        let consumer = create_consumer(self)?;
        let decode = self.decoding.build()?;

        Ok(Box::pin(kafka_source(
            consumer,
            self.key_field.clone(),
            self.topic_key.clone(),
            self.partition_key.clone(),
            self.offset_key.clone(),
            self.headers_key.clone(),
            cx.shutdown,
            cx.out,
            cx.acknowledgements,
            decode,
        )))
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn source_type(&self) -> &'static str {
        "kafka"
    }
}

async fn kafka_source(
    consumer: StreamConsumer<KafkaStatisticsContext>,
    key_field: String,
    topic_key: String,
    partition_key: String,
    offset_key: String,
    headers_key: String,
    shutdown: ShutdownSignal,
    mut out: Pipeline,
    acknowledgements: bool,
    decode: impl Fn(Bytes) -> crate::Result<Value> + Send,
) -> Result<(), ()> {
    let consumer = Arc::new(consumer);
    let shutdown = shutdown.shared();
    let mut finalizer = acknowledgements
        .then(|| OrderedFinalizer::new(shutdown.clone(), mark_done(Arc::clone(&consumer))));
    let mut stream = consumer.stream().take_until(shutdown);

    while let Some(message) = stream.next().await {
        match message {
            Err(error) => {
                emit!(KafkaEventFailed { error });
            }
            Ok(msg) => {
                emit!(KafkaEventReceived {
                    byte_size: msg.payload_len()
                });

                let payload = match msg.payload() {
                    None => continue, // skip messages with empty payload
                    Some(payload) => payload,
                };
                let mut log = LogEvent::default();

                let message = match decode(Bytes::from(payload.to_owned())) {
                    Ok(message) => message,
                    Err(error) => {
                        emit!(KafkaDecodeMessageFailed { error });
                        continue;
                    }
                };

                log.insert(log_schema().message_key(), message);

                // Extract timestamp from kafka message
                let timestamp = msg
                    .timestamp()
                    .to_millis()
                    .and_then(|millis| Utc.timestamp_millis_opt(millis).latest())
                    .unwrap_or_else(Utc::now);
                log.insert(log_schema().timestamp_key(), timestamp);

                // Add source type
                log.insert(log_schema().source_type_key(), Bytes::from("kafka"));

                let msg_key = msg
                    .key()
                    .map(|key| Value::from(String::from_utf8_lossy(key).to_string()))
                    .unwrap_or(Value::Null);
                log.insert(&key_field, msg_key);

                log.insert(&topic_key, Value::from(msg.topic().to_string()));

                log.insert(&partition_key, Value::from(msg.partition()));

                log.insert(&offset_key, Value::from(msg.offset()));

                let mut headers_map = BTreeMap::new();
                if let Some(headers) = msg.headers() {
                    // Using index-based for loop because rdkafka's `Headers` trait
                    // does not provide Iterator-based API
                    for i in 0..headers.count() {
                        if let Some(header) = headers.get(i) {
                            headers_map.insert(
                                header.0.to_string(),
                                Bytes::from(header.1.to_owned()).into(),
                            );
                        }
                    }
                }
                log.insert(&headers_key, Value::from(headers_map));

                match &mut finalizer {
                    Some(finalizer) => {
                        let (batch, receiver) = BatchNotifier::new_with_receiver();
                        let log = log.with_batch_notifier(&batch);
                        match out.send(log.into()).await {
                            Err(error) => error!(message = "Error sending to sink.", %error),
                            Ok(_) => finalizer.add(msg.into(), receiver),
                        }
                    }
                    None => match out.send(log.into()).await {
                        Err(error) => error!(message = "Error sending to sink.", %error),
                        Ok(_) => {
                            if let Err(error) = consumer.store_offset(&msg) {
                                emit!(KafkaOffsetUpdateFailed { error });
                            }
                        }
                    },
                }
            }
        }
    }

    Ok(())
}

#[derive(Debug)]
struct FinalizerEntry {
    topic: String,
    partition: i32,
    offset: i64,
}

impl<'a> From<BorrowedMessage<'a>> for FinalizerEntry {
    fn from(msg: BorrowedMessage<'a>) -> Self {
        Self {
            topic: msg.topic().into(),
            partition: msg.partition(),
            offset: msg.offset(),
        }
    }
}

fn mark_done(consumer: Arc<StreamConsumer<KafkaStatisticsContext>>) -> impl Fn(FinalizerEntry) {
    move |entry| {
        // Would like to use `consumer.store_offset` here, but types don't allow it.
        let mut tpl = TopicPartitionList::new();
        tpl.add_partition(&entry.topic, entry.partition)
            .set_offset(Offset::from_raw(entry.offset + 1)) // Not sure why this needs a +1
            .expect("Setting offset failed");
        if let Err(error) = consumer.store_offsets(&tpl) {
            emit!(KafkaOffsetUpdateFailed { error });
        }
    }
}

fn create_consumer(
    config: &KafkaSourceConfig,
) -> crate::Result<StreamConsumer<KafkaStatisticsContext>> {
    let mut client_config = ClientConfig::new();
    client_config
        .set("group.id", &config.group_id)
        .set("bootstrap.servers", &config.bootstrap_servers)
        .set("auto.offset.reset", &config.auto_offset_reset)
        .set("session.timeout.ms", &config.session_timeout_ms.to_string())
        .set("socket.timeout.ms", &config.socket_timeout_ms.to_string())
        .set("fetch.wait.max.ms", &config.fetch_wait_max_ms.to_string())
        .set("enable.partition.eof", "false")
        .set("enable.auto.commit", "true")
        .set(
            "auto.commit.interval.ms",
            &config.commit_interval_ms.to_string(),
        )
        .set("enable.auto.offset.store", "false")
        .set("statistics.interval.ms", "1000")
        .set("client.id", "vector");

    config.auth.apply(&mut client_config)?;

    if let Some(librdkafka_options) = &config.librdkafka_options {
        for (key, value) in librdkafka_options {
            client_config.set(key.as_str(), value.as_str());
        }
    }

    let consumer = client_config
        .create_with_context::<_, StreamConsumer<_>>(KafkaStatisticsContext)
        .context(KafkaCreateError)?;
    let topics: Vec<&str> = config.topics.iter().map(|s| s.as_str()).collect();
    consumer.subscribe(&topics).context(KafkaSubscribeError)?;

    Ok(consumer)
}

#[cfg(test)]
mod test {
    use super::*;

    pub(super) const BOOTSTRAP_SERVER: &str = "localhost:9091";

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<KafkaSourceConfig>();
    }

    pub(super) fn make_config(topic: &str, group: &str) -> KafkaSourceConfig {
        KafkaSourceConfig {
            bootstrap_servers: BOOTSTRAP_SERVER.into(),
            topics: vec![topic.into()],
            group_id: group.into(),
            auto_offset_reset: "beginning".into(),
            session_timeout_ms: 6000,
            commit_interval_ms: 5000,
            key_field: "message_key".to_string(),
            topic_key: "topic".to_string(),
            partition_key: "partition".to_string(),
            offset_key: "offset".to_string(),
            headers_key: "headers".to_string(),
            socket_timeout_ms: 60000,
            fetch_wait_max_ms: 100,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn consumer_create_ok() {
        let config = make_config("topic", "group");
        assert!(create_consumer(&config).is_ok());
    }

    #[tokio::test]
    async fn consumer_create_incorrect_auto_offset_reset() {
        let config = KafkaSourceConfig {
            auto_offset_reset: "incorrect-auto-offset-reset".to_string(),
            ..make_config("topic", "group")
        };
        assert!(create_consumer(&config).is_err());
    }
}

#[cfg(feature = "kafka-integration-tests")]
#[cfg(test)]
mod integration_test {
    use super::test::*;
    use super::*;
    use crate::{
        shutdown::ShutdownSignal,
        test_util::{collect_n, random_string},
        Pipeline,
    };
    use chrono::{SubsecRound, Utc};
    use rdkafka::{
        config::{ClientConfig, FromClientConfig},
        consumer::BaseConsumer,
        message::OwnedHeaders,
        producer::{FutureProducer, FutureRecord},
        util::Timeout,
    };
    use std::time::Duration;
    use vector_core::event::EventStatus;

    fn client_config<T: FromClientConfig>(group: Option<&str>) -> T {
        let mut client = ClientConfig::new();
        client.set("bootstrap.servers", BOOTSTRAP_SERVER);
        client.set("produce.offset.report", "true");
        client.set("message.timeout.ms", "5000");
        client.set("auto.commit.interval.ms", "1");
        if let Some(group) = group {
            client.set("group.id", group);
        }
        client.create().expect("Producer creation error")
    }

    async fn send_events(
        topic: String,
        count: usize,
        key: &str,
        text: &str,
        timestamp: i64,
        header_key: &str,
        header_value: &str,
    ) {
        let producer: FutureProducer = client_config(None);

        for i in 0..count {
            let text = format!("{} {}", text, i);
            let record = FutureRecord::to(&topic)
                .payload(&text)
                .key(key)
                .timestamp(timestamp)
                .headers(OwnedHeaders::new().add(header_key, header_value));

            if let Err(error) = producer.send(record, Timeout::Never).await {
                panic!("Cannot send event to Kafka: {:?}", error);
            }
        }
    }

    #[tokio::test]
    async fn consumes_event_with_acknowledgements() {
        consume_event(true).await;
    }

    #[tokio::test]
    async fn consumes_event_without_acknowledgements() {
        consume_event(false).await;
    }

    async fn consume_event(acknowledgements: bool) {
        let topic = format!("test-topic-{}", random_string(10));
        let group_id = format!("test-group-{}", random_string(10));
        let now = Utc::now();

        let config = make_config(&topic, &group_id);

        send_events(
            topic.clone(),
            10,
            "my key",
            "my message",
            now.timestamp_millis(),
            "my header",
            "my header value",
        )
        .await;

        let (trigger_shutdown, shutdown, shutdown_done) = ShutdownSignal::new_wired();
        let (tx, rx) = Pipeline::new_test_finalize(EventStatus::Delivered);
        tokio::spawn(kafka_source(
            create_consumer(&config).unwrap(),
            config.key_field,
            config.topic_key,
            config.partition_key,
            config.offset_key,
            config.headers_key,
            shutdown,
            tx,
            acknowledgements,
            None.build().unwrap(),
        ));
        let events = collect_n(rx, 10).await;
        drop(trigger_shutdown);
        shutdown_done.await;

        let client: BaseConsumer = client_config(Some(&group_id));
        client.subscribe(&[&topic]).expect("Subscribing failed");

        let mut tpl = TopicPartitionList::new();
        tpl.add_partition(&topic, 0);
        let tpl = client
            .committed_offsets(tpl, Duration::from_secs(1))
            .expect("Getting committed offsets failed");
        assert_eq!(
            tpl.find_partition(&topic, 0)
                .expect("TPL is missing topic")
                .offset(),
            Offset::from_raw(10)
        );

        assert_eq!(events.len(), 10);
        for (i, event) in events.into_iter().enumerate() {
            assert_eq!(
                event.as_log()[log_schema().message_key()],
                format!("my message {}", i).into()
            );
            assert_eq!(event.as_log()["message_key"], "my key".into());
            assert_eq!(
                event.as_log()[log_schema().source_type_key()],
                "kafka".into()
            );
            assert_eq!(
                event.as_log()[log_schema().timestamp_key()],
                now.trunc_subsecs(3).into()
            );
            assert_eq!(event.as_log()["topic"], topic.clone().into());
            assert!(event.as_log().contains("partition"));
            assert!(event.as_log().contains("offset"));
            let mut expected_headers = BTreeMap::new();
            expected_headers.insert("my header".to_string(), Value::from("my header value"));
            assert_eq!(event.as_log()["headers"], Value::from(expected_headers));
        }
    }
}
