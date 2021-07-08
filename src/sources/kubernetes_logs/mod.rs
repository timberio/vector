//! This mod implements `kubernetes_logs` source.
//! The scope of this source is to consume the log files that `kubelet` keeps
//! at `/var/log/pods` at the host of the k8s node when `vector` itself is
//! running inside the cluster as a `DaemonSet`.

#![deny(missing_docs)]

use crate::event::{Event, LogEvent};
use crate::internal_events::{
    FileSourceInternalEventsEmitter, KubernetesLogsEventAnnotationFailed,
    KubernetesLogsEventReceived,
};
use crate::kubernetes as k8s;
use crate::{
    config::{
        DataType, GenerateConfig, GlobalOptions, SourceConfig, SourceContext, SourceDescription,
    },
    shutdown::ShutdownSignal,
    sources,
    transforms::{FunctionTransform, TaskTransform},
};
use bytes::Bytes;
use file_source::{
    Checkpointer, FileServer, FileServerShutdown, FingerprintStrategy, Fingerprinter, Line,
    ReadFrom,
};
use k8s_openapi::api::core::v1::{Namespace, Pod};
use serde::{Deserialize, Serialize};
use shared::TimeZone;
use std::convert::TryInto;
use std::path::PathBuf;
use std::time::Duration;

mod k8s_paths_provider;
mod lifecycle;
mod namespace_metadata_annotator;
mod parser;
mod partial_events_merger;
mod path_helpers;
mod pod_metadata_annotator;
mod transform_utils;
mod util;

use futures::{future::FutureExt, sink::Sink, stream::StreamExt};
use k8s_paths_provider::K8sPathsProvider;
use lifecycle::Lifecycle;
use namespace_metadata_annotator::NamespaceMetadataAnnotator;
use pod_metadata_annotator::PodMetadataAnnotator;

/// The key we use for `file` field.
const FILE_KEY: &str = "file";

/// The `self_node_name` value env var key.
const SELF_NODE_NAME_ENV_KEY: &str = "VECTOR_SELF_NODE_NAME";

/// Configuration for the `kubernetes_logs` source.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// Specifies the label selector to filter `Pod`s with, to be used in
    /// addition to the built-in `vector.dev/exclude` filter.
    extra_label_selector: String,

    /// The `name` of the Kubernetes `Node` that Vector runs at.
    /// Required to filter the `Pod`s to only include the ones with the log
    /// files accessible locally.
    self_node_name: String,

    /// Specifies the field selector to filter `Pod`s with, to be used in
    /// addition to the built-in `Node` filter.
    extra_field_selector: String,

    /// Automatically merge partial events.
    auto_partial_merge: bool,

    /// Override global data_dir
    data_dir: Option<PathBuf>,

    /// Specifies the field names for metadata annotation.
    annotation_fields: pod_metadata_annotator::FieldsSpec,

    /// A list of glob patterns to exclude from reading the files.
    exclude_paths_glob_patterns: Vec<PathBuf>,

    /// Max amount of bytes to read from a single file before switching over
    /// to the next file.
    /// This allows distributing the reads more or less evenly accross
    /// the files.
    max_read_bytes: usize,

    /// The maximum number of a bytes a line can contain before being discarded. This protects
    /// against malformed lines or tailing incorrect files.
    max_line_bytes: usize,

    /// This value specifies not exactly the globbing, but interval
    /// between the polling the files to watch from the `paths_provider`.
    /// This is quite efficient, yet might still create some load of the
    /// file system; in addition, it is currently coupled with chechsum dumping
    /// in the underlying file server, so setting it too low may introduce
    /// a significant overhead.
    glob_minimum_cooldown_ms: usize,

    /// A field to use to set the timestamp when Vector ingested the event.
    /// This is useful to compute the latency between important event processing
    /// stages, i.e. the time delta between log line was written and when it was
    /// processed by the `kubernetes_logs` source.
    ingestion_timestamp_field: Option<String>,

    /// The default time zone for timestamps without an explicit zone.
    timezone: Option<TimeZone>,

    /// Optional path to a kubeconfig file readable by Vector. If not set,
    /// Vector will try to connect to Kubernetes using in-cluster configuration.
    kube_config_file: Option<PathBuf>,
}

inventory::submit! {
    SourceDescription::new::<Config>(COMPONENT_NAME)
}

