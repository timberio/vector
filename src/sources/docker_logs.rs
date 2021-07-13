use super::util::MultilineConfig;
use crate::{
    config::{log_schema, DataType, SourceConfig, SourceContext, SourceDescription},
    docker::{docker, DockerTlsConfig},
    event::merge_state::LogEventMergeState,
    event::{self, Event, LogEvent, PathComponent, PathIter, Value},
    internal_events::{
        DockerLogsCommunicationError, DockerLogsContainerEventReceived,
        DockerLogsContainerMetadataFetchFailed, DockerLogsContainerUnwatch,
        DockerLogsContainerWatch, DockerLogsEventReceived, DockerLogsLoggingDriverUnsupported,
        DockerLogsTimestampParseFailed,
    },
    line_agg::{self, LineAgg},
    shutdown::ShutdownSignal,
    Pipeline,
};
use bollard::{
    container::{InspectContainerOptions, ListContainersOptions, LogOutput, LogsOptions},
    errors::Error as DockerError,
    service::{ContainerInspectResponse, SystemEventsResponse},
    system::EventsOptions,
    Docker,
};
use bytes::{Buf, Bytes};
use chrono::{DateTime, FixedOffset, Local, ParseError, Utc};
use futures::{Stream, StreamExt};
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::{
    future::ready,
    pin::Pin,
    sync::Arc,
    time::Duration,
    {collections::HashMap, convert::TryFrom},
};

use tokio::sync::mpsc;

const IMAGE: &str = "image";
const CREATED_AT: &str = "container_created_at";
const NAME: &str = "container_name";
const STREAM: &str = "stream";
const CONTAINER: &str = "container_id";
// Prevent short hostname from being wrongly regconized as a container's short ID.
const MIN_HOSTNAME_LENGTH: usize = 6;

lazy_static! {
    static ref STDERR: Bytes = "stderr".into();
    static ref STDOUT: Bytes = "stdout".into();
    static ref CONSOLE: Bytes = "console".into();
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields, default)]
pub struct DockerLogsConfig {
    #[serde(default = "host_key")]
    host_key: String,
    docker_host: Option<String>,
    tls: Option<DockerTlsConfig>,
    exclude_containers: Option<Vec<String>>, // Starts with actually, not exclude
    include_containers: Option<Vec<String>>, // Starts with actually, not include
    include_labels: Option<Vec<String>>,
    include_images: Option<Vec<String>>,
    partial_event_marker_field: Option<String>,
    auto_partial_merge: bool,
    multiline: Option<MultilineConfig>,
    retry_backoff_secs: u64,
}

impl Default for DockerLogsConfig {
    fn default() -> Self {
        Self {
            host_key: host_key(),
            docker_host: None,
            tls: None,
            exclude_containers: None,
            include_containers: None,
            include_labels: None,
            include_images: None,
            partial_event_marker_field: Some(event::PARTIAL.to_string()),
            auto_partial_merge: true,
            multiline: None,
            retry_backoff_secs: 2,
        }
    }
}

fn host_key() -> String {
    log_schema().host_key().to_string()
}

impl DockerLogsConfig {
    fn container_name_or_id_included<'a>(
        &self,
        id: &str,
        names: impl IntoIterator<Item = &'a str>,
    ) -> bool {
        let containers: Vec<String> = names.into_iter().map(Into::into).collect();

        self.include_containers
            .as_ref()
            .map(|include_list| Self::name_or_id_matches(id, &containers, include_list))
            .unwrap_or(true)
            && !(self
                .exclude_containers
                .as_ref()
                .map(|exclude_list| Self::name_or_id_matches(id, &containers, exclude_list))
                .unwrap_or(false))
    }

    fn name_or_id_matches(id: &str, names: &[String], items: &[String]) -> bool {
        items.iter().any(|flag| id.starts_with(flag))
            || names
                .iter()
                .any(|name| items.iter().any(|item| name.starts_with(item)))
    }

    fn with_empty_partial_event_marker_field_as_none(mut self) -> Self {
        if let Some(val) = &self.partial_event_marker_field {
            if val.is_empty() {
                self.partial_event_marker_field = None;
            }
        }
        self
    }
}

inventory::submit! {
    SourceDescription::new::<DockerLogsConfig>("docker_logs")
}

impl_generate_config_from_default!(DockerLogsConfig);

#[async_trait::async_trait]
#[typetag::serde(name = "docker_logs")]
impl SourceConfig for DockerLogsConfig {
    async fn build(&self, cx: SourceContext) -> crate::Result<super::Source> {
        let source = DockerLogsSource::new(
            self.clone().with_empty_partial_event_marker_field_as_none(),
            cx.out,
            cx.shutdown.clone(),
        )?;

        // Capture currently running containers, and do main future(run)
        let fut = async move {
            match source.handle_running_containers().await {
                Ok(source) => source.run().await,
                Err(error) => {
                    error!(
                        message = "Listing currently running containers failed.",
                        %error
                    );
                }
            }
        };

        let shutdown = cx.shutdown;
        // Once this ShutdownSignal resolves it will drop DockerLogsSource and by extension it's ShutdownSignal.
        Ok(Box::pin(async move {
            Ok(tokio::select! {
                _ = fut => {}
                _ = shutdown => {}
            })
        }))
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn source_type(&self) -> &'static str {
        "docker_logs"
    }
}

// Add a compatibility alias to avoid breaking existing configs
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DockerCompatConfig {
    #[serde(flatten)]
    config: DockerLogsConfig,
}

#[async_trait::async_trait]
#[typetag::serde(name = "docker")]
impl SourceConfig for DockerCompatConfig {
    async fn build(&self, cx: SourceContext) -> crate::Result<super::Source> {
        self.config.build(cx).await
    }

    fn output_type(&self) -> DataType {
        self.config.output_type()
    }

    fn source_type(&self) -> &'static str {
        "docker"
    }
}

struct DockerLogsSourceCore {
    config: DockerLogsConfig,
    line_agg_config: Option<line_agg::Config>,
    docker: Docker,
    /// Only logs created at, or after this moment are logged.
    now_timestamp: DateTime<Utc>,
}

