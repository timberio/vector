use super::{TaskTransform, Transform};
use crate::{
    config::{DataType, GenerateConfig, GlobalOptions, TransformConfig, TransformDescription},
    event::Event,
    wasm::WasmModule,
};
use futures::{stream, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, future::ready, path::PathBuf, pin::Pin};
use vector_wasm::{Role, WasmModuleConfig};

pub mod defaults {
    pub(super) const HEAP_MEMORY_SIZE: usize = 16 * 64 * 1024 * 10; // 10MB
    pub const fn heap_memory_size() -> usize {
        HEAP_MEMORY_SIZE
    }
}

/// Transform specific information needed to construct a [`WasmModuleConfig`].
// Note: We have a separate type here for crate boundary purposes.
//       `WasmConfig` is in `vector-wasm`, so we can't do impl's on it here.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct WasmConfig {
    /// The location of the source WASM or WAT module.
    pub module: PathBuf,
    /// The location of the WASM artifact cache.
    pub artifact_cache: PathBuf,
    #[serde(default = "defaults::heap_memory_size")]
    pub heap_memory_size: usize,
    /// Options to be passed to the WASM module.
    #[serde(default)]
    pub options: HashMap<String, serde_json::Value>,
}

impl Into<WasmModuleConfig> for WasmConfig {
    fn into(self) -> WasmModuleConfig {
        WasmModuleConfig::new(
            Role::Transform,
            self.module,
            self.artifact_cache,
            self.options,
            defaults::HEAP_MEMORY_SIZE,
        )
    }
}

inventory::submit! {
    TransformDescription::new::<WasmConfig>("wasm")
}

impl GenerateConfig for WasmConfig {
    fn generate_config() -> toml::Value {
        toml::Value::try_from(Self {
            module: PathBuf::new(),
            artifact_cache: PathBuf::new(),
            heap_memory_size: defaults::HEAP_MEMORY_SIZE,
            options: HashMap::new(),
        })
        .unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "wasm")]
impl TransformConfig for WasmConfig {
    async fn build(&self, _globals: &GlobalOptions) -> crate::Result<Transform<Event>> {
        Ok(Transform::task(Wasm::new(self.clone())?))
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn transform_type(&self) -> &'static str {
        "wasm"
    }
}

#[derive(Debug)]
pub struct Wasm {
    module: WasmModule,
}

impl Wasm {
    pub fn new(config: WasmConfig) -> crate::Result<Self> {
        let module = WasmModule::build(config)?;

        Ok(Self { module })
    }
}

impl TaskTransform<Event> for Wasm {
    fn transform(
        self: Box<Self>,
        task: Pin<Box<dyn Stream<Item = Event> + Send>>,
    ) -> Pin<Box<dyn Stream<Item = Event> + Send>> {
        let mut inner = self;

        Box::pin(
            task.filter_map(move |event| {
                ready({
                    inner
                        .module
                        .process(event)
                        .map(|events| stream::iter(events))
                        .ok()
                })
            })
            .flatten(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Wasm, WasmConfig};
    use crate::{event::Event, transforms::TaskTransform};
    use futures::{stream, StreamExt};
    use serde_json::Value;
    use std::{collections::HashMap, fs, io::Read, path::Path};

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<WasmConfig>();
    }

    fn parse_config(s: &str) -> crate::Result<Box<Wasm>> {
        Wasm::new(toml::from_str(s).unwrap()).map(Box::new)
    }

    fn parse_event_artifact(path: impl AsRef<Path>) -> crate::Result<Option<Event>> {
        let mut event = Event::new_empty_log();
        let mut test_file = match fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => Err(e)?,
        };

        let mut buf = String::new();
        test_file.read_to_string(&mut buf)?;
        let test_json: HashMap<String, Value> = serde_json::from_str(&buf)?;

        for (key, value) in test_json {
            event.as_mut_log().insert(key, value.clone());
        }
        Ok(Some(event))
    }

