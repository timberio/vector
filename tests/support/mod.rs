// Using a shared mod like this is probably not the best idea, since we have to
// disable the `dead_code` lint, as we don't need all of the helpers from here
// all over the place.
#![allow(clippy::type_complexity)]
#![allow(dead_code)]

use async_trait::async_trait;
use futures::{
    channel::mpsc,
    future,
    stream::{self, BoxStream},
    task::Poll,
    FutureExt, Sink, SinkExt, StreamExt,
};
use serde::{Deserialize, Serialize};
use snafu::Snafu;
use std::{
    collections::BTreeSet,
    fs::{create_dir, OpenOptions},
    io::Write,
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    task::Context,
};
use tracing::{error, info};
use vector::{
    buffers::Acker,
    config::{
        DataType, GlobalOptions, SinkConfig, SinkContext, SourceConfig, SourceContext,
        TransformConfig,
    },
    event::{
        metric::{self, MetricData, MetricValue},
        Event, Value,
    },
    sinks::{util::StreamSink, Healthcheck, VectorSink},
    sources::Source,
    test_util::{temp_dir, temp_file},
    transforms::{FunctionTransform, Transform},
    Pipeline,
};

pub fn sink(channel_size: usize) -> (mpsc::Receiver<Event>, MockSinkConfig<Pipeline>) {
    let (tx, rx) = Pipeline::new_with_buffer(channel_size, vec![]);
    let sink = MockSinkConfig::new(tx, true);
    (rx, sink)
}

pub fn sink_failing_healthcheck(
    channel_size: usize,
) -> (mpsc::Receiver<Event>, MockSinkConfig<Pipeline>) {
    let (tx, rx) = Pipeline::new_with_buffer(channel_size, vec![]);
    let sink = MockSinkConfig::new(tx, false);
    (rx, sink)
}

pub fn sink_dead() -> MockSinkConfig<DeadSink<Event>> {
    MockSinkConfig::new(DeadSink::new(), false)
}

pub fn source() -> (Pipeline, MockSourceConfig) {
    let (tx, rx) = Pipeline::new_with_buffer(1, vec![]);
    let source = MockSourceConfig::new(rx);
    (tx, source)
}

pub fn source_with_event_counter() -> (Pipeline, MockSourceConfig, Arc<AtomicUsize>) {
    let event_counter = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = Pipeline::new_with_buffer(1, vec![]);
    let source = MockSourceConfig::new_with_event_counter(rx, event_counter.clone());
    (tx, source, event_counter)
}

pub fn transform(suffix: &str, increase: f64) -> MockTransformConfig {
    MockTransformConfig::new(suffix.to_owned(), increase)
}

/// Creates a file with given content
pub fn create_file(config: &str) -> PathBuf {
    let path = temp_file();
    overwrite_file(path.clone(), config);
    path
}

/// Overwrites file with given content
pub fn overwrite_file(path: PathBuf, config: &str) {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .unwrap();

    file.write_all(config.as_bytes()).unwrap();
    file.flush().unwrap();
    file.sync_all().unwrap();
}

pub fn create_directory() -> PathBuf {
    let path = temp_dir();
    create_dir(path.clone()).unwrap();
    path
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MockSourceConfig {
    #[serde(skip)]
    receiver: Arc<Mutex<Option<mpsc::Receiver<Event>>>>,
    #[serde(skip)]
    event_counter: Option<Arc<AtomicUsize>>,
    #[serde(skip)]
    data_type: Option<DataType>,
}

impl MockSourceConfig {
    pub fn new(receiver: mpsc::Receiver<Event>) -> Self {
        Self {
            receiver: Arc::new(Mutex::new(Some(receiver))),
            event_counter: None,
            data_type: Some(DataType::Any),
        }
    }

    pub fn new_with_event_counter(
        receiver: mpsc::Receiver<Event>,
        event_counter: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            receiver: Arc::new(Mutex::new(Some(receiver))),
            event_counter: Some(event_counter),
            data_type: Some(DataType::Any),
        }
    }

    pub fn set_data_type(&mut self, data_type: DataType) {
        self.data_type = Some(data_type)
    }
}

#[async_trait]
#[typetag::serde(name = "mock")]
impl SourceConfig for MockSourceConfig {
    async fn build(&self, cx: SourceContext) -> Result<Source, vector::Error> {
        let wrapped = self.receiver.clone();
        let event_counter = self.event_counter.clone();
        let mut recv = wrapped.lock().unwrap().take().unwrap();
        let mut shutdown = Some(cx.shutdown);
        let mut _token = None;
        let out = cx.out;
        Ok(Box::pin(async move {
            stream::poll_fn(move |cx| {
                if let Some(until) = shutdown.as_mut() {
                    match until.poll_unpin(cx) {
                        Poll::Ready(res) => {
                            _token = Some(res);
                            shutdown.take();
                            recv.close();
                        }
                        Poll::Pending => {}
                    }
                }

                recv.poll_next_unpin(cx)
            })
            .inspect(move |_| {
                if let Some(counter) = &event_counter {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            })
            .map(Ok)
            .forward(out.sink_map_err(|error| error!(message = "Error sending in sink..", %error)))
            .inspect(|_| info!("Finished sending."))
            .await
        }))
    }

    fn output_type(&self) -> DataType {
        self.data_type.clone().unwrap()
    }

    fn source_type(&self) -> &'static str {
        "mock"
    }
}