impl GenerateConfig for Config {
    fn generate_config() -> toml::Value {
        toml::Value::try_from(&Self {
            self_node_name: default_self_node_name_env_template(),
            auto_partial_merge: true,
            ..Default::default()
        })
        .unwrap()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            extra_label_selector: "".to_string(),
            self_node_name: default_self_node_name_env_template(),
            extra_field_selector: "".to_string(),
            auto_partial_merge: true,
            data_dir: None,
            annotation_fields: pod_metadata_annotator::FieldsSpec::default(),
            exclude_paths_glob_patterns: default_path_exclusion(),
            max_read_bytes: default_max_read_bytes(),
            max_line_bytes: default_max_line_bytes(),
            glob_minimum_cooldown_ms: default_glob_minimum_cooldown_ms(),
            ingestion_timestamp_field: None,
            timezone: None,
            kube_config_file: None,
        }
    }
}

const COMPONENT_NAME: &str = "kubernetes_logs";

#[async_trait::async_trait]
#[typetag::serde(name = "kubernetes_logs")]
impl SourceConfig for Config {
    async fn build(&self, cx: SourceContext) -> crate::Result<sources::Source> {
        let source = Source::new(self, &cx.globals, &cx.name)?;
        Ok(Box::pin(source.run(cx.out, cx.shutdown).map(|result| {
            result.map_err(|error| {
                error!(message = "Source future failed.", %error);
            })
        })))
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn source_type(&self) -> &'static str {
        COMPONENT_NAME
    }
}

#[derive(Clone)]
struct Source {
    client: k8s::client::Client,
    data_dir: PathBuf,
    auto_partial_merge: bool,
    fields_spec: pod_metadata_annotator::FieldsSpec,
    field_selector: String,
    label_selector: String,
    exclude_paths: Vec<glob::Pattern>,
    max_read_bytes: usize,
    max_line_bytes: usize,
    glob_minimum_cooldown: Duration,
    ingestion_timestamp_field: Option<String>,
    timezone: TimeZone,
}

impl Source {
    fn new(config: &Config, globals: &GlobalOptions, name: &str) -> crate::Result<Self> {
        let field_selector = prepare_field_selector(config)?;
        let label_selector = prepare_label_selector(config);

        let k8s_config = match &config.kube_config_file {
            Some(kc) => k8s::client::config::Config::kubeconfig(kc)?,
            None => k8s::client::config::Config::in_cluster()?,
        };
        let client = k8s::client::Client::new(k8s_config)?;

        let data_dir = globals.resolve_and_make_data_subdir(config.data_dir.as_ref(), name)?;
        let timezone = config.timezone.unwrap_or(globals.timezone);

        let exclude_paths = prepare_exclude_paths(config)?;

        let glob_minimum_cooldown =
            Duration::from_millis(config.glob_minimum_cooldown_ms.try_into().expect(
                "unable to convert glob_minimum_cooldown_ms from usize to u64 without data loss",
            ));

        Ok(Self {
            client,
            data_dir,
            auto_partial_merge: config.auto_partial_merge,
            fields_spec: config.annotation_fields.clone(),
            field_selector,
            label_selector,
            exclude_paths,
            max_read_bytes: config.max_read_bytes,
            max_line_bytes: config.max_line_bytes,
            glob_minimum_cooldown,
            ingestion_timestamp_field: config.ingestion_timestamp_field.clone(),
            timezone,
        })
    }

