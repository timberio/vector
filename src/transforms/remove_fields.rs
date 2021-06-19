use crate::{
    config::{DataType, GenerateConfig, GlobalOptions, TransformConfig, TransformDescription},
    event::Event,
    internal_events::RemoveFieldsFieldMissing,
    transforms::{FunctionTransform, Transform},
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct RemoveFieldsConfig {
    fields: Vec<String>,
    drop_empty: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct RemoveFields {
    fields: Vec<String>,
    drop_empty: bool,
}

inventory::submit! {
    TransformDescription::new::<RemoveFieldsConfig>("remove_fields")
}

impl GenerateConfig for RemoveFieldsConfig {
    fn generate_config() -> toml::Value {
        toml::Value::try_from(Self {
            fields: Vec::new(),
            drop_empty: None,
        })
        .unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "remove_fields")]
impl TransformConfig for RemoveFieldsConfig {
    async fn build(&self, _globals: &GlobalOptions) -> crate::Result<Transform<Event>> {
        RemoveFields::new(self.fields.clone(), self.drop_empty.unwrap_or(false))
            .map(Transform::function)
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn transform_type(&self) -> &'static str {
        "remove_fields"
    }
}

impl RemoveFields {
    pub fn new(fields: Vec<String>, drop_empty: bool) -> crate::Result<Self> {
        Ok(RemoveFields { fields, drop_empty })
    }
}

impl FunctionTransform<Event> for RemoveFields {
    fn transform(&mut self, output: &mut Vec<Event>, mut event: Event) {
        let log = event.as_mut_log();
        for field in &self.fields {
            let field_string = field.to_string();
            let old_val = log.remove_prune(&field_string, self.drop_empty);
            if old_val.is_none() {
                emit!(RemoveFieldsFieldMissing {
                    field: &field_string
                });
            }
        }

        output.push(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{event::LogEvent, transforms::test::transform_one};

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<RemoveFieldsConfig>();
    }

    #[test]
    fn remove_fields() {
        let mut log = LogEvent::from("message");
        log.insert("to_keep", "another value");
        let expected = log.clone();
        log.insert("to_remove", "some value");

        let mut transform =
            RemoveFields::new(vec!["to_remove".into(), "unknown".into()], false).unwrap();

        let result = transform_one(&mut transform, log.into())
            .unwrap()
            .into_log();

        assert_eq!(result, expected);
    }
}
