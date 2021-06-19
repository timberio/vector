use crate::transforms::TaskTransform;
use crate::{
    config::{DataType, GenerateConfig, GlobalOptions, TransformConfig, TransformDescription},
    event::Event,
    internal_events::{
        TagCardinalityLimitRejectingEvent, TagCardinalityLimitRejectingTag,
        TagCardinalityValueLimitReached,
    },
    transforms::Transform,
};
use bloom::{BloomFilter, ASMS};
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::{
    borrow::{Borrow, Cow},
    collections::{HashMap, HashSet},
    fmt,
    future::ready,
    pin::Pin,
};

#[derive(Deserialize, Serialize, Debug, Clone)]
// TODO: add back when serde-rs/serde#1358 is addressed
//#[serde(deny_unknown_fields)]
pub struct TagCardinalityLimitConfig {
    #[serde(default = "default_value_limit")]
    pub value_limit: u32,

    #[serde(default = "default_limit_exceeded_action")]
    pub limit_exceeded_action: LimitExceededAction,

    #[serde(flatten)]
    pub mode: Mode,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Mode {
    Exact,
    Probabilistic(BloomFilterConfig),
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct BloomFilterConfig {
    #[serde(default = "default_cache_size")]
    pub cache_size_per_key: usize,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "limit_exceeded_action", rename_all = "snake_case")]
pub enum LimitExceededAction {
    DropTag,
    DropEvent,
}

#[derive(Debug)]
pub struct TagCardinalityLimit {
    config: TagCardinalityLimitConfig,
    accepted_tags: HashMap<String, TagValueSet>,
}

fn default_limit_exceeded_action() -> LimitExceededAction {
    LimitExceededAction::DropTag
}

fn default_value_limit() -> u32 {
    500
}

fn default_cache_size() -> usize {
    5000 * 1024 // 5KB
}

inventory::submit! {
    TransformDescription::new::<TagCardinalityLimitConfig>("tag_cardinality_limit")
}

impl GenerateConfig for TagCardinalityLimitConfig {
    fn generate_config() -> toml::Value {
        toml::Value::try_from(Self {
            mode: Mode::Exact,
            value_limit: default_value_limit(),
            limit_exceeded_action: default_limit_exceeded_action(),
        })
        .unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "tag_cardinality_limit")]
impl TransformConfig for TagCardinalityLimitConfig {
    async fn build(&self, _globals: &GlobalOptions) -> crate::Result<Transform<Event>> {
        Ok(Transform::task(TagCardinalityLimit::new(self.clone())))
    }

    fn input_type(&self) -> DataType {
        DataType::Metric
    }

    fn output_type(&self) -> DataType {
        DataType::Metric
    }

    fn transform_type(&self) -> &'static str {
        "tag_cardinality_limit"
    }
}

/// Container for storing the set of accepted values for a given tag key.
#[derive(Debug)]
struct TagValueSet {
    storage: TagValueSetStorage,
    num_elements: usize,
}

enum TagValueSetStorage {
    Set(HashSet<String>),
    Bloom(BloomFilter),
}

impl fmt::Debug for TagValueSetStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TagValueSetStorage::Set(set) => write!(f, "Set({:?})", set),
            TagValueSetStorage::Bloom(_) => write!(f, "Bloom"),
        }
    }
}

impl TagValueSet {
    fn new(value_limit: u32, mode: &Mode) -> Self {
        match &mode {
            Mode::Exact => Self {
                storage: TagValueSetStorage::Set(HashSet::with_capacity(value_limit as usize)),
                num_elements: 0,
            },
            Mode::Probabilistic(config) => {
                let num_bits = config.cache_size_per_key / 8; // Convert bytes to bits
                let num_hashes = bloom::optimal_num_hashes(num_bits, value_limit);

                Self {
                    storage: TagValueSetStorage::Bloom(BloomFilter::with_size(
                        num_bits, num_hashes,
                    )),
                    num_elements: 0,
                }
            }
        }
    }

    fn contains(&self, value: Cow<'_, String>) -> bool {
        match &self.storage {
            TagValueSetStorage::Set(set) => set.contains(value.borrow() as &String),
            TagValueSetStorage::Bloom(bloom) => bloom.contains(&value),
        }
    }

    fn len(&self) -> usize {
        self.num_elements
    }

    fn insert(&mut self, value: Cow<'_, String>) -> bool {
        let inserted = match &mut self.storage {
            TagValueSetStorage::Set(set) => set.insert(value.into_owned()),
            TagValueSetStorage::Bloom(bloom) => bloom.insert(&value),
        };
        if inserted {
            self.num_elements += 1
        }
        inserted
    }
}

