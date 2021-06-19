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
    InvalidSearchDirs { source: rlua::Error },
    #[snafu(display("Cannot evaluate Lua code in \"source\": {}", source))]
    InvalidSource { source: rlua::Error },

    #[snafu(display("Cannot evaluate Lua code defining \"hooks.init\": {}", source))]
    InvalidHooksInit { source: rlua::Error },
    #[snafu(display("Cannot evaluate Lua code defining \"hooks.process\": {}", source))]
    InvalidHooksProcess { source: rlua::Error },
    #[snafu(display("Cannot evaluate Lua code defining \"hooks.shutdown\": {}", source))]
    InvalidHooksShutdown { source: rlua::Error },
    #[snafu(display("Cannot evaluate Lua code defining timer handler: {}", source))]
    InvalidTimerHandler { source: rlua::Error },

    #[snafu(display("Runtime error in \"hooks.init\" function: {}", source))]
    RuntimeErrorHooksInit { source: rlua::Error },
    #[snafu(display("Runtime error in \"hooks.process\" function: {}", source))]
    RuntimeErrorHooksProcess { source: rlua::Error },
    #[snafu(display("Runtime error in \"hooks.shutdown\" function: {}", source))]
    RuntimeErrorHooksShutdown { source: rlua::Error },
    #[snafu(display("Runtime error in timer handler: {}", source))]
    RuntimeErrorTimerHandler { source: rlua::Error },

    #[snafu(display("Cannot call GC in Lua runtime: {}", source))]
    RuntimeErrorGc { source: rlua::Error },
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
    pub fn build(&self) -> crate::Result<Transform<Event>> {
        Lua::new(&self).map(Transform::task)
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
    lua: rlua::Lua,
    invocations_after_gc: usize,
    timers: Vec<Timer>,
}

