use crate::{
    conditions::{AnyCondition, Condition},
    config::{DataType, GlobalOptions, TransformConfig, TransformDescription},
    event::{discriminant::Discriminant, Event, EventMetadata, LogEvent},
    internal_events::ReduceStaleEventFlushed,
    transforms::{TaskTransform, Transform},
};
use async_stream::stream;
use futures::{stream, Stream, StreamExt};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::{
    collections::{hash_map, HashMap},
    pin::Pin,
    time::{Duration, Instant},
};

mod merge_strategy;

use merge_strategy::*;

//------------------------------------------------------------------------------

#[derive(Deserialize, Serialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields, default)]
pub struct ReduceConfig {
    pub expire_after_ms: Option<u64>,

    pub flush_period_ms: Option<u64>,

    /// An ordered list of fields to distinguish reduces by. Each
    /// reduce has a separate event merging state.
    #[serde(default)]
    pub group_by: Vec<String>,

    #[serde(default)]
    pub merge_strategies: IndexMap<String, MergeStrategy>,

    /// An optional condition that determines when an event is the end of a
    /// reduce.
    pub ends_when: Option<AnyCondition>,
    pub starts_when: Option<AnyCondition>,
}

inventory::submit! {
    TransformDescription::new::<ReduceConfig>("reduce")
}

impl_generate_config_from_default!(ReduceConfig);

#[async_trait::async_trait]
#[typetag::serde(name = "reduce")]
impl TransformConfig for ReduceConfig {
    async fn build(&self, _globals: &GlobalOptions) -> crate::Result<Transform<Event>> {
        Reduce::new(self).map(Transform::task)
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn transform_type(&self) -> &'static str {
        "reduce"
    }
}

#[derive(Debug)]
struct ReduceState {
    fields: HashMap<String, Box<dyn ReduceValueMerger>>,
    stale_since: Instant,
    metadata: EventMetadata,
}

impl ReduceState {
    fn new(e: LogEvent, strategies: &IndexMap<String, MergeStrategy>) -> Self {
        let (fields, metadata) = e.into_parts();
        Self {
            stale_since: Instant::now(),
            fields: fields
                .into_iter()
                .filter_map(|(k, v)| {
                    if let Some(strat) = strategies.get(&k) {
                        match get_value_merger(v, strat) {
                            Ok(m) => Some((k, m)),
                            Err(error) => {
                                warn!(message = "Failed to create merger.", field = ?k, %error);
                                None
                            }
                        }
                    } else {
                        Some((k, v.into()))
                    }
                })
                .collect(),
            metadata,
        }
    }

    fn add_event(&mut self, e: LogEvent, strategies: &IndexMap<String, MergeStrategy>) {
        let (fields, metadata) = e.into_parts();
        self.metadata.merge(metadata);

        for (k, v) in fields.into_iter() {
            let strategy = strategies.get(&k);
            match self.fields.entry(k) {
                hash_map::Entry::Vacant(entry) => {
                    if let Some(strat) = strategy {
                        match get_value_merger(v, strat) {
                            Ok(m) => {
                                entry.insert(m);
                            }
                            Err(error) => {
                                warn!(message = "Failed to merge value.", %error);
                            }
                        }
                    } else {
                        entry.insert(v.clone().into());
                    }
                }
                hash_map::Entry::Occupied(mut entry) => {
                    if let Err(error) = entry.get_mut().add(v.clone()) {
                        warn!(message = "Failed to merge value.", %error);
                    }
                }
            }
        }
        self.stale_since = Instant::now();
    }

    fn flush(mut self) -> LogEvent {
        let mut event = LogEvent::new_with_metadata(self.metadata);
        for (k, v) in self.fields.drain() {
            if let Err(error) = v.insert_into(k, &mut event) {
                warn!(message = "Failed to merge values for field.", %error);
            }
        }
        event
    }
}

//------------------------------------------------------------------------------

pub struct Reduce {
    expire_after: Duration,
    flush_period: Duration,
    group_by: Vec<String>,
    merge_strategies: IndexMap<String, MergeStrategy>,
    reduce_merge_states: HashMap<Discriminant, ReduceState>,
    ends_when: Option<Box<dyn Condition>>,
    starts_when: Option<Box<dyn Condition>>,
}

