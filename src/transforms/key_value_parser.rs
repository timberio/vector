use crate::{
    config::{log_schema, DataType, GlobalOptions, TransformConfig, TransformDescription},
    event::{Event, Value},
    internal_events::{KeyValueFieldDoesNotExist, KeyValueParseFailed, KeyValueTargetExists},
    transforms::{FunctionTransform, Transform},
    types::{parse_conversion_map, Conversion},
};
use serde::{Deserialize, Serialize};
use shared::TimeZone;
use std::collections::HashMap;
use std::str;

#[derive(Clone, Debug, Derivative, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
#[derivative(Default)]
pub struct KeyValueConfig {
    #[derivative(Default(value = "true"))]
    pub drop_field: bool,
    pub field: Option<String>,
    pub field_split: Option<String>,
    #[derivative(Default(value = "true"))]
    pub overwrite_target: bool,
    pub separator: Option<String>,
    pub target_field: Option<String>,
    pub trim_key: Option<String>,
    pub trim_value: Option<String>,
    pub types: HashMap<String, String>,
    pub timezone: Option<TimeZone>,
}

inventory::submit! {
    TransformDescription::new::<KeyValueConfig>("key_value_parser")
}

impl_generate_config_from_default!(KeyValueConfig);

#[async_trait::async_trait]
#[typetag::serde(name = "key_value_parser")]
impl TransformConfig for KeyValueConfig {
    async fn build(&self, globals: &GlobalOptions) -> crate::Result<Transform<Event>> {
        let timezone = self.timezone.unwrap_or(globals.timezone);
        let conversions = parse_conversion_map(&self.types, timezone)?;
        let field = self
            .field
            .clone()
            .unwrap_or_else(|| log_schema().message_key().to_string());

        let separator = self.separator.clone().unwrap_or_else(|| " ".to_string());
        let trim_key = self.trim_key.as_ref().map(|key| key.chars().collect());
        let trim_value = self.trim_value.as_ref().map(|key| key.chars().collect());

        // Ensure the field being dropped is not the target field.
        let drop_field = self.drop_field
            && self
                .target_field
                .as_ref()
                .map(|target_field| &field != target_field)
                .unwrap_or(true);
        let target_field = self.target_field.clone();
        let overwrite_target = self.overwrite_target;

        let mut field_split = self.field_split.clone().unwrap_or_else(|| "=".to_string());
        if field_split.is_empty() {
            field_split = "=".to_string();
        }

        Ok(Transform::function(KeyValue {
            conversions,
            drop_field,
            field,
            field_split,
            overwrite_target,
            separator,
            target_field,
            trim_key,
            trim_value,
        }))
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn transform_type(&self) -> &'static str {
        "key_value_parser"
    }
}

#[derive(Debug, Clone)]
pub struct KeyValue {
    conversions: HashMap<String, Conversion>,
    drop_field: bool,
    field: String,
    field_split: String,
    overwrite_target: bool,
    separator: String,
    target_field: Option<String>,
    trim_key: Option<Vec<char>>,
    trim_value: Option<Vec<char>>,
}

impl KeyValue {
    fn parse_pair(&self, pair: &str) -> Option<(String, String)> {
        let pair = pair.trim();
        let field_split = &self.field_split;

        let split_index = pair.find(field_split).unwrap_or(0);
        let (key, _val) = pair.split_at(split_index);
        let key = key.trim();
        if key.is_empty() {
            return None;
        }
        let key = match &self.trim_key {
            Some(trim_key) => key.trim_matches(trim_key as &[_]),
            None => key,
        };

        let val = pair[split_index + field_split.len()..].trim();
        let val = match &self.trim_value {
            Some(trim_value) => val.trim_matches(trim_value as &[_]),
            None => val,
        };

        Some((key.to_string(), val.to_string()))
    }
}

impl FunctionTransform<Event> for KeyValue {
    fn transform(&mut self, output: &mut Vec<Event>, mut event: Event) {
        let log = event.as_mut_log();
        let value = log.get(&self.field).map(|s| s.to_string_lossy());

        if let Some(value) = &value {
            let pairs = value
                .split(&self.separator)
                .filter_map(|pair| self.parse_pair(pair));

            if let Some(target_field) = &self.target_field {
                if log.contains(target_field) {
                    if self.overwrite_target {
                        log.remove(target_field);
                    } else {
                        emit!(KeyValueTargetExists { target_field });
                        return output.push(event);
                    }
                }
            }

            for (mut key, val) in pairs {
                if let Some(target_field) = self.target_field.to_owned() {
                    key = format!("{}.{}", target_field, key);
                }

                if let Some(conv) = self.conversions.get(&key) {
                    match conv.convert::<Value>(val.into()) {
                        Ok(value) => {
                            log.insert(key, value);
                        }
                        Err(error) => {
                            emit!(KeyValueParseFailed { key, error });
                        }
                    }
                } else {
                    log.insert(key, val);
                }
            }

            if self.drop_field {
                log.remove(&self.field);
            }
        } else {
            emit!(KeyValueFieldDoesNotExist {
                field: self.field.to_string()
            });
        };

        output.push(event)
    }
}

#[cfg(test)]
mod tests {
    use super::KeyValueConfig;
    use crate::{
        config::{GlobalOptions, TransformConfig},
        event::{Event, LogEvent, Value},
    };