impl Lua {
    pub fn new(config: &LuaConfig) -> crate::Result<Self> {
        let lua = rlua::Lua::new();

        let additional_paths = config
            .search_dirs
            .iter()
            .map(|d| format!("{}/?.lua", d.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(";");

        let mut timers = Vec::new();
        lua.context(|ctx| -> crate::Result<()> {
            if !additional_paths.is_empty() {
                let package = ctx.globals().get::<_, rlua::Table<'_>>("package")?;
                let current_paths = package
                    .get::<_, String>("path")
                    .unwrap_or_else(|_| ";".to_string());
                let paths = format!("{};{}", additional_paths, current_paths);
                package.set("path", paths)?;
            }

            if let Some(source) = &config.source {
                ctx.load(source).eval().context(InvalidSource)?;
            }

            if let Some(hooks_init) = &config.hooks.init {
                let hooks_init: rlua::Function<'_> =
                    ctx.load(hooks_init).eval().context(InvalidHooksInit)?;
                ctx.set_named_registry_value("hooks_init", Some(hooks_init))?;
            }

            let hooks_process: rlua::Function<'_> = ctx
                .load(&config.hooks.process)
                .eval()
                .context(InvalidHooksProcess)?;
            ctx.set_named_registry_value("hooks_process", hooks_process)?;

            if let Some(hooks_shutdown) = &config.hooks.shutdown {
                let hooks_shutdown: rlua::Function<'_> = ctx
                    .load(hooks_shutdown)
                    .eval()
                    .context(InvalidHooksShutdown)?;
                ctx.set_named_registry_value("hooks_shutdown", Some(hooks_shutdown))?;
            }

            for (id, timer) in config.timers.iter().enumerate() {
                let handler: rlua::Function<'_> = ctx
                    .load(&timer.handler)
                    .eval()
                    .context(InvalidTimerHandler)?;

                ctx.set_named_registry_value(&format!("timer_handler_{}", id), handler)?;
                timers.push(Timer {
                    id: id as u32,
                    interval_seconds: timer.interval_seconds,
                });
            }

            Ok(())
        })?;

        Ok(Self {
            lua,
            invocations_after_gc: 0,
            timers,
        })
    }

    #[cfg(test)]
    fn process(&mut self, event: Event, output: &mut Vec<Event>) -> Result<(), rlua::Error> {
        let result = self.lua.context(|ctx: rlua::Context<'_>| {
            ctx.scope(|scope| {
                let emit = scope.create_function_mut(|_, event: Event| {
                    output.push(event);
                    Ok(())
                })?;
                let process = ctx.named_registry_value::<_, rlua::Function<'_>>("hooks_process")?;

                process.call((event, emit))
            })
        });

        self.attempt_gc();
        result
    }

    #[cfg(test)]
    fn process_single(&mut self, event: Event) -> Result<Option<Event>, rlua::Error> {
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
    scope: &rlua::Scope<'lua, 'scope>,
    mut emit_fn: F,
) -> rlua::Result<rlua::Function<'lua>>
where
    F: FnMut(Event),
{
    scope.create_function_mut(move |_, event: Event| -> rlua::Result<()> {
        emit_fn(event);
        Ok(())
    })
}

impl RuntimeTransform for Lua {
    fn hook_process<F>(&mut self, event: Event, emit_fn: F)
    where
        F: FnMut(Event),
    {
        let _ = self
            .lua
            .context(|ctx: rlua::Context<'_>| {
                ctx.scope(|scope| -> rlua::Result<()> {
                    let process =
                        ctx.named_registry_value::<_, rlua::Function<'_>>("hooks_process")?;
                    process.call((event, wrap_emit_fn(&scope, emit_fn)?))
                })
            })
            .context(RuntimeErrorHooksProcess)
            .map_err(|e| emit!(LuaBuildError { error: e }));

        self.attempt_gc();
    }

    fn hook_init<F>(&mut self, emit_fn: F)
    where
        F: FnMut(Event),
    {
        let _ = self
            .lua
            .context(|ctx: rlua::Context<'_>| {
                ctx.scope(|scope| -> rlua::Result<()> {
                    match ctx.named_registry_value::<_, Option<rlua::Function<'_>>>("hooks_init")? {
                        Some(init) => init.call((wrap_emit_fn(&scope, emit_fn)?,)),
                        None => Ok(()),
                    }
                })
            })
            .context(RuntimeErrorHooksInit)
            .map_err(|error| error!(%error, rate_limit = 30));

        self.attempt_gc();
    }

    fn hook_shutdown<F>(&mut self, emit_fn: F)
    where
        F: FnMut(Event),
    {
        let _ = self
            .lua
            .context(|ctx: rlua::Context<'_>| {
                ctx.scope(|scope| -> rlua::Result<()> {
                    match ctx
                        .named_registry_value::<_, Option<rlua::Function<'_>>>("hooks_shutdown")?
                    {
                        Some(shutdown) => shutdown.call((wrap_emit_fn(&scope, emit_fn)?,)),
                        None => Ok(()),
                    }
                })
            })
            .context(RuntimeErrorHooksInit)
            .map_err(|error| error!(%error, rate_limit = 30));

        self.attempt_gc();
    }

    fn timer_handler<F>(&mut self, timer: Timer, emit_fn: F)
    where
        F: FnMut(Event),
    {
        let _ = self
            .lua
            .context(|ctx: rlua::Context<'_>| {
                ctx.scope(|scope| -> rlua::Result<()> {
                    let handler_name = format!("timer_handler_{}", timer.id);
                    let handler =
                        ctx.named_registry_value::<_, rlua::Function<'_>>(&handler_name)?;

                    handler.call((wrap_emit_fn(&scope, emit_fn)?,))
                })
            })
            .context(RuntimeErrorTimerHandler)
            .map_err(|error| error!(%error, rate_limit = 30));

        self.attempt_gc();
    }

    fn timers(&self) -> Vec<Timer> {
        self.timers.clone()
    }
}

#[cfg(test)]
fn format_error(error: &rlua::Error) -> String {
    match error {
        rlua::Error::CallbackError { traceback, cause } => format_error(&cause) + "\n" + traceback,
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