    async fn run<O>(self, out: O, global_shutdown: ShutdownSignal) -> crate::Result<()>
    where
        O: Sink<Event> + Send + 'static + Unpin,
        <O as Sink<Event>>::Error: std::error::Error,
    {
        let Self {
            client,
            data_dir,
            auto_partial_merge,
            fields_spec,
            field_selector,
            label_selector,
            exclude_paths,
            max_read_bytes,
            max_line_bytes,
            glob_minimum_cooldown,
            ingestion_timestamp_field,
            timezone,
        } = self;

        let watcher =
            k8s::api_watcher::ApiWatcher::new(client.clone(), Pod::watch_pod_for_all_namespaces);
        let watcher = k8s::instrumenting_watcher::InstrumentingWatcher::new(watcher);
        let (state_reader, state_writer) = evmap::new();
        let state_writer =
            k8s::state::evmap::Writer::new(state_writer, Some(Duration::from_millis(10)));
        let state_writer = k8s::state::instrumenting::Writer::new(state_writer);
        let state_writer =
            k8s::state::delayed_delete::Writer::new(state_writer, Duration::from_secs(60));

        let mut reflector = k8s::reflector::Reflector::new(
            watcher,
            state_writer,
            Some(field_selector),
            Some(label_selector),
            Duration::from_secs(1),
        );
        let reflector_process = reflector.run();

        let paths_provider = K8sPathsProvider::new(state_reader.clone(), exclude_paths);
        let annotator = PodMetadataAnnotator::new(state_reader, fields_spec);

        // -----------------------------------------------------------------

        let ns_watcher =
            k8s::api_watcher::ApiWatcher::new(client.clone(), Namespace::watch_namespace);
        let ns_watcher = k8s::instrumenting_watcher::InstrumentingWatcher::new(ns_watcher);
        let (ns_state_reader, ns_state_writer) = evmap::new();
        let ns_state_writer =
            k8s::state::evmap::Writer::new(ns_state_writer, Some(Duration::from_millis(10)));
        let ns_state_writer = k8s::state::instrumenting::Writer::new(ns_state_writer);
        let ns_state_writer =
            k8s::state::delayed_delete::Writer::new(ns_state_writer, Duration::from_secs(60));

        let mut ns_reflector = k8s::reflector::Reflector::new(
            ns_watcher,
            ns_state_writer,
            None,
            None,
            Duration::from_secs(1),
        );
        let ns_reflector_process = ns_reflector.run();

        let ns_annotator = NamespaceMetadataAnnotator::new(
            ns_state_reader,
            namespace_metadata_annotator::FieldsSpec::default(),
        );

        // TODO: maybe more of the parameters have to be configurable.

        let checkpointer = Checkpointer::new(&data_dir);
        let file_server = FileServer {
            // Use our special paths provider.
            paths_provider,
            // Max amount of bytes to read from a single file before switching
            // over to the next file.
            // This allows distributing the reads more or less evenly accross
            // the files.
            max_read_bytes,
            // We want to use checkpoining mechanism, and resume from where we
            // left off.
            ignore_checkpoints: false,
            // Match the default behavior
            read_from: ReadFrom::Beginning,
            // We're now aware of the use cases that would require specifying
            // the starting point in time since when we should collect the logs,
            // so we just disable it. If users ask, we can expose it. There may
            // be other, more sound ways for users considering the use of this
            // option to solve their use case, so take consideration.
            ignore_before: None,
            // The maximum number of a bytes a line can contain before being discarded. This
            // protects against malformed lines or tailing incorrect files.
            max_line_bytes,
            // Delimiter bytes that is used to read the file line-by-line
            line_delimiter: Bytes::from("\n"),
            // The directory where to keep the checkpoints.
            data_dir,
            // This value specifies not exactly the globbing, but interval
            // between the polling the files to watch from the `paths_provider`.
            glob_minimum_cooldown,
            // The shape of the log files is well-known in the Kubernetes
            // environment, so we pick the a specially crafted fingerprinter
            // for the log files.
            fingerprinter: Fingerprinter {
                strategy: FingerprintStrategy::FirstLinesChecksum {
                    // Max line length to expect during fingerprinting, see the
                    // explanation above.
                    ignored_header_bytes: 0,
                    lines: 1,
                },
                max_line_length: max_line_bytes,
                ignore_not_found: true,
            },
            // We expect the files distribution to not be a concern because of
            // the way we pick files for gathering: for each container, only the
            // last log file is currently picked. Thus there's no need for
            // ordering, as each logical log stream is guaranteed to start with
            // just one file, makis it impossible to interleave with other
            // relevant log lines in the absense of such relevant log lines.
            oldest_first: false,
            // We do not remove the log files, `kubelet` is responsible for it.
            remove_after: None,
            // The standard emitter.
            emitter: FileSourceInternalEventsEmitter,
            // A handle to the current tokio runtime
            handle: tokio::runtime::Handle::current(),
        };

        let (file_source_tx, file_source_rx) = futures::channel::mpsc::channel::<Vec<Line>>(2);

        let mut parser = parser::build(timezone);
        let partial_events_merger = Box::new(partial_events_merger::build(auto_partial_merge));

        let checkpoints = checkpointer.view();
        let events = file_source_rx.map(futures::stream::iter);
        let events = events.flatten();
        let events = events.map(move |line| {
            let byte_size = line.text.len();
            let mut event = create_event(
                line.text,
                &line.filename,
                ingestion_timestamp_field.as_deref(),
            );
            let file_info = annotator.annotate(&mut event, &line.filename);

            emit!(KubernetesLogsEventReceived {
                file: &line.filename,
                byte_size,
                pod_name: file_info.as_ref().map(|info| info.pod_name),
            });

            if file_info.is_some() {
              let pod_namespace = file_info.as_ref().map(|info| info.pod_namespace);
              match pod_namespace {
                Some(name) => {
                  ns_annotator.annotate(&mut event, name);
                }
                None => {
                  emit!(KubernetesLogsEventAnnotationFailed { event: &event });
                }
              }
            }
            if file_info.is_none() {
                emit!(KubernetesLogsEventAnnotationFailed { event: &event });
            }

            checkpoints.update(line.file_id, line.offset);
            event
        });
        let events = events.flat_map(move |event| {
            let mut buf = Vec::with_capacity(1);
            parser.transform(&mut buf, event);
            futures::stream::iter(buf)
        });

        let event_processing_loop = partial_events_merger
            .transform(Box::pin(events))
            .map(Ok)
            .forward(out);

        let mut lifecycle = Lifecycle::new();
        {
            let (slot, shutdown) = lifecycle.add();
            let fut =
                util::cancel_on_signal(reflector_process, shutdown).map(|result| match result {
                    Ok(()) => info!(message = "Reflector process completed gracefully."),
                    Err(error) => {
                        error!(message = "Reflector process exited with an error.", %error)
                    }
                });
            slot.bind(Box::pin(fut));
        }
        {
            let (slot, shutdown) = lifecycle.add();
            let fut =
                util::cancel_on_signal(ns_reflector_process, shutdown).map(|result| match result {
                    Ok(()) => info!(message = "Namespace reflector process completed gracefully."),
                    Err(error) => {
                        error!(message = "Namespace reflector process exited with an error.", %error)
                    }
                });
            slot.bind(Box::pin(fut));
        }
        {
            let (slot, shutdown) = lifecycle.add();
            let fut = util::run_file_server(file_server, file_source_tx, shutdown, checkpointer)
                .map(|result| match result {
                    Ok(FileServerShutdown) => info!(message = "File server completed gracefully."),
                    Err(error) => error!(message = "File server exited with an error.", %error),
                });
            slot.bind(Box::pin(fut));
        }
        {
            let (slot, shutdown) = lifecycle.add();
            let fut = util::complete_with_deadline_on_signal(
                event_processing_loop,
                shutdown,
                Duration::from_secs(30), // more than enough time to propagate
            )
            .map(|result| {
                match result {
                    Ok(Ok(())) => info!(message = "Event processing loop completed gracefully."),
                    Ok(Err(error)) => error!(
                        message = "Event processing loop exited with an error.",
                        %error
                    ),
                    Err(error) => error!(
                        message = "Event processing loop timed out during the shutdown.",
                        %error
                    ),
                };
            });
            slot.bind(Box::pin(fut));
        }

        lifecycle.run(global_shutdown).await;
        info!(message = "Done.");
        Ok(())
    }
}