impl TagCardinalityLimit {
    fn new(config: TagCardinalityLimitConfig) -> TagCardinalityLimit {
        TagCardinalityLimit {
            config,
            accepted_tags: HashMap::new(),
        }
    }

    /// Takes in key and a value corresponding to a tag on an incoming Metric Event.
    /// If that value is already part of set of accepted values for that key, then simply returns
    /// true.  If that value is not yet part of the accepted values for that key, checks whether
    /// we have hit the value_limit for that key yet and if not adds the value to the set of
    /// accepted values for the key and returns true, otherwise returns false.  A false return
    /// value indicates to the caller that the value is not accepted for this key, and the
    /// configured limit_exceeded_action should be taken.
    fn try_accept_tag(&mut self, key: &str, value: Cow<'_, String>) -> bool {
        if !self.accepted_tags.contains_key(key) {
            self.accepted_tags.insert(
                key.to_string(),
                TagValueSet::new(self.config.value_limit, &self.config.mode),
            );
        }
        let tag_value_set = self.accepted_tags.get_mut(key).unwrap();

        if tag_value_set.contains(value.clone()) {
            // Tag value has already been accepted, nothing more to do.
            return true;
        }

        // Tag value not yet part of the accepted set.
        if tag_value_set.len() < self.config.value_limit as usize {
            // accept the new value
            tag_value_set.insert(value);

            if tag_value_set.len() == self.config.value_limit as usize {
                emit!(TagCardinalityValueLimitReached { key });
            }

            true
        } else {
            // New tag value is rejected.
            false
        }
    }

    fn transform_one(&mut self, mut event: Event) -> Option<Event> {
        let metric = event.as_mut_metric();
        if let Some(tags_map) = metric.tags() {
            match self.config.limit_exceeded_action {
                LimitExceededAction::DropEvent => {
                    for (key, value) in tags_map {
                        if !self.try_accept_tag(key, Cow::Borrowed(value)) {
                            emit!(TagCardinalityLimitRejectingEvent {
                                tag_key: &key,
                                tag_value: &value,
                            });
                            return None;
                        }
                    }
                }
                LimitExceededAction::DropTag => {
                    let mut to_delete = Vec::new();
                    for (key, value) in tags_map {
                        if !self.try_accept_tag(key, Cow::Borrowed(value)) {
                            emit!(TagCardinalityLimitRejectingTag {
                                tag_key: &key,
                                tag_value: &value,
                            });
                            to_delete.push(key.clone());
                        }
                    }
                    for key in to_delete {
                        metric.remove_tag(&key);
                    }
                }
            }
        }
        Some(event)
    }
}

