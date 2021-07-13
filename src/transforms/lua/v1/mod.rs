use crate::transforms::TaskTransform;
use crate::{
    config::DataType,
    event::{Event, Value},
    internal_events::{LuaGcTriggered, LuaScriptError},
    transforms::Transform,
};
use futures::{stream, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use std::{future::ready, pin::Pin};

#[derive(Debug, Snafu)]
enum BuildError {
    #[snafu(display("Lua error: {}", source))]
    InvalidLua { source: mlua::Error },
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct LuaConfig {
    source: String,
    #[serde(default)]
    search_dirs: Vec<String>,
}

// Implementation of methods from `TransformConfig`
// Note that they are implemented as struct methods instead of trait implementation methods
// because `TransformConfig` trait requires specification of a unique `typetag::serde` name.
// Specifying some name (for example, "lua_v*") results in this name being listed among
// possible configuration options for `transforms` section, but such internal name should not
// be exposed to users.
impl LuaConfig {
    pub fn build(&self) -> crate::Result<Transform> {
        Lua::new(self.source.clone(), self.search_dirs.clone()).map(Transform::task)
    }

    pub fn input_type(&self) -> DataType {
        DataType::Log
    }

    pub fn output_type(&self) -> DataType {
        DataType::Log
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

#[derive(Derivative)]
#[derivative(Debug)]
pub struct Lua {
    #[derivative(Debug = "ignore")]
    source: String,
    #[derivative(Debug = "ignore")]
    search_dirs: Vec<String>,
    #[derivative(Debug = "ignore")]
    lua: mlua::Lua,
    vector_func: mlua::RegistryKey,
    invocations_after_gc: usize,
}

impl Clone for Lua {
    fn clone(&self) -> Self {
        Lua::new(self.source.clone(), self.search_dirs.clone())
            .expect("Tried to clone existing valid lua transform. This is an invariant.")
    }
}

// This wrapping structure is added in order to make it possible to have independent implementations
// of `mlua::UserData` trait for event in version 1 and version 2 of the transform.
#[derive(Clone)]
struct LuaEvent {
    inner: Event,
}

impl Lua {
    pub fn new(source: String, search_dirs: Vec<String>) -> crate::Result<Self> {
        // In order to support loading C modules in Lua, we need to create unsafe instance
        // without debug library.
        let lua = unsafe {
            mlua::Lua::unsafe_new_with(mlua::StdLib::ALL_SAFE, mlua::LuaOptions::default())
        };

        let additional_paths = search_dirs
            .iter()
            .map(|d| format!("{}/?.lua", d))
            .collect::<Vec<_>>()
            .join(";");

        if !additional_paths.is_empty() {
            let package = lua
                .globals()
                .get::<_, mlua::Table<'_>>("package")
                .context(InvalidLua)?;
            let current_paths = package
                .get::<_, String>("path")
                .unwrap_or_else(|_| ";".to_string());
            let paths = format!("{};{}", additional_paths, current_paths);
            package.set("path", paths).context(InvalidLua)?;
        }

        let func = lua.load(&source).into_function().context(InvalidLua)?;
        let vector_func = lua.create_registry_value(func).context(InvalidLua)?;

        Ok(Self {
            source,
            search_dirs,
            lua,
            vector_func,
            invocations_after_gc: 0,
        })
    }

    fn process(&mut self, event: Event) -> Result<Option<Event>, mlua::Error> {
        let lua = &self.lua;
        let globals = lua.globals();

        globals.raw_set("event", LuaEvent { inner: event })?;

        let func = lua.registry_value::<mlua::Function<'_>>(&self.vector_func)?;
        func.call(())?;

        let result = globals
            .raw_get::<_, Option<LuaEvent>>("event")
            .map(|option| option.map(|lua_event| lua_event.inner));

        self.invocations_after_gc += 1;
        if self.invocations_after_gc % GC_INTERVAL == 0 {
            emit!(LuaGcTriggered {
                used_memory: self.lua.used_memory()
            });
            self.lua.gc_collect()?;
            self.invocations_after_gc = 0;
        }

        result
    }

    pub fn transform_one(&mut self, event: Event) -> Option<Event> {
        match self.process(event) {
            Ok(event) => event,
            Err(error) => {
                emit!(LuaScriptError { error });
                None
            }
        }
    }
}

impl TaskTransform for Lua {
    fn transform(
        self: Box<Self>,
        task: Pin<Box<dyn Stream<Item = Event> + Send>>,
    ) -> Pin<Box<dyn Stream<Item = Event> + Send>>
    where
        Self: 'static,
    {
        let mut inner = self;
        Box::pin(
            task.filter_map(move |event| {
                let mut output = Vec::with_capacity(1);
                ready(match inner.process(event) {
                    Ok(event) => {
                        output.extend(event.into_iter());
                        Some(stream::iter(output))
                    }
                    Err(error) => {
                        emit!(LuaScriptError { error });
                        None
                    }
                })
            })
            .flatten(),
        )
    }
}

impl mlua::UserData for LuaEvent {
    fn add_methods<'lua, M: mlua::UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_meta_method_mut(
            mlua::MetaMethod::NewIndex,
            |_lua, this, (key, value): (String, Option<mlua::Value<'lua>>)| {
                match value {
                    Some(mlua::Value::String(string)) => {
                        this.inner.as_mut_log().insert(
                            key,
                            Value::from(string.to_str().expect("Expected UTF-8.").to_owned()),
                        );
                    }
                    Some(mlua::Value::Integer(integer)) => {
                        this.inner.as_mut_log().insert(key, Value::Integer(integer));
                    }
                    Some(mlua::Value::Number(number)) => {
                        this.inner.as_mut_log().insert(key, Value::Float(number));
                    }
                    Some(mlua::Value::Boolean(boolean)) => {
                        this.inner.as_mut_log().insert(key, Value::Boolean(boolean));
                    }
                    Some(mlua::Value::Nil) | None => {
                        this.inner.as_mut_log().remove(key);
                    }
                    _ => {
                        info!(
                            message =
                                "Could not set field to Lua value of invalid type, dropping field.",
                            field = key.as_str(),
                            internal_log_rate_secs = 30
                        );
                        this.inner.as_mut_log().remove(key);
                    }
                }

                Ok(())
            },
        );

        methods.add_meta_method(mlua::MetaMethod::Index, |lua, this, key: String| {
            if let Some(value) = this.inner.as_log().get(key) {
                let string = lua.create_string(&value.as_bytes())?;
                Ok(Some(string))
            } else {
                Ok(None)
            }
        });

        methods.add_meta_function(mlua::MetaMethod::Pairs, |lua, event: LuaEvent| {
            let state = lua.create_table()?;
            {
                let keys = lua.create_table_from(event.inner.as_log().keys().map(|k| (k, true)))?;
                state.raw_set("event", event)?;
                state.raw_set("keys", keys)?;
            }
            let function =
                lua.create_function(|lua, (state, prev): (mlua::Table, Option<String>)| {
                    let event: LuaEvent = state.raw_get("event")?;
                    let keys: mlua::Table = state.raw_get("keys")?;
                    let next: mlua::Function = lua.globals().raw_get("next")?;
                    let key: Option<String> = next.call((keys, prev))?;
                    match key.clone().and_then(|k| event.inner.as_log().get(k)) {
                        Some(value) => Ok((key, Some(lua.create_string(&value.as_bytes())?))),
                        None => Ok((None, None)),
                    }
                })?;
            Ok((function, state))
        });
    }
}

pub fn format_error(error: &mlua::Error) -> String {
    match error {
        mlua::Error::CallbackError { traceback, cause } => format_error(cause) + "\n" + traceback,
        err => err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, Value};

    #[test]
    fn lua_add_field() {
        crate::test_util::trace_init();
        let mut transform = Lua::new(
            r#"
              event["hello"] = "goodbye"
            "#
            .to_string(),
            vec![],
        )
        .unwrap();

        let event = Event::from("program me");

        let event = transform.transform_one(event).unwrap();

        assert_eq!(event.as_log()["hello"], "goodbye".into());
    }

    #[test]
    fn lua_read_field() {
        crate::test_util::trace_init();
        let mut transform = Lua::new(
            r#"
              _, _, name = string.find(event["message"], "Hello, my name is (%a+).")
              event["name"] = name
            "#
            .to_string(),
            vec![],
        )
        .unwrap();

        let event = Event::from("Hello, my name is Bob.");

        let event = transform.transform_one(event).unwrap();

        assert_eq!(event.as_log()["name"], "Bob".into());
    }

    #[test]
    fn lua_remove_field() {
        crate::test_util::trace_init();
        let mut transform = Lua::new(
            r#"
              event["name"] = nil
            "#
            .to_string(),
            vec![],
        )
        .unwrap();

        let mut event = Event::new_empty_log();
        event.as_mut_log().insert("name", "Bob");
        let event = transform.transform_one(event).unwrap();

        assert!(event.as_log().get("name").is_none());
    }

    #[test]
    fn lua_drop_event() {
        let mut transform = Lua::new(
            r#"
              event = nil
            "#
            .to_string(),
            vec![],
        )
        .unwrap();

        let mut event = Event::new_empty_log();
        event.as_mut_log().insert("name", "Bob");
        let event = transform.transform_one(event);

        assert!(event.is_none());
    }

    #[test]
    fn lua_read_empty_field() {
        crate::test_util::trace_init();
        let mut transform = Lua::new(
            r#"
              if event["non-existant"] == nil then
                event["result"] = "empty"
              else
                event["result"] = "found"
              end
            "#
            .to_string(),
            vec![],
        )
        .unwrap();

        let event = Event::new_empty_log();
        let event = transform.transform_one(event).unwrap();

        assert_eq!(event.as_log()["result"], "empty".into());
    }

    #[test]
    fn lua_integer_value() {
        crate::test_util::trace_init();
        let mut transform = Lua::new(
            r#"
              event["number"] = 3
            "#
            .to_string(),
            vec![],
        )
        .unwrap();

        let event = transform.transform_one(Event::new_empty_log()).unwrap();
        assert_eq!(event.as_log()["number"], Value::Integer(3));
    }

    #[test]
    fn lua_numeric_value() {
        crate::test_util::trace_init();
        let mut transform = Lua::new(
            r#"
              event["number"] = 3.14159
            "#
            .to_string(),
            vec![],
        )
        .unwrap();

        let event = transform.transform_one(Event::new_empty_log()).unwrap();
        assert_eq!(event.as_log()["number"], Value::Float(3.14159));
    }

    #[test]
    fn lua_boolean_value() {
        crate::test_util::trace_init();
        let mut transform = Lua::new(
            r#"
              event["bool"] = true
            "#
            .to_string(),
            vec![],
        )
        .unwrap();

        let event = transform.transform_one(Event::new_empty_log()).unwrap();
        assert_eq!(event.as_log()["bool"], Value::Boolean(true));
    }

    #[test]
    fn lua_non_coercible_value() {
        crate::test_util::trace_init();
        let mut transform = Lua::new(
            r#"
              event["junk"] = {"asdf"}
            "#
            .to_string(),
            vec![],
        )
        .unwrap();

        let event = transform.transform_one(Event::new_empty_log()).unwrap();
        assert_eq!(event.as_log().get("junk"), None);
    }

    #[test]
    fn lua_non_string_key_write() {
        crate::test_util::trace_init();
        let mut transform = Lua::new(
            r#"
              event[false] = "hello"
            "#
            .to_string(),
            vec![],
        )
        .unwrap();

        let err = transform.process(Event::new_empty_log()).unwrap_err();
        let err = format_error(&err);
        assert!(
            err.contains("error converting Lua boolean to String"),
            "{}",
            err
        );
    }

    #[test]
    fn lua_non_string_key_read() {
        crate::test_util::trace_init();
        let mut transform = Lua::new(
            r#"
              print(event[false])
            "#
            .to_string(),
            vec![],
        )
        .unwrap();

        let err = transform.process(Event::new_empty_log()).unwrap_err();
        let err = format_error(&err);
        assert!(
            err.contains("error converting Lua boolean to String"),
            "{}",
            err
        );
    }

    #[test]
    fn lua_script_error() {
        crate::test_util::trace_init();
        let mut transform = Lua::new(
            r#"
              error("this is an error")
            "#
            .to_string(),
            vec![],
        )
        .unwrap();

        let err = transform.process(Event::new_empty_log()).unwrap_err();
        let err = format_error(&err);
        assert!(err.contains("this is an error"), "{}", err);
    }

    #[test]
    fn lua_syntax_error() {
        crate::test_util::trace_init();
        let err = Lua::new(
            r#"
              1234 = sadf <>&*!#@
            "#
            .to_string(),
            vec![],
        )
        .map(|_| ())
        .unwrap_err()
        .to_string();

        assert!(err.contains("syntax error:"), "{}", err);
    }

    #[test]
    fn lua_load_file() {
        use std::fs::File;
        use std::io::Write;
        crate::test_util::trace_init();

        let dir = tempfile::tempdir().unwrap();

        let mut file = File::create(dir.path().join("script2.lua")).unwrap();
        write!(
            &mut file,
            r#"
              local M = {{}}

              local function modify(event2)
                event2["new field"] = "new value"
              end
              M.modify = modify

              return M
            "#
        )
        .unwrap();

        let source = r#"
          local script2 = require("script2")
          script2.modify(event)
        "#
        .to_string();

        let mut transform =
            Lua::new(source, vec![dir.path().to_string_lossy().into_owned()]).unwrap();
        let event = Event::new_empty_log();
        let event = transform.transform_one(event).unwrap();

        assert_eq!(event.as_log()["new field"], "new value".into());
    }

    #[test]
    fn lua_pairs() {
        crate::test_util::trace_init();
        let mut transform = Lua::new(
            r#"
              for k,v in pairs(event) do
                event[k] = k .. v
              end
            "#
            .to_string(),
            vec![],
        )
        .unwrap();

        let mut event = Event::new_empty_log();
        event.as_mut_log().insert("name", "Bob");
        event.as_mut_log().insert("friend", "Alice");

        let event = transform.transform_one(event).unwrap();

        assert_eq!(event.as_log()["name"], "nameBob".into());
        assert_eq!(event.as_log()["friend"], "friendAlice".into());
    }
}
