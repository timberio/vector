//! Topology contains all topology based types.
//!
//! Topology is broken up into two main sections. The first
//! section contains all the main topology types include `Topology`
//! and the ability to start, stop and reload a config. The second
//! part contains config related items including config traits for
//! each type of component.

pub mod builder;
pub mod fanout;
mod task;

use crate::{
    buffers::{self, EventStream},
    config::{Config, ConfigDiff, HealthcheckOptions, Resource},
    event::Event,
    shutdown::SourceShutdownCoordinator,
    topology::{
        builder::Pieces,
        task::{Task, TaskOutput},
    },
    trigger::DisabledTrigger,
};
use futures::{future, Future, FutureExt, SinkExt};
use std::{
    collections::{HashMap, HashSet},
    panic::AssertUnwindSafe,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::{
    sync::{mpsc, watch},
    time::{interval, sleep_until, Duration, Instant},
};
use tracing_futures::Instrument;

type TaskHandle = tokio::task::JoinHandle<Result<TaskOutput, ()>>;

type BuiltBuffer = (
    buffers::BufferInputCloner<Event>,
    Arc<Mutex<Option<Pin<EventStream>>>>,
    buffers::Acker,
);

type Outputs = HashMap<String, fanout::ControlChannel>;

// Watcher types for topology changes. These are currently specific to receiving
// `Outputs`. This could be expanded in the future to send an enum of types if, for example,
// this included a new 'Inputs' type.
type WatchTx = watch::Sender<Outputs>;
pub type WatchRx = watch::Receiver<Outputs>;

#[allow(dead_code)]
pub struct RunningTopology {
    inputs: HashMap<String, buffers::BufferInputCloner<Event>>,
    outputs: HashMap<String, fanout::ControlChannel>,
    source_tasks: HashMap<String, TaskHandle>,
    tasks: HashMap<String, TaskHandle>,
    shutdown_coordinator: SourceShutdownCoordinator,
    detach_triggers: HashMap<String, DisabledTrigger>,
    config: Config,
    abort_tx: mpsc::UnboundedSender<()>,
    watch: (WatchTx, WatchRx),
}

pub async fn start_validated(
    config: Config,
    diff: ConfigDiff,
    mut pieces: Pieces,
) -> Option<(RunningTopology, mpsc::UnboundedReceiver<()>)> {
    let (abort_tx, abort_rx) = mpsc::unbounded_channel();

    let mut running_topology = RunningTopology {
        inputs: HashMap::new(),
        outputs: HashMap::new(),
        config,
        shutdown_coordinator: SourceShutdownCoordinator::default(),
        detach_triggers: HashMap::new(),
        source_tasks: HashMap::new(),
        tasks: HashMap::new(),
        abort_tx,
        watch: watch::channel(HashMap::new()),
    };

    if !running_topology
        .run_healthchecks(&diff, &mut pieces, running_topology.config.healthchecks)
        .await
    {
        return None;
    }
    running_topology.connect_diff(&diff, &mut pieces).await;
    running_topology.spawn_diff(&diff, pieces);

    Some((running_topology, abort_rx))
}

pub async fn build_or_log_errors(
    config: &Config,
    diff: &ConfigDiff,
    buffers: HashMap<String, BuiltBuffer>,
) -> Option<Pieces> {
    match builder::build_pieces(config, diff, buffers).await {
        Err(errors) => {
            for error in errors {
                error!(message = "Configuration error.", %error);
            }
            None
        }
        Ok(new_pieces) => Some(new_pieces),
    }
}

pub fn take_healthchecks(diff: &ConfigDiff, pieces: &mut Pieces) -> Vec<(String, Task)> {
    (&diff.sinks.to_change | &diff.sinks.to_add)
        .into_iter()
        .filter_map(|name| {
            pieces
                .healthchecks
                .remove(&name)
                .map(move |task| (name, task))
        })
        .collect()
}

impl RunningTopology {
    /// Returned future will finish once all current sources have finished.
    pub fn sources_finished(&self) -> future::BoxFuture<'static, ()> {
        self.shutdown_coordinator.shutdown_tripwire()
    }

    /// Sends the shutdown signal to all sources and returns a future that resolves
    /// once all components (sources, transforms, and sinks) have finished shutting down.
    /// Transforms and sinks should shut down automatically once their input tasks finish.
    /// Note that this takes ownership of `self`, so once this function returns everything in the
    /// RunningTopology instance has been dropped except for the `tasks` map, which gets moved
    /// into the returned future and is used to poll for when the tasks have completed. Once the
    /// returned future is dropped then everything from this RunningTopology instance is fully
    /// dropped.
    pub fn stop(self) -> impl Future<Output = ()> {
        // Create handy handles collections of all tasks for the subsequent operations.
        let mut wait_handles = Vec::new();
        // We need a Vec here since source components have two tasks. One for pump in self.tasks,
        // and the other for source in self.source_tasks.
        let mut check_handles = HashMap::<String, Vec<_>>::new();

        // We need to give some time to the sources to gracefully shutdown, so we will merge
        // them with other tasks.
        for (name, task) in self.tasks.into_iter().chain(self.source_tasks.into_iter()) {
            let task = task.map(|_result| ()).shared();

            wait_handles.push(task.clone());
            check_handles.entry(name).or_default().push(task);
        }

        // If we reach this, we will forcefully shutdown the sources.
        let deadline = Instant::now() + Duration::from_secs(60);

        // If we reach the deadline, this future will print out which components won't
        // gracefully shutdown since we will start to forcefully shutdown the sources.
        let mut check_handles2 = check_handles.clone();
        let timeout = async move {
            sleep_until(deadline).await;
            // Remove all tasks that have shutdown.
            check_handles2.retain(|_name, handles| {
                retain(handles, |handle| handle.peek().is_none());
                !handles.is_empty()
            });
            let remaining_components = check_handles2.keys().cloned().collect::<Vec<_>>();

            error!(
              message = "Failed to gracefully shut down in time. Killing components.",
                components = ?remaining_components.join(", ")
            );
        };

        // Reports in intervals which components are still running.
        let mut interval = interval(Duration::from_secs(5));
        let reporter = async move {
            loop {
                interval.tick().await;
                // Remove all tasks that have shutdown.
                check_handles.retain(|_name, handles| {
                    retain(handles, |handle| handle.peek().is_none());
                    !handles.is_empty()
                });
                let remaining_components = check_handles.keys().cloned().collect::<Vec<_>>();

                // TODO: replace with checked_duration_since once it's stable
                let time_remaining = if deadline > Instant::now() {
                    format!("{} seconds left", (deadline - Instant::now()).as_secs())
                } else {
                    "overdue".to_string()
                };

                info!(
                    message = "Shutting down... Waiting on running components.", remaining_components = ?remaining_components.join(", "), time_remaining = ?time_remaining
                );
            }
        };

        // Finishes once all tasks have shutdown.
        let success = futures::future::join_all(wait_handles).map(|_| ());

        // Aggregate future that ends once anything detects that all tasks have shutdown.
        let shutdown_complete_future = future::select_all(vec![
            Box::pin(timeout) as future::BoxFuture<'static, ()>,
            Box::pin(reporter) as future::BoxFuture<'static, ()>,
            Box::pin(success) as future::BoxFuture<'static, ()>,
        ]);

        // Now kick off the shutdown process by shutting down the sources.
        let source_shutdown_complete = self.shutdown_coordinator.shutdown_all(deadline);

        futures::future::join(source_shutdown_complete, shutdown_complete_future).map(|_| ())
    }

    /// On Error, topology is in invalid state.
    /// May change componenets even if reload fails.
    pub async fn reload_config_and_respawn(&mut self, new_config: Config) -> Result<bool, ()> {
        if self.config.global != new_config.global {
            error!(
                message =
                "Global options can't be changed while reloading config file; reload aborted. Please restart vector to reload the configuration file."
            );
            return Ok(false);
        }

        let diff = ConfigDiff::new(&self.config, &new_config);

        // Checks passed so let's shutdown the difference.
        let buffers = self.shutdown_diff(&diff, &new_config).await;

        // Gives windows some time to make available any port
        // released by shutdown componenets.
        // Issue: https://github.com/timberio/vector/issues/3035
        if cfg!(windows) {
            // This value is guess work.
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        // Now let's actually build the new pieces.
        if let Some(mut new_pieces) = build_or_log_errors(&new_config, &diff, buffers.clone()).await
        {
            if self
                .run_healthchecks(&diff, &mut new_pieces, new_config.healthchecks)
                .await
            {
                self.connect_diff(&diff, &mut new_pieces).await;
                self.spawn_diff(&diff, new_pieces);
                self.config = new_config;
                // We have successfully changed to new config.
                return Ok(true);
            }
        }

        // We need to rebuild the removed.
        info!("Rebuilding old configuration.");
        let diff = diff.flip();
        if let Some(mut new_pieces) = build_or_log_errors(&self.config, &diff, buffers).await {
            if self
                .run_healthchecks(&diff, &mut new_pieces, self.config.healthchecks)
                .await
            {
                self.connect_diff(&diff, &mut new_pieces).await;
                self.spawn_diff(&diff, new_pieces);
                // We have successfully returned to old config.
                return Ok(false);
            }
        }

        // We failed in rebuilding the old state.
        error!("Failed in rebuilding the old configuration.");

        Err(())
    }

    async fn run_healthchecks(
        &mut self,
        diff: &ConfigDiff,
        pieces: &mut Pieces,
        options: HealthcheckOptions,
    ) -> bool {
        if options.enabled {
            let healthchecks = take_healthchecks(diff, pieces)
                .into_iter()
                .map(|(_, task)| task);
            let healthchecks = future::try_join_all(healthchecks);

            info!("Running healthchecks.");
            if options.require_healthy {
                let success = healthchecks.await;

                if success.is_ok() {
                    info!("All healthchecks passed.");
                    true
                } else {
                    error!("Sinks unhealthy.");
                    false
                }
            } else {
                tokio::spawn(healthchecks);
                true
            }
        } else {
            true
        }
    }

    /// Shutdowns removed and replaced pieces of topology.
    /// Returns buffers to be reused.
    async fn shutdown_diff(
        &mut self,
        diff: &ConfigDiff,
        new_config: &Config,
    ) -> HashMap<String, BuiltBuffer> {
        // Sources
        let timeout = Duration::from_secs(30); //sec

        // First pass to tell the sources to shut down.
        let mut source_shutdown_complete_futures = Vec::new();

        // Only log that we are waiting for shutdown if we are actually removing
        // sources.
        if !diff.sources.to_remove.is_empty() {
            info!(
                message = "Waiting for sources to finish shutting down.", timeout = ?timeout.as_secs()
            );
        }

        let deadline = Instant::now() + timeout;
        for name in &diff.sources.to_remove {
            info!(message = "Removing source.", name = ?name);

            let previous = self.tasks.remove(name).unwrap();
            drop(previous); // detach and forget

            self.remove_outputs(name);
            source_shutdown_complete_futures
                .push(self.shutdown_coordinator.shutdown_source(name, deadline));
        }
        for name in &diff.sources.to_change {
            self.remove_outputs(name);
            source_shutdown_complete_futures
                .push(self.shutdown_coordinator.shutdown_source(name, deadline));
        }

        // Wait for the shutdowns to complete

        // Only log message if there are actual futures to check.
        if !source_shutdown_complete_futures.is_empty() {
            info!(
                "Waiting for up to {} seconds for sources to finish shutting down.",
                timeout.as_secs()
            );
        }

        futures::future::join_all(source_shutdown_complete_futures).await;

        // Second pass now that all sources have shut down for final cleanup.
        for name in diff.sources.removed_and_changed() {
            if let Some(task) = self.source_tasks.remove(name) {
                task.await.unwrap().unwrap();
            }
        }

        // Transforms
        for name in &diff.transforms.to_remove {
            info!(message = "Removing transform.", name = ?name);

            let previous = self.tasks.remove(name).unwrap();
            drop(previous); // detach and forget

            self.remove_inputs(&name).await;
            self.remove_outputs(&name);
        }

        // Sinks

        // Resource conflicts
        // At this point both the old and the new config don't have
        // conflicts in their resource usage. So if we combine their
        // resources, all found conflicts are between
        // to be removed and to be added components.
        let remove_sink = diff
            .sinks
            .removed_and_changed()
            .map(|name| (name, self.config.sinks[name].resources(name)));
        let add_source = diff
            .sources
            .changed_and_added()
            .map(|name| (name, new_config.sources[name].inner.resources()));
        let add_sink = diff
            .sinks
            .changed_and_added()
            .map(|name| (name, new_config.sinks[name].resources(name)));
        let conflicts = Resource::conflicts(
            remove_sink.map(|(key, value)| ((true, key), value)).chain(
                add_sink
                    .chain(add_source)
                    .map(|(key, value)| ((false, key), value)),
            ),
        )
        .into_iter()
        .flat_map(|(_, components)| components)
        .collect::<HashSet<_>>();
        // Existing conflicting sinks
        let conflicting_sinks = conflicts
            .into_iter()
            .filter(|&(existing_sink, _)| existing_sink)
            .map(|(_, name)| name.clone());

        // Buffer reuse
        // We can reuse buffers whose configuration wasn't changed.
        let reuse_buffers = diff
            .sinks
            .to_change
            .iter()
            .filter(|&name| self.config.sinks[name].buffer == new_config.sinks[name].buffer)
            .cloned()
            .collect::<HashSet<_>>();

        let wait_for_sinks = conflicting_sinks
            .chain(reuse_buffers.iter().cloned())
            .collect::<HashSet<_>>();

        // First pass

        // Detach removed sinks
        for name in &diff.sinks.to_remove {
            info!(message = "Removing sink.", name = ?name);
            self.remove_inputs(&name).await;
        }

        // Detach changed sinks
        for name in &diff.sinks.to_change {
            if reuse_buffers.contains(name) {
                self.detach_triggers
                    .remove(name)
                    .unwrap()
                    .into_inner()
                    .cancel();
            } else if wait_for_sinks.contains(name) {
                self.detach_inputs(name).await;
            }
        }

        // Second pass for final cleanup

        // Cleanup removed
        for name in &diff.sinks.to_remove {
            let previous = self.tasks.remove(name).unwrap();
            if wait_for_sinks.contains(name) {
                debug!(message = "Waiting for sink to shutdown.", %name);
                previous.await.unwrap().unwrap();
            } else {
                drop(previous); // detach and forget
            }
        }

        // Cleanup changed and collect buffers to be reused
        let mut buffers = HashMap::new();
        for name in &diff.sinks.to_change {
            if wait_for_sinks.contains(name) {
                let previous = self.tasks.remove(name).unwrap();
                debug!(message = "Waiting for sink to shutdown.", %name);
                let buffer = previous.await.unwrap().unwrap();

                if reuse_buffers.contains(name) {
                    let tx = self.inputs.remove(name).unwrap();
                    let (rx, acker) = match buffer {
                        TaskOutput::Sink(rx, acker) => (rx, acker),
                        _ => unreachable!(),
                    };

                    buffers.insert(name.clone(), (tx, Arc::new(Mutex::new(Some(rx))), acker));
                }
            }
        }

        buffers
    }

    /// Rewires topology
    async fn connect_diff(&mut self, diff: &ConfigDiff, new_pieces: &mut Pieces) {
        // Sources
        for name in diff.sources.changed_and_added() {
            self.setup_outputs(&name, new_pieces).await;
        }

        // Transforms
        // Make sure all transform outputs are set up before another transform might try use
        // it as an input
        for name in diff.transforms.changed_and_added() {
            self.setup_outputs(&name, new_pieces).await;
        }

        for name in &diff.transforms.to_change {
            self.replace_inputs(&name, new_pieces).await;
        }

        for name in &diff.transforms.to_add {
            self.setup_inputs(&name, new_pieces).await;
        }

        // Sinks
        for name in &diff.sinks.to_change {
            self.replace_inputs(&name, new_pieces).await;
        }

        for name in &diff.sinks.to_add {
            self.setup_inputs(&name, new_pieces).await;
        }

        // Broadcast changes to subscribers.
        if !self.watch.0.is_closed() {
            self.watch
                .0
                .send(self.outputs.clone())
                .expect("Couldn't broadcast config changes.");
        }
    }

    /// Starts new and changed pieces of topology.
    fn spawn_diff(&mut self, diff: &ConfigDiff, mut new_pieces: Pieces) {
        // Sources
        for name in &diff.sources.to_change {
            info!(message = "Rebuilding source.", name = ?name);
            self.spawn_source(name, &mut new_pieces);
        }

        for name in &diff.sources.to_add {
            info!(message = "Starting source.", name = ?name);
            self.spawn_source(&name, &mut new_pieces);
        }

        // Transforms
        for name in &diff.transforms.to_change {
            info!(message = "Rebuilding transform.", name = ?name);
            self.spawn_transform(&name, &mut new_pieces);
        }

        for name in &diff.transforms.to_add {
            info!(message = "Starting transform.", name = ?name);
            self.spawn_transform(&name, &mut new_pieces);
        }

        // Sinks
        for name in &diff.sinks.to_change {
            info!(message = "Rebuilding sink.", name = ?name);
            self.spawn_sink(&name, &mut new_pieces);
        }

        for name in &diff.sinks.to_add {
            info!(message = "Starting sink.", name = ?name);
            self.spawn_sink(&name, &mut new_pieces);
        }
    }

    fn spawn_sink(&mut self, name: &str, new_pieces: &mut builder::Pieces) {
        let task = new_pieces.tasks.remove(name).unwrap();
        let span = error_span!(
            "sink",
            component_kind = "sink",
            component_name = %task.name(),
            component_type = %task.typetag(),
        );
        let task = handle_errors(task, self.abort_tx.clone()).instrument(span);
        let spawned = tokio::spawn(task);
        if let Some(previous) = self.tasks.insert(name.to_string(), spawned) {
            drop(previous); // detach and forget
        }
    }

    fn spawn_transform(&mut self, name: &str, new_pieces: &mut builder::Pieces) {
        let task = new_pieces.tasks.remove(name).unwrap();
        let span = error_span!(
            "transform",
            component_kind = "transform",
            component_name = %task.name(),
            component_type = %task.typetag(),
        );
        let task = handle_errors(task, self.abort_tx.clone()).instrument(span);
        let spawned = tokio::spawn(task);
        if let Some(previous) = self.tasks.insert(name.to_string(), spawned) {
            drop(previous); // detach and forget
        }
    }

    fn spawn_source(&mut self, name: &str, new_pieces: &mut builder::Pieces) {
        let task = new_pieces.tasks.remove(name).unwrap();
        let span = error_span!(
            "source",
            component_kind = "source",
            component_name = %task.name(),
            component_type = %task.typetag(),
        );
        let task = handle_errors(task, self.abort_tx.clone()).instrument(span.clone());
        let spawned = tokio::spawn(task);
        if let Some(previous) = self.tasks.insert(name.to_string(), spawned) {
            drop(previous); // detach and forget
        }

        self.shutdown_coordinator
            .takeover_source(name, &mut new_pieces.shutdown_coordinator);

        let source_task = new_pieces.source_tasks.remove(name).unwrap();
        let source_task = handle_errors(source_task, self.abort_tx.clone()).instrument(span);
        self.source_tasks
            .insert(name.to_string(), tokio::spawn(source_task));
    }

    fn remove_outputs(&mut self, name: &str) {
        self.outputs.remove(name);
    }

    async fn remove_inputs(&mut self, name: &str) {
        self.inputs.remove(name);
        self.detach_triggers.remove(name);

        let sink_inputs = self.config.sinks.get(name).map(|s| &s.inputs);
        let trans_inputs = self.config.transforms.get(name).map(|t| &t.inputs);

        let inputs = sink_inputs.or(trans_inputs);

        if let Some(inputs) = inputs {
            for input in inputs {
                if let Some(output) = self.outputs.get_mut(input) {
                    // This can only fail if we are disconnected, which is a valid situation.
                    let _ = output
                        .send(fanout::ControlMessage::Remove(name.to_string()))
                        .await;
                }
            }
        }
    }

    async fn setup_outputs(&mut self, name: &str, new_pieces: &mut builder::Pieces) {
        let mut output = new_pieces.outputs.remove(name).unwrap();

        for (sink_name, sink) in &self.config.sinks {
            if sink.inputs.iter().any(|i| i == name) {
                // Sink may have been removed with the new config so it may not be present.
                if let Some(input) = self.inputs.get(sink_name) {
                    let _ = output
                        .send(fanout::ControlMessage::Add(sink_name.clone(), input.get()))
                        .await;
                }
            }
        }
        for (transform_name, transform) in &self.config.transforms {
            if transform.inputs.iter().any(|i| i == name) {
                // Transform may have been removed with the new config so it may not be present.
                if let Some(input) = self.inputs.get(transform_name) {
                    let _ = output
                        .send(fanout::ControlMessage::Add(
                            transform_name.clone(),
                            input.get(),
                        ))
                        .await;
                }
            }
        }

        self.outputs.insert(name.to_string(), output);
    }

    async fn setup_inputs(&mut self, name: &str, new_pieces: &mut builder::Pieces) {
        let (tx, inputs) = new_pieces.inputs.remove(name).unwrap();

        for input in inputs {
            // This can only fail if we are disconnected, which is a valid situation.
            let _ = self
                .outputs
                .get_mut(&input)
                .unwrap()
                .send(fanout::ControlMessage::Add(name.to_string(), tx.get()))
                .await;
        }

        self.inputs.insert(name.to_string(), tx);
        new_pieces.detach_triggers.remove(name).map(|trigger| {
            self.detach_triggers
                .insert(name.to_string(), trigger.into())
        });
    }

    async fn replace_inputs(&mut self, name: &str, new_pieces: &mut builder::Pieces) {
        let (tx, inputs) = new_pieces.inputs.remove(name).unwrap();

        let sink_inputs = self.config.sinks.get(name).map(|s| &s.inputs);
        let trans_inputs = self.config.transforms.get(name).map(|t| &t.inputs);
        let old_inputs = sink_inputs
            .or(trans_inputs)
            .unwrap()
            .iter()
            .collect::<HashSet<_>>();

        let new_inputs = inputs.iter().collect::<HashSet<_>>();

        let inputs_to_remove = &old_inputs - &new_inputs;
        let inputs_to_add = &new_inputs - &old_inputs;
        let inputs_to_replace = old_inputs.intersection(&new_inputs);

        for input in inputs_to_remove {
            if let Some(output) = self.outputs.get_mut(input) {
                // This can only fail if we are disconnected, which is a valid situation.
                let _ = output
                    .send(fanout::ControlMessage::Remove(name.to_string()))
                    .await;
            }
        }

        for input in inputs_to_add {
            // This can only fail if we are disconnected, which is a valid situation.
            let _ = self
                .outputs
                .get_mut(input)
                .unwrap()
                .send(fanout::ControlMessage::Add(name.to_string(), tx.get()))
                .await;
        }

        for &input in inputs_to_replace {
            // This can only fail if we are disconnected, which is a valid situation.
            let _ = self
                .outputs
                .get_mut(input)
                .unwrap()
                .send(fanout::ControlMessage::Replace(
                    name.to_string(),
                    Some(tx.get()),
                ))
                .await;
        }

        self.inputs.insert(name.to_string(), tx);
        new_pieces.detach_triggers.remove(name).map(|trigger| {
            self.detach_triggers
                .insert(name.to_string(), trigger.into())
        });
    }

    async fn detach_inputs(&mut self, name: &str) {
        self.inputs.remove(name);
        self.detach_triggers.remove(name);

        let sink_inputs = self.config.sinks.get(name).map(|s| &s.inputs);
        let trans_inputs = self.config.transforms.get(name).map(|t| &t.inputs);
        let old_inputs = sink_inputs.or(trans_inputs).unwrap();

        for input in old_inputs {
            // This can only fail if we are disconnected, which is a valid situation.
            let _ = self
                .outputs
                .get_mut(input)
                .unwrap()
                .send(fanout::ControlMessage::Replace(name.to_string(), None))
                .await;
        }
    }

    /// Borrows the Config
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Subscribe to topology changes. This will receive an `Outputs` currently, but may be
    /// expanded in the future to accommodate `Inputs`. This is used by the 'tap' API to observe
    /// config changes, and re-wire tap sinks.
    pub fn watch(&self) -> watch::Receiver<Outputs> {
        self.watch.1.clone()
    }
}

async fn handle_errors(
    task: impl Future<Output = Result<TaskOutput, ()>>,
    abort_tx: mpsc::UnboundedSender<()>,
) -> Result<TaskOutput, ()> {
    AssertUnwindSafe(task)
        .catch_unwind()
        .await
        .map_err(|_| ())
        .and_then(|res| res)
        .map_err(|_| {
            error!("An error occurred that vector couldn't handle.");
            let _ = abort_tx.send(());
        })
}

/// If the closure returns false, then the element is removed
fn retain<T>(vec: &mut Vec<T>, mut retain_filter: impl FnMut(&mut T) -> bool) {
    let mut i = 0;
    while let Some(data) = vec.get_mut(i) {
        if retain_filter(data) {
            i += 1;
        } else {
            let _ = vec.remove(i);
        }
    }
}

#[cfg(all(test, feature = "sinks-console", feature = "sources-socket"))]
mod tests {
    use crate::{
        config::Config,
        sinks::console::{ConsoleSinkConfig, Encoding, Target},
        sources::socket::SocketConfig,
        test_util::{next_addr, start_topology},
    };
    use std::path::Path;

    #[tokio::test]
    async fn topology_doesnt_reload_new_data_dir() {
        let mut old_config = Config::builder();
        old_config.add_source(
            "in",
            SocketConfig::make_basic_tcp_config(next_addr()),
            None,
            None,
        );
        old_config.add_sink(
            "out",
            &[&"in"],
            ConsoleSinkConfig {
                target: Target::Stdout,
                encoding: Encoding::Text.into(),
            },
        );
        old_config.global.data_dir = Some(Path::new("/asdf").to_path_buf());
        let mut new_config = old_config.clone();

        let (mut topology, _crash) = start_topology(old_config.build().unwrap(), false).await;

        new_config.global.data_dir = Some(Path::new("/qwerty").to_path_buf());

        topology
            .reload_config_and_respawn(new_config.build().unwrap())
            .await
            .unwrap();

        assert_eq!(
            topology.config.global.data_dir,
            Some(Path::new("/asdf").to_path_buf())
        );
    }
}

#[cfg(all(
    test,
    feature = "sinks-console",
    feature = "sources-splunk_hec",
    feature = "sources-generator",
    feature = "sinks-prometheus",
    feature = "transforms-log_to_metric",
    feature = "sinks-socket",
    feature = "leveldb"
))]
mod reload_tests {
    use crate::buffers::{BufferConfig, WhenFull};
    use crate::config::Config;
    use crate::sinks::console::{ConsoleSinkConfig, Encoding, Target};
    use crate::sinks::prometheus::exporter::PrometheusExporterConfig;
    use crate::sources::generator::GeneratorConfig;
    use crate::sources::splunk_hec::SplunkConfig;
    use crate::test_util::{next_addr, start_topology, temp_dir, wait_for_tcp};
    use crate::transforms::log_to_metric::{GaugeConfig, LogToMetricConfig, MetricConfig};
    use futures::StreamExt;
    use std::net::{SocketAddr, TcpListener};
    use std::time::Duration;
    use tokio::time::sleep;
    use tokio_stream::wrappers::UnboundedReceiverStream;