impl TaskTransform<Event> for TagCardinalityLimit {
    fn transform(
        self: Box<Self>,
        task: Pin<Box<dyn Stream<Item = Event> + Send>>,
    ) -> Pin<Box<dyn Stream<Item = Event> + Send>>
    where
        Self: 'static,
    {
        let mut inner = self;
        Box::pin(task.filter_map(move |v| ready(inner.transform_one(v))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transforms::tag_cardinality_limit::{default_cache_size, BloomFilterConfig, Mode};
    use crate::{event::metric, event::Event, event::Metric};
    use std::collections::BTreeMap;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<TagCardinalityLimitConfig>();
    }

    fn make_metric(tags: BTreeMap<String, String>) -> Event {
        Event::Metric(
            Metric::new(
                "event",
                metric::MetricKind::Incremental,
                metric::MetricValue::Counter { value: 1.0 },
            )
            .with_tags(Some(tags)),
        )
    }

    fn make_transform_hashset(
        value_limit: u32,
        limit_exceeded_action: LimitExceededAction,
    ) -> TagCardinalityLimit {
        TagCardinalityLimit::new(TagCardinalityLimitConfig {
            value_limit,
            limit_exceeded_action,
            mode: Mode::Exact,
        })
    }

    fn make_transform_bloom(
        value_limit: u32,
        limit_exceeded_action: LimitExceededAction,
    ) -> TagCardinalityLimit {
        TagCardinalityLimit::new(TagCardinalityLimitConfig {
            value_limit,
            limit_exceeded_action,
            mode: Mode::Probabilistic(BloomFilterConfig {
                cache_size_per_key: default_cache_size(),
            }),
        })
    }

    #[test]
    fn tag_cardinality_limit_drop_event_hashset() {
        drop_event(make_transform_hashset(2, LimitExceededAction::DropEvent));
    }

    #[test]
    fn tag_cardinality_limit_drop_event_bloom() {
        drop_event(make_transform_bloom(2, LimitExceededAction::DropEvent));
    }

    fn drop_event(mut transform: TagCardinalityLimit) {
        let tags1: BTreeMap<String, String> =
            vec![("tag1".into(), "val1".into())].into_iter().collect();
        let event1 = make_metric(tags1);

        let tags2: BTreeMap<String, String> =
            vec![("tag1".into(), "val2".into())].into_iter().collect();
        let event2 = make_metric(tags2);

        let tags3: BTreeMap<String, String> =
            vec![("tag1".into(), "val3".into())].into_iter().collect();
        let event3 = make_metric(tags3);

        let new_event1 = transform.transform_one(event1.clone()).unwrap();
        let new_event2 = transform.transform_one(event2.clone()).unwrap();
        let new_event3 = transform.transform_one(event3);

        assert_eq!(new_event1, event1);
        assert_eq!(new_event2, event2);
        // Third value rejected since value_limit is 2.
        assert_eq!(None, new_event3);
    }

    #[test]
    fn tag_cardinality_limit_drop_tag_hashset() {
        drop_tag(make_transform_hashset(2, LimitExceededAction::DropTag));
    }

    #[test]
    fn tag_cardinality_limit_drop_tag_bloom() {
        drop_tag(make_transform_bloom(2, LimitExceededAction::DropTag));
    }

    fn drop_tag(mut transform: TagCardinalityLimit) {
        let tags1: BTreeMap<String, String> = vec![
            ("tag1".into(), "val1".into()),
            ("tag2".into(), "val1".into()),
        ]
        .into_iter()
        .collect();
        let event1 = make_metric(tags1);

        let tags2: BTreeMap<String, String> = vec![
            ("tag1".into(), "val2".into()),
            ("tag2".into(), "val1".into()),
        ]
        .into_iter()
        .collect();
        let event2 = make_metric(tags2);

        let tags3: BTreeMap<String, String> = vec![
            ("tag1".into(), "val3".into()),
            ("tag2".into(), "val1".into()),
        ]
        .into_iter()
        .collect();
        let event3 = make_metric(tags3);

        let new_event1 = transform.transform_one(event1.clone()).unwrap();
        let new_event2 = transform.transform_one(event2.clone()).unwrap();
        let new_event3 = transform.transform_one(event3.clone()).unwrap();

        assert_eq!(new_event1, event1);
        assert_eq!(new_event2, event2);
        // The third event should have been modified to remove "tag1"
        assert_ne!(new_event3, event3);
        assert!(!new_event3.as_metric().tags().unwrap().contains_key("tag1"));
        assert_eq!(
            "val1",
            new_event3.as_metric().tags().unwrap().get("tag2").unwrap()
        );
    }

    #[test]
    fn tag_cardinality_limit_separate_value_limit_per_tag_hashset() {
        separate_value_limit_per_tag(make_transform_hashset(2, LimitExceededAction::DropEvent));
    }

    #[test]
    fn tag_cardinality_limit_separate_value_limit_per_tag_bloom() {
        separate_value_limit_per_tag(make_transform_bloom(2, LimitExceededAction::DropEvent));
    }

    /// Test that hitting the value limit on one tag does not affect the ability to take new
    /// values for other tags.
    fn separate_value_limit_per_tag(mut transform: TagCardinalityLimit) {
        let tags1: BTreeMap<String, String> = vec![
            ("tag1".into(), "val1".into()),
            ("tag2".into(), "val1".into()),
        ]
        .into_iter()
        .collect();
        let event1 = make_metric(tags1);

        let tags2: BTreeMap<String, String> = vec![
            ("tag1".into(), "val2".into()),
            ("tag2".into(), "val1".into()),
        ]
        .into_iter()
        .collect();
        let event2 = make_metric(tags2);

        // Now value limit is reached for "tag1", but "tag2" still has values available.
        let tags3: BTreeMap<String, String> = vec![
            ("tag1".into(), "val1".into()),
            ("tag1".into(), "val2".into()),
        ]
        .into_iter()
        .collect();
        let event3 = make_metric(tags3);

        let new_event1 = transform.transform_one(event1.clone()).unwrap();
        let new_event2 = transform.transform_one(event2.clone()).unwrap();
        let new_event3 = transform.transform_one(event3.clone()).unwrap();

        assert_eq!(new_event1, event1);
        assert_eq!(new_event2, event2);
        assert_eq!(new_event3, event3);
    }
}