    async fn parse_log(
        text: &str,
        separator: Option<String>,
        field_split: Option<String>,
        drop_field: bool,
        types: &[(&str, &str)],
        target_field: Option<String>,
        trim_key: Option<String>,
        trim_value: Option<String>,
    ) -> LogEvent {
        let event = Event::from(text);
        let metadata = event.metadata().clone();

        let mut parser = KeyValueConfig {
            separator,
            field_split,
            field: None,
            drop_field,
            types: types.iter().map(|&(k, v)| (k.into(), v.into())).collect(),
            target_field,
            overwrite_target: false,
            trim_key,
            trim_value,
            timezone: Default::default(),
        }
        .build(&GlobalOptions::default())
        .await
        .unwrap();

        let parser = parser.as_function();

        let mut buf = Vec::with_capacity(1);
        parser.transform(&mut buf, event);
        let result = buf.pop().unwrap().into_log();
        assert_eq!(result.metadata(), &metadata);
        result
    }

    #[tokio::test]
    async fn it_separates_whitespace() {
        let log = parse_log("foo=bar beep=bop", None, None, true, &[], None, None, None).await;
        assert_eq!(log["foo"], Value::Bytes("bar".into()));
        assert_eq!(log["beep"], Value::Bytes("bop".into()));
    }

    #[tokio::test]
    async fn it_separates_csv_kv() {
        let log = parse_log(
            "foo=bar, beep=bop, score=10",
            Some(",".to_string()),
            None,
            false,
            &[],
            None,
            None,
            None,
        )
        .await;
        assert_eq!(log["foo"], Value::Bytes("bar".into()));
        assert_eq!(log["beep"], Value::Bytes("bop".into()));
    }

    #[tokio::test]
    async fn it_handles_whitespace_in_fields() {
        let log = parse_log(
            "foo:bar, beep : bop, score :10",
            Some(",".to_string()),
            Some(":".to_string()),
            false,
            &[("score", "integer")],
            None,
            None,
            None,
        )
        .await;
        assert_eq!(log["foo"], Value::Bytes("bar".into()));
        assert_eq!(log["beep"], Value::Bytes("bop".into()));
        assert_eq!(log["score"], Value::Integer(10));
    }

    #[tokio::test]
    async fn it_handles_multi_char_splitters() {
        let log = parse_log(
            "foo=>bar || beep => bop || score=>10",
            Some("||".to_string()),
            Some("=>".to_string()),
            false,
            &[("score", "integer")],
            None,
            None,
            None,
        )
        .await;

        assert_eq!(log["foo"], Value::Bytes("bar".into()));
        assert_eq!(log["beep"], Value::Bytes("bop".into()));
        assert_eq!(log["score"], Value::Integer(10));
    }

    #[tokio::test]
    async fn it_handles_splitters_in_value() {
        let log = parse_log(
            "foo==bar, beep=bop=bap , score=10",
            Some(",".to_string()),
            None,
            false,
            &[("score", "integer")],
            None,
            None,
            None,
        )
        .await;
        assert_eq!(log["foo"], Value::Bytes("=bar".into()));
        assert_eq!(log["beep"], Value::Bytes("bop=bap".into()));
        assert_eq!(log["score"], Value::Integer(10));
    }

    #[tokio::test]
    async fn it_handles_empty_values() {
        let log = parse_log(
            "foo::0, bop::beep, score::",
            Some(",".to_string()),
            Some("::".to_string()),
            false,
            &[],
            None,
            None,
            None,
        )
        .await;
        assert!(log.contains("score"));
        assert_eq!(log["score"], Value::Bytes("".into()))
    }

    #[tokio::test]
    async fn it_handles_empty_keys() {
        let log = parse_log(
            "foo::0, ::beep, score::12",
            Some(",".to_string()),
            Some("::".to_string()),
            false,
            &[],
            None,
            None,
            None,
        )
        .await;
        assert!(log.contains("foo"));
        assert!(!log.contains("beep"));
        assert!(log.contains("score"));
    }

    #[tokio::test]
    async fn it_accepts_brackets() {
        let log = parse_log(
            r#"{"foo"}:0, ""bop":[beep], [score]:78"#,
            Some(",".to_string()),
            Some(":".to_string()),
            false,
            &[],
            None,
            Some("\"{}".to_string()),
            None,
        )
        .await;
        assert_eq!(log["bop"], Value::Bytes("[beep]".into()))
    }

    #[tokio::test]
    async fn it_trims_keys() {
        let log = parse_log(
            "{\"foo\"}:0, \"\"bop\":beep, {({score})}:78",
            Some(",".to_string()),
            Some(":".to_string()),
            false,
            &[],
            None,
            Some("\"{}".to_string()),
            None,
        )
        .await;
        assert!(log.contains("foo"));
        assert!(log.contains("bop"));
        assert!(log.contains(&"({score})".to_string()));
    }

    #[tokio::test]
    async fn it_trims_values() {
        let log = parse_log(
            "foo:{\"0\"}, bop:\"beep\", score:{78}",
            Some(",".to_string()),
            Some(":".to_string()),
            false,
            &[("foo", "integer"), ("score", "integer")],
            None,
            None,
            Some("\"{}".to_string()),
        )
        .await;
        assert_eq!(log["foo"], Value::Integer(0));
        assert_eq!(log["bop"], Value::Bytes("beep".into()));
        assert_eq!(log["score"], Value::Integer(78));
    }
}
