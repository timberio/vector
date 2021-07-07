use crate::{
    buffers::Acker,
    conditions,
    event::Metric,
    shutdown::ShutdownSignal,
    sinks::{self, util::UriSerde},
    sources, transforms, Pipeline,
};
use async_trait::async_trait;
use component::ComponentDescription;
use indexmap::IndexMap; // IndexMap preserves insertion order, allowing us to output errors in the same order they are present in the file
use serde::{Deserialize, Serialize};
use shared::TimeZone;
use snafu::{ResultExt, Snafu};
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Display, Formatter};
use std::fs::DirBuilder;
use std::hash::Hash;
use std::net::SocketAddr;
use std::path::PathBuf;

pub mod api;
mod builder;
mod compiler;
pub mod component;
mod diff;
pub mod format;
mod loading;
pub mod provider;
pub mod proxy;
mod unit_test;
mod validation;
mod vars;
pub mod watcher;

pub use builder::ConfigBuilder;
pub use diff::ConfigDiff;
pub use format::{Format, FormatHint};
pub use loading::{
    load, load_builder_from_paths, load_from_paths, load_from_paths_with_provider, load_from_str,
    merge_path_lists, process_paths, CONFIG_PATHS,
};
pub use proxy::ProxyConfig;
pub use unit_test::build_unit_tests_main as build_unit_tests;
pub use validation::warnings;
pub use vector_core::config::{log_schema, LogSchema};

/// Loads Log Schema from configurations and sets global schema.
/// Once this is done, configurations can be correctly loaded using
/// configured log schema defaults.
/// If deny is set, will panic if schema has already been set.
pub fn init_log_schema(config_paths: &[ConfigPath], deny_if_set: bool) -> Result<(), Vec<String>> {
    vector_core::config::init_log_schema(
        || {
            let (builder, _) = load_builder_from_paths(config_paths)?;
            Ok(builder.global.log_schema)
        },
        deny_if_set,
    )
}

#[derive(Debug, Clone, Ord, PartialOrd, Eq, PartialEq)]
pub enum ConfigPath {
    File(PathBuf, FormatHint),
    Dir(PathBuf),
}

impl<'a> From<&'a ConfigPath> for &'a PathBuf {
    fn from(config_path: &'a ConfigPath) -> &'a PathBuf {
        match config_path {
            ConfigPath::File(path, _) => path,
            ConfigPath::Dir(path) => path,
        }
    }
}

#[derive(Debug, Default)]
pub struct Config {
    pub global: GlobalOptions,
    #[cfg(feature = "api")]
    pub api: api::Options,
    pub healthchecks: HealthcheckOptions,
    pub sources: IndexMap<String, SourceOuter>,
    pub sinks: IndexMap<String, SinkOuter>,
    pub transforms: IndexMap<String, TransformOuter>,
    tests: Vec<TestDefinition>,
    expansions: IndexMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct GlobalOptions {
    #[serde(default = "default_data_dir")]
    pub data_dir: Option<PathBuf>,
    #[serde(skip_serializing_if = "crate::serde::skip_serializing_if_default")]
    pub log_schema: LogSchema,
    #[serde(skip_serializing_if = "crate::serde::skip_serializing_if_default")]
    pub timezone: TimeZone,
    #[serde(skip_serializing_if = "crate::serde::skip_serializing_if_default")]
    pub proxy: ProxyConfig,
}

pub fn default_data_dir() -> Option<PathBuf> {
    Some(PathBuf::from("/var/lib/vector/"))
}

#[derive(Debug, Snafu)]
pub enum DataDirError {
    #[snafu(display("data_dir option required, but not given here or globally"))]
    MissingDataDir,
    #[snafu(display("data_dir {:?} does not exist", data_dir))]
    DoesNotExist { data_dir: PathBuf },
    #[snafu(display("data_dir {:?} is not writable", data_dir))]
    NotWritable { data_dir: PathBuf },
    #[snafu(display(
        "Could not create subdirectory {:?} inside of data dir {:?}: {}",
        subdir,
        data_dir,
        source
    ))]
    CouldNotCreate {
        subdir: PathBuf,
        data_dir: PathBuf,
        source: std::io::Error,
    },
}

