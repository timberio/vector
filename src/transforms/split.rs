use crate::{
    config::{DataType, GlobalOptions, TransformConfig, TransformDescription},
    event::{Event, Value},
    internal_events::{SplitConvertFailed, SplitFieldMissing},
    transforms::{FunctionTransform, Transform},
    types::{parse_check_conversion_map, Conversion},
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use shared::TimeZone;
use std::collections::HashMap;
use std::str;

#[derive(Deserialize, Serialize, Debug, Default, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct SplitConfig {
    pub field_names: Vec<String>,
    pub separator: Option<String>,
    pub field: Option<String>,
    pub drop_field: bool,
    pub types: HashMap<String, String>,
    pub timezone: Option<TimeZone>,
}

inventory::submit! {
    TransformDescription::new::<SplitConfig>("split")
}

impl_generate_config_from_default!(SplitConfig);

#[async_trait::async_trait]
#[typetag::serde(name = "split")]
impl TransformConfig for SplitConfig {
    async fn build(&self, globals: &GlobalOptions) -> crate::Result<Transform<Event>> {
        let field = self
            .field
            .clone()
            .unwrap_or_else(|| crate::config::log_schema().message_key().to_string());

        let timezone = self.timezone.unwrap_or(globals.timezone);
        let types = parse_check_conversion_map(&self.types, &self.field_names, timezone)
            .map_err(|error| format!("{}", error))?;

        // don't drop the source field if it's getting overwritten by a parsed value
        let drop_field = self.drop_field && !self.field_names.iter().any(|f| **f == *field);

        Ok(Transform::function(Split::new(
            self.field_names.clone(),
            self.separator.clone(),
            field,
            drop_field,
            types,
        )))
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn transform_type(&self) -> &'static str {
        "split"
    }
}

#[derive(Clone, Debug)]
pub struct Split {
    field_names: Vec<(String, Conversion)>,
    separator: Option<String>,
    field: String,
    drop_field: bool,
}

impl Split {
    pub fn new(
        field_names: Vec<String>,
        separator: Option<String>,
        field: String,
        drop_field: bool,
        types: HashMap<String, Conversion>,
    ) -> Self {
        let field_names = field_names
            .into_iter()
            .map(|name| {
                let conversion = types.get(&name).unwrap_or(&Conversion::Bytes).clone();
                (name, conversion)
            })
            .collect();

        Self {
            field_names,
            separator,
            field,
            drop_field,
        }
    }
}

impl FunctionTransform<Event> for Split {
    fn transform(&mut self, output: &mut Vec<Event>, mut event: Event) {
        let value = event.as_log().get(&self.field).map(|s| s.to_string_lossy());

        if let Some(value) = &value {
            for ((name, conversion), value) in self
                .field_names
                .iter()
                .zip(split(value, self.separator.clone()).into_iter())
            {
                match conversion.convert::<Value>(Bytes::copy_from_slice(value.as_bytes())) {
                    Ok(value) => {
                        event.as_mut_log().insert(name.clone(), value);
                    }
                    Err(error) => {
                        emit!(SplitConvertFailed { field: name, error });
                    }
                }
            }
            if self.drop_field {
                event.as_mut_log().remove(&self.field);
            }
        } else {
            emit!(SplitFieldMissing { field: &self.field });
        };

        output.push(event);
    }
}

// Splits the given input by a separator.
// If the separator is `None`, then it will split on whitespace.
pub fn split(input: &str, separator: Option<String>) -> Vec<&str> {
    match separator {
        Some(separator) => input.split(&separator).collect(),
        None => input.split_whitespace().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TransformConfig;
    use crate::event::{Event, LogEvent, Value};

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<SplitConfig>();
    }

    #[test]
    fn split_whitespace() {
        assert_eq!(split("foo bar", None), &["foo", "bar"]);
        assert_eq!(split("foo\t bar", None), &["foo", "bar"]);
        assert_eq!(split("foo  \t bar     baz", None), &["foo", "bar", "baz"]);
    }

    #[test]
    fn split_comma() {
        assert_eq!(split("foo", Some(",".to_string())), &["foo"]);
        assert_eq!(split("foo,bar", Some(",".to_string())), &["foo", "bar"]);
    }

    #[test]
    fn split_semicolon() {
        assert_eq!(
            split("foo,bar;baz", Some(";".to_string())),
            &["foo,bar", "baz"]
        );
    }

    async fn parse_log(
        text: &str,
        fields: &str,
        separator: Option<String>,
        field: Option<&str>,
        drop_field: bool,
        types: &[(&str, &str)],
    ) -> LogEvent {
        let event = Event::from(text);
        let field_names = fields.split(' ').map(|s| s.into()).collect::<Vec<String>>();
        let field = field.map(|f| f.into());
        let mut parser = SplitConfig {
            field_names,
            separator,
            field,
            drop_field,
            types: types.iter().map(|&(k, v)| (k.into(), v.into())).collect(),
            timezone: Default::default(),
        }
        .build(&GlobalOptions::default())
        .await
        .unwrap();
        let parser = parser.as_function();

        let metadata = event.metadata().clone();
        let mut buf = Vec::with_capacity(1);
        parser.transform(&mut buf, event);
        let result = buf.pop().unwrap().into_log();
        assert_eq!(result.metadata(), &metadata);
        result
    }

    #[tokio::test]
    async fn split_adds_parsed_field_to_event() {
        let log = parse_log("1234 5678", "status time", None, None, false, &[]).await;

        assert_eq!(log["status"], "1234".into());
        assert_eq!(log["time"], "5678".into());
        assert!(log.get("message").is_some());
    }

    #[tokio::test]
    async fn split_does_drop_parsed_field() {
        let log = parse_log("1234 5678", "status time", None, Some("message"), true, &[]).await;

        assert_eq!(log["status"], "1234".into());
        assert_eq!(log["time"], "5678".into());
        assert!(log.get("message").is_none());
    }

    #[tokio::test]
    async fn split_does_not_drop_same_name_parsed_field() {
        let log = parse_log(
            "1234 yes",
            "status message",
            None,
            Some("message"),
            true,
            &[],
        )
        .await;

        assert_eq!(log["status"], "1234".into());
        assert_eq!(log["message"], "yes".into());
    }

    #[tokio::test]
    async fn split_coerces_fields_to_types() {
        let log = parse_log(
            "1234 yes 42.3 word",
            "code flag number rest",
            None,
            None,
            false,
            &[("flag", "bool"), ("code", "integer"), ("number", "float")],
        )
        .await;

        assert_eq!(log["number"], Value::Float(42.3));
        assert_eq!(log["flag"], Value::Boolean(true));
        assert_eq!(log["code"], Value::Integer(1234));
        assert_eq!(log["rest"], Value::Bytes("word".into()));
    }

    #[tokio::test]
    async fn split_works_with_different_separator() {
        let log = parse_log(
            "1234,foo,bar",
            "code who why",
            Some(",".into()),
            None,
            false,
            &[("code", "integer"), ("who", "string"), ("why", "string")],
        )
        .await;

        assert_eq!(log["code"], Value::Integer(1234));
        assert_eq!(log["who"], Value::Bytes("foo".into()));
        assert_eq!(log["why"], Value::Bytes("bar".into()));
    }
}
