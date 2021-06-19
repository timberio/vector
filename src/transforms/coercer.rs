use crate::{
    config::{DataType, GlobalOptions, TransformConfig, TransformDescription},
    event::{Event, LogEvent, Value},
    internal_events::CoercerConversionFailed,
    transforms::{FunctionTransform, Transform},
    types::{parse_conversion_map, Conversion},
};
use serde::{Deserialize, Serialize};
use shared::TimeZone;
use std::collections::HashMap;
use std::str;

#[derive(Deserialize, Serialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields, default)]
pub struct CoercerConfig {
    types: HashMap<String, String>,
    drop_unspecified: bool,
    timezone: Option<TimeZone>,
}

inventory::submit! {
    TransformDescription::new::<CoercerConfig>("coercer")
}

impl_generate_config_from_default!(CoercerConfig);

#[async_trait::async_trait]
#[typetag::serde(name = "coercer")]
impl TransformConfig for CoercerConfig {
    async fn build(&self, globals: &GlobalOptions) -> crate::Result<Transform<Event>> {
        let timezone = self.timezone.unwrap_or(globals.timezone);
        let types = parse_conversion_map(&self.types, timezone)?;
        Ok(Transform::function(Coercer {
            types,
            drop_unspecified: self.drop_unspecified,
        }))
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn transform_type(&self) -> &'static str {
        "coercer"
    }
}

#[derive(Clone, Debug)]
pub struct Coercer {
    types: HashMap<String, Conversion>,
    drop_unspecified: bool,
}

impl FunctionTransform<Event> for Coercer {
    fn transform(&mut self, output: &mut Vec<Event>, event: Event) {
        let mut log = event.into_log();

        if self.drop_unspecified {
            // This uses a different algorithm from the default path
            // below, as it will be fewer steps to fully recreate the
            // event than to scan the event for extraneous fields after
            // conversion.
            let mut new_log = LogEvent::new_with_metadata(log.metadata().clone());
            for (field, conv) in &self.types {
                if let Some(value) = log.remove(field) {
                    match conv.convert::<Value>(value.into_bytes()) {
                        Ok(converted) => {
                            new_log.insert(field, converted);
                        }
                        Err(error) => emit!(CoercerConversionFailed { field, error }),
                    }
                }
            }
            output.push(new_log.into());
            return;
        } else {
            for (field, conv) in &self.types {
                if let Some(value) = log.remove(field) {
                    match conv.convert::<Value>(value.into_bytes()) {
                        Ok(converted) => {
                            log.insert(field, converted);
                        }
                        Err(error) => emit!(CoercerConversionFailed { field, error }),
                    }
                }
            }
        }
        output.push(Event::Log(log));
    }
}

#[cfg(test)]
mod tests {
    use super::CoercerConfig;
    use crate::{
        config::{GlobalOptions, TransformConfig},
        event::{Event, LogEvent, Value},
    };
    use pretty_assertions::assert_eq;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<CoercerConfig>();
    }

    async fn parse_it(extra: &str) -> LogEvent {
        let mut event = Event::from("dummy message");
        for &(key, value) in &[
            ("number", "1234"),
            ("bool", "yes"),
            ("other", "no"),
            ("float", "broken"),
        ] {
            event.as_mut_log().insert(key, value);
        }
        let metadata = event.metadata().clone();

        let mut coercer = toml::from_str::<CoercerConfig>(&format!(
            r#"{}
            [types]
            number = "int"
            float = "float"
            bool = "bool"
            "#,
            extra
        ))
        .unwrap()
        .build(&GlobalOptions::default())
        .await
        .unwrap();
        let coercer = coercer.as_function();
        let mut buf = Vec::with_capacity(1);
        coercer.transform(&mut buf, event);
        let result = buf.pop().unwrap().into_log();
        assert_eq!(&metadata, result.metadata());
        result
    }

    #[tokio::test]
    async fn converts_valid_fields() {
        let log = parse_it("").await;
        assert_eq!(log["number"], Value::Integer(1234));
        assert_eq!(log["bool"], Value::Boolean(true));
    }

    #[tokio::test]
    async fn leaves_unnamed_fields_as_is() {
        let log = parse_it("").await;
        assert_eq!(log["other"], Value::Bytes("no".into()));
    }

    #[tokio::test]
    async fn drops_nonconvertible_fields() {
        let log = parse_it("").await;
        assert!(log.get("float").is_none());
    }

    #[tokio::test]
    async fn drops_unspecified_fields() {
        let log = parse_it("drop_unspecified = true").await;

        let mut expected = Event::new_empty_log();
        expected.as_mut_log().insert("bool", true);
        expected.as_mut_log().insert("number", 1234);

        shared::assert_event_data_eq!(log, expected.into_log());
    }
}
