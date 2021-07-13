use crate::{
    config::{self, DataType, CONFIG_PATHS},
    event::Event,
    internal_events::{LuaBuildError, LuaGcTriggered},
    transforms::Transform,
};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use std::path::PathBuf;
pub use vector_core::event::lua;
use vector_core::transform::runtime_transform::{RuntimeTransform, Timer};

#[derive(Debug, Snafu)]
pub enum BuildError {
    #[snafu(display("Invalid \"search_dirs\": {}", source))]
    InvalidSearchDirs { source: mlua::Error },
    #[snafu(display("Cannot evaluate Lua code in \"source\": {}", source))]
    InvalidSource { source: mlua::Error },

    #[snafu(display("Cannot evaluate Lua code defining \"hooks.init\": {}", source))]
    InvalidHooksInit { source: mlua::Error },
    #[snafu(display("Cannot evaluate Lua code defining \"hooks.process\": {}", source))]
    InvalidHooksProcess { source: mlua::Error },
    #[snafu(display("Cannot evaluate Lua code defining \"hooks.shutdown\": {}", source))]
    InvalidHooksShutdown { source: mlua::Error },
    #[snafu(display("Cannot evaluate Lua code defining timer handler: {}", source))]
    InvalidTimerHandler { source: mlua::Error },

    #[snafu(display("Runtime error in \"hooks.init\" function: {}", source))]
    RuntimeErrorHooksInit { source: mlua::Error },
    #[snafu(display("Runtime error in \"hooks.process\" function: {}", source))]
    RuntimeErrorHooksProcess { source: mlua::Error },
    #[snafu(display("Runtime error in \"hooks.shutdown\" function: {}", source))]
    RuntimeErrorHooksShutdown { source: mlua::Error },
    #[snafu(display("Runtime error in timer handler: {}", source))]
    RuntimeErrorTimerHandler { source: mlua::Error },