impl DockerLogsSourceCore {
    fn new(config: DockerLogsConfig) -> crate::Result<Self> {
        // ?NOTE: Constructs a new Docker instance for a docker host listening at url specified by an env var DOCKER_HOST.
        // ?      Otherwise connects to unix socket which requires sudo privileges, or docker group membership.
        let docker = docker(config.docker_host.clone(), config.tls.clone())?;

        // Only log events created at-or-after this moment are logged.
        let now = Local::now();
        info!(
            message = "Capturing logs from now on.",
            now = %now.to_rfc3339()
        );

        let line_agg_config = if let Some(ref multiline_config) = config.multiline {
            Some(line_agg::Config::try_from(multiline_config)?)
        } else {
            None
        };

        Ok(DockerLogsSourceCore {
            config,
            line_agg_config,
            docker,
            now_timestamp: now.into(),
        })
    }

    /// Returns event stream coming from docker.
    fn docker_logs_event_stream(
        &self,
    ) -> impl Stream<Item = Result<SystemEventsResponse, DockerError>> + Send {
        let mut filters = HashMap::new();

        // event  | emitted on commands
        // -------+-------------------
        // start  | docker start, docker run, restart policy, docker restart
        // unpause | docker unpause
        // die    | docker restart, docker stop, docker kill, process exited, oom
        // pause  | docker pause
        filters.insert(
            "event".to_owned(),
            vec![
                "start".to_owned(),
                "unpause".to_owned(),
                "die".to_owned(),
                "pause".to_owned(),
            ],
        );
        filters.insert("type".to_owned(), vec!["container".to_owned()]);

        // Apply include filters
        if let Some(include_labels) = &self.config.include_labels {
            filters.insert("label".to_owned(), include_labels.clone());
        }

        if let Some(include_images) = &self.config.include_images {
            filters.insert("image".to_owned(), include_images.clone());
        }

        self.docker.events(Some(EventsOptions {
            since: Some(self.now_timestamp),
            until: None,
            filters,
        }))
    }
}

/// Main future which listens for events coming from docker, and maintains
/// a fan of event_stream futures.
/// Where each event_stream corresponds to a running container marked with ContainerLogInfo.
/// While running, event_stream streams Events to out channel.
/// Once a log stream has ended, it sends ContainerLogInfo back to main.
///
/// Future  channel     Future      channel
///           |<---- event_stream ---->out
/// main <----|<---- event_stream ---->out
///           | ...                 ...out
///
struct DockerLogsSource {
    esb: EventStreamBuilder,
    /// event stream from docker
    events: Pin<Box<dyn Stream<Item = Result<SystemEventsResponse, DockerError>> + Send>>,
    ///  mappings of seen container_id to their data
    containers: HashMap<ContainerId, ContainerState>,
    ///receives ContainerLogInfo coming from event stream futures
    main_recv: mpsc::UnboundedReceiver<Result<ContainerLogInfo, (ContainerId, Persistence)>>,
    /// It may contain shortened container id.
    hostname: Option<String>,
    backoff_duration: Duration,
}

impl DockerLogsSource {
    fn new(
        config: DockerLogsConfig,
        out: Pipeline,
        shutdown: ShutdownSignal,
    ) -> crate::Result<DockerLogsSource> {
        let backoff_secs = config.retry_backoff_secs;

        let host_key = config.host_key.clone();
        let hostname = crate::get_hostname().ok();

        // Only logs created at, or after this moment are logged.
        let core = DockerLogsSourceCore::new(config)?;

        // main event stream, with whom only newly started/restarted containers will be logged.
        let events = core.docker_logs_event_stream();
        info!(message = "Listening to docker log events.");

        // Channel of communication between main future and event_stream futures
        let (main_send, main_recv) =
            mpsc::unbounded_channel::<Result<ContainerLogInfo, (ContainerId, Persistence)>>();

        // Starting with logs from now.
        // TODO: Is this exception acceptable?
        // Only somewhat exception to this is case where:
        // t0 -- outside: container running
        // t1 -- now_timestamp
        // t2 -- outside: container stopped
        // t3 -- list_containers
        // In that case, logs between [t1,t2] will be pulled to vector only on next start/unpause of that container.
        let esb = EventStreamBuilder {
            host_key,
            hostname: hostname.clone(),
            core: Arc::new(core),
            out,
            main_send,
            shutdown,
        };

        Ok(DockerLogsSource {
            esb,
            events: Box::pin(events),
            containers: HashMap::new(),
            main_recv,
            hostname,
            backoff_duration: Duration::from_secs(backoff_secs),
        })
    }

    /// Future that captures currently running containers, and starts event streams for them.
    async fn handle_running_containers(mut self) -> crate::Result<Self> {
        let mut filters = HashMap::new();

        // Apply include filters
        if let Some(include_labels) = &self.esb.core.config.include_labels {
            filters.insert("label".to_owned(), include_labels.clone());
        }

        if let Some(include_images) = &self.esb.core.config.include_images {
            filters.insert("ancestor".to_owned(), include_images.clone());
        }

        self.esb
            .core
            .docker
            .list_containers(Some(ListContainersOptions {
                all: false, // only running containers
                filters,
                ..Default::default()
            }))
            .await?
            .into_iter()
            .for_each(|container| {
                let id = container.id.unwrap();
                let names = container.names.unwrap();

                trace!(message = "Found already running container.", id = %id, names = ?names);

                if self.exclude_self(id.as_str()) {
                    info!(message = "Excluded self container.", id = %id);
                    return;
                }

                if !self.esb.core.config.container_name_or_id_included(
                    id.as_str(),
                    names.iter().map(|s| {
                        // In this case bollard / shiplift gives names with starting '/' so it needs to be removed.
                        let s = s.as_str();
                        if s.starts_with('/') {
                            s.split_at('/'.len_utf8()).1
                        } else {
                            s
                        }
                    }),
                ) {
                    info!(message = "Excluded container.", id = %id);
                    return;
                }

                let id = ContainerId::new(id);
                self.containers.insert(id.clone(), self.esb.start(id, None));
            });

        Ok(self)
    }

