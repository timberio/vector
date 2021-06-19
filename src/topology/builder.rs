use super::{
    fanout::{self, Fanout},
    task::{Task, TaskOutput},
    BuiltBuffer, ConfigDiff,
};
use crate::{
    buffers,
    codecs::CodecTransform,
    config::{DataType, SinkContext, SourceContext},
    event::Event,
    internal_events::{EventIn, EventOut, EventZeroIn},
    shutdown::SourceShutdownCoordinator,
    transform::{TaskTransform, Transform},
    Pipeline,
};
use futures::{future, FutureExt, SinkExt, StreamExt, TryFutureExt};
use std::pin::Pin;
use std::{
    collections::HashMap,
    future::ready,
    sync::{Arc, Mutex},
};
use stream_cancel::{StreamExt as StreamCancelExt, Trigger, Tripwire};
use tokio::time::{timeout, Duration};

pub struct Pieces {
    pub inputs: HashMap<String, (buffers::BufferInputCloner<Event>, Vec<String>)>,
    pub outputs: HashMap<String, fanout::ControlChannel>,
    pub tasks: HashMap<String, Task>,
    pub source_tasks: HashMap<String, Task>,
    pub healthchecks: HashMap<String, Task>,
    pub shutdown_coordinator: SourceShutdownCoordinator,
    pub detach_triggers: HashMap<String, Trigger>,
}

