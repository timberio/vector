use crate::{
    conditions::{AnyCondition, Condition},
    config::{DataType, GenerateConfig, GlobalOptions, TransformConfig, TransformDescription},
    event::Event,
    internal_events::SampleEventDiscarded,
    transforms::{FunctionTransform, Transform},
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct SampleConfig {
    pub rate: u64,
    pub key_field: Option<String>,
    pub exclude: Option<AnyCondition>,
}

inventory::submit! {
    TransformDescription::new::<SampleConfig>("sampler")
}

inventory::submit! {
    TransformDescription::new::<SampleConfig>("sample")
}

impl GenerateConfig for SampleConfig {
    fn generate_config() -> toml::Value {
        toml::Value::try_from(Self {
            rate: 10,
            key_field: None,
            exclude: None::<AnyCondition>,
        })
        .unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "sample")]
impl TransformConfig for SampleConfig {
    async fn build(&self, _globals: &GlobalOptions) -> crate::Result<Transform<Event>> {
        Ok(Transform::function(Sample::new(
            self.rate,
            self.key_field.clone(),
            self.exclude
                .as_ref()
                .map(|condition| condition.build())
                .transpose()?,
        )))
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn transform_type(&self) -> &'static str {
        "sample"
    }
}

// Add a compatibility alias to avoid breaking existing configs
#[derive(Deserialize, Serialize, Debug, Clone)]
struct SampleCompatConfig(SampleConfig);

#[async_trait::async_trait]
#[typetag::serde(name = "sampler")]
impl TransformConfig for SampleCompatConfig {
    async fn build(&self, globals: &GlobalOptions) -> crate::Result<Transform<Event>> {
        self.0.build(globals).await
    }

    fn input_type(&self) -> DataType {
        self.0.input_type()
    }

    fn output_type(&self) -> DataType {
        self.0.output_type()
    }

    fn transform_type(&self) -> &'static str {
        self.0.transform_type()
    }
}

#[derive(Clone)]
pub struct Sample {
    rate: u64,
    key_field: Option<String>,
    exclude: Option<Box<dyn Condition>>,
    count: u64,
}

impl Sample {
    pub fn new(rate: u64, key_field: Option<String>, exclude: Option<Box<dyn Condition>>) -> Self {
        Self {
            rate,
            key_field,
            exclude,
            count: 0,
        }
    }
}