    async fn run(mut self) {
        loop {
            tokio::select! {
                value = self.main_recv.recv() => {
                    match value {
                        Some(message) => {
                            match message {
                                Ok(info) => {
                                    let state = self
                                        .containers
                                        .get_mut(&info.id)
                                        .expect("Every ContainerLogInfo has it's ContainerState");
                                    if state.return_info(info) {
                                        self.esb.restart(state);
                                    }
                                },
                                Err((id,persistence)) => {
                                    let state = self
                                        .containers
                                        .remove(&id)
                                        .expect("Every started ContainerId has it's ContainerState");
                                    match persistence{
                                        Persistence::Transient => if state.is_running() {
                                            let backoff= Some(self.backoff_duration);
                                            self.containers.insert(id.clone(), self.esb.start(id, backoff));
                                        }
                                        // Forget the container since the error is permanent.
                                        Persistence::Permanent => (),
                                    }
                                }
                            }
                        }
                        None => {
                            error!(message = "The docker_logs source main stream has ended unexpectedly.");
                            info!(message = "Shutting down docker_logs source.");
                            return;
                        }
                    };
                }
                value = self.events.next() => {
                    match value {
                        Some(Ok(mut event)) => {
                            let action = event.action.unwrap();
                            let actor = event.actor.take().unwrap();
                            let id = actor.id.unwrap();
                            let attributes = actor.attributes.unwrap();

                            emit!(DockerLogsContainerEventReceived { container_id: &id, action: &action });

                            let id = ContainerId::new(id);

                            // Update container status
                            match action.as_str() {
                                "die" | "pause" => {
                                    if let Some(state) = self.containers.get_mut(&id) {
                                        state.stopped();
                                    }
                                }
                                "start" | "unpause" => {
                                    if let Some(state) = self.containers.get_mut(&id) {
                                        state.running();
                                        self.esb.restart(state);
                                    } else {
                                        let include_name =
                                            self.esb.core.config.container_name_or_id_included(
                                                id.as_str(),
                                                attributes.get("name").map(|s| s.as_str()),
                                            );

                                        let exclude_self = self.exclude_self(id.as_str());

                                        if include_name && !exclude_self {
                                            self.containers.insert(id.clone(), self.esb.start(id, None));
                                        }
                                    }
                                }
                                _ => {},
                            };
                        }
                        Some(Err(error)) => emit!(DockerLogsCommunicationError{error,container_id:None}),
                        None => {
                            // TODO: this could be fixed, but should be tried with some timeoff and exponential backoff
                            error!(message = "Docker log event stream has ended unexpectedly.");
                            info!(message = "Shutting down docker_logs source.");
                            return;
                        }
                    };
                }
            };
        }
    }

    fn exclude_self(&self, id: &str) -> bool {
        self.hostname
            .as_ref()
            .map(|hostname| id.starts_with(hostname) && hostname.len() >= MIN_HOSTNAME_LENGTH)
            .unwrap_or(false)
    }
}

/// Used to construct and start event stream futures
#[derive(Clone)]
struct EventStreamBuilder {
    host_key: String,
    hostname: Option<String>,
    core: Arc<DockerLogsSourceCore>,
    /// Event stream futures send events through this
    out: Pipeline,
    /// End through which event stream futures send ContainerLogInfo to main future
    main_send: mpsc::UnboundedSender<Result<ContainerLogInfo, (ContainerId, Persistence)>>,
    /// Self and event streams will end on this.
    shutdown: ShutdownSignal,
}

impl EventStreamBuilder {
    /// Spawn a task to runs event stream until shutdown.
    fn start(&self, id: ContainerId, backoff: Option<Duration>) -> ContainerState {
        let this = self.clone();
        tokio::spawn(async move {
            if let Some(duration) = backoff {
                tokio::time::sleep(duration).await;
            }
            match this
                .core
                .docker
                .inspect_container(id.as_str(), None::<InspectContainerOptions>)
                .await
            {
                Ok(details) => match ContainerMetadata::from_details(details) {
                    Ok(metadata) => {
                        let info = ContainerLogInfo::new(id, metadata, this.core.now_timestamp);
                        this.run_event_stream(info).await;
                        return;
                    }
                    Err(error) => emit!(DockerLogsTimestampParseFailed {
                        error,
                        container_id: id.as_str()
                    }),
                },
                Err(error) => emit!(DockerLogsContainerMetadataFetchFailed {
                    error,
                    container_id: id.as_str()
                }),
            }

            this.finish(Err((id, Persistence::Transient)));
        });

        ContainerState::new_running()
    }

    /// If info is present, restarts event stream which will run until shutdown.
    fn restart(&self, container: &mut ContainerState) {
        if let Some(info) = container.take_info() {
            let this = self.clone();
            tokio::spawn(async move { this.run_event_stream(info).await });
        }
    }