#[derive(Clone, Debug)]
pub struct MockTransform {
    suffix: String,
    increase: f64,
}

impl FunctionTransform<Event> for MockTransform {
    fn transform(&mut self, output: &mut Vec<Event>, mut event: Event) {
        match &mut event {
            Event::Log(log) => {
                let mut v = log
                    .get(vector::config::log_schema().message_key())
                    .unwrap()
                    .to_string_lossy();
                v.push_str(&self.suffix);
                log.insert(vector::config::log_schema().message_key(), Value::from(v));
            }
            Event::Metric(metric) => {
                let increment = match metric.value() {
                    MetricValue::Counter { .. } => Some(MetricValue::Counter {
                        value: self.increase,
                    }),
                    MetricValue::Gauge { .. } => Some(MetricValue::Gauge {
                        value: self.increase,
                    }),
                    MetricValue::Distribution { statistic, .. } => {
                        Some(MetricValue::Distribution {
                            samples: vec![metric::Sample {
                                value: self.increase,
                                rate: 1,
                            }],
                            statistic: *statistic,
                        })
                    }
                    MetricValue::AggregatedHistogram { .. } => None,
                    MetricValue::AggregatedSummary { .. } => None,
                    MetricValue::Set { .. } => {
                        let mut values = BTreeSet::new();
                        values.insert(self.suffix.clone());
                        Some(MetricValue::Set { values })
                    }
                };
                if let Some(increment) = increment {
                    assert!(metric.add(&MetricData {
                        kind: metric.kind(),
                        timestamp: metric.timestamp(),
                        value: increment,
                    }));
                }
            }
        };
        output.push(event);
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MockTransformConfig {
    suffix: String,
    increase: f64,
}

impl MockTransformConfig {
    pub fn new(suffix: String, increase: f64) -> Self {
        Self { suffix, increase }
    }
}

#[async_trait]
#[typetag::serde(name = "mock")]
impl TransformConfig for MockTransformConfig {
    async fn build(&self, _globals: &GlobalOptions) -> Result<Transform<Event>, vector::Error> {
        Ok(Transform::function(MockTransform {
            suffix: self.suffix.clone(),
            increase: self.increase,
        }))
    }

    fn input_type(&self) -> DataType {
        DataType::Any
    }

    fn output_type(&self) -> DataType {
        DataType::Any
    }

    fn transform_type(&self) -> &'static str {
        "mock"
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MockSinkConfig<T>
where
    T: Sink<Event> + Unpin + std::fmt::Debug + Clone + Send + Sync + 'static,
    <T as Sink<Event>>::Error: std::fmt::Display,
{
    #[serde(skip)]
    sink: Option<T>,
    #[serde(skip)]
    healthy: bool,
}

impl<T> MockSinkConfig<T>
where
    T: Sink<Event> + Unpin + std::fmt::Debug + Clone + Send + Sync + 'static,
    <T as Sink<Event>>::Error: std::fmt::Display,
{
    pub fn new(sink: T, healthy: bool) -> Self {
        Self {
            sink: Some(sink),
            healthy,
        }
    }
}

#[derive(Debug, Snafu)]
enum HealthcheckError {
    #[snafu(display("unhealthy"))]
    Unhealthy,
}

#[async_trait]
#[typetag::serialize(name = "mock")]
impl<T> SinkConfig for MockSinkConfig<T>
where
    T: Sink<Event> + Unpin + std::fmt::Debug + Clone + Send + Sync + 'static,
    <T as Sink<Event>>::Error: std::fmt::Display,
{
    async fn build(&self, cx: SinkContext) -> Result<(VectorSink, Healthcheck), vector::Error> {
        let sink = MockSink {
            acker: cx.acker(),
            sink: self.sink.clone().unwrap(),
        };

        let healthcheck = if self.healthy {
            future::ok(())
        } else {
            future::err(HealthcheckError::Unhealthy.into())
        };

        Ok((VectorSink::Stream(Box::new(sink)), healthcheck.boxed()))
    }

    fn input_type(&self) -> DataType {
        DataType::Any
    }

    fn sink_type(&self) -> &'static str {
        "mock"
    }

    fn typetag_deserialize(&self) {
        unimplemented!("not intended for use in real configs")
    }
}

struct MockSink<S> {
    acker: Acker,
    sink: S,
}

#[async_trait]
impl<S> StreamSink for MockSink<S>
where
    S: Sink<Event> + Send + std::marker::Unpin,
    <S as Sink<Event>>::Error: std::fmt::Display,
{
    async fn run(&mut self, mut input: BoxStream<'_, Event>) -> Result<(), ()> {
        while let Some(event) = input.next().await {
            if let Err(error) = self.sink.send(event).await {
                error!(message = "Ingesting an event failed at mock sink.", %error);
            }

            self.acker.ack(1);
        }

        Ok(())
    }
}

/// Represents a sink that's never ready.
/// Useful to simulate an upstream sink server that is down.
#[derive(Debug, Clone)]
pub struct DeadSink<T>(std::marker::PhantomData<T>);

impl<T> DeadSink<T> {
    pub fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<T> Sink<T> for DeadSink<T> {
    type Error = &'static str;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Pending
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Pending
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Pending
    }

    fn start_send(self: Pin<&mut Self>, _item: T) -> Result<(), Self::Error> {
        Err("never ready")
    }
}
