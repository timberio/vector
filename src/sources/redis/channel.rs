use crate::{
    internal_events::{RedisEventReceived, RedisReceiveEventFailed},
    shutdown::ShutdownSignal,
    sources::redis::create_event,
    sources::Source,
    Pipeline,
};
use futures::{SinkExt, StreamExt};

pub fn subscribe(
    client: redis::Client,
    key: String,
    redis_key: Option<String>,
    mut shutdown: ShutdownSignal,
    out: Pipeline,
) -> Source {
    let mut out = out.sink_map_err(|error| error!(message="Error sending event.", %error));
    let fut = async move {
        trace!("Get redis async connection.");
        let conn = client
            .get_async_connection()
            .await
            .expect("Failed to get redis async connection.");
        trace!("Get redis async connection success.");
        let mut pubsub_conn = conn.into_pubsub();
        trace!("Subscrib channel:{}.", key.as_str());
        pubsub_conn
            .subscribe(key.as_str())
            .await
            .unwrap_or_else(|_| panic!("Failed to subscribe channel:{}.", key.as_str()));
        trace!("Subscribed to channel:{}.", key.as_str());
        let mut pubsub_stream = pubsub_conn.on_message();
        loop {
            let msg = pubsub_stream.next().await.unwrap();
            let line = msg
                .get_payload::<String>()
                .map_err(|error| emit!(RedisReceiveEventFailed { error }))
                .unwrap_or_default();
            emit!(RedisEventReceived {
                byte_size: line.len()
            });
            let event = create_event(line.as_str(), key.clone(), &redis_key);
            tokio::select! {
                result = out.send(event) => {match result {
                    Ok(()) => { },
                    Err(()) => return Ok(()),
                }}
                _ = &mut shutdown => return Ok(()),
            }
        }
    };
    Box::pin(fut)
}
