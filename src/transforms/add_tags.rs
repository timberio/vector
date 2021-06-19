use crate::{
    config::{DataType, GenerateConfig, GlobalOptions, TransformConfig, TransformDescription},
    event::Event,
    internal_events::{AddTagsTagNotOverwritten, AddTagsTagOverwritten},
    transforms::{FunctionTransform, Transform},
};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::btree_map::Entry;

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct AddTagsConfig {
    pub tags: IndexMap<String, String>,
    #[serde(default = "crate::serde::default_true")]
    pub overwrite: bool,
}

#[derive(Clone, Debug)]
pub struct AddTags {
    tags: IndexMap<String, String>,
    overwrite: bool,
}

inventory::submit! {
    TransformDescription::new::<AddTagsConfig>("add_tags")
}

impl GenerateConfig for AddTagsConfig {
    fn generate_config() -> toml::Value {
        toml::Value::try_from(Self {
            tags: std::iter::once(("name".to_owned(), "value".to_owned())).collect(),
            overwrite: true,
        })
        .unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "add_tags")]
impl TransformConfig for AddTagsConfig {
    async fn build(&self, _globals: &GlobalOptions) -> crate::Result<Transform<Event>> {
        Ok(Transform::function(AddTags::new(
            self.tags.clone(),
            self.overwrite,
        )))
    }

    fn input_type(&self) -> DataType {
        DataType::Metric
    }

    fn output_type(&self) -> DataType {
        DataType::Metric
    }

    fn transform_type(&self) -> &'static str {
        "add_tags"
    }
}

impl AddTags {
    pub fn new(tags: IndexMap<String, String>, overwrite: bool) -> Self {
        AddTags { tags, overwrite }
    }
}

impl FunctionTransform<Event> for AddTags {
    fn transform(&mut self, output: &mut Vec<Event>, mut event: Event) {
        if !self.tags.is_empty() {
            let metric = event.as_mut_metric();

            for (name, value) in &self.tags {
                let entry = metric.tag_entry(name.to_string());
                match (entry, self.overwrite) {
                    (Entry::Vacant(entry), _) => {
                        entry.insert(value.clone());
                    }
                    (Entry::Occupied(mut entry), true) => {
                        emit!(AddTagsTagOverwritten { tag: name.as_ref() });
                        entry.insert(value.clone());
                    }
                    (Entry::Occupied(_entry), false) => {
                        emit!(AddTagsTagNotOverwritten { tag: name.as_ref() })
                    }
                }
            }
        }

        output.push(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        event::metric::{Metric, MetricKind, MetricValue},
        transforms::test::transform_one,
    };
    use shared::btreemap;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<AddTagsConfig>();
    }

    #[test]
    fn add_tags() {
        let metric = Metric::new(
            "bar",
            MetricKind::Absolute,
            MetricValue::Gauge { value: 10.0 },
        );
        let expected = metric.clone().with_tags(Some(btreemap! {
            "region" => "us-east-1",
            "host" => "localhost",
        }));

        let map = vec![
            ("region".into(), "us-east-1".into()),
            ("host".into(), "localhost".into()),
        ]
        .into_iter()
        .collect();

        let mut transform = AddTags::new(map, true);
        let event = transform_one(&mut transform, metric.into()).unwrap();
        assert_eq!(event, expected.into());
    }

    #[test]
    fn add_tags_override() {
        let metric = Metric::new(
            "bar",
            MetricKind::Absolute,
            MetricValue::Gauge { value: 10.0 },
        )
        .with_tags(Some(btreemap! {"region" => "us-east-1"}));
        let expected = metric.clone();

        let map = vec![("region".to_string(), "overridden".to_string())]
            .into_iter()
            .collect();

        let mut transform = AddTags::new(map, false);
        let event = transform_one(&mut transform, metric.into()).unwrap();
        assert_eq!(event, expected.into());
    }
}