/// Builds only the new pieces, and doesn't check their topology.
pub async fn build_pieces(
    config: &super::Config,
    diff: &ConfigDiff,
    mut buffers: HashMap<String, BuiltBuffer>,
) -> Result<Pieces, Vec<String>> {
    let mut inputs = HashMap::new();
    let mut outputs = HashMap::new();
    let mut tasks = HashMap::new();
    let mut source_tasks = HashMap::new();
    let mut healthchecks = HashMap::new();
    let mut shutdown_coordinator = SourceShutdownCoordinator::default();
    let mut detach_triggers = HashMap::new();

    let mut errors = vec![];

    // Build sources
    for (name, source) in config
        .sources
        .iter()
        .filter(|(name, _)| diff.sources.contains_new(&name))
    {
        let (tx, rx) = futures::channel::mpsc::channel(1000);
        let pipeline = Pipeline::from_sender(tx, vec![]);

        let typetag = source.inner.source_type();

        /// Helper struct used to chain multiple framing transformations
        /// together, resulting in a single transform.
        struct FramingTransform {
            framing: Vec<Transform<Vec<u8>>>,
        }

        impl TaskTransform<Vec<u8>> for FramingTransform {
            fn transform(
                self: Box<Self>,
                task: Pin<Box<dyn futures::Stream<Item = Vec<u8>> + Send>>,
            ) -> Pin<Box<dyn futures::Stream<Item = Vec<u8>> + Send>>
            where
                Self: 'static,
            {
                let mut stream = task;
                for framing in self.framing {
                    stream = framing.transform(stream).boxed();
                }
                stream
            }
        }

        // Build framers and merge them into a single transform that can be
        // passed into the source context.
        let framing = Transform::task(FramingTransform {
            framing: source
                .framing
                .clone()
                .into_iter()
                .filter_map(|config| match config.build() {
                    Ok(framer) => Some(framer),
                    Err(error) => {
                        errors.push(format!("Framer \"{}\": {}", name, error));
                        None
                    }
                })
                .collect(),
        });

        let (shutdown_signal, force_shutdown_tripwire) = shutdown_coordinator.register_source(name);

        let context = SourceContext {
            name: name.into(),
            framing,
            globals: config.global.clone(),
            shutdown: shutdown_signal,
            out: pipeline,
            acknowledgements: source.acknowledgements,
        };

        let server = match source.inner.build(context).await {
            Err(error) => {
                errors.push(format!("Source \"{}\": {}", name, error));
                continue;
            }
            Ok(server) => server,
        };

        let (output, control) = Fanout::new();

        // Build and apply codecs to the receiver stream before it is forwarded
        // into the output.
        let mut rx = rx.boxed();
        for codec in source.codec.clone() {
            let CodecTransform {
                transform,
                input_type,
            } = match codec.build_decoder() {
                Ok(transform) => transform,
                Err(error) => {
                    errors.push(format!("Decoder \"{}\": {}", name, error));
                    continue;
                }
            };

            let filtered = rx.filter(move |event| ready(filter_event_type(event, input_type)));
            rx = transform.transform(filtered).boxed();
        }

        let pump = rx.map(Ok).forward(output).map_ok(|_| TaskOutput::Source);
        let pump = Task::new(name, typetag, pump);

        // The force_shutdown_tripwire is a Future that when it resolves means that this source
        // has failed to shut down gracefully within its allotted time window and instead should be
        // forcibly shut down. We accomplish this by select()-ing on the server Task with the
        // force_shutdown_tripwire. That means that if the force_shutdown_tripwire resolves while
        // the server Task is still running the Task will simply be dropped on the floor.
        let server = async {
            emit!(EventZeroIn);
            match future::try_select(server, force_shutdown_tripwire.unit_error().boxed()).await {
                Ok(_) => {
                    debug!("Finished.");
                    Ok(TaskOutput::Source)
                }
                Err(_) => Err(()),
            }
        };
        let server = Task::new(name, typetag, server);

        outputs.insert(name.clone(), control);
        tasks.insert(name.clone(), pump);
        source_tasks.insert(name.clone(), server);
    }

    // Build transforms
    for (name, transform) in config
        .transforms
        .iter()
        .filter(|(name, _)| diff.transforms.contains_new(&name))
    {
        let trans_inputs = &transform.inputs;

        let typetag = transform.inner.transform_type();

        let input_type = transform.inner.input_type();
        let transform = match transform.inner.build(&config.global).await {
            Err(error) => {
                errors.push(format!("Transform \"{}\": {}", name, error));
                continue;
            }
            Ok(transform) => transform,
        };

        let (input_tx, input_rx, _) =
            vector_core::buffers::build(vector_core::buffers::Variant::Memory {
                max_events: 100,
                when_full: vector_core::buffers::WhenFull::Block,
            })
            .unwrap();
        let input_rx = crate::utilization::wrap(Pin::new(input_rx));

        let (output, control) = Fanout::new();

        let transform = {
            let filtered = input_rx
                .filter(move |event| ready(filter_event_type(event, input_type)))
                .inspect(|_| emit!(EventIn));

            transform
                .transform(filtered)
                .map(Ok)
                .forward(output.with(|event| async {
                    emit!(EventOut { count: 1 });
                    Ok(event)
                }))
                .boxed()
                .map_ok(|_| {
                    debug!("Finished.");
                    TaskOutput::Transform
                })
        };
        let task = Task::new(name, typetag, transform);

        inputs.insert(name.clone(), (input_tx, trans_inputs.clone()));
        outputs.insert(name.clone(), control);
        tasks.insert(name.clone(), task);
    }

    // Build sinks
    for (name, sink) in config
        .sinks
        .iter()
        .filter(|(name, _)| diff.sinks.contains_new(&name))
    {
        let sink_inputs = &sink.inputs;
        let healthcheck = sink.healthcheck();
        let enable_healthcheck = healthcheck.enabled && config.healthchecks.enabled;

        let typetag = sink.inner.sink_type();
        let input_type = sink.inner.input_type();

        let (tx, rx, acker) = if let Some(buffer) = buffers.remove(name) {
            buffer
        } else {
            let buffer = sink.buffer.build(&config.global.data_dir, &name);
            match buffer {
                Err(error) => {
                    errors.push(format!("Sink \"{}\": {}", name, error));
                    continue;
                }
                Ok((tx, rx, acker)) => (tx, Arc::new(Mutex::new(Some(rx.into()))), acker),
            }
        };

        let cx = SinkContext {
            acker: acker.clone(),
            healthcheck,
            globals: config.global.clone(),
        };

        let (sink, healthcheck) = match sink.inner.build(cx).await {
            Err(error) => {
                errors.push(format!("Sink \"{}\": {}", name, error));
                continue;
            }
            Ok(built) => built,
        };

        let (trigger, tripwire) = Tripwire::new();

        let sink = async move {
            // Why is this Arc<Mutex<Option<_>>> needed you ask.
            // In case when this function build_pieces errors
            // this future won't be run so this rx won't be taken
            // which will enable us to reuse rx to rebuild
            // old configuration by passing this Arc<Mutex<Option<_>>>
            // yet again.
            let rx = rx
                .lock()
                .unwrap()
                .take()
                .expect("Task started but input has been taken.");

            let mut rx = Box::pin(crate::utilization::wrap(rx));

            sink.run(
                rx.by_ref()
                    .filter(|event| ready(filter_event_type(event, input_type)))
                    .inspect(|_| emit!(EventIn))
                    .take_until_if(tripwire),
            )
            .await
            .map(|_| {
                debug!("Finished.");
                TaskOutput::Sink(rx, acker)
            })
        };
        let task = Task::new(name, typetag, sink);

        let component_name = name.clone();
        let healthcheck_task = async move {
            if enable_healthcheck {
                let duration = Duration::from_secs(10);
                timeout(duration, healthcheck)
                    .map(|result| match result {
                        Ok(Ok(_)) => {
                            info!("Healthcheck: Passed.");
                            Ok(TaskOutput::Healthcheck)
                        }
                        Ok(Err(error)) => {
                            error!(
                                msg = "Healthcheck: Failed Reason.",
                                %error,
                                component_kind = "sink",
                                component_type = typetag,
                                ?component_name,
                            );
                            Err(())
                        }
                        Err(_) => {
                            error!(
                                msg = "Healthcheck: timeout.",
                                component_kind = "sink",
                                component_type = typetag,
                                ?component_name,
                            );
                            Err(())
                        }
                    })
                    .await
            } else {
                info!("Healthcheck: Disabled.");
                Ok(TaskOutput::Healthcheck)
            }
        };
        let healthcheck_task = Task::new(name, typetag, healthcheck_task);

        inputs.insert(name.clone(), (tx, sink_inputs.clone()));
        healthchecks.insert(name.clone(), healthcheck_task);
        tasks.insert(name.clone(), task);
        detach_triggers.insert(name.clone(), trigger);
    }

    if errors.is_empty() {
        let pieces = Pieces {
            inputs,
            outputs,
            tasks,
            source_tasks,
            healthchecks,
            shutdown_coordinator,
            detach_triggers,
        };

        Ok(pieces)
    } else {
        Err(errors)
    }
}

fn filter_event_type(event: &Event, data_type: DataType) -> bool {
    match data_type {
        DataType::Any => true,
        DataType::Log => matches!(event, Event::Log(_)),
        DataType::Metric => matches!(event, Event::Metric(_)),
    }
}
