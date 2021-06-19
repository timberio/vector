use crate::serde::Fields;
use crate::{
    config::{DataType, GenerateConfig, GlobalOptions, TransformConfig, TransformDescription},
    event::{Event, Value},
    internal_events::{
        AddFieldsFieldNotOverwritten, AddFieldsFieldOverwritten, TemplateRenderingFailed,
    },
    template::Template,
    transforms::{FunctionTransform, Transform},
};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use toml::value::Value as TomlValue;

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct AddFieldsConfig {
    pub fields: Fields<TomlValue>,
    #[serde(default = "crate::serde::default_true")]
    pub overwrite: bool,
}

#[derive(Clone)]
enum TemplateOrValue {
    Template(Template),
    Value(Value),
}

impl From<Template> for TemplateOrValue {
    fn from(v: Template) -> Self {
        TemplateOrValue::Template(v)
    }
}

impl From<Value> for TemplateOrValue {
    fn from(v: Value) -> Self {
        TemplateOrValue::Value(v)
    }
}

#[derive(Clone)]
pub struct AddFields {
    fields: IndexMap<String, TemplateOrValue>,
    overwrite: bool,
}

inventory::submit! {
    TransformDescription::new::<AddFieldsConfig>("add_fields")
}

impl GenerateConfig for AddFieldsConfig {
    fn generate_config() -> toml::Value {
        toml::from_str(r#"fields.name = "field_name""#).unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "add_fields")]
impl TransformConfig for AddFieldsConfig {
    async fn build(&self, _globals: &GlobalOptions) -> crate::Result<Transform<Event>> {
        let all_fields = self.fields.clone().all_fields().collect::<IndexMap<_, _>>();
        let mut fields = IndexMap::with_capacity(all_fields.len());
        for (key, value) in all_fields {
            fields.insert(key, Value::try_from(value)?);
        }
        Ok(Transform::function(AddFields::new(fields, self.overwrite)?))
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn transform_type(&self) -> &'static str {
        "add_fields"
    }
}

impl AddFields {
    pub fn new(mut fields: IndexMap<String, Value>, overwrite: bool) -> crate::Result<Self> {
        let mut with_templates = IndexMap::with_capacity(fields.len());
        for (k, v) in fields.drain(..) {
            let maybe_template = match v {
                Value::Bytes(s) => match Template::try_from(String::from_utf8(s.to_vec())?) {
                    Ok(t) => TemplateOrValue::from(t),
                    Err(_) => TemplateOrValue::from(Value::Bytes(s)),
                },
                v => TemplateOrValue::from(v),
            };
            with_templates.insert(k, maybe_template);
        }

        Ok(AddFields {
            fields: with_templates,
            overwrite,
        })
    }
}

impl FunctionTransform<Event> for AddFields {
    fn transform(&mut self, output: &mut Vec<Event>, mut event: Event) {
        for (key, value_or_template) in self.fields.clone() {
            let key_string = key.to_string(); // TODO: Step 6 of https://github.com/timberio/vector/blob/c4707947bd876a0ff7d7aa36717ae2b32b731593/rfcs/2020-05-25-more-usable-logevents.md#sales-pitch.
            let value = match value_or_template {
                TemplateOrValue::Template(v) => match v.render_string(&event) {
                    Ok(v) => v,
                    Err(error) => {
                        emit!(TemplateRenderingFailed {
                            error,
                            field: Some(&key),
                            drop_event: false
                        });
                        continue;
                    }
                }
                .into(),
                TemplateOrValue::Value(v) => v,
            };
            if self.overwrite {
                if event.as_mut_log().insert(&key_string, value).is_some() {
                    emit!(AddFieldsFieldOverwritten { field: &key });
                }
            } else if event.as_mut_log().contains(&key_string) {
                emit!(AddFieldsFieldNotOverwritten { field: &key });
            } else {
                event.as_mut_log().insert(&key_string, value);
            }
        }

        output.push(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::LogEvent;
    use crate::transforms::test::transform_one;
    use std::iter::FromIterator;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<AddFieldsConfig>();
    }

    #[test]
    fn add_fields_event() {
        let log = LogEvent::from("augment me");
        let mut expected = log.clone();
        expected.insert("some_key", "some_val");

        let mut fields = IndexMap::new();
        fields.insert("some_key".into(), "some_val".into());
        let mut augment = AddFields::new(fields, true).unwrap();

        let result = transform_one(&mut augment, log.into()).unwrap();
        assert_eq!(result, expected.into());
    }

    #[test]
    fn add_fields_templating() {
        let log = LogEvent::from("augment me");
        let mut expected = log.clone();
        expected.insert("some_key", "augment me augment me");

        let mut fields = IndexMap::new();
        fields.insert("some_key".into(), "{{message}} {{message}}".into());
        let mut augment = AddFields::new(fields, true).unwrap();

        let result = transform_one(&mut augment, log.into()).unwrap();
        assert_eq!(result, expected.into());
    }

    #[test]
    fn add_fields_overwrite() {
        let mut log = LogEvent::from("");
        log.insert("some_key", "some_message");
        let expected = log.clone();

        let mut fields = IndexMap::new();
        fields.insert("some_key".into(), "some_overwritten_message".into());

        let mut augment = AddFields::new(fields, false).unwrap();

        let result = transform_one(&mut augment, log.into()).unwrap();
        assert_eq!(result, expected.into());
    }

    #[test]
    fn add_fields_preserves_types() {
        crate::test_util::trace_init();
        let log = LogEvent::from("hello world");
        let mut expected = log.clone();
        expected.insert("float", 4.5);
        expected.insert("int", 4);
        expected.insert("string", "thisisastring");
        expected.insert("bool", true);
        expected.insert("array", Value::Array(vec![1.into(), 2.into(), 3.into()]));
        expected.insert(
            "table",
            Value::Map(vec![("key".into(), "value".into())].into_iter().collect()),
        );

        let mut fields = IndexMap::new();
        fields.insert(String::from("float"), Value::from(4.5));
        fields.insert(String::from("int"), Value::from(4));
        fields.insert(String::from("string"), Value::from("thisisastring"));
        fields.insert(String::from("bool"), Value::from(true));
        fields.insert(String::from("array"), Value::from(vec![1_isize, 2, 3]));

        let mut map = IndexMap::new();
        map.insert(String::from("key"), Value::from("value"));

        fields.insert(String::from("table"), Value::from_iter(map));

        let mut augment = AddFields::new(fields, false).unwrap();

        let result = transform_one(&mut augment, log.into()).unwrap();
        assert_eq!(result, expected.into());
    }
}
