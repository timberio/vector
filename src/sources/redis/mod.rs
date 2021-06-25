use crate::{
    config::{
        log_schema, DataType, GenerateConfig, SourceConfig, SourceContext, SourceDescription,
    },
    event::Event,
    shutdown::ShutdownSignal,
    Pipeline,
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

mod channel;
mod list;

#[derive(Copy, Clone, Debug, Derivative, Deserialize, Serialize)]
#[derivative(Default)]
#[serde(rename_all = "lowercase")]
pub enum DataTypeConfig {
    #[derivative(Default)]
    List,
    Channel,
}

#[derive(Copy, Clone, Debug, Derivative, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub struct ListOption {
    method: Method,
}

#[derive(Clone, Copy, Debug, Derivative, Deserialize, Serialize, Eq, PartialEq)]
#[derivative(Default)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    #[derivative(Default)]
    Blpop,
    Brpop,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RedisSourceConfig {
    #[serde(default)]
    pub data_type: DataTypeConfig,
    #[serde(alias = "list")]
    list_option: Option<ListOption>,
    pub url: String,
    pub key: String,
    pub redis_key: Option<String>,
}

impl GenerateConfig for RedisSourceConfig {
    fn generate_config() -> toml::Value {
        toml::from_str(
            r#"
            url = "redis://127.0.0.1:6379/0"
            key = "vector"
            data_type = "list"
            list.method = "brpop"
            redis_key = "redis_key"
            "#,
        )
        .unwrap()
    }
}

inventory::submit! {
    SourceDescription::new::<RedisSourceConfig>("redis")
}

#[async_trait::async_trait]
#[typetag::serde(name = "redis")]
impl SourceConfig for RedisSourceConfig {
    async fn build(&self, cx: SourceContext) -> crate::Result<super::Source> {
        redis_source(self, cx.shutdown, cx.out)
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn source_type(&self) -> &'static str {
        "redis"
    }
}

fn redis_source(
    config: &RedisSourceConfig,
    shutdown: ShutdownSignal,
    out: Pipeline,
) -> crate::Result<super::Source> {
    if config.key.is_empty() {
        return Err("`key` cannot be empty.".into());
    } else if let DataTypeConfig::List = config.data_type {
        if config.list_option.is_none() {
            return Err("When `data_type` is `list`, `list.method` cannot be empty.".into());
        }
    }

    let client = build_client(config).expect("Failed to open redis client.");
    let key = config.key.clone();
    let redis_key = config.redis_key.clone();

    match config.data_type {
        DataTypeConfig::List => match config.list_option {
            Some(option) => Ok(list::watch(
                client,
                key,
                redis_key,
                option.method,
                shutdown,
                out,
            )),
            None => {
                panic!("When `data_type` is `list`, `method` cannot be empty.")
            }
        },
        DataTypeConfig::Channel => Ok(channel::subscribe(client, key, redis_key, shutdown, out)),
    }
}

fn build_client(config: &RedisSourceConfig) -> crate::Result<redis::Client> {
    trace!("Open redis client.");
    let client = redis::Client::open(config.url.as_str())?;
    trace!("Open redis client successed.");
    Ok(client)
}

fn create_event(line: &str, key: String, redis_key: &Option<String>) -> Event {
    let mut event = Event::from(line);
    event
        .as_mut_log()
        .insert(log_schema().source_type_key(), Bytes::from("redis"));
    if let Some(redis_key) = &redis_key {
        event.as_mut_log().insert(redis_key.clone(), key);
    }
    event
}

#[cfg(test)]
mod test {
    use super::{redis_source, DataTypeConfig, ListOption, Method, RedisSourceConfig};
    use crate::{shutdown::ShutdownSignal, Pipeline};

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<RedisSourceConfig>();
    }

    #[test]
    fn redis_list_source_create_ok() {
        let config = RedisSourceConfig {
            data_type: DataTypeConfig::List,
            list_option: Some(ListOption {
                method: Method::Brpop,
            }),
            url: String::from("redis://127.0.0.1:6379/0"),
            key: String::from("vector"),
            redis_key: None,
        };
        assert!(redis_source(&config, ShutdownSignal::noop(), Pipeline::new_test().0).is_ok());
    }

    #[test]
    fn redis_channel_source_create_ok() {
        let config = RedisSourceConfig {
            data_type: DataTypeConfig::Channel,
            list_option: None,
            url: String::from("redis://127.0.0.1:6379/0"),
            key: String::from("vector"),
            redis_key: None,
        };
        assert!(redis_source(&config, ShutdownSignal::noop(), Pipeline::new_test().0).is_ok());
    }
}

#[cfg(feature = "redis-integration-tests")]
#[cfg(test)]
mod integration_test {
    use super::*;
    use crate::config::log_schema;
    use crate::{
        shutdown::ShutdownSignal,
        test_util::{collect_n, random_string},
        Pipeline,
    };
    use core::time;
    use redis::{AsyncCommands, RedisResult};
    use std::thread;