impl GlobalOptions {
    /// Resolve the `data_dir` option in either the global or local
    /// config, and validate that it exists and is writable.
    pub fn resolve_and_validate_data_dir(
        &self,
        local_data_dir: Option<&PathBuf>,
    ) -> crate::Result<PathBuf> {
        let data_dir = local_data_dir
            .or_else(|| self.data_dir.as_ref())
            .ok_or(DataDirError::MissingDataDir)
            .map_err(Box::new)?
            .to_path_buf();
        if !data_dir.exists() {
            return Err(DataDirError::DoesNotExist { data_dir }.into());
        }
        let readonly = std::fs::metadata(&data_dir)
            .map(|meta| meta.permissions().readonly())
            .unwrap_or(true);
        if readonly {
            return Err(DataDirError::NotWritable { data_dir }.into());
        }
        Ok(data_dir)
    }

    /// Resolve the `data_dir` option using
    /// `resolve_and_validate_data_dir` and then ensure a named
    /// subdirectory exists.
    pub fn resolve_and_make_data_subdir(
        &self,
        local: Option<&PathBuf>,
        subdir: &str,
    ) -> crate::Result<PathBuf> {
        let data_dir = self.resolve_and_validate_data_dir(local)?;

        let mut data_subdir = data_dir.clone();
        data_subdir.push(subdir);

        DirBuilder::new()
            .recursive(true)
            .create(&data_subdir)
            .with_context(|| CouldNotCreate { subdir, data_dir })?;
        Ok(data_subdir)
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(default)]
pub struct HealthcheckOptions {
    pub enabled: bool,
    pub require_healthy: bool,
}

impl HealthcheckOptions {
    pub fn set_require_healthy(&mut self, require_healthy: impl Into<Option<bool>>) {
        if let Some(require_healthy) = require_healthy.into() {
            self.require_healthy = require_healthy;
        }
    }

    fn merge(&mut self, other: Self) {
        self.enabled &= other.enabled;
        self.require_healthy |= other.require_healthy;
    }
}

impl Default for HealthcheckOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            require_healthy: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Copy)]
pub enum DataType {
    Any,
    Log,
    Metric,
}

pub trait GenerateConfig {
    fn generate_config() -> toml::Value;
}