    async fn test_config(config: &str, input: &str, output: &str) {
        let transform = parse_config(config).expect("could not init transform");

        let input = vec![parse_event_artifact(input)
            .expect("could not load input")
            .expect("input cannot be empty")];
        let expected: Vec<_> =
            std::iter::once(parse_event_artifact(output).expect("could not load output"))
                .filter_map(|e| e)
                .collect();

        let actual = transform
            .transform(Box::pin(stream::iter(input)))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Vec<_>>();

        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn protobuf_happy() {
        crate::test_util::trace_init();
        let span = span!(tracing::Level::TRACE, "transforms::wasm::protobuf::happy");
        let _enter = span.enter();

        let config = r#"
            module = "tests/data/wasm/protobuf/target/wasm32-wasi/release/protobuf.wasm"
            artifact_cache = "target/artifacts"
            "#;

        test_config(
            config,
            "tests/data/wasm/protobuf/fixtures/happy/input.json",
            "tests/data/wasm/protobuf/fixtures/happy/expected.json",
        )
        .await;
    }

    #[tokio::test]
    async fn protobuf_sad() {
        crate::test_util::trace_init();
        let span = span!(tracing::Level::TRACE, "transforms::wasm::protobuf::sad");
        let _enter = span.enter();

        let config = r#"
            module = "tests/data/wasm/protobuf/target/wasm32-wasi/release/protobuf.wasm"
            artifact_cache = "target/artifacts"
            "#;

        test_config(
            config,
            "tests/data/wasm/protobuf/fixtures/sad/input.json",
            "tests/data/wasm/protobuf/fixtures/sad/expected.json",
        )
        .await;
    }

    #[tokio::test]
    async fn add_fields() {
        crate::test_util::trace_init();
        let span = span!(tracing::Level::TRACE, "transforms::wasm::add_fields");
        let _enter = span.enter();

        let config = r#"
    module = "tests/data/wasm/add_fields/target/wasm32-wasi/release/add_fields.wasm"
    artifact_cache = "target/artifacts"
    options.new_field = "new_value"
    options.new_field_2 = "new_value_2"
            "#;

        test_config(
            config,
            "tests/data/wasm/add_fields/fixtures/a/input.json",
            "tests/data/wasm/add_fields/fixtures/a/expected.json",
        )
        .await;
    }

    #[tokio::test]
    async fn drop() {
        crate::test_util::trace_init();
        let span = span!(tracing::Level::TRACE, "transforms::wasm::drop");
        let _enter = span.enter();

        let config = r#"
    module = "tests/data/wasm/drop/target/wasm32-wasi/release/drop.wasm"
    artifact_cache = "target/artifacts"
            "#;

        test_config(
            config,
            "tests/data/wasm/drop/fixtures/a/input.json",
            "tests/data/wasm/drop/fixtures/a/expected.json",
        )
        .await;
    }

    #[tokio::test]
    async fn panic() {
        crate::test_util::trace_init();
        let span = span!(tracing::Level::TRACE, "transforms::wasm::panic");
        let _enter = span.enter();

        let config = r#"
    module = "tests/data/wasm/panic/target/wasm32-wasi/release/panic.wasm"
    artifact_cache = "target/artifacts"
            "#;

        test_config(
            config,
            "tests/data/wasm/panic/fixtures/a/input.json",
            "tests/data/wasm/panic/fixtures/a/expected.json",
        )
        .await;
    }

    #[tokio::test]
    async fn assert_config() {
        crate::test_util::trace_init();
        let span = span!(tracing::Level::TRACE, "transforms::wasm::assert_config");
        let _enter = span.enter();

        let config = r#"
    module = "tests/data/wasm/assert_config/target/wasm32-wasi/release/assert_config.wasm"
    artifact_cache = "target/artifacts"
    options.takes_string = "test"
    options.takes_number = 123
    options.takes_bool = true
    options.takes_array = [1, 2, 3]
    options.takes_map.one = "a"
    options.takes_map.two = "b"
            "#;

        test_config(
            config,
            "tests/data/wasm/assert_config/fixtures/a/input.json",
            "tests/data/wasm/assert_config/fixtures/a/expected.json",
        )
        .await;
    }

    #[tokio::test]
    async fn parse_syslog() {
        crate::test_util::trace_init();
        let span = span!(tracing::Level::TRACE, "transforms::wasm::parse_syslog");
        let _enter = span.enter();

        let config = r#"
            module = "tests/data/wasm/parse_syslog/target/wasm32-wasi/release/parse_syslog.wasm"
            artifact_cache = "target/artifacts"
            "#;

        test_config(
            config,
            "tests/data/wasm/parse_syslog/fixtures/a/input.json",
            "tests/data/wasm/parse_syslog/fixtures/a/expected.json",
        )
        .await;
    }

    #[tokio::test]
    async fn parse_json() {
        crate::test_util::trace_init();
        let span = span!(tracing::Level::TRACE, "transforms::wasm::parse_json");
        let _enter = span.enter();

        let config = r#"
            module = "tests/data/wasm/parse_json/target/wasm32-wasi/release/parse_json.wasm"
            artifact_cache = "target/artifacts"
            "#;

        test_config(
            config,
            "tests/data/wasm/parse_json/fixtures/a/input.json",
            "tests/data/wasm/parse_json/fixtures/a/expected.json",
        )
        .await;
    }

    #[tokio::test]
    async fn multifaceted() {
        crate::test_util::trace_init();
        let span = span!(tracing::Level::TRACE, "transforms::wasm::multifaceted");
        let _enter = span.enter();

        let config = r#"
            module = "tests/data/wasm/multifaceted/target/wasm32-wasi/release/multifaceted.wasm"
            artifact_cache = "target/artifacts"
            "#;

        test_config(
            config,
            "tests/data/wasm/multifaceted/fixtures/a/input.json",
            "tests/data/wasm/multifaceted/fixtures/a/expected.json",
        )
        .await;
    }
}
