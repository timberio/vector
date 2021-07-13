use crate::event::{LogEvent, Value};
use mlua::prelude::*;

impl<'a> ToLua<'a> for LogEvent {
    #![allow(clippy::wrong_self_convention)] // this trait is defined by mlua
    fn to_lua(self, lua: &'a Lua) -> LuaResult<LuaValue> {
        let (fields, _metadata) = self.into_parts();
        // The metadata is handled when converting the enclosing `Event`.
        lua.create_table_from(fields).map(LuaValue::Table)
    }
}

impl<'a> FromLua<'a> for LogEvent {
    fn from_lua(value: LuaValue<'a>, _: &'a Lua) -> LuaResult<Self> {
        match value {
            LuaValue::Table(t) => {
                let mut log = LogEvent::default();
                for pair in t.pairs() {
                    let (key, value): (String, Value) = pair?;
                    log.insert_flat(key, value);
                }
                Ok(log)
            }
            _ => Err(mlua::Error::FromLuaConversionError {
                from: value.type_name(),
                to: "LogEvent",
                message: Some("LogEvent should ba a Lua table".to_string()),
            }),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::event::Event;

    #[test]
    fn to_lua() {
        let mut event = Event::new_empty_log();
        let log = event.as_mut_log();
        log.insert("a", 1);
        log.insert("nested.field", "2");
        log.insert("nested.array[0]", "example value");
        log.insert("nested.array[2]", "another value");

        let assertions = vec![
            "type(log) == 'table'",
            "log.a == 1",
            "type(log.nested) == 'table'",
            "log.nested.field == '2'",
            "#log.nested.array == 3",
            "log.nested.array[1] == 'example value'",
            "log.nested.array[2] == ''",
            "log.nested.array[3] == 'another value'",
        ];

        let lua = Lua::new();
        lua.globals().set("log", log.clone()).unwrap();
        for assertion in assertions {
            let result: bool = lua
                .load(assertion)
                .eval()
                .unwrap_or_else(|_| panic!("Failed to verify assertion {:?}", assertion));
            assert!(result, "{}", assertion);
        }
    }

    #[test]
    fn from_lua() {
        let lua_event = r#"
        {
            a = 1,
            nested = {
                field = '2',
                array = {'example value', '', 'another value'}
            }
        }
        "#;

        let event: LogEvent = Lua::new().load(lua_event).eval().unwrap();

        assert_eq!(event["a"], Value::Integer(1));
        assert_eq!(event["nested.field"], Value::Bytes("2".into()));
        assert_eq!(
            event["nested.array[0]"],
            Value::Bytes("example value".into())
        );
        assert_eq!(event["nested.array[1]"], Value::Bytes("".into()));
        assert_eq!(
            event["nested.array[2]"],
            Value::Bytes("another value".into())
        );
    }
}