#[macro_export]
macro_rules! impl_generate_config_from_default {
    ($type:ty) => {
        impl $crate::config::GenerateConfig for $type {
            fn generate_config() -> toml::Value {
                toml::Value::try_from(&Self::default()).unwrap()
            }
        }
    };
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SourceOuter {
    #[serde(default = "default_acknowledgements")]
    pub acknowledgements: bool,
    #[serde(flatten)]
    pub(super) inner: Box<dyn SourceConfig>,
}

fn default_acknowledgements() -> bool {
    false
}

impl SourceOuter {
    pub(crate) fn new(source: impl SourceConfig + 'static) -> Self {
        Self {
            acknowledgements: default_acknowledgements(),
            inner: Box::new(source),
        }
    }
}

#[async_trait]
#[typetag::serde(tag = "type")]
pub trait SourceConfig: core::fmt::Debug + Send + Sync {
    async fn build(&self, cx: SourceContext) -> crate::Result<sources::Source>;

    fn output_type(&self) -> DataType;

    fn source_type(&self) -> &'static str;

    /// Resources that the source is using.
    fn resources(&self) -> Vec<Resource> {
        Vec::new()
    }
}

pub struct SourceContext {
    pub name: String,
    pub globals: GlobalOptions,
    pub shutdown: ShutdownSignal,
    pub out: Pipeline,
    pub acknowledgements: bool,
}

impl SourceContext {
    #[cfg(test)]
    pub fn new_shutdown(
        name: &str,
        out: Pipeline,
    ) -> (Self, crate::shutdown::SourceShutdownCoordinator) {
        let mut shutdown = crate::shutdown::SourceShutdownCoordinator::default();
        let (shutdown_signal, _) = shutdown.register_source(name);
        (
            Self {
                name: name.into(),
                globals: GlobalOptions::default(),
                shutdown: shutdown_signal,
                out,
                acknowledgements: default_acknowledgements(),
            },
            shutdown,
        )
    }

    #[cfg(test)]
    pub fn new_test(out: Pipeline) -> Self {
        Self {
            name: "default".into(),
            globals: GlobalOptions::default(),
            shutdown: ShutdownSignal::noop(),
            out,
            acknowledgements: default_acknowledgements(),
        }
    }
}

pub type SourceDescription = ComponentDescription<Box<dyn SourceConfig>>;

inventory::collect!(SourceDescription);

#[derive(Deserialize, Serialize, Debug)]
pub struct SinkOuter {
    pub inputs: Vec<String>,

    // We are accepting this option for backward compatibility.
    healthcheck_uri: Option<UriSerde>,

    // We are accepting bool for backward compatibility.
    #[serde(deserialize_with = "crate::serde::bool_or_struct")]
    #[serde(default)]
    healthcheck: SinkHealthcheckOptions,

    #[serde(default)]
    pub buffer: crate::buffers::BufferConfig,

    #[serde(flatten)]
    pub inner: Box<dyn SinkConfig>,
}

impl SinkOuter {
    pub fn new(inputs: Vec<String>, inner: Box<dyn SinkConfig>) -> Self {
        SinkOuter {
            buffer: Default::default(),
            healthcheck: SinkHealthcheckOptions::default(),
            healthcheck_uri: None,
            inner,
            inputs,
        }
    }

    pub fn resources(&self, name: &str) -> Vec<Resource> {
        let mut resources = self.inner.resources();
        resources.append(&mut self.buffer.resources(name));
        resources
    }

    pub fn healthcheck(&self) -> SinkHealthcheckOptions {
        if self.healthcheck_uri.is_some() && self.healthcheck.uri.is_some() {
            warn!("Both `healthcheck.uri` and `healthcheck_uri` options are specified. Using value of `healthcheck.uri`.")
        } else if self.healthcheck_uri.is_some() {
            warn!("`healthcheck_uri` option has been deprecated, use `healthcheck.uri` instead. ")
        }
        SinkHealthcheckOptions {
            uri: self
                .healthcheck
                .uri
                .clone()
                .or_else(|| self.healthcheck_uri.clone()),
            ..self.healthcheck.clone()
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(default)]
pub struct SinkHealthcheckOptions {
    pub enabled: bool,
    pub uri: Option<UriSerde>,
}

impl Default for SinkHealthcheckOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            uri: None,
        }
    }
}

impl From<bool> for SinkHealthcheckOptions {
    fn from(enabled: bool) -> Self {
        Self { enabled, uri: None }
    }
}

impl From<UriSerde> for SinkHealthcheckOptions {
    fn from(uri: UriSerde) -> Self {
        Self {
            enabled: true,
            uri: Some(uri),
        }
    }
}

#[async_trait]
#[typetag::serde(tag = "type")]
pub trait SinkConfig: core::fmt::Debug + Send + Sync {
    async fn build(
        &self,
        cx: SinkContext,
    ) -> crate::Result<(sinks::VectorSink, sinks::Healthcheck)>;

    fn input_type(&self) -> DataType;

    fn sink_type(&self) -> &'static str;

    /// Resources that the sink is using.
    fn resources(&self) -> Vec<Resource> {
        Vec::new()
    }
}

#[derive(Debug, Clone)]
pub struct SinkContext {
    pub(super) acker: Acker,
    pub(super) healthcheck: SinkHealthcheckOptions,
    pub(super) globals: GlobalOptions,
}

impl SinkContext {
    #[cfg(test)]
    pub fn new_test() -> Self {
        Self {
            acker: Acker::Null,
            healthcheck: SinkHealthcheckOptions::default(),
            globals: GlobalOptions::default(),
        }
    }

    pub fn acker(&self) -> Acker {
        self.acker.clone()
    }

    pub fn globals(&self) -> &GlobalOptions {
        &self.globals
    }
}

pub type SinkDescription = ComponentDescription<Box<dyn SinkConfig>>;

inventory::collect!(SinkDescription);

#[derive(Deserialize, Serialize, Debug)]
pub struct TransformOuter {
    pub inputs: Vec<String>,
    #[serde(flatten)]
    pub inner: Box<dyn TransformConfig>,
}