    async fn run_event_stream(self, mut info: ContainerLogInfo) {
        // Establish connection
        let options = Some(LogsOptions::<String> {
            follow: true,
            stdout: true,
            stderr: true,
            since: info.log_since(),
            timestamps: true,
            ..Default::default()
        });

        let stream = self.core.docker.logs(info.id.as_str(), options);
        emit!(DockerLogsContainerWatch {
            container_id: info.id.as_str()
        });

        // Create event streamer
        let mut partial_event_merge_state = None;

        let core = Arc::clone(&self.core);

        let mut error = None;
        let events_stream = stream
            .map(|value| {
                match value {
                    Ok(message) => Ok(info.new_event(
                        message,
                        core.config.partial_event_marker_field.clone(),
                        core.config.auto_partial_merge,
                        &mut partial_event_merge_state,
                    )),
                    Err(error) => {
                        // On any error, restart connection
                        match &error {
                            DockerError::DockerResponseServerError { status_code, .. }
                                if *status_code == http::StatusCode::NOT_IMPLEMENTED =>
                            {
                                emit!(DockerLogsLoggingDriverUnsupported {
                                    error,
                                    container_id: info.id.as_str(),
                                });
                                Err(Persistence::Permanent)
                            }
                            _ => {
                                emit!(DockerLogsCommunicationError {
                                    error,
                                    container_id: Some(info.id.as_str())
                                });
                                Err(Persistence::Transient)
                            }
                        }
                    }
                }
            })
            .take_while(|v| {
                error = v.as_ref().err().cloned();
                ready(v.is_ok())
            })
            .filter_map(|v| ready(v.unwrap()))
            .take_until(self.shutdown.clone());

        let events_stream: Box<dyn Stream<Item = Event> + Unpin + Send> =
            if let Some(ref line_agg_config) = core.line_agg_config {
                Box::new(line_agg_adapter(
                    events_stream,
                    line_agg::Logic::new(line_agg_config.clone()),
                ))
            } else {
                Box::new(events_stream)
            };

        let host_key = self.host_key.clone();
        let hostname = self.hostname.clone();
        let result = events_stream
            .map(move |event| add_hostname(event, &host_key, &hostname))
            .map(Ok)
            .forward(self.out.clone())
            .await;

        // End of stream
        emit!(DockerLogsContainerUnwatch {
            container_id: info.id.as_str()
        });

        let result = match (result, error) {
            (Ok(()), None) => Ok(info),
            (Err(crate::pipeline::ClosedError), _) => Err((info.id, Persistence::Permanent)),
            (_, Some(occurrence)) => Err((info.id, occurrence)),
        };

        self.finish(result);
    }

    fn finish(self, result: Result<ContainerLogInfo, (ContainerId, Persistence)>) {
        // This can legaly fail when shutting down, and any other
        // reason should have been logged in the main future.
        let _ = self.main_send.send(result);
    }
}

fn add_hostname(mut event: Event, host_key: &str, hostname: &Option<String>) -> Event {
    if let Some(hostname) = hostname {
        event.as_mut_log().insert(host_key, hostname.clone());
    }

    event
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum Persistence {
    Transient,
    Permanent,
}

/// Container ID as assigned by Docker.
/// Is actually a string.
#[derive(Hash, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct ContainerId(Bytes);

impl ContainerId {
    fn new(id: String) -> Self {
        ContainerId(id.into())
    }

    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("Container Id Bytes aren't String")
    }
}

/// Kept by main to keep track of container state
struct ContainerState {
    /// None if there is a event_stream of this container.
    info: Option<ContainerLogInfo>,
    /// True if Container is currently running
    running: bool,
    /// Of running
    generation: u64,
}

impl ContainerState {
    /// It's ContainerLogInfo pair must be created exactly once.
    fn new_running() -> Self {
        ContainerState {
            info: None,
            running: true,
            generation: 0,
        }
    }

    fn running(&mut self) {
        self.running = true;
        self.generation += 1;
    }

    fn stopped(&mut self) {
        self.running = false;
    }

    fn is_running(&self) -> bool {
        self.running
    }

    /// True if it needs to be restarted.
    #[must_use]
    fn return_info(&mut self, info: ContainerLogInfo) -> bool {
        debug_assert!(self.info.is_none());
        // Generation is the only one strictly necessary,
        // but with v.running, restarting event_stream is automatically done.
        let restart = self.running || info.generation < self.generation;
        self.info = Some(info);
        restart
    }

    fn take_info(&mut self) -> Option<ContainerLogInfo> {
        self.info.take().map(|mut info| {
            // Update info
            info.generation = self.generation;
            info
        })
    }
}

/// Exchanged between main future and event_stream futures
struct ContainerLogInfo {
    /// Container docker ID
    id: ContainerId,
    /// Timestamp of event which created this struct
    created: DateTime<Utc>,
    /// Timestamp of last log message with it's generation
    last_log: Option<(DateTime<FixedOffset>, u64)>,
    /// generation of ContainerState at event_stream creation
    generation: u64,
    metadata: ContainerMetadata,
}

impl ContainerLogInfo {
    /// Container docker ID
    /// Unix timestamp of event which created this struct
    fn new(id: ContainerId, metadata: ContainerMetadata, created: DateTime<Utc>) -> Self {
        ContainerLogInfo {
            id,
            created,
            last_log: None,
            generation: 0,
            metadata,
        }
    }

    /// Only logs after or equal to this point need to be fetched
    fn log_since(&self) -> i64 {
        self.last_log
            .as_ref()
            .map(|&(ref d, _)| d.timestamp())
            .unwrap_or_else(|| self.created.timestamp())
            - 1
    }

