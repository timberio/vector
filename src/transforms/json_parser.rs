use crate::{
    config::{log_schema, DataType, GlobalOptions, TransformConfig, TransformDescription},
    event::{Event, LookupBuf, Value},
    internal_events::{JsonParserFailedParse, JsonParserTargetExists},
    transforms::{FunctionTransform, Transform},
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone, Derivative)]
#[serde(deny_unknown_fields, default)]
#[derivative(Default)]
pub struct JsonParserConfig {
    pub field: Option<LookupBuf>,
    pub drop_invalid: bool,
    #[derivative(Default(value = "true"))]
    pub drop_field: bool,
    pub target_field: Option<LookupBuf>,
    pub overwrite_target: Option<bool>,
}

inventory::submit! {
    TransformDescription::new::<JsonParserConfig>("json_parser")
}

impl_generate_config_from_default!(JsonParserConfig);

#[async_trait::async_trait]
#[typetag::serde(name = "json_parser")]
impl TransformConfig for JsonParserConfig {
    async fn build(&self, _globals: &GlobalOptions) -> crate::Result<Transform> {
        Ok(Transform::function(JsonParser::from(self.clone())))
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn transform_type(&self) -> &'static str {
        "json_parser"
    }
}

#[derive(Debug, Clone)]
pub struct JsonParser {
    field: LookupBuf,
    drop_invalid: bool,
    drop_field: bool,
    target_field: Option<LookupBuf>,
    overwrite_target: bool,
}

impl From<JsonParserConfig> for JsonParser {
    fn from(config: JsonParserConfig) -> JsonParser {
        let field = config
            .field
            .unwrap_or_else(|| log_schema().message_key().clone());

        JsonParser {
            field,
            drop_invalid: config.drop_invalid,
            drop_field: config.drop_field,
            target_field: config.target_field,
            overwrite_target: config.overwrite_target.unwrap_or(false),
        }
    }
}

impl FunctionTransform for JsonParser {
    fn transform(&mut self, output: &mut Vec<Event>, mut event: Event) {
        let log = event.as_mut_log();
        let value = log.get(&self.field);

        let parsed = value
            .and_then(|value| {
                let to_parse = value.clone_into_bytes();
                serde_json::from_slice::<serde_json::Value>(to_parse.as_ref())
                    .map_err(|error| {
                        emit!(JsonParserFailedParse {
                            field: &self.field,
                            value: value.to_string_lossy().as_str(),
                            error,
                            drop_invalid: self.drop_invalid,
                        })
                    })
                    .ok()
            })
            .and_then(|value| {
                if let serde_json::Value::Object(object) = value {
                    Some(object)
                } else {
                    None
                }
            });

        if let Some(object) = parsed {
            match self.target_field.clone() {
                Some(target_field) => {
                    let contains_target = log.contains(&target_field);

                    if contains_target && !self.overwrite_target {
                        emit!(JsonParserTargetExists {
                            target_field: &target_field
                        })
                    } else {
                        if self.drop_field {
                            log.remove(&self.field, false);
                        }

                        log.insert(target_field, Value::from(object));
                    }
                }
                None => {
                    if self.drop_field {
                        log.remove(&self.field, false);
                    }

                    for (key, value) in object {
                        log.insert(LookupBuf::from(key), value);
                    }
                }
            }
        } else if self.drop_invalid {
            return;
        }

        output.push(event);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{
        config::log_schema,
        event::{Lookup, LookupBuf},
        log_event,
    };
    use serde_json::json;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<JsonParserConfig>();
    }