impl Reduce {
    pub fn new(config: &ReduceConfig) -> crate::Result<Self> {
        if config.ends_when.is_some() && config.starts_when.is_some() {
            return Err("only one of `ends_when` and `starts_when` can be provided".into());
        }

        let ends_when = config.ends_when.as_ref().map(|c| c.build()).transpose()?;
        let starts_when = config.starts_when.as_ref().map(|c| c.build()).transpose()?;
        let group_by = config.group_by.clone().into_iter().collect();

        Ok(Reduce {
            expire_after: Duration::from_millis(config.expire_after_ms.unwrap_or(30000)),
            flush_period: Duration::from_millis(config.flush_period_ms.unwrap_or(1000)),
            group_by,
            merge_strategies: config.merge_strategies.clone(),
            reduce_merge_states: HashMap::new(),
            ends_when,
            starts_when,
        })
    }

    fn flush_into(&mut self, output: &mut Vec<Event>) {
        let mut flush_discriminants = Vec::new();
        for (k, t) in &self.reduce_merge_states {
            if t.stale_since.elapsed() >= self.expire_after {
                flush_discriminants.push(k.clone());
            }
        }
        for k in &flush_discriminants {
            if let Some(t) = self.reduce_merge_states.remove(k) {
                emit!(ReduceStaleEventFlushed);
                output.push(Event::from(t.flush()));
            }
        }
    }

    fn flush_all_into(&mut self, output: &mut Vec<Event>) {
        self.reduce_merge_states
            .drain()
            .for_each(|(_, s)| output.push(Event::from(s.flush())));
    }

    fn push_or_new_reduce_state(&mut self, event: LogEvent, discriminant: Discriminant) {
        match self.reduce_merge_states.entry(discriminant) {
            hash_map::Entry::Vacant(entry) => {
                entry.insert(ReduceState::new(event, &self.merge_strategies));
            }
            hash_map::Entry::Occupied(mut entry) => {
                entry.get_mut().add_event(event, &self.merge_strategies);
            }
        }
    }

    fn transform_one(&mut self, output: &mut Vec<Event>, event: Event) {
        let starts_here = self
            .starts_when
            .as_ref()
            .map(|c| c.check(&event))
            .unwrap_or(false);
        let ends_here = self
            .ends_when
            .as_ref()
            .map(|c| c.check(&event))
            .unwrap_or(false);

        let event = event.into_log();
        let discriminant = Discriminant::from_log_event(&event, &self.group_by);

        if starts_here {
            if let Some(state) = self.reduce_merge_states.remove(&discriminant) {
                output.push(state.flush().into());
            }

            self.push_or_new_reduce_state(event, discriminant)
        } else if ends_here {
            output.push(match self.reduce_merge_states.remove(&discriminant) {
                Some(mut state) => {
                    state.add_event(event, &self.merge_strategies);
                    state.flush().into()
                }
                None => ReduceState::new(event, &self.merge_strategies)
                    .flush()
                    .into(),
            })
        } else {
            self.push_or_new_reduce_state(event, discriminant)
        }

        self.flush_into(output);
    }
}

impl TaskTransform<Event> for Reduce {
    fn transform(
        self: Box<Self>,
        mut input_rx: Pin<Box<dyn Stream<Item = Event> + Send>>,
    ) -> Pin<Box<dyn Stream<Item = Event> + Send>>
    where
        Self: 'static,
    {
        let mut me = self;

        let poll_period = me.flush_period;

        let mut flush_stream = tokio::time::interval(poll_period);

        Box::pin(
            stream! {
              loop {
                let mut output = Vec::new();
                let done = tokio::select! {
                    _ = flush_stream.tick() => {
                      me.flush_into(&mut output);
                      false
                    }
                    maybe_event = input_rx.next() => {
                      match maybe_event {
                        None => {
                          me.flush_all_into(&mut output);
                          true
                        }
                        Some(event) => {
                          me.transform_one(&mut output, event);
                          false
                        }
                      }
                    }
                };
                yield stream::iter(output.into_iter());
                if done { break }
              }
            }
            .flatten(),
        )
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{
        config::TransformConfig,
        event::{LogEvent, Value},
    };
    use serde_json::json;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<ReduceConfig>();
    }