    /// Expects timestamp at the beggining of message.
    /// Expects messages to be ordered by timestamps.
    fn new_event(
        &mut self,
        log_output: LogOutput,
        partial_event_marker_field: Option<String>,
        auto_partial_merge: bool,
        partial_event_merge_state: &mut Option<LogEventMergeState>,
    ) -> Option<Event> {
        let (stream, mut bytes_message) = match log_output {
            LogOutput::StdErr { message } => (STDERR.clone(), message),
            LogOutput::StdOut { message } => (STDOUT.clone(), message),
            LogOutput::Console { message } => (CONSOLE.clone(), message),
            LogOutput::StdIn { message: _ } => return None,
        };

        let byte_size = bytes_message.len();
        let message = String::from_utf8_lossy(&bytes_message);
        let mut splitter = message.splitn(2, char::is_whitespace);
        let timestamp_str = splitter.next()?;
        let timestamp = match DateTime::parse_from_rfc3339(timestamp_str) {
            Ok(timestamp) => {
                // Timestamp check
                match self.last_log.as_ref() {
                    // Received log has not already been processed
                    Some(&(ref last, gen))
                        if *last < timestamp || (*last == timestamp && gen == self.generation) =>
                    {
                        // noop
                    }
                    // Received log is not from before of creation
                    None if self.created <= timestamp.with_timezone(&Utc) => (),
                    _ => {
                        trace!(
                            message = "Received older log.",
                            timestamp = %timestamp_str
                        );
                        return None;
                    }
                }

                self.last_log = Some((timestamp, self.generation));

                let log_len = splitter.next().map(|log| log.len()).unwrap_or(0);
                let remove_len = message.len() - log_len;
                bytes_message.advance(remove_len);

                // Provide the timestamp.
                Some(timestamp.with_timezone(&Utc))
            }
            Err(error) => {
                // Received bad timestamp, if any at all.
                emit!(DockerLogsTimestampParseFailed {
                    error,
                    container_id: self.id.as_str()
                });
                // So continue normally but without a timestamp.
                None
            }
        };

        // Message is actually one line from stderr or stdout, and they are
        // delimited with newline, so that newline needs to be removed.
        // If there's no newline, the event is considered partial, and will
        // either be merged within the docker source, or marked accordingly
        // before sending out, depending on the configuration.
        let is_partial = if bytes_message
            .last()
            .map(|&b| b as char == '\n')
            .unwrap_or(false)
        {
            bytes_message.truncate(bytes_message.len() - 1);
            if bytes_message
                .last()
                .map(|&b| b as char == '\r')
                .unwrap_or(false)
            {
                bytes_message.truncate(bytes_message.len() - 1);
            }
            false
        } else {
            true
        };

        // Prepare the log event.
        let mut log_event = {
            let mut log_event = LogEvent::default();

            // Source type
            log_event.insert(log_schema().source_type_key(), Bytes::from("docker"));

            // The log message.
            log_event.insert(log_schema().message_key(), bytes_message);

            // Stream we got the message from.
            log_event.insert(STREAM, stream);

            // Timestamp of the event.
            if let Some(timestamp) = timestamp {
                log_event.insert(log_schema().timestamp_key(), timestamp);
            }

            // Container ID.
            log_event.insert(CONTAINER, self.id.0.clone());

            // Labels.
            if !self.metadata.labels.is_empty() {
                let prefix_path = PathIter::new("label").collect::<Vec<_>>();
                for (key, value) in self.metadata.labels.iter() {
                    let mut path = prefix_path.clone();
                    path.push(PathComponent::Key(key.clone()));
                    log_event.insert_path(path, value.clone());
                }
            }

            // Container name.
            log_event.insert(NAME, self.metadata.name.clone());

            // Container image.
            log_event.insert(IMAGE, self.metadata.image.clone());

            // Timestamp of the container creation.
            log_event.insert(CREATED_AT, self.metadata.created_at);

            // Return the resulting log event.
            log_event
        };

        // If automatic partial event merging is requested - perform the
        // merging.
        // Otherwise mark partial events and return all the events with no
        // merging.
        let log_event = if auto_partial_merge {
            // Partial event events merging logic.

            // If event is partial, stash it and return `None`.
            if is_partial {
                // If we already have a partial event merge state, the current
                // message has to be merged into that existing state.
                // Otherwise, create a new partial event merge state with the
                // current message being the initial one.
                if let Some(partial_event_merge_state) = partial_event_merge_state {
                    partial_event_merge_state
                        .merge_in_next_event(log_event, &[log_schema().message_key().to_string()]);
                } else {
                    *partial_event_merge_state = Some(LogEventMergeState::new(log_event));
                };
                return None;
            };

            // This is not a partial event. If we have a partial event merge
            // state from before, the current event must be a final event, that
            // would give us a merged event we can return.
            // Otherwise it's just a regular event that we return as-is.
            match partial_event_merge_state.take() {
                Some(partial_event_merge_state) => partial_event_merge_state
                    .merge_in_final_event(log_event, &[log_schema().message_key().to_string()]),
                None => log_event,
            }
        } else {
            // If the event is partial, just set the partial event marker field.
            if is_partial {
                // Only add partial event marker field if it's requested.
                if let Some(partial_event_marker_field) = partial_event_marker_field {
                    log_event.insert(partial_event_marker_field, true);
                }
            }
            // Return the log event as is, partial or not. No merging here.
            log_event
        };

        // Partial or not partial - we return the event we got here, because all
        // other cases were handled earlier.
        let event = Event::Log(log_event);

        emit!(DockerLogsEventReceived {
            byte_size,
            container_id: self.id.as_str(),
            container_name: &self.metadata.name_str
        });

        Some(event)
    }
}

struct ContainerMetadata {
    /// label.key -> String
    labels: HashMap<String, String>,
    /// name -> String
    name: Value,
    /// name
    name_str: String,
    /// image -> String
    image: Value,
    /// created_at
    created_at: DateTime<Utc>,
}

impl ContainerMetadata {
    fn from_details(details: ContainerInspectResponse) -> Result<Self, ParseError> {
        let config = details.config.unwrap();
        let name = details.name.unwrap();
        let created = details.created.unwrap();

        let labels = config.labels.unwrap_or_default();

        Ok(ContainerMetadata {
            labels,
            name: name.as_str().trim_start_matches('/').to_owned().into(),
            name_str: name,
            image: config.image.unwrap().into(),
            created_at: DateTime::parse_from_rfc3339(created.as_str())?.with_timezone(&Utc),
        })
    }
}