    #[snafu(display("Cannot call GC in Lua runtime: {}", source))]
    RuntimeErrorGc { source: mlua::Error },
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct LuaConfig {
    #[serde(default = "default_config_paths")]
    search_dirs: Vec<PathBuf>,
    hooks: HooksConfig,
    #[serde(default)]
    timers: Vec<TimerConfig>,
    source: Option<String>,
}

fn default_config_paths() -> Vec<PathBuf> {
    match CONFIG_PATHS.lock().ok() {
        Some(config_paths) => config_paths
            .clone()
            .into_iter()
            .map(|config_path| match config_path {
                config::ConfigPath::File(mut path, _format) => {
                    path.pop();
                    path
                }
                config::ConfigPath::Dir(path) => path,
            })
            .collect(),
        None => vec![],
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
struct HooksConfig {
    init: Option<String>,
    process: String,
    shutdown: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
struct TimerConfig {
    interval_seconds: u64,
    handler: String,
}

// Implementation of methods from `TransformConfig`
// Note that they are implemented as struct methods instead of trait implementation methods
// because `TransformConfig` trait requires specification of a unique `typetag::serde` name.
// Specifying some name (for example, "lua_v*") results in this name being listed among
// possible configuration options for `transforms` section, but such internal name should not
// be exposed to users.
impl LuaConfig {
    pub fn build(&self) -> crate::Result<Transform> {
        Lua::new(self).map(Transform::task)
    }

    pub fn input_type(&self) -> DataType {
        DataType::Any
    }

    pub fn output_type(&self) -> DataType {
        DataType::Any
    }

    pub fn transform_type(&self) -> &'static str {
        "lua"
    }
}

// Lua's garbage collector sometimes seems to be not executed automatically on high event rates,
// which leads to leak-like RAM consumption pattern. This constant sets the number of invocations of
// the Lua transform after which GC would be called, thus ensuring that the RAM usage is not too high.
//
// This constant is larger than 1 because calling GC is an expensive operation, so doing it
// after each transform would have significant footprint on the performance.
const GC_INTERVAL: usize = 16;

pub struct Lua {
    lua: mlua::Lua,
    invocations_after_gc: usize,
    hook_init: Option<mlua::RegistryKey>,
    hook_process: mlua::RegistryKey,
    hook_shutdown: Option<mlua::RegistryKey>,
    timers: Vec<(Timer, mlua::RegistryKey)>,
}

// Helper to create `RegistryKey` from Lua function code
fn make_registry_value(lua: &mlua::Lua, source: &str) -> mlua::Result<mlua::RegistryKey> {
    lua.load(source)
        .eval::<mlua::Function>()
        .and_then(|f| lua.create_registry_value(f))
}

impl Lua {
    pub fn new(config: &LuaConfig) -> crate::Result<Self> {
        // In order to support loading C modules in Lua, we need to create unsafe instance
        // without debug library.
        let lua = unsafe {
            mlua::Lua::unsafe_new_with(mlua::StdLib::ALL_SAFE, mlua::LuaOptions::default())
        };

        let additional_paths = config
            .search_dirs
            .iter()
            .map(|d| format!("{}/?.lua", d.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(";");

        let mut timers = Vec::new();

        if !additional_paths.is_empty() {
            let package = lua.globals().get::<_, mlua::Table<'_>>("package")?;
            let current_paths = package
                .get::<_, String>("path")
                .unwrap_or_else(|_| ";".to_string());
            let paths = format!("{};{}", additional_paths, current_paths);
            package.set("path", paths)?;
        }

        if let Some(source) = &config.source {
            lua.load(source).eval().context(InvalidSource)?;
        }

        let hook_init_code = config.hooks.init.as_ref();
        let hook_init = hook_init_code
            .map(|code| make_registry_value(&lua, code))
            .transpose()
            .context(InvalidHooksInit)?;

        let hook_process =
            make_registry_value(&lua, &config.hooks.process).context(InvalidHooksProcess)?;

        let hook_shutdown_code = config.hooks.shutdown.as_ref();
        let hook_shutdown = hook_shutdown_code
            .map(|code| make_registry_value(&lua, &code))
            .transpose()
            .context(InvalidHooksShutdown)?;

        for (id, timer) in config.timers.iter().enumerate() {
            let handler_key = lua
                .load(&timer.handler)
                .eval::<mlua::Function>()
                .and_then(|f| lua.create_registry_value(f))
                .context(InvalidTimerHandler)?;

            let timer = Timer {
                id: id as u32,
                interval_seconds: timer.interval_seconds,
            };
            timers.push((timer, handler_key));
        }

        Ok(Self {
            lua,
            invocations_after_gc: 0,
            timers,
            hook_init,
            hook_process,
            hook_shutdown,
        })
    }

    #[cfg(test)]
    fn process(&mut self, event: Event, output: &mut Vec<Event>) -> Result<(), mlua::Error> {
        let lua = &self.lua;
        let result = lua.scope(|scope| {
            let emit = scope.create_function_mut(|_, event: Event| {
                output.push(event);
                Ok(())
            })?;

            lua.registry_value::<mlua::Function>(&self.hook_process)?
                .call((event, emit))
        });

        self.attempt_gc();
        result
    }

    #[cfg(test)]
    fn process_single(&mut self, event: Event) -> Result<Option<Event>, mlua::Error> {
        let mut out = Vec::new();
        self.process(event, &mut out)?;
        assert!(out.len() <= 1);
        Ok(out.into_iter().next())
    }

    fn attempt_gc(&mut self) {
        self.invocations_after_gc += 1;
        if self.invocations_after_gc % GC_INTERVAL == 0 {
            emit!(LuaGcTriggered {
                used_memory: self.lua.used_memory()
            });
            let _ = self
                .lua
                .gc_collect()
                .context(RuntimeErrorGc)
                .map_err(|error| error!(%error, rate_limit = 30));
            self.invocations_after_gc = 0;
        }
    }
}

// A helper that reduces code duplication.
fn wrap_emit_fn<'lua, 'scope, F: 'scope>(
    scope: &mlua::Scope<'lua, 'scope>,
    mut emit_fn: F,
) -> mlua::Result<mlua::Function<'lua>>
where
    F: FnMut(Event),
{
    scope.create_function_mut(move |_, event: Event| -> mlua::Result<()> {
        emit_fn(event);
        Ok(())
    })
}

impl RuntimeTransform for Lua {
    fn hook_process<F>(&mut self, event: Event, emit_fn: F)
    where
        F: FnMut(Event),
    {
        let lua = &self.lua;
        let _ = lua
            .scope(|scope| -> mlua::Result<()> {
                lua.registry_value::<mlua::Function>(&self.hook_process)?
                    .call((event, wrap_emit_fn(scope, emit_fn)?))
            })
            .context(RuntimeErrorHooksProcess)
            .map_err(|e| emit!(LuaBuildError { error: e }));

        self.attempt_gc();
    }

    fn hook_init<F>(&mut self, emit_fn: F)
    where
        F: FnMut(Event),
    {
        let lua = &self.lua;
        let _ = lua
            .scope(|scope| -> mlua::Result<()> {
                match &self.hook_init {
                    Some(key) => lua
                        .registry_value::<mlua::Function>(key)?
                        .call(wrap_emit_fn(scope, emit_fn)?),
                    None => Ok(()),
                }
            })
            .context(RuntimeErrorHooksInit)
            .map_err(|error| error!(%error, rate_limit = 30));

        self.attempt_gc();
    }

    fn hook_shutdown<F>(&mut self, emit_fn: F)
    where
        F: FnMut(Event),
    {
        let lua = &self.lua;
        let _ = lua
            .scope(|scope| -> mlua::Result<()> {
                match &self.hook_shutdown {
                    Some(key) => lua
                        .registry_value::<mlua::Function>(key)?
                        .call(wrap_emit_fn(scope, emit_fn)?),
                    None => Ok(()),
                }
            })
            .context(RuntimeErrorHooksShutdown)
            .map_err(|error| error!(%error, rate_limit = 30));

        self.attempt_gc();
    }

    fn timer_handler<F>(&mut self, timer: Timer, emit_fn: F)
    where
        F: FnMut(Event),
    {
        let lua = &self.lua;
        let _ = lua
            .scope(|scope| -> mlua::Result<()> {
                let handler_key = &self.timers[timer.id as usize].1;
                lua.registry_value::<mlua::Function>(handler_key)?
                    .call(wrap_emit_fn(scope, emit_fn)?)
            })
            .context(RuntimeErrorTimerHandler)
            .map_err(|error| error!(%error, rate_limit = 30));

        self.attempt_gc();
    }

    fn timers(&self) -> Vec<Timer> {
        self.timers.iter().map(|(timer, _)| *timer).collect()
    }
}

#[cfg(test)]
fn format_error(error: &mlua::Error) -> String {
    match error {
        mlua::Error::CallbackError { traceback, cause } => format_error(cause) + "\n" + traceback,
        err => err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        event::{
            metric::{Metric, MetricKind, MetricValue},
            Event, Value,
        },
        test_util::trace_init,
        transforms::TaskTransform,
    };
    use futures::{stream, StreamExt};

    fn from_config(config: &str) -> crate::Result<Box<Lua>> {
        Lua::new(&toml::from_str(config).unwrap()).map(Box::new)
    }

    #[tokio::test]
    async fn lua_add_field() -> crate::Result<()> {
        trace_init();

        let transform = from_config(
            r#"
            hooks.process = """function (event, emit)
                event["log"]["hello"] = "goodbye"
                emit(event)
            end
            """
            "#,
        )
        .unwrap();

        let event = Event::from("program me");
        let in_stream = Box::pin(stream::iter(vec![event]));
        let mut out_stream = transform.transform(in_stream);
        let output = out_stream.next().await.unwrap();

        assert_eq!(output.as_log()["hello"], "goodbye".into());
        Ok(())
    }

    #[tokio::test]
    async fn lua_read_field() -> crate::Result<()> {
        trace_init();

        let transform = from_config(
            r#"
            hooks.process = """function (event, emit)
                _, _, name = string.find(event.log.message, "Hello, my name is (%a+).")
                event.log.name = name
                emit(event)
            end
            """
            "#,
        )
        .unwrap();

        let event = Event::from("Hello, my name is Bob.");
        let in_stream = Box::pin(stream::iter(vec![event]));
        let mut out_stream = transform.transform(in_stream);
        let output = out_stream.next().await.unwrap();

        assert_eq!(output.as_log()["name"], "Bob".into());
        Ok(())
    }

    #[tokio::test]
    async fn lua_remove_field() -> crate::Result<()> {
        trace_init();

        let transform = from_config(
            r#"
            hooks.process = """function (event, emit)
                event.log.name = nil
                emit(event)
            end
            """
            "#,
        )
        .unwrap();

        let mut event = Event::new_empty_log();
        event.as_mut_log().insert("name", "Bob");

        let in_stream = Box::pin(stream::iter(vec![event]));
        let mut out_stream = transform.transform(in_stream);
        let output = out_stream.next().await.unwrap();

        assert!(output.as_log().get("name").is_none());
        Ok(())
    }

    #[tokio::test]
    async fn lua_drop_event() -> crate::Result<()> {
        trace_init();

        let transform = from_config(
            r#"
            hooks.process = """function (event, emit)
                -- emit nothing
            end
            """
            "#,
        )
        .unwrap();

        let event = Event::new_empty_log();
        let in_stream = Box::pin(stream::iter(vec![event]));
        let mut out_stream = transform.transform(in_stream);
        let output = out_stream.next().await;

        assert!(output.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn lua_duplicate_event() -> crate::Result<()> {
        trace_init();

        let transform = from_config(
            r#"
            hooks.process = """function (event, emit)
                emit(event)
                emit(event)
            end
            """
            "#,
        )
        .unwrap();

        let mut event = Event::new_empty_log();
        event.as_mut_log().insert("host", "127.0.0.1");
        let input = Box::pin(stream::iter(vec![event]));
        let output = transform.transform(input);
        let out = output.collect::<Vec<_>>().await;

        assert_eq!(out.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn lua_read_empty_field() -> crate::Result<()> {
        trace_init();

        let transform = from_config(
            r#"
            hooks.process = """function (event, emit)
                if event["log"]["non-existant"] == nil then
                  event["log"]["result"] = "empty"
                else
                  event["log"]["result"] = "found"
                end
                emit(event)
            end
            """
            "#,
        )
        .unwrap();

        let event = Event::new_empty_log();

        let in_stream = Box::pin(stream::iter(vec![event]));
        let mut out_stream = transform.transform(in_stream);
        let output = out_stream.next().await.unwrap();

        assert_eq!(output.as_log()["result"], "empty".into());
        Ok(())
    }

    #[tokio::test]
    async fn lua_integer_value() -> crate::Result<()> {
        trace_init();

        let transform = from_config(
            r#"
            hooks.process = """function (event, emit)
                event["log"]["number"] = 3
                emit(event)
            end
            """
            "#,
        )
        .unwrap();

        let event = Event::new_empty_log();
        let in_stream = Box::pin(stream::iter(vec![event]));
        let mut out_stream = transform.transform(in_stream);
        let output = out_stream.next().await.unwrap();

        assert_eq!(output.as_log()["number"], Value::Integer(3));
        Ok(())
    }

    #[tokio::test]
    async fn lua_numeric_value() -> crate::Result<()> {
        trace_init();

        let transform = from_config(
            r#"
            hooks.process = """function (event, emit)
                event["log"]["number"] = 3.14159
                emit(event)
            end
            """
            "#,
        )
        .unwrap();

        let event = Event::new_empty_log();
        let in_stream = Box::pin(stream::iter(vec![event]));
        let mut out_stream = transform.transform(in_stream);
        let output = out_stream.next().await.unwrap();

        assert_eq!(output.as_log()["number"], Value::Float(3.14159));
        Ok(())
    }

    #[tokio::test]
    async fn lua_boolean_value() -> crate::Result<()> {
        trace_init();

        let transform = from_config(
            r#"
            hooks.process = """function (event, emit)
                event["log"]["bool"] = true
                emit(event)
            end
            """
            "#,
        )
        .unwrap();

        let event = Event::new_empty_log();
        let in_stream = Box::pin(stream::iter(vec![event]));
        let mut out_stream = transform.transform(in_stream);
        let output = out_stream.next().await.unwrap();

        assert_eq!(output.as_log()["bool"], Value::Boolean(true));
        Ok(())
    }

    #[tokio::test]
    async fn lua_non_coercible_value() -> crate::Result<()> {
        trace_init();

        let transform = from_config(
            r#"
            hooks.process = """function (event, emit)
                event["log"]["junk"] = nil
                emit(event)
            end
            """
            "#,
        )
        .unwrap();

        let event = Event::new_empty_log();
        let in_stream = Box::pin(stream::iter(vec![event]));
        let mut out_stream = transform.transform(in_stream);
        let output = out_stream.next().await.unwrap();

        assert_eq!(output.as_log().get("junk"), None);
        Ok(())
    }

    #[tokio::test]
    async fn lua_non_string_key_write() -> crate::Result<()> {
        trace_init();

        let mut transform = from_config(
            r#"
            hooks.process = """function (event, emit)
                event["log"][false] = "hello"
                emit(event)
            end
            """
            "#,
        )
        .unwrap();

        let err = transform
            .process_single(Event::new_empty_log())
            .unwrap_err();
        let err = format_error(&err);
        assert!(
            err.contains("error converting Lua boolean to String"),
            "{}",
            err
        );
        Ok(())
    }

    #[tokio::test]
    async fn lua_non_string_key_read() -> crate::Result<()> {
        trace_init();

        let transform = from_config(
            r#"
            hooks.process = """function (event, emit)
                event.log.result = event.log[false]
                emit(event)
            end
            """
            "#,
        )
        .unwrap();

        let event = Event::new_empty_log();
        let in_stream = Box::pin(stream::iter(vec![event]));
        let mut out_stream = transform.transform(in_stream);
        let output = out_stream.next().await.unwrap();
        assert_eq!(output.as_log().get("result"), None);
        Ok(())
    }

    #[tokio::test]
    async fn lua_script_error() -> crate::Result<()> {
        trace_init();

        let mut transform = from_config(
            r#"
            hooks.process = """function (event, emit)
                error("this is an error")
            end
            """
            "#,
        )
        .unwrap();

        let err = transform
            .process_single(Event::new_empty_log())
            .unwrap_err();
        let err = format_error(&err);
        assert!(err.contains("this is an error"), "{}", err);
        Ok(())
    }

    #[tokio::test]
    async fn lua_syntax_error() -> crate::Result<()> {
        trace_init();

        let err = from_config(
            r#"
            hooks.process = """function (event, emit)
                1234 = sadf <>&*!#@
            end
            """
            "#,
        )
        .map(|_| ())
        .unwrap_err()
        .to_string();

        assert!(err.contains("syntax error:"), "{}", err);
        Ok(())
    }

    #[tokio::test]
    async fn lua_load_file() -> crate::Result<()> {
        use std::fs::File;
        use std::io::Write;
        trace_init();

        let dir = tempfile::tempdir().unwrap();
        let mut file = File::create(dir.path().join("script2.lua")).unwrap();
        write!(
            &mut file,
            r#"
            local M = {{}}

            local function modify(event2)
              event2["log"]["new field"] = "new value"
            end
            M.modify = modify

            return M
            "#
        )
        .unwrap();

        let config = format!(
            r#"
            hooks.process = """function (event, emit)
                local script2 = require("script2")
                script2.modify(event)
                emit(event)
            end
            """
            search_dirs = [{:?}]
            "#,
            dir.path().as_os_str() // This seems a bit weird, but recall we also support windows.
        );
        let transform = from_config(&config).unwrap();

        let event = Event::new_empty_log();
        let in_stream = Box::pin(stream::iter(vec![event]));
        let mut out_stream = transform.transform(in_stream);
        let output = out_stream.next().await.unwrap();

        assert_eq!(output.as_log()["new field"], "new value".into());
        Ok(())
    }

    #[tokio::test]
    async fn lua_pairs() -> crate::Result<()> {
        trace_init();

        let transform = from_config(
            r#"
            hooks.process = """function (event, emit)
                for k,v in pairs(event.log) do
                  event.log[k] = k .. v
                end
                emit(event)
            end
            """
            "#,
        )
        .unwrap();

        let mut event = Event::new_empty_log();
        event.as_mut_log().insert("name", "Bob");
        event.as_mut_log().insert("friend", "Alice");

        let in_stream = Box::pin(stream::iter(vec![event]));
        let mut out_stream = transform.transform(in_stream);
        let output = out_stream.next().await.unwrap();

        assert_eq!(output.as_log()["name"], "nameBob".into());
        assert_eq!(output.as_log()["friend"], "friendAlice".into());
        Ok(())
    }

    #[tokio::test]
    async fn lua_metric() -> crate::Result<()> {
        trace_init();

        let transform = from_config(
            r#"
            hooks.process = """function (event, emit)
                event.metric.counter.value = event.metric.counter.value + 1
                emit(event)
            end
            """
            "#,
        )
        .unwrap();

        let metric = Metric::new(
            "example counter",
            MetricKind::Absolute,
            MetricValue::Counter { value: 1.0 },
        );

        let expected = metric
            .clone()
            .with_value(MetricValue::Counter { value: 2.0 });

        let in_stream = Box::pin(stream::iter(vec![metric.into()]));
        let mut out_stream = transform.transform(in_stream);
        let output = out_stream.next().await.unwrap();

        assert_eq!(output, expected.into());
        Ok(())
    }

    #[tokio::test]
    async fn lua_multiple_events() -> crate::Result<()> {
        trace_init();

        let transform = from_config(
            r#"
            hooks.process = """function (event, emit)
                event["log"]["hello"] = "goodbye"
                emit(event)
            end
            """
            "#,
        )
        .unwrap();

        let n: usize = 10;

        let events = (0..n).map(|i| Event::from(format!("program me {}", i)));

        let in_stream = Box::pin(stream::iter(events));
        let out_stream = transform.transform(in_stream);
        let output = out_stream.collect::<Vec<_>>().await;

        assert_eq!(output.len(), n);
        Ok(())
    }
}