fn create_event(line: Bytes, file: &str, ingestion_timestamp_field: Option<&str>) -> Event {
    let mut event = LogEvent::from(line);

    // Add source type.
    event.insert(
        crate::config::log_schema().source_type_key(),
        COMPONENT_NAME.to_owned(),
    );

    // Add file.
    event.insert(FILE_KEY, file.to_owned());

    // Add ingestion timestamp if requested.
    if let Some(ingestion_timestamp_field) = ingestion_timestamp_field {
        event.insert(ingestion_timestamp_field, chrono::Utc::now());
    }

    event.into()
}

/// This function returns the default value for `self_node_name` variable
/// as it should be at the generated config file.
fn default_self_node_name_env_template() -> String {
    format!("${{{}}}", SELF_NODE_NAME_ENV_KEY.to_owned())
}

fn default_path_exclusion() -> Vec<PathBuf> {
    vec![PathBuf::from("**/*.gz"), PathBuf::from("**/*.tmp")]
}

fn default_max_read_bytes() -> usize {
    2048
}

fn default_max_line_bytes() -> usize {
    // NOTE: The below comment documents an incorrect assumption, see
    // https://github.com/timberio/vector/issues/6967
    //
    // The 16KB is the maximum size of the payload at single line for both
    // docker and CRI log formats.
    // We take a double of that to account for metadata and padding, and to
    // have a power of two rounding. Line splitting is countered at the
    // parsers, see the `partial_events_merger` logic.

    32 * 1024 // 32 KiB
}

fn default_glob_minimum_cooldown_ms() -> usize {
    60000
}