fn line_agg_adapter(
    inner: impl Stream<Item = Event> + Unpin,
    logic: line_agg::Logic<Bytes, LogEvent>,
) -> impl Stream<Item = Event> {
    let line_agg_in = inner.map(|event| {
        let mut log_event = event.into_log();

        let message_value = log_event
            .remove(log_schema().message_key())
            .expect("message must exist in the event");
        let stream_value = log_event
            .get(&*STREAM)
            .expect("stream must exist in the event");

        let stream = stream_value.as_bytes();
        let message = message_value.into_bytes();
        (stream, message, log_event)
    });
    let line_agg_out = LineAgg::<_, Bytes, LogEvent>::new(line_agg_in, logic);
    line_agg_out.map(|(_, message, mut log_event)| {
        log_event.insert(log_schema().message_key(), message);
        Event::Log(log_event)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<DockerLogsConfig>();
    }

    #[test]
    fn exclude_self() {
        let (tx, _rx) = Pipeline::new_test();
        let mut source =
            DockerLogsSource::new(DockerLogsConfig::default(), tx, ShutdownSignal::noop()).unwrap();
        source.hostname = Some("451062c59603".to_owned());
        assert!(
            source.exclude_self("451062c59603a1cf0c6af3e74a31c0ae63d8275aa16a5fc78ef31b923baaffc3")
        );

        // hostname too short
        source.hostname = Some("a".to_owned());
        assert!(!source.exclude_self("a29d569bd46c"));
    }
}

#[cfg(all(test, feature = "docker-logs-integration-tests"))]
mod integration_tests {
    use super::*;
    use crate::{
        test_util::{collect_n, collect_ready, trace_init},
        Pipeline,
    };
    use bollard::{
        container::{
            Config as ContainerConfig, CreateContainerOptions, KillContainerOptions,
            RemoveContainerOptions, StartContainerOptions, WaitContainerOptions,
        },
        image::{CreateImageOptions, ListImagesOptions},
    };
    use futures::{channel::mpsc, stream::TryStreamExt, FutureExt};

    /// None if docker is not present on the system
    fn source_with<'a, L: Into<Option<&'a str>>>(
        names: &[&str],
        label: L,
    ) -> mpsc::Receiver<Event> {
        source_with_config(DockerLogsConfig {
            include_containers: Some(names.iter().map(|&s| s.to_owned()).collect()),
            include_labels: Some(label.into().map(|l| vec![l.to_owned()]).unwrap_or_default()),
            ..DockerLogsConfig::default()
        })
    }

    fn source_with_config(config: DockerLogsConfig) -> mpsc::Receiver<Event> {
        let (sender, recv) = Pipeline::new_test();
        tokio::spawn(async move {
            config
                .build(SourceContext::new_test(sender))
                .await
                .unwrap()
                .await
                .unwrap();
        });
        recv
    }

    /// Users should ensure to remove container before exiting.
    async fn log_container(
        name: &str,
        label: Option<&str>,
        log: &str,
        docker: &Docker,
        tty: bool,
    ) -> String {
        cmd_container(name, label, vec!["echo", log], docker, tty).await
    }

    /// Users should ensure to remove container before exiting.
    /// Will resend message every so often.
    async fn eternal_container(
        name: &str,
        label: Option<&str>,
        log: &str,
        docker: &Docker,
    ) -> String {
        cmd_container(
            name,
            label,
            vec![
                "sh",
                "-c",
                format!("echo before; i=0; while [ $i -le 50 ]; do sleep 0.1; echo {}; i=$((i+1)); done", log).as_str(),
            ],
            docker,
            false
        ).await
    }

    /// Users should ensure to remove container before exiting.
    async fn cmd_container(
        name: &str,
        label: Option<&str>,
        cmd: Vec<&str>,
        docker: &Docker,
        tty: bool,
    ) -> String {
        if let Some(id) = cmd_container_for_real(name, label, cmd, docker, tty).await {
            id
        } else {
            // Maybe a before created container is present
            info!(
                message = "Assumes that named container remained from previous tests.",
                name = name
            );
            name.to_owned()
        }
    }

    /// Users should ensure to remove container before exiting.
    async fn cmd_container_for_real(
        name: &str,
        label: Option<&str>,
        cmd: Vec<&str>,
        docker: &Docker,
        tty: bool,
    ) -> Option<String> {
        pull_busybox(docker).await;

        trace!("Creating container.");

        let options = Some(CreateContainerOptions { name });
        let config = ContainerConfig {
            image: Some("busybox"),
            cmd: Some(cmd),
            labels: label.map(|label| vec![(label, "")].into_iter().collect()),
            tty: Some(tty),
            ..Default::default()
        };

        let container = docker.create_container(options, config).await;
        container.ok().map(|c| c.id)
    }

    async fn pull_busybox(docker: &Docker) {
        let mut filters = HashMap::new();
        filters.insert("reference", vec!["busybox:latest"]);

        let options = Some(ListImagesOptions {
            filters,
            ..Default::default()
        });

        let images = docker.list_images(options).await.unwrap();
        if images.is_empty() {
            // If `busybox:latest` not found, pull it
            let options = Some(CreateImageOptions {
                from_image: "busybox",
                tag: "latest",
                ..Default::default()
            });

            docker
                .create_image(options, None, None)
                .for_each(|item| async move {
                    let info = item.unwrap();
                    if let Some(error) = info.error {
                        panic!("{:?}", error);
                    }
                })
                .await
        }
    }

    /// Returns once container has started
    async fn container_start(id: &str, docker: &Docker) -> Result<(), bollard::errors::Error> {
        trace!("Starting container.");

        let options = None::<StartContainerOptions<&str>>;
        docker.start_container(id, options).await
    }

    /// Returns once container is done running
    async fn container_wait(id: &str, docker: &Docker) -> Result<(), bollard::errors::Error> {
        trace!("Waiting for container.");

        docker
            .wait_container(id, None::<WaitContainerOptions<&str>>)
            .try_for_each(|exit| async move {
                info!(message = "Container exited with status code.", status_code = ?exit.status_code);
                Ok(())
            })
            .await
    }

    /// Returns once container is killed
    async fn container_kill(id: &str, docker: &Docker) -> Result<(), bollard::errors::Error> {
        trace!("Waiting for container to be killed.");

        docker
            .kill_container(id, None::<KillContainerOptions<&str>>)
            .await
    }

    /// Returns once container is done running
    async fn container_run(id: &str, docker: &Docker) -> Result<(), bollard::errors::Error> {
        container_start(id, docker).await?;
        container_wait(id, docker).await
    }

    async fn container_remove(id: &str, docker: &Docker) {
        trace!("Removing container.");

        // Don't panic, as this is unrelated to the test, and there are possibly other containers that need to be removed
        let _ = docker
            .remove_container(id, None::<RemoveContainerOptions>)
            .await
            .map_err(|e| error!(%e));
    }

    /// Returns once it's certain that log has been made
    /// Expects that this is the only one with a container
    async fn container_log_n(
        n: usize,
        name: &str,
        label: Option<&str>,
        log: &str,
        docker: &Docker,
    ) -> String {
        container_with_optional_tty_log_n(n, name, label, log, docker, false).await
    }
    async fn container_with_optional_tty_log_n(
        n: usize,
        name: &str,
        label: Option<&str>,
        log: &str,
        docker: &Docker,
        tty: bool,
    ) -> String {
        let id = log_container(name, label, log, docker, tty).await;
        for _ in 0..n {
            if let Err(error) = container_run(&id, docker).await {
                container_remove(&id, docker).await;
                panic!("Container failed to start with error: {:?}", error);
            }
        }
        id
    }

    /// Once function returns, the container has entered into running state.
    /// Container must be killed before removed.
    async fn running_container(
        name: &'static str,
        label: Option<&'static str>,
        log: &'static str,
        docker: &Docker,
    ) -> String {
        let out = source_with(&[name], None);
        let docker = docker.clone();

        let id = eternal_container(name, label, log, &docker).await;
        if let Err(error) = container_start(&id, &docker).await {
            container_remove(&id, &docker).await;
            panic!("Container start failed with error: {:?}", error);
        }

        // Wait for before message
        let events = collect_n(out, 1).await;
        assert_eq!(
            events[0].as_log()[log_schema().message_key()],
            "before".into()
        );

        id
    }

    fn is_empty<T>(mut rx: mpsc::Receiver<T>) -> bool {
        rx.next().now_or_never().is_none()
    }

    #[tokio::test]
    async fn container_with_tty() {
        trace_init();

        let message = "log container_with_tty";
        let name = "container_with_tty";

        let out = source_with(&[name], None);

        let docker = docker(None, None).unwrap();

        let id = container_with_optional_tty_log_n(1, name, None, message, &docker, true).await;
        let events = collect_n(out, 1).await;
        container_remove(&id, &docker).await;

        assert_eq!(
            events[0].as_log()[log_schema().message_key()],
            message.into()
        );
    }

    #[tokio::test]
    async fn newly_started() {
        trace_init();

        let message = "9";
        let name = "vector_test_newly_started";
        let label = "vector_test_label_newly_started";

        let out = source_with(&[name], None);

        let docker = docker(None, None).unwrap();

        let id = container_log_n(1, name, Some(label), message, &docker).await;
        let events = collect_n(out, 1).await;
        container_remove(&id, &docker).await;

        let log = events[0].as_log();
        assert_eq!(log[log_schema().message_key()], message.into());
        assert_eq!(log[&*super::CONTAINER], id.into());
        assert!(log.get(&*super::CREATED_AT).is_some());
        assert_eq!(log[&*super::IMAGE], "busybox".into());
        assert!(log.get(format!("label.{}", label)).is_some());
        assert_eq!(events[0].as_log()[&super::NAME], name.into());
        assert_eq!(
            events[0].as_log()[log_schema().source_type_key()],
            "docker".into()
        );
    }

    #[tokio::test]
    async fn restart() {
        trace_init();

        let message = "10";
        let name = "vector_test_restart";

        let out = source_with(&[name], None);

        let docker = docker(None, None).unwrap();

        let id = container_log_n(2, name, None, message, &docker).await;
        let events = collect_n(out, 2).await;
        container_remove(&id, &docker).await;

        assert_eq!(
            events[0].as_log()[log_schema().message_key()],
            message.into()
        );
        assert_eq!(
            events[1].as_log()[log_schema().message_key()],
            message.into()
        );
    }

    #[tokio::test]
    async fn include_containers() {
        trace_init();

        let message = "11";
        let name0 = "vector_test_include_container_0";
        let name1 = "vector_test_include_container_1";

        let out = source_with(&[name1], None);

        let docker = docker(None, None).unwrap();

        let id0 = container_log_n(1, name0, None, "11", &docker).await;
        let id1 = container_log_n(1, name1, None, message, &docker).await;
        let events = collect_n(out, 1).await;
        container_remove(&id0, &docker).await;
        container_remove(&id1, &docker).await;

        assert_eq!(
            events[0].as_log()[log_schema().message_key()],
            message.into()
        );
    }

    #[tokio::test]
    async fn exclude_containers() {
        trace_init();

        let will_be_read = "12";

        let prefix = "vector_test_exclude_containers";
        let included0 = format!("{}_{}", prefix, "include0");
        let included1 = format!("{}_{}", prefix, "include1");
        let excluded0 = format!("{}_{}", prefix, "excluded0");

        let docker = docker(None, None).unwrap();

        let out = source_with_config(DockerLogsConfig {
            include_containers: Some(vec![prefix.to_owned()]),
            exclude_containers: Some(vec![excluded0.to_owned()]),
            ..DockerLogsConfig::default()
        });

        let id0 = container_log_n(1, &excluded0, None, "will not be read", &docker).await;
        let id1 = container_log_n(1, &included0, None, will_be_read, &docker).await;
        let id2 = container_log_n(1, &included1, None, will_be_read, &docker).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let events = collect_ready(out).await;
        container_remove(&id0, &docker).await;
        container_remove(&id1, &docker).await;
        container_remove(&id2, &docker).await;

        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0].as_log()[log_schema().message_key()],
            will_be_read.into()
        );

        assert_eq!(
            events[1].as_log()[log_schema().message_key()],
            will_be_read.into()
        );
    }

    #[tokio::test]
    async fn include_labels() {
        trace_init();

        let message = "13";
        let name0 = "vector_test_include_labels_0";
        let name1 = "vector_test_include_labels_1";
        let label = "vector_test_include_label";

        let out = source_with(&[name0, name1], label);

        let docker = docker(None, None).unwrap();

        let id0 = container_log_n(1, name0, None, "13", &docker).await;
        let id1 = container_log_n(1, name1, Some(label), message, &docker).await;
        let events = collect_n(out, 1).await;
        container_remove(&id0, &docker).await;
        container_remove(&id1, &docker).await;

        assert_eq!(
            events[0].as_log()[log_schema().message_key()],
            message.into()
        );
    }

    #[tokio::test]
    async fn currently_running() {
        trace_init();

        let message = "14";
        let name = "vector_test_currently_running";
        let label = "vector_test_label_currently_running";

        let docker = docker(None, None).unwrap();
        let id = running_container(name, Some(label), message, &docker).await;
        let out = source_with(&[name], None);

        let events = collect_n(out, 1).await;
        let _ = container_kill(&id, &docker).await;
        container_remove(&id, &docker).await;

        let log = events[0].as_log();
        assert_eq!(log[log_schema().message_key()], message.into());
        assert_eq!(log[&*super::CONTAINER], id.into());
        assert!(log.get(&*super::CREATED_AT).is_some());
        assert_eq!(log[&*super::IMAGE], "busybox".into());
        assert!(log.get(format!("label.{}", label)).is_some());
        assert_eq!(events[0].as_log()[&super::NAME], name.into());
        assert_eq!(
            events[0].as_log()[log_schema().source_type_key()],
            "docker".into()
        );
    }

    #[tokio::test]
    async fn include_image() {
        trace_init();

        let message = "15";
        let name = "vector_test_include_image";
        let config = DockerLogsConfig {
            include_containers: Some(vec![name.to_owned()]),
            include_images: Some(vec!["busybox".to_owned()]),
            ..DockerLogsConfig::default()
        };

        let out = source_with_config(config);

        let docker = docker(None, None).unwrap();

        let id = container_log_n(1, name, None, message, &docker).await;
        let events = collect_n(out, 1).await;
        container_remove(&id, &docker).await;

        assert_eq!(
            events[0].as_log()[log_schema().message_key()],
            message.into()
        );
    }

    #[tokio::test]
    async fn not_include_image() {
        trace_init();

        let message = "16";
        let name = "vector_test_not_include_image";
        let config_ex = DockerLogsConfig {
            include_images: Some(vec!["some_image".to_owned()]),
            ..DockerLogsConfig::default()
        };

        let exclude_out = source_with_config(config_ex);

        let docker = docker(None, None).unwrap();

        let id = container_log_n(1, name, None, message, &docker).await;
        container_remove(&id, &docker).await;

        assert!(is_empty(exclude_out));
    }

    #[tokio::test]
    async fn not_include_running_image() {
        trace_init();

        let message = "17";
        let name = "vector_test_not_include_running_image";
        let config_ex = DockerLogsConfig {
            include_images: Some(vec!["some_image".to_owned()]),
            ..DockerLogsConfig::default()
        };
        let config_in = DockerLogsConfig {
            include_containers: Some(vec![name.to_owned()]),
            include_images: Some(vec!["busybox".to_owned()]),
            ..DockerLogsConfig::default()
        };

        let docker = docker(None, None).unwrap();

        let id = running_container(name, None, message, &docker).await;
        let exclude_out = source_with_config(config_ex);
        let include_out = source_with_config(config_in);

        let _ = collect_n(include_out, 1).await;
        let _ = container_kill(&id, &docker).await;
        container_remove(&id, &docker).await;

        assert!(is_empty(exclude_out));
    }

    #[tokio::test]
    async fn flat_labels() {
        trace_init();

        let message = "18";
        let name = "vector_test_flat_labels";
        let label = "vector.test.label.flat.labels";

        let docker = docker(None, None).unwrap();
        let id = running_container(name, Some(label), message, &docker).await;
        let out = source_with(&[name], None);

        let events = collect_n(out, 1).await;
        let _ = container_kill(&id, &docker).await;
        container_remove(&id, &docker).await;

        let log = events[0].as_log();
        assert_eq!(log[log_schema().message_key()], message.into());
        assert_eq!(log[&*super::CONTAINER], id.into());
        assert!(log.get(&*super::CREATED_AT).is_some());
        assert_eq!(log[&*super::IMAGE], "busybox".into());
        assert!(log
            .get("label")
            .unwrap()
            .as_map()
            .unwrap()
            .get(label)
            .is_some());
        assert_eq!(events[0].as_log()[&super::NAME], name.into());
        assert_eq!(
            events[0].as_log()[log_schema().source_type_key()],
            "docker".into()
        );
    }

    #[tokio::test]
    async fn log_longer_than_16kb() {
        trace_init();

        let mut message = String::with_capacity(20 * 1024);
        for _ in 0..message.capacity() {
            message.push('0');
        }
        let name = "vector_test_log_longer_than_16kb";

        let out = source_with(&[name], None);

        let docker = docker(None, None).unwrap();

        let id = container_log_n(1, name, None, message.as_str(), &docker).await;
        let events = collect_n(out, 1).await;
        container_remove(&id, &docker).await;

        let log = events[0].as_log();
        assert_eq!(log[log_schema().message_key()], message.into());
    }

    #[tokio::test]
    async fn merge_multiline() {
        trace_init();

        let emitted_messages = vec![
            "java.lang.Exception",
            "    at com.foo.bar(bar.java:123)",
            "    at com.foo.baz(baz.java:456)",
        ];
        let expected_messages = vec![concat!(
            "java.lang.Exception\n",
            "    at com.foo.bar(bar.java:123)\n",
            "    at com.foo.baz(baz.java:456)",
        )];
        let name = "vector_test_merge_multiline";
        let config = DockerLogsConfig {
            include_containers: Some(vec![name.to_owned()]),
            include_images: Some(vec!["busybox".to_owned()]),
            multiline: Some(MultilineConfig {
                start_pattern: "^[^\\s]".to_owned(),
                condition_pattern: "^[\\s]+at".to_owned(),
                mode: line_agg::Mode::ContinueThrough,
                timeout_ms: 10,
            }),
            ..DockerLogsConfig::default()
        };

        let out = source_with_config(config);

        let docker = docker(None, None).unwrap();

        let command = emitted_messages
            .into_iter()
            .map(|message| format!("echo {:?}", message))
            .collect::<Box<_>>()
            .join(" && ");

        let id = cmd_container(name, None, vec!["sh", "-c", &command], &docker, false).await;
        if let Err(error) = container_run(&id, &docker).await {
            container_remove(&id, &docker).await;
            panic!("Container failed to start with error: {:?}", error);
        }
        let events = collect_n(out, expected_messages.len()).await;
        container_remove(&id, &docker).await;

        let actual_messages = events
            .into_iter()
            .map(|event| {
                event
                    .into_log()
                    .remove(&*crate::config::log_schema().message_key())
                    .unwrap()
                    .to_string_lossy()
            })
            .collect::<Vec<_>>();
        assert_eq!(actual_messages, expected_messages);
    }
}
