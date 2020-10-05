use crate::{
    buffers::Acker,
    config::{DataType, SinkConfig, SinkContext, SinkDescription},
    emit,
    internal_events::BlackholeEventReceived,
    sinks::util::StreamSink,
    Event,
};
use async_trait::async_trait;
use futures::{future, stream::BoxStream, FutureExt, StreamExt};
use serde::{Deserialize, Serialize};
use string_cache::DefaultAtom as Atom;

pub struct BlackholeSink {
    total_events: usize,
    total_raw_bytes: usize,
    config: BlackholeConfig,
    acker: Acker,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct BlackholeConfig {
    pub print_amount: usize,
}

inventory::submit! {
    SinkDescription::new_without_default::<BlackholeConfig>("blackhole")
}

#[async_trait::async_trait]
#[typetag::serde(name = "blackhole")]
impl SinkConfig for BlackholeConfig {
    async fn build(
        &self,
        cx: SinkContext,
    ) -> crate::Result<(super::VectorSink, super::Healthcheck)> {
        let sink = BlackholeSink::new(self.clone(), cx.acker());
        let healthcheck = future::ok(()).boxed();

        Ok((super::VectorSink::Stream(Box::new(sink)), healthcheck))
    }

    fn input_type(&self) -> DataType {
        DataType::Any
    }

    fn sink_type(&self) -> &'static str {
        "blackhole"
    }
}

impl BlackholeSink {
    pub fn new(config: BlackholeConfig, acker: Acker) -> Self {
        BlackholeSink {
            config,
            total_events: 0,
            total_raw_bytes: 0,
            acker,
        }
    }
}

#[async_trait]
impl StreamSink for BlackholeSink {
    async fn run(&mut self, mut input: BoxStream<'_, Event>) -> Result<(), ()> {
        while let Some(event) = input.next().await {
            let message_len = match event {
                Event::Log(log) => log
                    .get(&Atom::from(crate::config::log_schema().message_key()))
                    .map(|v| v.as_bytes().len())
                    .unwrap_or(0),
                Event::Metric(metric) => {
                    serde_json::to_string(&metric).map(|v| v.len()).unwrap_or(0)
                }
            };

            self.total_events += 1;
            self.total_raw_bytes += message_len;

            emit!(BlackholeEventReceived {
                byte_size: message_len
            });

            if self.total_events % self.config.print_amount == 0 {
                info!({
                    events = self.total_events,
                    raw_bytes_collected = self.total_raw_bytes
                }, "Total events collected");
            }

            self.acker.ack(1);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::random_events_with_stream;

    #[tokio::test]
    async fn blackhole() {
        let config = BlackholeConfig { print_amount: 10 };
        let mut sink = BlackholeSink::new(config, Acker::Null);

        let (_input_lines, events) = random_events_with_stream(100, 10);
        let _ = sink.run(Box::pin(events)).await.unwrap();
    }
}