// This function constructs the patterns we exclude from file watching, created
// from the defaults or user provided configuration.
fn prepare_exclude_paths(config: &Config) -> crate::Result<Vec<glob::Pattern>> {
    let exclude_paths = config
        .exclude_paths_glob_patterns
        .iter()
        .map(|pattern| {
            let pattern = pattern
                .to_str()
                .ok_or("glob pattern is not a valid UTF-8 string")?;
            Ok(glob::Pattern::new(pattern)?)
        })
        .collect::<crate::Result<Vec<_>>>()?;

    info!(
        message = "Excluding matching files.",
        exclude_paths = ?exclude_paths
            .iter()
            .map(glob::Pattern::as_str)
            .collect::<Vec<_>>()
    );

    Ok(exclude_paths)
}

// This function constructs the effective field selector to use, based on
// the specified configuration.
fn prepare_field_selector(config: &Config) -> crate::Result<String> {
    let self_node_name = if config.self_node_name.is_empty()
        || config.self_node_name == default_self_node_name_env_template()
    {
        std::env::var(SELF_NODE_NAME_ENV_KEY).map_err(|_| {
            format!(
                "self_node_name config value or {} env var is not set",
                SELF_NODE_NAME_ENV_KEY
            )
        })?
    } else {
        config.self_node_name.clone()
    };
    info!(
        message = "Obtained Kubernetes Node name to collect logs for (self).",
        ?self_node_name
    );

    let field_selector = format!("spec.nodeName={}", self_node_name);

    if config.extra_field_selector.is_empty() {
        return Ok(field_selector);
    }

    Ok(format!(
        "{},{}",
        field_selector, config.extra_field_selector
    ))
}

// This function constructs the effective label selector to use, based on
// the specified configuration.
fn prepare_label_selector(config: &Config) -> String {
    const BUILT_IN: &str = "vector.dev/exclude!=true";

    if config.extra_label_selector.is_empty() {
        return BUILT_IN.to_string();
    }

    format!("{},{}", BUILT_IN, config.extra_label_selector)
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<Config>();
    }

    #[test]
    fn prepare_exclude_paths() {
        let cases = vec![
            (
                Config::default(),
                vec![
                    glob::Pattern::new("**/*.gz").unwrap(),
                    glob::Pattern::new("**/*.tmp").unwrap(),
                ],
            ),
            (
                Config {
                    exclude_paths_glob_patterns: vec![std::path::PathBuf::from("**/*.tmp")],
                    ..Default::default()
                },
                vec![glob::Pattern::new("**/*.tmp").unwrap()],
            ),
            (
                Config {
                    exclude_paths_glob_patterns: vec![
                        std::path::PathBuf::from("**/kube-system_*/**"),
                        std::path::PathBuf::from("**/*.gz"),
                        std::path::PathBuf::from("**/*.tmp"),
                    ],
                    ..Default::default()
                },
                vec![
                    glob::Pattern::new("**/kube-system_*/**").unwrap(),
                    glob::Pattern::new("**/*.gz").unwrap(),
                    glob::Pattern::new("**/*.tmp").unwrap(),
                ],
            ),
        ];

        for (input, mut expected) in cases {
            let mut output = super::prepare_exclude_paths(&input).unwrap();
            expected.sort();
            output.sort();
            assert_eq!(expected, output, "expected left, actual right");
        }
    }

    #[test]
    fn prepare_field_selector() {
        let cases = vec![
            // We're not testing `Config::default()` or empty `self_node_name`
            // as passing env vars in the concurrent tests is diffucult.
            (
                Config {
                    self_node_name: "qwe".to_owned(),
                    ..Default::default()
                },
                "spec.nodeName=qwe",
            ),
            (
                Config {
                    self_node_name: "qwe".to_owned(),
                    extra_field_selector: "".to_owned(),
                    ..Default::default()
                },
                "spec.nodeName=qwe",
            ),
            (
                Config {
                    self_node_name: "qwe".to_owned(),
                    extra_field_selector: "foo=bar".to_owned(),
                    ..Default::default()
                },
                "spec.nodeName=qwe,foo=bar",
            ),
        ];

        for (input, expected) in cases {
            let output = super::prepare_field_selector(&input).unwrap();
            assert_eq!(expected, output, "expected left, actual right");
        }
    }

    #[test]
    fn prepare_label_selector() {
        let cases = vec![
            (Config::default(), "vector.dev/exclude!=true"),
            (
                Config {
                    extra_label_selector: "".to_owned(),
                    ..Default::default()
                },
                "vector.dev/exclude!=true",
            ),
            (
                Config {
                    extra_label_selector: "qwe".to_owned(),
                    ..Default::default()
                },
                "vector.dev/exclude!=true,qwe",
            ),
        ];

        for (input, expected) in cases {
            let output = super::prepare_label_selector(&input);
            assert_eq!(expected, output, "expected left, actual right");
        }
    }
}