    const REDIS_SERVER: &str = "redis://127.0.0.1:6379/0";

    async fn send_event_by_list(key: String, text: &str) {
        let client = redis::Client::open(REDIS_SERVER).unwrap();
        trace!("Get redis connection manager.");
        let mut conn = client
            .get_tokio_connection_manager()
            .await
            .expect("Failed to get redis async connection.");
        trace!("Get redis connection manager success.");
        let res: RedisResult<i32> = conn.lpush(key, text).await;
        match res {
            Ok(len) => {
                debug!("Send event for list success, len: {:?}.", len);
            }
            Err(err) => {
                panic!("Send event for list error: {:?}.", err);
            }
        }
    }

    #[tokio::test]
    async fn redis_source_list_brpop() {
        let key = format!("test-key-{}", random_string(10));
        debug!("Test key name: {}.", key);

        let config = RedisSourceConfig {
            data_type: DataTypeConfig::List,
            list_option: Some(ListOption {
                method: Method::Brpop,
            }),
            url: REDIS_SERVER.to_owned(),
            key: key.clone(),
            redis_key: None,
        };

        debug!("Sending event.");
        send_event_by_list(key.clone(), "test message for list(brpop)").await;

        debug!("Receiving event.");
        let (tx, rx) = Pipeline::new_test();
        tokio::spawn(redis_source(&config, ShutdownSignal::noop(), tx).unwrap());
        let events = collect_n(rx, 1).await;

        assert_eq!(
            events[0].as_log()[log_schema().message_key()],
            "test message for list(brpop)".into()
        );
        assert_eq!(
            events[0].as_log()[log_schema().source_type_key()],
            "redis".into()
        );
    }

    #[tokio::test]
    async fn redis_source_list_blpop() {
        let key = format!("test-key-{}", random_string(10));
        debug!("Test key name: {}.", key);

        let config = RedisSourceConfig {
            data_type: DataTypeConfig::List,
            list_option: Some(ListOption {
                method: Method::Blpop,
            }),
            url: REDIS_SERVER.to_owned(),
            key: key.clone(),
            redis_key: None,
        };

        debug!("Sending event.");
        send_event_by_list(key.clone(), "test message for list(blpop)").await;

        debug!("Receiving event.");
        let (tx, rx) = Pipeline::new_test();
        tokio::spawn(redis_source(&config, ShutdownSignal::noop(), tx).unwrap());
        let events = collect_n(rx, 1).await;

        assert_eq!(
            events[0].as_log()[log_schema().message_key()],
            "test message for list(blpop)".into()
        );
        assert_eq!(
            events[0].as_log()[log_schema().source_type_key()],
            "redis".into()
        );
    }

    #[tokio::test]
    async fn redis_source_channel_consume_event() {
        let key = "test-channel".to_owned();
        debug!("Test key name: {}.", key);

        let text = "test message for channel";

        let config = RedisSourceConfig {
            data_type: DataTypeConfig::Channel,
            list_option: None,
            url: REDIS_SERVER.to_owned(),
            key: key.clone(),
            redis_key: None,
        };

        debug!("Receiving event.");
        let (tx, rx) = Pipeline::new_test();
        tokio::spawn(redis_source(&config, ShutdownSignal::noop(), tx).unwrap());
        let events = collect_n(rx, 10000).await;

        assert_eq!(events.len(), 10000);

        for event in events.iter().take(10000) {
            assert_eq!(event.as_log()[log_schema().message_key()], text.into());
            assert_eq!(
                event.as_log()[log_schema().source_type_key()],
                "redis".into()
            );
        }
    }

    #[tokio::test]
    async fn redis_source_channel_product_event() {
        debug!("Sending event by channel.");
        let key = "test-channel".to_owned();
        debug!("Test key name: {}.", key);
        let text = "test message for channel";

        let client = redis::Client::open(REDIS_SERVER).unwrap();

        // must subscribe the channel before publish msg
        let mut conn = client.get_connection().unwrap();
        let mut pubsub = conn.as_pubsub();
        pubsub.subscribe(key.clone()).unwrap();

        trace!("Get redis async connection.");
        let mut async_conn = client
            .get_async_connection()
            .await
            .expect("Failed to get redis async connection.");
        trace!("Get redis async connection success.");

        // wait fo redis_source subscribe the channel.
        thread::sleep(time::Duration::from_secs(1));

        for _i in 0..10000 {
            let res: RedisResult<i32> = async_conn.publish(key.clone(), text).await;
            match res {
                Ok(len) => {
                    debug!("Send event by channel success, len: {:?}.", len);
                    assert_ne!(len, 0);
                }
                Err(err) => {
                    panic!("Send event by channel error: {:?}.", err);
                }
            }
        }
    }
}