    #[tokio::test]
    async fn reduce_from_condition() {
        let reduce = toml::from_str::<ReduceConfig>(
            r#"
group_by = [ "request_id" ]

[ends_when]
  type = "check_fields"
  "test_end.exists" = true
"#,
        )
        .unwrap()
        .build(&GlobalOptions::default())
        .await
        .unwrap();
        let reduce = reduce.into_task();

        let mut e_1 = LogEvent::from("test message 1");
        e_1.insert("counter", 1);
        e_1.insert("request_id", "1");
        let metadata_1 = e_1.metadata().clone();

        let mut e_2 = LogEvent::from("test message 2");
        e_2.insert("counter", 2);
        e_2.insert("request_id", "2");
        let metadata_2 = e_2.metadata().clone();

        let mut e_3 = LogEvent::from("test message 3");
        e_3.insert("counter", 3);
        e_3.insert("request_id", "1");

        let mut e_4 = LogEvent::from("test message 4");
        e_4.insert("counter", 4);
        e_4.insert("request_id", "1");
        e_4.insert("test_end", "yep");

        let mut e_5 = LogEvent::from("test message 5");
        e_5.insert("counter", 5);
        e_5.insert("request_id", "2");
        e_5.insert("extra_field", "value1");
        e_5.insert("test_end", "yep");

        let inputs = vec![e_1.into(), e_2.into(), e_3.into(), e_4.into(), e_5.into()];
        let in_stream = Box::pin(stream::iter(inputs));
        let mut out_stream = reduce.transform(in_stream);

        let output_1 = out_stream.next().await.unwrap().into_log();
        assert_eq!(output_1["message"], "test message 1".into());
        assert_eq!(output_1["counter"], Value::from(8));
        assert_eq!(output_1.metadata(), &metadata_1);

        let output_2 = out_stream.next().await.unwrap().into_log();
        assert_eq!(output_2["message"], "test message 2".into());
        assert_eq!(output_2["extra_field"], "value1".into());
        assert_eq!(output_2["counter"], Value::from(7));
        assert_eq!(output_2.metadata(), &metadata_2);
    }

    #[tokio::test]
    async fn reduce_merge_strategies() {
        let reduce = toml::from_str::<ReduceConfig>(
            r#"
group_by = [ "request_id" ]

merge_strategies.foo = "concat"
merge_strategies.bar = "array"
merge_strategies.baz = "max"

[ends_when]
  type = "check_fields"
  "test_end.exists" = true
"#,
        )
        .unwrap()
        .build(&GlobalOptions::default())
        .await
        .unwrap();
        let reduce = reduce.into_task();

        let mut e_1 = LogEvent::from("test message 1");
        e_1.insert("foo", "first foo");
        e_1.insert("bar", "first bar");
        e_1.insert("baz", 2);
        e_1.insert("request_id", "1");
        let metadata = e_1.metadata().clone();

        let mut e_2 = LogEvent::from("test message 2");
        e_2.insert("foo", "second foo");
        e_2.insert("bar", 2);
        e_2.insert("baz", "not number");
        e_2.insert("request_id", "1");

        let mut e_3 = LogEvent::from("test message 3");
        e_3.insert("foo", 10);
        e_3.insert("bar", "third bar");
        e_3.insert("baz", 3);
        e_3.insert("request_id", "1");
        e_3.insert("test_end", "yep");

        let inputs = vec![e_1.into(), e_2.into(), e_3.into()];
        let in_stream = Box::pin(stream::iter(inputs));
        let mut out_stream = reduce.transform(in_stream);

        let output_1 = out_stream.next().await.unwrap().into_log();
        assert_eq!(output_1["message"], "test message 1".into());
        assert_eq!(output_1["foo"], "first foo second foo".into());
        assert_eq!(
            output_1["bar"],
            Value::Array(vec!["first bar".into(), 2.into(), "third bar".into()]),
        );
        assert_eq!(output_1["baz"], 3.into());
        assert_eq!(output_1.metadata(), &metadata);
    }