#[async_trait]
#[typetag::serde(tag = "type")]
pub trait TransformConfig: core::fmt::Debug + Send + Sync + dyn_clone::DynClone {
    async fn build(&self, globals: &GlobalOptions) -> crate::Result<transforms::Transform>;

    fn input_type(&self) -> DataType;

    fn output_type(&self) -> DataType;

    fn transform_type(&self) -> &'static str;

    /// Allows a transform configuration to expand itself into multiple "child"
    /// transformations to replace it. This allows a transform to act as a macro
    /// for various patterns.
    fn expand(&mut self) -> crate::Result<Option<IndexMap<String, Box<dyn TransformConfig>>>> {
        Ok(None)
    }
}

dyn_clone::clone_trait_object!(TransformConfig);

pub type TransformDescription = ComponentDescription<Box<dyn TransformConfig>>;

inventory::collect!(TransformDescription);

/// Unique thing, like port, of which only one owner can be.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum Resource {
    Port(SocketAddr, Protocol),
    SystemFdOffset(usize),
    Stdin,
    DiskBuffer(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Copy)]
pub enum Protocol {
    Tcp,
    Udp,
}

impl Resource {
    pub fn tcp(addr: SocketAddr) -> Self {
        Self::Port(addr, Protocol::Tcp)
    }

    pub fn udp(addr: SocketAddr) -> Self {
        Self::Port(addr, Protocol::Udp)
    }

    /// From given components returns all that have a resource conflict with any other component.
    pub fn conflicts<K: Eq + Hash + Clone>(
        components: impl IntoIterator<Item = (K, Vec<Resource>)>,
    ) -> HashMap<Resource, HashSet<K>> {
        let mut resource_map = HashMap::<Resource, HashSet<K>>::new();
        let mut unspecified = Vec::new();

        // Find equality based conflicts
        for (key, resources) in components {
            for resource in resources {
                if let Resource::Port(address, protocol) = &resource {
                    if address.ip().is_unspecified() {
                        unspecified.push((key.clone(), address.port(), *protocol));
                    }
                }

                resource_map
                    .entry(resource)
                    .or_default()
                    .insert(key.clone());
            }
        }

        // Port with unspecified address will bind to all network interfaces
        // so we have to check for all Port resources if they share the same
        // port.
        for (key, port, protocol0) in unspecified {
            for (resource, components) in resource_map.iter_mut() {
                if let Resource::Port(address, protocol) = resource {
                    if address.port() == port && &protocol0 == protocol {
                        components.insert(key.clone());
                    }
                }
            }
        }

        resource_map.retain(|_, components| components.len() > 1);

        resource_map
    }
}

impl Display for Protocol {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            Protocol::Udp => write!(fmt, "udp"),
            Protocol::Tcp => write!(fmt, "tcp"),
        }
    }
}

impl Display for Resource {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            Resource::Port(address, protocol) => write!(fmt, "{} {}", protocol, address),
            Resource::SystemFdOffset(offset) => write!(fmt, "systemd {}th socket", offset + 1),
            Resource::Stdin => write!(fmt, "stdin"),
            Resource::DiskBuffer(name) => write!(fmt, "disk buffer {:?}", name),
        }
    }
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct TestDefinition {
    pub name: String,
    pub input: Option<TestInput>,
    #[serde(default)]
    pub inputs: Vec<TestInput>,
    #[serde(default)]
    pub outputs: Vec<TestOutput>,
    #[serde(default)]
    pub no_outputs_from: Vec<String>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(untagged)]
pub enum TestInputValue {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct TestInput {
    pub insert_at: String,
    #[serde(default = "default_test_input_type", rename = "type")]
    pub type_str: String,
    pub value: Option<String>,
    pub log_fields: Option<IndexMap<String, TestInputValue>>,
    pub metric: Option<Metric>,
}

fn default_test_input_type() -> String {
    "raw".to_string()
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct TestOutput {
    pub extract_from: String,
    pub conditions: Option<Vec<conditions::AnyCondition>>,
}

impl Config {
    pub fn builder() -> builder::ConfigBuilder {
        Default::default()
    }

    /// Expand a logical component name (i.e. from the config file) into the names of the
    /// components it was expanded to as part of the macro process. Does not check that the
    /// identifier is otherwise valid.
    pub fn get_inputs(&self, identifier: &str) -> Vec<String> {
        self.expansions
            .get(identifier)
            .cloned()
            .unwrap_or_else(|| vec![String::from(identifier)])
    }
}

#[cfg(all(
    test,
    feature = "sources-file",
    feature = "sinks-console",
    feature = "transforms-json_parser"
))]
mod test {
    use super::{builder::ConfigBuilder, format, load_from_str, Format};
    use indoc::indoc;
    use std::path::PathBuf;