    #[tokio::test]
    async fn topology_reuse_old_port() {
        let address = next_addr();

        let mut old_config = Config::builder();
        old_config.add_source("in1", SplunkConfig::on(address));
        old_config.add_sink(
            "out",
            &[&"in1"],
            ConsoleSinkConfig {
                target: Target::Stdout,
                encoding: Encoding::Text.into(),
            },
        );

        let mut new_config = Config::builder();
        new_config.add_source("in2", SplunkConfig::on(address));
        new_config.add_sink(
            "out",
            &[&"in2"],
            ConsoleSinkConfig {
                target: Target::Stdout,
                encoding: Encoding::Text.into(),
            },
        );

        let (mut topology, _crash) = start_topology(old_config.build().unwrap(), false).await;
        assert!(topology
            .reload_config_and_respawn(new_config.build().unwrap())
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn topology_rebuild_old() {
        let address_0 = next_addr();
        let address_1 = next_addr();

        let mut old_config = Config::builder();
        old_config.add_source("in1", SplunkConfig::on(address_0));
        old_config.add_sink(
            "out",
            &[&"in1"],
            ConsoleSinkConfig {
                target: Target::Stdout,
                encoding: Encoding::Text.into(),
            },
        );

        let mut new_config = Config::builder();
        new_config.add_source("in1", SplunkConfig::on(address_1));
        new_config.add_sink(
            "out",
            &[&"in1"],
            ConsoleSinkConfig {
                target: Target::Stdout,
                encoding: Encoding::Text.into(),
            },
        );

        // Will cause the new_config to fail on build
        let _bind = TcpListener::bind(address_1).unwrap();

        let (mut topology, _crash) = start_topology(old_config.build().unwrap(), false).await;
        assert!(!topology
            .reload_config_and_respawn(new_config.build().unwrap())
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn topology_old() {
        let address = next_addr();

        let mut old_config = Config::builder();
        old_config.add_source("in1", SplunkConfig::on(address));
        old_config.add_sink(
            "out",
            &[&"in1"],
            ConsoleSinkConfig {
                target: Target::Stdout,
                encoding: Encoding::Text.into(),
            },
        );

        let (mut topology, _crash) =
            start_topology(old_config.clone().build().unwrap(), false).await;
        assert!(topology
            .reload_config_and_respawn(old_config.build().unwrap())
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn topology_reuse_old_port_sink() {
        let address = next_addr();

        let source = GeneratorConfig::repeat(vec!["msg".to_string()], usize::MAX, Some(0.001));
        let transform = LogToMetricConfig {
            metrics: vec![MetricConfig::Gauge(GaugeConfig {
                field: "message".to_string(),
                name: None,
                namespace: None,
                tags: None,
            })],
        };

        let mut old_config = Config::builder();
        old_config.add_source("in", source.clone());
        old_config.add_transform("trans", &[&"in"], transform.clone());
        old_config.add_sink(
            "out1",
            &[&"trans"],
            PrometheusExporterConfig {
                address,
                flush_period_secs: 1,
                ..PrometheusExporterConfig::default()
            },
        );

        let mut new_config = Config::builder();
        new_config.add_source("in", source.clone());
        new_config.add_transform("trans", &[&"in"], transform.clone());
        new_config.add_sink(
            "out1",
            &[&"trans"],
            PrometheusExporterConfig {
                address,
                flush_period_secs: 2,
                ..PrometheusExporterConfig::default()
            },
        );

        reload_sink_test(
            old_config.build().unwrap(),
            new_config.build().unwrap(),
            address,
            address,
        )
        .await;
    }

    #[tokio::test]
    async fn topology_reuse_old_port_cross_dependecy() {
        // Reload with source that uses address of changed sink.
        let address_0 = next_addr();
        let address_1 = next_addr();

        let transform = LogToMetricConfig {
            metrics: vec![MetricConfig::Gauge(GaugeConfig {
                field: "message".to_string(),
                name: None,
                namespace: None,
                tags: None,
            })],
        };

        let mut old_config = Config::builder();
        old_config.add_source(
            "in",
            GeneratorConfig::repeat(vec!["msg".to_string()], usize::MAX, Some(0.001)),
        );
        old_config.add_transform("trans", &[&"in"], transform.clone());
        old_config.add_sink(
            "out1",
            &[&"trans"],
            PrometheusExporterConfig {
                address: address_0,
                flush_period_secs: 1,
                ..PrometheusExporterConfig::default()
            },
        );

        let mut new_config = Config::builder();
        new_config.add_source("in", SplunkConfig::on(address_0));
        new_config.add_transform("trans", &[&"in"], transform.clone());
        new_config.add_sink(
            "out1",
            &[&"trans"],
            PrometheusExporterConfig {
                address: address_1,
                flush_period_secs: 1,
                ..PrometheusExporterConfig::default()
            },
        );

        reload_sink_test(
            old_config.build().unwrap(),
            new_config.build().unwrap(),
            address_0,
            address_1,
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn topology_disk_buffer_conflict() {
        let address_0 = next_addr();
        let address_1 = next_addr();
        let data_dir = temp_dir();
        std::fs::create_dir(&data_dir).unwrap();

        let mut old_config = Config::builder();
        old_config.global.data_dir = Some(data_dir);
        old_config.add_source(
            "in",
            GeneratorConfig::repeat(vec!["msg".to_string()], usize::MAX, Some(0.001)),
        );
        old_config.add_transform(
            "trans",
            &[&"in"],
            LogToMetricConfig {
                metrics: vec![MetricConfig::Gauge(GaugeConfig {
                    field: "message".to_string(),
                    name: None,
                    namespace: None,
                    tags: None,
                })],
            },
        );
        old_config.add_sink(
            "out",
            &[&"trans"],
            PrometheusExporterConfig {
                address: address_0,
                flush_period_secs: 1,
                ..PrometheusExporterConfig::default()
            },
        );
        old_config.sinks["out"].buffer = BufferConfig::Disk {
            max_size: 1024,
            when_full: WhenFull::Block,
        };

        let mut new_config = old_config.clone();
        new_config.sinks["out"].inner = Box::new(PrometheusExporterConfig {
            address: address_1,
            flush_period_secs: 1,
            ..PrometheusExporterConfig::default()
        });
        new_config.sinks["out"].buffer = BufferConfig::Disk {
            max_size: 2048,
            when_full: WhenFull::Block,
        };

        reload_sink_test(
            old_config.build().unwrap(),
            new_config.build().unwrap(),
            address_0,
            address_1,
        )
        .await;
    }
    async fn reload_sink_test(
        old_config: Config,
        new_config: Config,
        old_address: SocketAddr,
        new_address: SocketAddr,
    ) {
        let (mut topology, crash) = start_topology(old_config, false).await;
        let mut crash_stream = UnboundedReceiverStream::new(crash);

        // Wait for sink to come online
        wait_for_tcp(old_address).await;

        // Give topology some time to run
        sleep(Duration::from_secs(1)).await;

        assert!(topology
            .reload_config_and_respawn(new_config)
            .await
            .unwrap());

        // Give old time to shutdown if it didn't, and new one to come online.
        sleep(Duration::from_secs(2)).await;

        tokio::select! {
            _ = wait_for_tcp(new_address) => {}//Success
            _ = crash_stream.next() => panic!(),
        }
    }
}

#[cfg(all(test, feature = "sinks-console", feature = "sources-generator"))]
mod source_finished_tests {
    use crate::{
        config::Config,
        sinks::console::{ConsoleSinkConfig, Encoding, Target},
        sources::generator::GeneratorConfig,
        test_util::start_topology,
    };
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    async fn sources_finished() {
        let mut old_config = Config::builder();
        let generator = GeneratorConfig::repeat(vec!["text".to_owned()], 1, None);
        old_config.add_source("in", generator, None, None);
        old_config.add_sink(
            "out",
            &[&"in"],
            ConsoleSinkConfig {
                target: Target::Stdout,
                encoding: Encoding::Text.into(),
            },
        );

        let (topology, _crash) = start_topology(old_config.build().unwrap(), false).await;

        timeout(Duration::from_secs(2), topology.sources_finished())
            .await
            .unwrap();
    }
}

#[cfg(all(
    test,
    feature = "sinks-blackhole",
    feature = "sources-stdin",
    feature = "transforms-json_parser"
))]
mod transient_state_tests {
    use crate::{
        config::{Config, DataType, SourceConfig, SourceContext},
        sinks::blackhole::BlackholeConfig,
        sources::stdin::StdinConfig,
        sources::Source,
        test_util::{start_topology, trace_init},
        transforms::json_parser::JsonParserConfig,
        Error,
    };
    use futures::{future, FutureExt};
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;
    use stream_cancel::{Trigger, Tripwire};
    use tokio::sync::Mutex;

    #[derive(Debug, Deserialize, Serialize)]
    pub struct MockSourceConfig {
        #[serde(skip)]
        tripwire: Arc<Mutex<Option<Tripwire>>>,
    }

    impl MockSourceConfig {
        pub fn new() -> (Trigger, Self) {
            let (trigger, tripwire) = Tripwire::new();
            (
                trigger,
                Self {
                    tripwire: Arc::new(Mutex::new(Some(tripwire))),
                },
            )
        }
    }

    #[async_trait::async_trait]
    #[typetag::serde(name = "mock")]
    impl SourceConfig for MockSourceConfig {
        async fn build(&self, cx: SourceContext) -> Result<Source, Error> {
            let tripwire = self.tripwire.lock().await;

            let out = cx.out;
            Ok(Box::pin(
                future::select(
                    cx.shutdown.map(|_| ()).boxed(),
                    tripwire
                        .clone()
                        .unwrap()
                        .then(crate::stream::tripwire_handler)
                        .boxed(),
                )
                .map(|_| std::mem::drop(out))
                .unit_error(),
            ))
        }

        fn output_type(&self) -> DataType {
            DataType::Log
        }

        fn source_type(&self) -> &'static str {
            "mock"
        }
    }

    #[tokio::test]
    async fn closed_source() {
        let mut old_config = Config::builder();
        let (trigger_old, source) = MockSourceConfig::new();
        old_config.add_source("in", source, None, None);
        old_config.add_transform(
            "trans",
            &["in"],
            JsonParserConfig {
                drop_field: true,
                ..JsonParserConfig::default()
            },
        );
        old_config.add_sink(
            "out1",
            &["trans"],
            BlackholeConfig {
                print_amount: 1000,
                rate: None,
            },
        );
        old_config.add_sink(
            "out2",
            &["trans"],
            BlackholeConfig {
                print_amount: 1000,
                rate: None,
            },
        );

        let mut new_config = Config::builder();
        let (_trigger_new, source) = MockSourceConfig::new();
        new_config.add_source("in", source, None, None);
        new_config.add_transform(
            "trans",
            &["in"],
            JsonParserConfig {
                drop_field: false,
                ..JsonParserConfig::default()
            },
        );
        new_config.add_sink(
            "out1",
            &["trans"],
            BlackholeConfig {
                print_amount: 1000,
                rate: None,
            },
        );

        let (mut topology, _crash) = start_topology(old_config.build().unwrap(), false).await;

        trigger_old.cancel();

        topology.sources_finished().await;

        assert!(topology
            .reload_config_and_respawn(new_config.build().unwrap())
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn remove_sink() {
        trace_init();

        let mut old_config = Config::builder();
        old_config.add_source("in", StdinConfig::default(), None, None);
        old_config.add_transform(
            "trans",
            &["in"],
            JsonParserConfig {
                drop_field: true,
                ..JsonParserConfig::default()
            },
        );
        old_config.add_sink(
            "out1",
            &["trans"],
            BlackholeConfig {
                print_amount: 1000,
                rate: None,
            },
        );
        old_config.add_sink(
            "out2",
            &["trans"],
            BlackholeConfig {
                print_amount: 1000,
                rate: None,
            },
        );

        let mut new_config = Config::builder();
        new_config.add_source("in", StdinConfig::default(), None, None);
        new_config.add_transform(
            "trans",
            &["in"],
            JsonParserConfig {
                drop_field: false,
                ..JsonParserConfig::default()
            },
        );
        new_config.add_sink(
            "out1",
            &["trans"],
            BlackholeConfig {
                print_amount: 1000,
                rate: None,
            },
        );

        let (mut topology, _crash) = start_topology(old_config.build().unwrap(), false).await;
        assert!(topology
            .reload_config_and_respawn(new_config.build().unwrap())
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn remove_transform() {
        trace_init();

        let mut old_config = Config::builder();
        old_config.add_source("in", StdinConfig::default(), None, None);
        old_config.add_transform(
            "trans1",
            &["in"],
            JsonParserConfig {
                drop_field: true,
                ..JsonParserConfig::default()
            },
        );
        old_config.add_transform(
            "trans2",
            &["trans1"],
            JsonParserConfig {
                drop_field: true,
                ..JsonParserConfig::default()
            },
        );
        old_config.add_sink(
            "out1",
            &["trans1"],
            BlackholeConfig {
                print_amount: 1000,
                rate: None,
            },
        );
        old_config.add_sink(
            "out2",
            &["trans2"],
            BlackholeConfig {
                print_amount: 1000,
                rate: None,
            },
        );

        let mut new_config = Config::builder();
        new_config.add_source("in", StdinConfig::default(), None, None);
        new_config.add_transform(
            "trans1",
            &["in"],
            JsonParserConfig {
                drop_field: false,
                ..JsonParserConfig::default()
            },
        );
        new_config.add_sink(
            "out1",
            &["trans1"],
            BlackholeConfig {
                print_amount: 1000,
                rate: None,
            },
        );

        let (mut topology, _crash) = start_topology(old_config.build().unwrap(), false).await;
        assert!(topology
            .reload_config_and_respawn(new_config.build().unwrap())
            .await
            .unwrap());
    }
}