    #[test]
    fn json_parser_drop_field() {
        let mut parser = JsonParser::from(JsonParserConfig::default());

        let event = log_event! {
            log_schema().message_key().clone() => r#"{"greeting": "hello", "name": "bob"}"#.to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };

        let event = parser.transform_one(event).unwrap();

        assert!(event.as_log().get(log_schema().message_key()).is_none());
    }

    #[test]
    fn json_parser_doesnt_drop_field() {
        let mut parser = JsonParser::from(JsonParserConfig {
            drop_field: false,
            ..Default::default()
        });

        let event = log_event! {
            log_schema().message_key().clone() => r#"{"greeting": "hello", "name": "bob"}"#.to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };

        let event = parser.transform_one(event).unwrap();

        assert!(event.as_log().get(log_schema().message_key()).is_some());
    }

    #[test]
    fn json_parser_parse_raw() {
        let mut parser = JsonParser::from(JsonParserConfig {
            drop_field: false,
            ..Default::default()
        });

        let event = log_event! {
            log_schema().message_key().clone() => r#"{"greeting": "hello", "name": "bob"}"#.to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };

        let event = parser.transform_one(event).unwrap();

        assert_eq!(event.as_log()[Lookup::from("greeting")], "hello".into());
        assert_eq!(event.as_log()[Lookup::from("name")], "bob".into());
        assert_eq!(
            event.as_log()[log_schema().message_key()],
            r#"{"greeting": "hello", "name": "bob"}"#.into()
        );
    }

    // Ensure the JSON parser doesn't take strings as toml paths.
    // This is a regression test, see: https://github.com/timberio/vector/issues/2814
    #[test]
    fn json_parser_parse_periods() {
        crate::test_util::trace_init();
        let mut parser = JsonParser::from(JsonParserConfig {
            drop_field: false,
            ..Default::default()
        });

        let test_json = json!({
            "field.with.dots": "hello",
            "sub.field": { "another.one": "bob"},
        });

        let event = log_event! {
            log_schema().message_key().clone() => test_json.to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };

        let event = parser.transform_one(event).unwrap();

        assert_eq!(
            event.as_log().get(Lookup::from("field.with.dots")),
            Some(&crate::event::Value::from("hello")),
        );
        assert_eq!(
            event.as_log().get(Lookup::from("sub.field")),
            Some(&crate::event::Value::from(json!({ "another.one": "bob", }))),
        );
    }

    #[test]
    fn json_parser_parse_raw_with_whitespace() {
        let mut parser = JsonParser::from(JsonParserConfig {
            drop_field: false,
            ..Default::default()
        });

        let event = log_event! {
            log_schema().message_key().clone() => r#" {"greeting": "hello", "name": "bob"}    "#.to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };

        let event = parser.transform_one(event).unwrap();

        assert_eq!(event.as_log()[Lookup::from("greeting")], "hello".into());
        assert_eq!(event.as_log()[Lookup::from("name")], "bob".into());
        assert_eq!(
            event.as_log()[log_schema().message_key()],
            r#" {"greeting": "hello", "name": "bob"}    "#.into()
        );
    }

    #[test]
    fn json_parser_parse_field() {
        crate::test_util::trace_init();
        let mut parser = JsonParser::from(JsonParserConfig {
            field: Some("data".into()),
            drop_field: false,
            ..Default::default()
        });

        // Field present

        let mut event = log_event! {
            log_schema().message_key().clone() => "message".to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };
        event.as_mut_log().insert(
            LookupBuf::from("data"),
            r#"{"greeting": "hello", "name": "bob"}"#,
        );

        let event = parser.transform_one(event).unwrap();

        assert_eq!(event.as_log()[Lookup::from("greeting")], "hello".into(),);
        assert_eq!(event.as_log()[Lookup::from("name")], "bob".into());
        assert_eq!(
            event.as_log()[Lookup::from("data")],
            r#"{"greeting": "hello", "name": "bob"}"#.into()
        );

        // Field missing
        let event = log_event! {
            log_schema().message_key().clone() => "message".to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };

        let parsed = parser.transform_one(event.clone()).unwrap();

        assert_eq!(event, parsed);
    }

    #[test]
    fn json_parser_parse_inner_json() {
        crate::test_util::trace_init();
        let mut parser_outer = JsonParser::from(JsonParserConfig {
            ..Default::default()
        });

        let mut parser_inner = JsonParser::from(JsonParserConfig {
            field: Some("log".into()),
            ..Default::default()
        });

        let event = log_event! {
            log_schema().message_key().clone() => r#"{"log":"{\"type\":\"response\",\"@timestamp\":\"2018-10-04T21:12:33Z\",\"tags\":[],\"pid\":1,\"method\":\"post\",\"statusCode\":200,\"req\":{\"url\":\"/elasticsearch/_msearch\",\"method\":\"post\",\"headers\":{\"host\":\"logs.com\",\"connection\":\"close\",\"x-real-ip\":\"120.21.3.1\",\"x-forwarded-for\":\"121.91.2.2\",\"x-forwarded-host\":\"logs.com\",\"x-forwarded-port\":\"443\",\"x-forwarded-proto\":\"https\",\"x-original-uri\":\"/elasticsearch/_msearch\",\"x-scheme\":\"https\",\"content-length\":\"1026\",\"accept\":\"application/json, text/plain, */*\",\"origin\":\"https://logs.com\",\"kbn-version\":\"5.2.3\",\"user-agent\":\"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_12_6) AppleWebKit/532.30 (KHTML, like Gecko) Chrome/62.0.3361.210 Safari/533.21\",\"content-type\":\"application/x-ndjson\",\"referer\":\"https://domain.com/app/kibana\",\"accept-encoding\":\"gzip, deflate, br\",\"accept-language\":\"en-US,en;q=0.8\"},\"remoteAddress\":\"122.211.22.11\",\"userAgent\":\"22.322.32.22\",\"referer\":\"https://domain.com/app/kibana\"},\"res\":{\"statusCode\":200,\"responseTime\":417,\"contentLength\":9},\"message\":\"POST /elasticsearch/_msearch 200 225ms - 8.0B\"}\n","stream":"stdout","time":"2018-10-02T21:14:48.2233245241Z"}"#.to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };

        let parsed_event = parser_outer.transform_one(event).unwrap();

        assert_eq!(
            parsed_event.as_log()[Lookup::from("stream")],
            "stdout".into()
        );

        let parsed_inner_event = parser_inner.transform_one(parsed_event).unwrap();
        let log = parsed_inner_event.into_log();

        assert_eq!(log[Lookup::from("type")], "response".into());
        assert_eq!(log[Lookup::from("statusCode")], 200.into());
    }

    #[test]
    fn json_parser_invalid_json() {
        crate::test_util::trace_init();
        let invalid = r#"{"greeting": "hello","#;

        // Raw
        let mut parser = JsonParser::from(JsonParserConfig {
            drop_field: false,
            ..Default::default()
        });

        let event = log_event! {
            log_schema().message_key().clone() => invalid.to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };

        let parsed = parser.transform_one(event.clone()).unwrap();

        assert_eq!(event, parsed);
        assert_eq!(event.as_log()[log_schema().message_key()], invalid.into());

        // Field
        let mut parser = JsonParser::from(JsonParserConfig {
            field: Some("data".into()),
            drop_field: false,
            ..Default::default()
        });

        let mut event = log_event! {
            log_schema().message_key().clone() => "message".to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };
        event.as_mut_log().insert(LookupBuf::from("data"), invalid);

        let event = parser.transform_one(event).unwrap();

        assert_eq!(event.as_log()[Lookup::from("data")], invalid.into());
        assert!(event.as_log().get(Lookup::from("greeting")).is_none());
    }

    #[test]
    fn json_parser_drop_invalid() {
        crate::test_util::trace_init();
        let valid = r#"{"greeting": "hello", "name": "bob"}"#;
        let invalid = r#"{"greeting": "hello","#;
        let not_object = r#""hello""#;

        // Raw
        let mut parser = JsonParser::from(JsonParserConfig {
            drop_invalid: true,
            ..Default::default()
        });

        let event = log_event! {
            log_schema().message_key().clone() => valid.to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };
        assert!(parser.transform_one(event).is_some());

        let event = log_event! {
            log_schema().message_key().clone() => invalid.to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };
        assert!(parser.transform_one(event).is_none());

        let event = log_event! {
            log_schema().message_key().clone() => not_object.to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };
        assert!(parser.transform_one(event).is_none());

        // Field
        let mut parser = JsonParser::from(JsonParserConfig {
            field: Some("data".into()),
            drop_invalid: true,
            ..Default::default()
        });

        let mut event = log_event! {
            log_schema().message_key().clone() => "message".to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };
        event.as_mut_log().insert(LookupBuf::from("data"), valid);
        assert!(parser.transform_one(event).is_some());

        let mut event = log_event! {
            log_schema().message_key().clone() => "message".to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };
        event.as_mut_log().insert(LookupBuf::from("data"), invalid);
        assert!(parser.transform_one(event).is_none());

        let mut event = log_event! {
            log_schema().message_key().clone() => "message".to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };
        event
            .as_mut_log()
            .insert(LookupBuf::from("data"), not_object);
        assert!(parser.transform_one(event).is_none());

        // Missing field
        let event = log_event! {
            log_schema().message_key().clone() => "message".to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };
        assert!(parser.transform_one(event).is_none());
    }

    #[test]
    fn json_parser_chained() {
        crate::test_util::trace_init();
        let mut parser1 = JsonParser::from(JsonParserConfig {
            ..Default::default()
        });
        let mut parser2 = JsonParser::from(JsonParserConfig {
            field: Some("nested".into()),
            ..Default::default()
        });

        let event = log_event! {
            crate::config::log_schema().message_key().clone() => r#"{"greeting": "hello", "name": "bob", "nested": "{\"message\": \"help i'm trapped under many layers of json\"}"}"#.to_string(),
            crate::config::log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };
        let event = parser1.transform_one(event).unwrap();
        let event = parser2.transform_one(event).unwrap();

        assert_eq!(event.as_log()[Lookup::from("greeting")], "hello".into());
        assert_eq!(event.as_log()[Lookup::from("name")], "bob".into());
        assert_eq!(
            event.as_log()[Lookup::from("message")],
            "help i'm trapped under many layers of json".into()
        );
    }

    #[test]
    fn json_parser_types() {
        crate::test_util::trace_init();
        let mut parser = JsonParser::from(JsonParserConfig {
            ..Default::default()
        });

        let event = log_event! {
            crate::config::log_schema().message_key().clone() => r#"{
              "string": "this is text",
              "null": null,
              "float": 12.34,
              "int": 56,
              "bool true": true,
              "bool false": false,
              "array": ["z", 7],
              "object": { "nested": "data", "more": "values" },
              "deep": [[[{"a": { "b": { "c": [[[1234]]]}}}]]]
            }"#.to_string(),
            crate::config::log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };
        let event = parser.transform_one(event).unwrap();

        assert_eq!(
            event.as_log()[Lookup::from_str("string").unwrap()],
            "this is text".into()
        );
        assert_eq!(
            event.as_log()[Lookup::from_str("null").unwrap()],
            crate::event::Value::Null
        );
        assert_eq!(
            event.as_log()[Lookup::from_str("float").unwrap()],
            12.34.into()
        );
        assert_eq!(event.as_log()[Lookup::from_str("int").unwrap()], 56.into());
        assert_eq!(event.as_log()[Lookup::from("bool true")], true.into());
        assert_eq!(event.as_log()[Lookup::from("bool false")], false.into());
        assert_eq!(
            event.as_log()[Lookup::from_str("array[0]").unwrap()],
            "z".into()
        );
        assert_eq!(
            event.as_log()[Lookup::from_str("array[1]").unwrap()],
            7.into()
        );
        assert_eq!(
            event.as_log()[Lookup::from_str("object.nested").unwrap()],
            "data".into()
        );
        assert_eq!(
            event.as_log()[Lookup::from_str("object.more").unwrap()],
            "values".into()
        );
        assert_eq!(
            event.as_log()[Lookup::from_str("deep[0][0][0].a.b.c[0][0][0]").unwrap()],
            1234.into()
        );
    }

    #[test]
    fn drop_field_before_adding() {
        let mut parser = JsonParser::from(JsonParserConfig {
            drop_field: true,
            ..Default::default()
        });

        let event = log_event! {
            crate::config::log_schema().message_key().clone() => r#"{
                "key": "data",
                "message": "inner"
            }"#.to_string(),
            crate::config::log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };

        let event = parser.transform_one(event).unwrap();

        assert_eq!(event.as_log()[Lookup::from("key")], "data".into());
        assert_eq!(event.as_log()[Lookup::from("message")], "inner".into());
    }

    #[test]
    fn doesnt_drop_field_after_failed_parse() {
        let mut parser = JsonParser::from(JsonParserConfig {
            drop_field: true,
            ..Default::default()
        });

        let event = log_event! {
            crate::config::log_schema().message_key().clone() => r#"invalid json"#.to_string(),
            crate::config::log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };

        let event = parser.transform_one(event).unwrap();

        assert_eq!(
            event.as_log()[Lookup::from("message")],
            "invalid json".into()
        );
    }

    #[test]
    fn target_field_works() {
        let mut parser = JsonParser::from(JsonParserConfig {
            drop_field: false,
            target_field: Some("that".into()),
            ..Default::default()
        });

        let event = log_event! {
            log_schema().message_key().clone() => r#"{"greeting": "hello", "name": "bob"}"#.to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };
        let event = parser.transform_one(event).unwrap();
        let event = event.as_log();

        assert_eq!(
            event[Lookup::from_str("that.greeting").unwrap()],
            "hello".into()
        );
        assert_eq!(event[Lookup::from_str("that.name").unwrap()], "bob".into());
    }

    #[test]
    fn target_field_preserves_existing() {
        let mut parser = JsonParser::from(JsonParserConfig {
            drop_field: false,
            target_field: Some("message".into()),
            ..Default::default()
        });

        let message = r#"{"greeting": "hello", "name": "bob"}"#;
        let event = log_event! {
            log_schema().message_key().clone() => message.to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };
        let event = parser.transform_one(event).unwrap();
        let event = event.as_log();

        assert_eq!(event[Lookup::from("message")], message.into());
        assert_eq!(
            event.get(Lookup::from_str("message.greeting").unwrap()),
            None
        );
        assert_eq!(event.get(Lookup::from_str("message.name").unwrap()), None);
    }

    #[test]
    fn target_field_overwrites_existing() {
        let mut parser = JsonParser::from(JsonParserConfig {
            drop_field: false,
            target_field: Some("message".into()),
            overwrite_target: Some(true),
            ..Default::default()
        });

        let message = r#"{"greeting": "hello", "name": "bob"}"#;
        let event = log_event! {
            log_schema().message_key().clone() => message.to_string(),
            log_schema().timestamp_key().clone() => chrono::Utc::now(),
        };
        let event = parser.transform_one(event).unwrap();
        let event = event.as_log();

        match event.get(Lookup::from("message")) {
            Some(crate::event::Value::Map(_)) => (),
            _ => panic!("\"message\" is not a map"),
        }
        assert_eq!(
            event[Lookup::from_str("message.greeting").unwrap()],
            "hello".into()
        );
        assert_eq!(
            event[Lookup::from_str("message.name").unwrap()],
            "bob".into()
        );
    }
}