    #[test]
    fn default_data_dir() {
        let config = load_from_str(
            indoc! {r#"
                [sources.in]
                  type = "file"
                  include = ["/var/log/messages"]

                [sinks.out]
                  type = "console"
                  inputs = ["in"]
                  encoding = "json"
            "#},
            Some(Format::Toml),
        )
        .unwrap();

        assert_eq!(
            Some(PathBuf::from("/var/lib/vector")),
            config.global.data_dir
        )
    }

    #[test]
    fn default_schema() {
        let config = load_from_str(
            indoc! {r#"
                [sources.in]
                  type = "file"
                  include = ["/var/log/messages"]

                [sinks.out]
                  type = "console"
                  inputs = ["in"]
                  encoding = "json"
            "#},
            Some(Format::Toml),
        )
        .unwrap();

        assert_eq!("host", config.global.log_schema.host_key().to_string());
        assert_eq!(
            "message",
            config.global.log_schema.message_key().to_string()
        );
        assert_eq!(
            "timestamp",
            config.global.log_schema.timestamp_key().to_string()
        );
    }

    #[test]
    fn custom_schema() {
        let config = load_from_str(
            indoc! {r#"
                [log_schema]
                  host_key = "this"
                  message_key = "that"
                  timestamp_key = "then"

                [sources.in]
                  type = "file"
                  include = ["/var/log/messages"]

                [sinks.out]
                  type = "console"
                  inputs = ["in"]
                  encoding = "json"
            "#},
            Some(Format::Toml),
        )
        .unwrap();

        assert_eq!("this", config.global.log_schema.host_key().to_string());
        assert_eq!("that", config.global.log_schema.message_key().to_string());
        assert_eq!("then", config.global.log_schema.timestamp_key().to_string());
    }

    #[test]
    fn config_append() {
        let mut config: ConfigBuilder = format::deserialize(
            indoc! {r#"
                [sources.in]
                  type = "file"
                  include = ["/var/log/messages"]

                [sinks.out]
                  type = "console"
                  inputs = ["in"]
                  encoding = "json"
            "#},
            Some(Format::Toml),
        )
        .unwrap();

        assert_eq!(
            config.append(
                format::deserialize(
                    indoc! {r#"
                        data_dir = "/foobar"

                        [transforms.foo]
                          type = "json_parser"
                          inputs = [ "in" ]

                        [[tests]]
                          name = "check_simple_log"
                          [tests.input]
                            insert_at = "foo"
                            type = "raw"
                            value = "2019-11-28T12:00:00+00:00 info Sorry, I'm busy this week Cecil"
                          [[tests.outputs]]
                            extract_from = "foo"
                            [[tests.outputs.conditions]]
                              type = "check_fields"
                              "message.equals" = "Sorry, I'm busy this week Cecil"
                    "#},
                    Some(Format::Toml),
                )
                .unwrap()
            ),
            Ok(())
        );

        assert_eq!(Some(PathBuf::from("/foobar")), config.global.data_dir);
        assert!(config.sources.contains_key("in"));
        assert!(config.sinks.contains_key("out"));
        assert!(config.transforms.contains_key("foo"));
        assert_eq!(config.tests.len(), 1);
    }

    #[test]
    fn config_append_collisions() {
        let mut config: ConfigBuilder = format::deserialize(
            indoc! {r#"
                [sources.in]
                  type = "file"
                  include = ["/var/log/messages"]

                [sinks.out]
                  type = "console"
                  inputs = ["in"]
                  encoding = "json"
            "#},
            Some(Format::Toml),
        )
        .unwrap();

        assert_eq!(
            config.append(
                format::deserialize(
                    indoc! {r#"
                        [sources.in]
                          type = "file"
                          include = ["/var/log/messages"]

                        [transforms.foo]
                          type = "json_parser"
                          inputs = [ "in" ]

                        [sinks.out]
                          type = "console"
                          inputs = ["in"]
                          encoding = "json"
                    "#},
                    Some(Format::Toml),
                )
                .unwrap()
            ),
            Err(vec![
                "duplicate source name found: in".into(),
                "duplicate sink name found: out".into(),
            ])
        );
    }
}

#[cfg(all(test, feature = "sources-stdin", feature = "sinks-console"))]
mod resource_tests {
    use super::{load_from_str, Format, Resource};
    use indoc::indoc;
    use std::collections::{HashMap, HashSet};
    use std::net::{Ipv4Addr, SocketAddr};

    fn localhost(port: u16) -> Resource {
        Resource::tcp(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port))
    }

    fn hashmap(conflicts: Vec<(Resource, Vec<&str>)>) -> HashMap<Resource, HashSet<&str>> {
        conflicts
            .into_iter()
            .map(|(key, values)| (key, values.into_iter().collect()))
            .collect()
    }

    #[test]
    fn valid() {
        let components = vec![
            ("sink_0", vec![localhost(0)]),
            ("sink_1", vec![localhost(1)]),
            ("sink_2", vec![localhost(2)]),
        ];
        let conflicting = Resource::conflicts(components);
        assert_eq!(conflicting, HashMap::new());
    }

    #[test]
    fn conflicting_pair() {
        let components = vec![
            ("sink_0", vec![localhost(0)]),
            ("sink_1", vec![localhost(2)]),
            ("sink_2", vec![localhost(2)]),
        ];
        let conflicting = Resource::conflicts(components);
        assert_eq!(
            conflicting,
            hashmap(vec![(localhost(2), vec!["sink_1", "sink_2"])])
        );
    }

    #[test]
    fn conflicting_multi() {
        let components = vec![
            ("sink_0", vec![localhost(0)]),
            ("sink_1", vec![localhost(2), localhost(0)]),
            ("sink_2", vec![localhost(2)]),
        ];
        let conflicting = Resource::conflicts(components);
        assert_eq!(
            conflicting,
            hashmap(vec![
                (localhost(0), vec!["sink_0", "sink_1"]),
                (localhost(2), vec!["sink_1", "sink_2"])
            ])
        );
    }

    #[test]
    fn different_network_interface() {
        let components = vec![
            ("sink_0", vec![localhost(0)]),
            (
                "sink_1",
                vec![Resource::tcp(SocketAddr::new(
                    Ipv4Addr::new(127, 0, 0, 2).into(),
                    0,
                ))],
            ),
        ];
        let conflicting = Resource::conflicts(components);
        assert_eq!(conflicting, HashMap::new());
    }

    #[test]
    fn unspecified_network_interface() {
        let components = vec![
            ("sink_0", vec![localhost(0)]),
            (
                "sink_1",
                vec![Resource::tcp(SocketAddr::new(
                    Ipv4Addr::UNSPECIFIED.into(),
                    0,
                ))],
            ),
        ];
        let conflicting = Resource::conflicts(components);
        assert_eq!(
            conflicting,
            hashmap(vec![(localhost(0), vec!["sink_0", "sink_1"])])
        );
    }

    #[test]
    fn different_protocol() {
        let components = vec![
            (
                "sink_0",
                vec![Resource::tcp(SocketAddr::new(
                    Ipv4Addr::LOCALHOST.into(),
                    0,
                ))],
            ),
            (
                "sink_1",
                vec![Resource::udp(SocketAddr::new(
                    Ipv4Addr::LOCALHOST.into(),
                    0,
                ))],
            ),
        ];
        let conflicting = Resource::conflicts(components);
        assert_eq!(conflicting, HashMap::new());
    }

    #[test]
    fn config_conflict_detected() {
        assert!(load_from_str(
            indoc! {r#"
                [sources.in0]
                  type = "stdin"

                [sources.in1]
                  type = "stdin"

                [sinks.out]
                  type = "console"
                  inputs = ["in0","in1"]
                  encoding = "json"
            "#},
            Some(Format::Toml),
        )
        .is_err());
    }
}