    #[tokio::test]
    async fn missing_group_by() {
        let reduce = toml::from_str::<ReduceConfig>(
            r#"
group_by = [ "request_id" ]

[ends_when]
  type = "check_fields"
  "test_end.exists" = true
"#,
        )
        .unwrap()
        .build(&GlobalOptions::default())
        .await
        .unwrap();
        let reduce = reduce.into_task();

        let mut e_1 = LogEvent::from("test message 1");
        e_1.insert("counter", 1);
        e_1.insert("request_id", "1");
        let metadata_1 = e_1.metadata().clone();

        let mut e_2 = LogEvent::from("test message 2");
        e_2.insert("counter", 2);
        let metadata_2 = e_2.metadata().clone();

        let mut e_3 = LogEvent::from("test message 3");
        e_3.insert("counter", 3);
        e_3.insert("request_id", "1");

        let mut e_4 = LogEvent::from("test message 4");
        e_4.insert("counter", 4);
        e_4.insert("request_id", "1");
        e_4.insert("test_end", "yep");

        let mut e_5 = LogEvent::from("test message 5");
        e_5.insert("counter", 5);
        e_5.insert("extra_field", "value1");
        e_5.insert("test_end", "yep");

        let inputs = vec![e_1.into(), e_2.into(), e_3.into(), e_4.into(), e_5.into()];
        let in_stream = Box::pin(stream::iter(inputs));
        let mut out_stream = reduce.transform(in_stream);

        let output_1 = out_stream.next().await.unwrap().into_log();
        assert_eq!(output_1["message"], "test message 1".into());
        assert_eq!(output_1["counter"], Value::from(8));
        assert_eq!(output_1.metadata(), &metadata_1);

        let output_2 = out_stream.next().await.unwrap().into_log();
        assert_eq!(output_2["message"], "test message 2".into());
        assert_eq!(output_2["extra_field"], "value1".into());
        assert_eq!(output_2["counter"], Value::from(7));
        assert_eq!(output_2.metadata(), &metadata_2);
    }

    #[tokio::test]
    async fn arrays() {
        let reduce = toml::from_str::<ReduceConfig>(
            r#"
group_by = [ "request_id" ]

merge_strategies.foo = "array"
merge_strategies.bar = "concat"

[ends_when]
  type = "check_fields"
  "test_end.exists" = true
"#,
        )
        .unwrap()
        .build(&GlobalOptions::default())
        .await
        .unwrap();
        let reduce = reduce.into_task();

        let mut e_1 = LogEvent::from("test message 1");
        e_1.insert("foo", json!([1, 3]));
        e_1.insert("bar", json!([1, 3]));
        e_1.insert("request_id", "1");
        let metadata_1 = e_1.metadata().clone();

        let mut e_2 = LogEvent::from("test message 2");
        e_2.insert("foo", json!([2, 4]));
        e_2.insert("bar", json!([2, 4]));
        e_2.insert("request_id", "2");
        let metadata_2 = e_2.metadata().clone();

        let mut e_3 = LogEvent::from("test message 3");
        e_3.insert("foo", json!([5, 7]));
        e_3.insert("bar", json!([5, 7]));
        e_3.insert("request_id", "1");

        let mut e_4 = LogEvent::from("test message 4");
        e_4.insert("foo", json!("done"));
        e_4.insert("bar", json!("done"));
        e_4.insert("request_id", "1");
        e_4.insert("test_end", "yep");

        let mut e_5 = LogEvent::from("test message 5");
        e_5.insert("foo", json!([6, 8]));
        e_5.insert("bar", json!([6, 8]));
        e_5.insert("request_id", "2");

        let mut e_6 = LogEvent::from("test message 6");
        e_6.insert("foo", json!("done"));
        e_6.insert("bar", json!("done"));
        e_6.insert("request_id", "2");
        e_6.insert("test_end", "yep");

        let inputs = vec![
            e_1.into(),
            e_2.into(),
            e_3.into(),
            e_4.into(),
            e_5.into(),
            e_6.into(),
        ];
        let in_stream = Box::pin(stream::iter(inputs));
        let mut out_stream = reduce.transform(in_stream);

        let output_1 = out_stream.next().await.unwrap().into_log();
        assert_eq!(output_1["foo"], json!([[1, 3], [5, 7], "done"]).into());
        assert_eq!(output_1["bar"], json!([1, 3, 5, 7, "done"]).into());
        assert_eq!(output_1.metadata(), &metadata_1);

        let output_2 = out_stream.next().await.unwrap().into_log();
        assert_eq!(output_2["foo"], json!([[2, 4], [6, 8], "done"]).into());
        assert_eq!(output_2["bar"], json!([2, 4, 6, 8, "done"]).into());
        assert_eq!(output_2.metadata(), &metadata_2);
    }
}
