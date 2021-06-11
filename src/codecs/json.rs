use super::{Codec, CodecTransform};
use crate::{
    config::DataType,
    transforms::json_parser::{JsonParser, JsonParserConfig},
};
use serde::{Deserialize, Serialize};
use vector_core::{
    config::log_schema,
    event::{Event, Value},
    transform::{FunctionTransform, Transform},
};

#[derive(Debug, Clone, Serialize, Deserialize, Derivative)]
#[serde(deny_unknown_fields, default)]
#[derivative(Default)]
pub struct JsonCodec {
    mode: Option<Mode>,
    field: Option<String>,
    drop_invalid: bool,
    #[derivative(Default(value = "true"))]
    drop_field: bool,
    target_field: Option<String>,
    #[derivative(Default(value = "false"))]
    overwrite_target: bool,
}

#[typetag::serde(name = "json")]
impl Codec for JsonCodec {
    fn name(&self) -> &'static str {
        "json"
    }

    fn build_decoder(&self) -> crate::Result<CodecTransform> {
        let config = JsonParserConfig {
            field: self.field.clone(),
            drop_invalid: self.drop_invalid,
            drop_field: self.drop_field,
            target_field: self.target_field.clone(),
            overwrite_target: Some(self.overwrite_target),
        };

        Ok(CodecTransform {
            input_type: DataType::Log,
            transform: Transform::function(JsonParser::from(config)),
        })
    }

    fn build_encoder(&self) -> crate::Result<CodecTransform> {
        #[derive(Debug, Clone)]
        struct SerializeJsonTransform(JsonCodec);

        impl FunctionTransform<Event> for SerializeJsonTransform {
            fn transform(&mut self, output: &mut Vec<Event>, mut event: Event) {
                let options = &self.0;
                let log = event.as_mut_log();

                let json = match &options.field {
                    Some(field) => match log.get(field) {
                        Some(value) => value,
                        None => &Value::Null,
                    },
                    None => log.get_root(),
                };

                let serialized = match serde_json::to_string(json) {
                    Ok(string) => string,
                    Err(_) => return,
                };

                if options.drop_field {
                    match &options.field {
                        Some(field) => {
                            log.remove(field);
                        }
                        None => {
                            log.remove_root();
                        }
                    }
                }

                let target_field = options
                    .target_field
                    .as_ref()
                    .map(AsRef::as_ref)
                    .unwrap_or_else(|| log_schema().message_key());

                let contains_target = log.contains(&target_field);
                if !contains_target || options.overwrite_target {
                    log.insert(&target_field, serialized);
                }

                output.push(event)
            }
        }

        Ok(CodecTransform {
            transform: Transform::function(SerializeJsonTransform(self.clone())),
            input_type: DataType::Log,
        })
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Encode,
    Decode,
}

inventory::submit! {
    Box::new(JsonCodec::default()) as Box<dyn Codec>
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream::{self, StreamExt};
    use shared::btreemap;

    #[tokio::test]
    async fn default_decoder() {
        let codec = JsonCodec::default();
        let transform = codec.build_decoder().unwrap().transform;
        let mut event = Event::new_empty_log();
        let log = event.as_mut_log();
        log.insert(log_schema().message_key(), r#"{"foo":"bar"}"#);
        let input = stream::once(async { event });
        let mut output = transform.transform(input);
        let transformed = output.next().await.unwrap();

        assert_eq!(
            transformed,
            btreemap! {
                "foo" => "bar"
            }
            .into(),
        );
        assert_eq!(output.next().await, None);
    }

    #[tokio::test]
    async fn default_encoder() {
        let codec = JsonCodec::default();
        let transform = codec.build_encoder().unwrap().transform;
        let mut event = Event::new_empty_log();
        let log = event.as_mut_log();
        log.insert(
            log_schema().message_key(),
            btreemap! {
                "foo" => "bar"
            },
        );
        let input = stream::once(async { event });
        let mut output = transform.transform(input);
        let transformed = output.next().await.unwrap();

        assert_eq!(
            transformed
                .as_log()
                .get(log_schema().message_key())
                .unwrap()
                .clone(),
            r#"{"message":{"foo":"bar"}}"#.to_owned().into()
        );
        assert_eq!(output.next().await, None);
    }
}