impl FunctionTransform<Event> for Sample {
    fn transform(&mut self, output: &mut Vec<Event>, mut event: Event) {
        if let Some(condition) = self.exclude.as_ref() {
            if condition.check(&event) {
                output.push(event);
                return;
            }
        }

        let value = self
            .key_field
            .as_ref()
            .and_then(|key_field| event.as_log().get(key_field))
            .map(|v| v.to_string_lossy());

        let num = if let Some(value) = value {
            seahash::hash(value.as_bytes())
        } else {
            self.count
        };

        self.count = (self.count + 1) % self.rate;

        if num % self.rate == 0 {
            event
                .as_mut_log()
                .insert("sample_rate", self.rate.to_string());
            output.push(event);
        } else {
            emit!(SampleEventDiscarded);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        conditions::{ConditionConfig, VrlConfig},
        config::log_schema,
        event::Event,
        test_util::random_lines,
        transforms::test::transform_one,
    };
    use approx::assert_relative_eq;

    fn condition_contains(key: &str, needle: &str) -> Box<dyn Condition> {
        VrlConfig {
            source: format!(r#"contains!(."{}", "{}")"#, key, needle),
        }
        .build()
        .unwrap()
    }

    #[test]
    fn genreate_config() {
        crate::test_util::test_generate_config::<SampleConfig>();
    }

    #[test]
    fn hash_samples_at_roughly_the_configured_rate() {
        let num_events = 10000;

        let events = random_events(num_events);
        let mut sampler = Sample::new(
            2,
            Some(log_schema().message_key().into()),
            Some(condition_contains(log_schema().message_key(), "na")),
        );
        let total_passed = events
            .into_iter()
            .filter_map(|event| {
                let mut buf = Vec::with_capacity(1);
                sampler.transform(&mut buf, event);
                buf.pop()
            })
            .count();
        let ideal = 1.0f64 / 2.0f64;
        let actual = total_passed as f64 / num_events as f64;
        assert_relative_eq!(ideal, actual, epsilon = ideal * 0.5);

        let events = random_events(num_events);
        let mut sampler = Sample::new(
            25,
            Some(log_schema().message_key().into()),
            Some(condition_contains(log_schema().message_key(), "na")),
        );
        let total_passed = events
            .into_iter()
            .filter_map(|event| {
                let mut buf = Vec::with_capacity(1);
                sampler.transform(&mut buf, event);
                buf.pop()
            })
            .count();
        let ideal = 1.0f64 / 25.0f64;
        let actual = total_passed as f64 / num_events as f64;
        assert_relative_eq!(ideal, actual, epsilon = ideal * 0.5);
    }

    #[test]
    fn hash_consistently_samples_the_same_events() {
        let events = random_events(1000);
        let mut sampler = Sample::new(
            2,
            Some(log_schema().message_key().into()),
            Some(condition_contains(log_schema().message_key(), "na")),
        );

        let first_run = events
            .clone()
            .into_iter()
            .filter_map(|event| {
                let mut buf = Vec::with_capacity(1);
                sampler.transform(&mut buf, event);
                buf.pop()
            })
            .collect::<Vec<_>>();
        let second_run = events
            .into_iter()
            .filter_map(|event| {
                let mut buf = Vec::with_capacity(1);
                sampler.transform(&mut buf, event);
                buf.pop()
            })
            .collect::<Vec<_>>();

        assert_eq!(first_run, second_run);
    }

    #[test]
    fn always_passes_events_matching_pass_list() {
        for key_field in &[None, Some(log_schema().message_key().into())] {
            let event = Event::from("i am important");
            let mut sampler = Sample::new(
                0,
                key_field.clone(),
                Some(condition_contains(log_schema().message_key(), "important")),
            );
            let iterations = 0..1000;
            let total_passed = iterations
                .filter_map(|_| {
                    transform_one(&mut sampler, event.clone())
                        .map(|result| assert_eq!(result, event))
                })
                .count();
            assert_eq!(total_passed, 1000);
        }
    }

    #[test]
    fn handles_key_field() {
        for key_field in &[None, Some("other_field".into())] {
            let mut event = Event::from("nananana");
            let log = event.as_mut_log();
            log.insert("other_field", "foo");
            let mut sampler = Sample::new(
                0,
                key_field.clone(),
                Some(condition_contains("other_field", "foo")),
            );
            let iterations = 0..1000;
            let total_passed = iterations
                .filter_map(|_| {
                    transform_one(&mut sampler, event.clone())
                        .map(|result| assert_eq!(result, event))
                })
                .count();
            assert_eq!(total_passed, 1000);
        }
    }

    #[test]
    fn sampler_adds_sampling_rate_to_event() {
        for key_field in &[None, Some(log_schema().message_key().into())] {
            let events = random_events(10000);
            let mut sampler = Sample::new(
                10,
                key_field.clone(),
                Some(condition_contains(log_schema().message_key(), "na")),
            );
            let passing = events
                .into_iter()
                .filter(|s| {
                    !s.as_log()[log_schema().message_key()]
                        .to_string_lossy()
                        .contains("na")
                })
                .find_map(|event| transform_one(&mut sampler, event))
                .unwrap();
            assert_eq!(passing.as_log()["sample_rate"], "10".into());

            let events = random_events(10000);
            let mut sampler = Sample::new(
                25,
                key_field.clone(),
                Some(condition_contains(log_schema().message_key(), "na")),
            );
            let passing = events
                .into_iter()
                .filter(|s| {
                    !s.as_log()[log_schema().message_key()]
                        .to_string_lossy()
                        .contains("na")
                })
                .find_map(|event| transform_one(&mut sampler, event))
                .unwrap();
            assert_eq!(passing.as_log()["sample_rate"], "25".into());

            // If the event passed the regex check, don't include the sampling rate
            let mut sampler = Sample::new(
                25,
                key_field.clone(),
                Some(condition_contains(log_schema().message_key(), "na")),
            );
            let event = Event::from("nananana");
            let passing = transform_one(&mut sampler, event).unwrap();
            assert!(passing.as_log().get("sample_rate").is_none());
        }
    }

    fn random_events(n: usize) -> Vec<Event> {
        random_lines(10).take(n).map(Event::from).collect()
    }
}
