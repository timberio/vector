use super::{
    finalization::{BatchNotifier, EventFinalizer},
    legacy_lookup::Segment,
    metadata::EventMetadata,
    util, Lookup, PathComponent, Value,
};
use crate::config::log_schema;
use bytes::Bytes;
use chrono::Utc;
use derivative::Derivative;
use getset::{Getters, MutGetters};
use serde::{Deserialize, Serialize, Serializer};
use shared::EventDataEq;
use std::sync::Arc;
use std::{
    collections::{btree_map::Entry, BTreeMap, HashMap},
    convert::{TryFrom, TryInto},
    fmt::{Debug, Display},
    iter::FromIterator,
};

#[derive(Clone, Debug, Getters, MutGetters, PartialEq, PartialOrd, Derivative, Deserialize)]
pub struct LogEvent {
    // **IMPORTANT:** Due to numerous legacy reasons this **must** be a Map variant.
    #[derivative(Default(value = "Value::from(BTreeMap::default())"))]
    #[serde(flatten)]
    fields: Value,

    #[getset(get = "pub", get_mut = "pub")]
    #[serde(skip)]
    metadata: EventMetadata,
}

impl Default for LogEvent {
    fn default() -> Self {
        Self {
            fields: Value::Map(BTreeMap::new()),
            metadata: EventMetadata::default(),
        }
    }
}

impl LogEvent {
    #[must_use]
    pub fn new_with_metadata(metadata: EventMetadata) -> Self {
        Self {
            fields: Value::Map(Default::default()),
            metadata,
        }
    }

    ///  Create a `LogEvent` into a tuple of its components
    pub fn from_parts(map: BTreeMap<String, Value>, metadata: EventMetadata) -> Self {
        let fields = Value::Map(map);
        Self { fields, metadata }
    }

    /// Convert a `LogEvent` into a tuple of its components
    ///
    /// # Panics
    ///
    /// Panics if the fields of the `LogEvent` are not a `Value::Map`.
    pub fn into_parts(self) -> (BTreeMap<String, Value>, EventMetadata) {
        (
            self.fields
                .into_map()
                .unwrap_or_else(|| unreachable!("fields must be a map")),
            self.metadata,
        )
    }

    pub fn with_batch_notifier(mut self, batch: &Arc<BatchNotifier>) -> Self {
        self.metadata = self.metadata.with_batch_notifier(batch);
        self
    }

    pub fn add_finalizer(&mut self, finalizer: EventFinalizer) {
        self.metadata.add_finalizer(finalizer);
    }

    #[instrument(level = "trace")]
    pub fn get_root(&self) -> &Value {
        &self.fields
    }

    #[instrument(level = "trace", skip(self, key), fields(key = %key.as_ref()))]
    pub fn get(&self, key: impl AsRef<str>) -> Option<&Value> {
        util::log::get(self.as_map(), key.as_ref())
    }

    #[instrument(level = "trace", skip(self, key), fields(key = %key.as_ref()))]
    pub fn get_flat(&self, key: impl AsRef<str>) -> Option<&Value> {
        self.as_map().get(key.as_ref())
    }

    #[instrument(level = "trace", skip(self, key), fields(key = %key.as_ref()))]
    pub fn get_mut(&mut self, key: impl AsRef<str>) -> Option<&mut Value> {
        util::log::get_mut(self.as_map_mut(), key.as_ref())
    }

    #[instrument(level = "trace", skip(self, key), fields(key = %key.as_ref()))]
    pub fn contains(&self, key: impl AsRef<str>) -> bool {
        util::log::contains(self.as_map(), key.as_ref())
    }

    #[instrument(level = "trace", skip(self, key), fields(key = %key.as_ref()))]
    pub fn insert(
        &mut self,
        key: impl AsRef<str>,
        value: impl Into<Value> + Debug,
    ) -> Option<Value> {
        util::log::insert(self.as_map_mut(), key.as_ref(), value.into())
    }

    #[instrument(level = "trace", skip(self, key), fields(key = ?key))]
    pub fn insert_path<V>(&mut self, key: Vec<PathComponent>, value: V) -> Option<Value>
    where
        V: Into<Value> + Debug,
    {
        util::log::insert_path(self.as_map_mut(), key, value.into())
    }

    #[instrument(level = "trace", skip(self, key), fields(key = %key))]
    pub fn insert_flat<K, V>(&mut self, key: K, value: V)
    where
        K: Into<String> + Display,
        V: Into<Value> + Debug,
    {
        self.as_map_mut().insert(key.into(), value.into());
    }

    #[instrument(level = "trace", skip(self, key), fields(key = %key.as_ref()))]
    pub fn try_insert(&mut self, key: impl AsRef<str>, value: impl Into<Value> + Debug) {
        let key = key.as_ref();
        if !self.contains(key) {
            self.insert(key, value);
        }
    }

    #[instrument(level = "trace")]
    pub fn remove_root(&mut self) -> Value {
        let root = self.fields.clone();
        self.fields = BTreeMap::new().into();
        root
    }

    #[instrument(level = "trace", skip(self, key), fields(key = %key.as_ref()))]
    pub fn remove(&mut self, key: impl AsRef<str>) -> Option<Value> {
        util::log::remove(self.as_map_mut(), key.as_ref(), false)
    }

    #[instrument(level = "trace", skip(self, key), fields(key = %key.as_ref()))]
    pub fn remove_prune(&mut self, key: impl AsRef<str>, prune: bool) -> Option<Value> {
        util::log::remove(self.as_map_mut(), key.as_ref(), prune)
    }

    #[instrument(level = "trace", skip(self))]
    pub fn keys<'a>(&'a self) -> impl Iterator<Item = String> + 'a {
        match &self.fields {
            Value::Map(map) => util::log::keys(&map),
            _ => unreachable!(),
        }
    }

    #[instrument(level = "trace", skip(self))]
    pub fn all_fields(&self) -> impl Iterator<Item = (String, &Value)> + Serialize {
        util::log::all_fields(self.as_map())
    }

    #[instrument(level = "trace", skip(self))]
    pub fn is_empty(&self) -> bool {
        self.as_map().is_empty()
    }

    #[instrument(level = "trace", skip(self))]
    pub fn as_map(&self) -> &BTreeMap<String, Value> {
        match &self.fields {
            Value::Map(map) => &map,
            _ => unreachable!(),
        }
    }

    #[instrument(level = "trace", skip(self))]
    pub fn as_map_mut(&mut self) -> &mut BTreeMap<String, Value> {
        match self.fields {
            Value::Map(ref mut map) => map,
            _ => unreachable!(),
        }
    }

    #[instrument(level = "trace", skip(self, lookup), fields(lookup = %lookup), err)]
    fn entry(&mut self, lookup: Lookup) -> crate::Result<Entry<String, Value>> {
        let mut walker = lookup.into_iter().enumerate();

        let mut current_pointer = if let Some((_index, Segment::Field(segment))) = walker.next() {
            self.as_map_mut().entry(segment)
        } else {
            // It should be noted that Remap can create a lookup without a contained segment.
            // This is the root `.` path. That is handled explicitly by the Target implementation
            // on Value so shouldn't reach here.
            // However, we should probably handle this better.
            unreachable!(
                "It is an invariant to have a `Lookup` without a contained `Segment`.\
                `Lookup::is_valid` should catch this during `Lookup` creation, maybe it was not \
                called?."
            );
        };

        for (_index, segment) in walker {
            current_pointer = match (segment, current_pointer) {
                (Segment::Field(field), Entry::Occupied(entry)) => match entry.into_mut() {
                    Value::Map(map) => map.entry(field),
                    v => return Err(format!("Looking up field on a non-map value: {:?}", v).into()),
                },
                (Segment::Field(field), Entry::Vacant(entry)) => {
                    return Err(format!(
                        "Tried to step into `{}` of `{}`, but it did not exist.",
                        field,
                        entry.key()
                    )
                    .into());
                }
                _ => return Err("The entry API cannot yet descend into array indices.".into()),
            };
        }
        Ok(current_pointer)
    }

    /// Merge all fields specified at `fields` from `incoming` to `current`.
    pub fn merge(&mut self, mut incoming: LogEvent, fields: &[impl AsRef<str>]) {
        for field in fields {
            let incoming_val = match incoming.remove(field) {
                None => continue,
                Some(val) => val,
            };
            match self.get_mut(&field) {
                None => {
                    self.insert(field, incoming_val);
                }
                Some(current_val) => current_val.merge(incoming_val),
            }
        }
        self.metadata.merge(incoming.metadata);
    }
}

impl EventDataEq for LogEvent {
    fn event_data_eq(&self, other: &Self) -> bool {
        self.fields == other.fields && self.metadata.event_data_eq(&other.metadata)
    }
}

impl From<Bytes> for LogEvent {
    fn from(message: Bytes) -> Self {
        let mut log = LogEvent::default();

        log.insert(log_schema().message_key(), message);
        log.insert(log_schema().timestamp_key(), Utc::now());

        log
    }
}

impl From<&str> for LogEvent {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

impl From<String> for LogEvent {
    fn from(message: String) -> Self {
        Bytes::from(message).into()
    }
}

impl From<BTreeMap<String, Value>> for LogEvent {
    fn from(map: BTreeMap<String, Value>) -> Self {
        LogEvent {
            fields: Value::Map(map),
            metadata: EventMetadata::default(),
        }
    }
}

impl From<LogEvent> for BTreeMap<String, Value> {
    fn from(event: LogEvent) -> BTreeMap<String, Value> {
        match event.fields {
            Value::Map(map) => map,
            _ => unreachable!(),
        }
    }
}

impl From<HashMap<String, Value>> for LogEvent {
    fn from(map: HashMap<String, Value>) -> Self {
        LogEvent {
            fields: map.into_iter().collect(),
            metadata: EventMetadata::default(),
        }
    }
}

impl<S> From<LogEvent> for HashMap<String, Value, S>
where
    S: std::hash::BuildHasher + Default,
{
    fn from(event: LogEvent) -> HashMap<String, Value, S> {
        let fields: BTreeMap<_, _> = event.into();
        fields.into_iter().collect()
    }
}

impl TryFrom<serde_json::Value> for LogEvent {
    type Error = crate::Error;

    fn try_from(map: serde_json::Value) -> Result<Self, Self::Error> {
        match map {
            serde_json::Value::Object(fields) => Ok(LogEvent::from(
                fields
                    .into_iter()
                    .map(|(k, v)| (k, v.into()))
                    .collect::<BTreeMap<_, _>>(),
            )),
            _ => Err(crate::Error::from(
                "Attempted to convert non-Object JSON into a LogEvent.",
            )),
        }
    }
}

impl TryInto<serde_json::Value> for LogEvent {
    type Error = crate::Error;

    fn try_into(self) -> Result<serde_json::Value, Self::Error> {
        Ok(serde_json::to_value(self.fields)?)
    }
}

impl<T> std::ops::Index<T> for LogEvent
where
    T: AsRef<str>,
{
    type Output = Value;

    fn index(&self, key: T) -> &Value {
        self.get(key.as_ref())
            .expect(&*format!("Key is not found: {:?}", key.as_ref()))
    }
}

impl<K, V> Extend<(K, V)> for LogEvent
where
    K: AsRef<str>,
    V: Into<Value>,
{
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        for (k, v) in iter {
            self.insert(k.as_ref(), v.into());
        }
    }
}

// Allow converting any kind of appropriate key/value iterator directly into a LogEvent.
impl<K: AsRef<str>, V: Into<Value>> FromIterator<(K, V)> for LogEvent {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        let mut log_event = Self::default();
        log_event.extend(iter);
        log_event
    }
}

impl Serialize for LogEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_map(self.as_map().iter())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_util::open_fixture;
    use serde_json::json;
    use std::str::FromStr;

    // This test iterates over the `tests/data/fixtures/log_event` folder and:
    //   * Ensures the EventLog parsed from bytes and turned into a serde_json::Value are equal to the
    //     item being just plain parsed as json.
    //
    // Basically: This test makes sure we aren't mutilating any content users might be sending.
    #[test]
    fn json_value_to_vector_log_event_to_json_value() {
        const FIXTURE_ROOT: &str = "tests/data/fixtures/log_event";

        std::fs::read_dir(FIXTURE_ROOT)
            .unwrap()
            .for_each(|fixture_file| match fixture_file {
                Ok(fixture_file) => {
                    let path = fixture_file.path();
                    tracing::trace!(?path, "Opening.");
                    let serde_value = open_fixture(&path).unwrap();

                    let vector_value = LogEvent::try_from(serde_value.clone()).unwrap();
                    let serde_value_again: serde_json::Value = vector_value.try_into().unwrap();

                    assert_eq!(serde_value, serde_value_again);
                }
                _ => panic!("This test should never read Err'ing test fixtures."),
            });
    }

    // We use `serde_json` pointers in this test to ensure we're validating that Vector correctly inputs and outputs things as expected.
    #[test]
    fn entry() {
        let fixture =
            open_fixture("tests/data/fixtures/log_event/motivatingly-complex.json").unwrap();
        let mut event = LogEvent::try_from(fixture).unwrap();

        let lookup = Lookup::from_str("non-existing").unwrap();
        let entry = event.entry(lookup).unwrap();
        let fallback = json!(
            "If you don't see this, the `LogEvent::entry` API is not working on non-existing lookups."
        );
        entry.or_insert_with(|| fallback.clone().into());
        let json: serde_json::Value = event.clone().try_into().unwrap();
        assert_eq!(json.pointer("/non-existing"), Some(&fallback));

        let lookup = Lookup::from_str("nulled").unwrap();
        let entry = event.entry(lookup).unwrap();
        let fallback = json!(
            "If you see this, the `LogEvent::entry` API is not working on existing, single segment lookups."
        );
        entry.or_insert_with(|| fallback.clone().into());
        let json: serde_json::Value = event.clone().try_into().unwrap();
        assert_eq!(json.pointer("/nulled"), Some(&serde_json::Value::Null));

        let lookup = Lookup::from_str("map.basic").unwrap();
        let entry = event.entry(lookup).unwrap();
        let fallback = json!(
            "If you see this, the `LogEvent::entry` API is not working on existing, double segment lookups."
        );
        entry.or_insert_with(|| fallback.clone().into());
        let json: serde_json::Value = event.clone().try_into().unwrap();
        assert_eq!(
            json.pointer("/map/basic"),
            Some(&serde_json::Value::Bool(true))
        );

        let lookup = Lookup::from_str("map.map.buddy").unwrap();
        let entry = event.entry(lookup).unwrap();
        let fallback = json!(
            "If you see this, the `LogEvent::entry` API is not working on existing, multi-segment lookups."
        );
        entry.or_insert_with(|| fallback.clone().into());
        let json: serde_json::Value = event.clone().try_into().unwrap();
        assert_eq!(
            json.pointer("/map/map/buddy"),
            Some(&serde_json::Value::Number((-1).into()))
        );

        let lookup = Lookup::from_str("map.map.non-existing").unwrap();
        let entry = event.entry(lookup).unwrap();
        let fallback = json!(
            "If you don't see this, the `LogEvent::entry` API is not working on non-existing multi-segment lookups."
        );
        entry.or_insert_with(|| fallback.clone().into());
        let json: serde_json::Value = event.clone().try_into().unwrap();
        assert_eq!(json.pointer("/map/map/non-existing"), Some(&fallback));
    }

    fn assert_merge_value(
        current: impl Into<Value>,
        incoming: impl Into<Value>,
        expected: impl Into<Value>,
    ) {
        let mut merged = current.into();
        merged.merge(incoming.into());
        assert_eq!(merged, expected.into());
    }

    #[test]
    fn merge_value_works_correctly() {
        assert_merge_value("hello ", "world", "hello world");

        assert_merge_value(true, false, false);
        assert_merge_value(false, true, true);

        assert_merge_value("my_val", true, true);
        assert_merge_value(true, "my_val", "my_val");

        assert_merge_value(1, 2, 2);
    }

    #[test]
    fn merge_event_combines_values_accordingly() {
        // Specify the fields that will be merged.
        // Only the ones listed will be merged from the `incoming` event
        // to the `current`.
        let fields_to_merge = vec![
            "merge".to_string(),
            "merge_a".to_string(),
            "merge_b".to_string(),
            "merge_c".to_string(),
        ];

        let current = {
            let mut log = LogEvent::default();

            log.insert("merge", "hello "); // will be concatenated with the `merged` from `incoming`.
            log.insert("do_not_merge", "my_first_value"); // will remain as is, since it's not selected for merging.

            log.insert("merge_a", true); // will be overwritten with the `merge_a` from `incoming` (since it's a non-bytes kind).
            log.insert("merge_b", 123); // will be overwritten with the `merge_b` from `incoming` (since it's a non-bytes kind).

            log.insert("a", true); // will remain as is since it's not selected for merge.
            log.insert("b", 123); // will remain as is since it's not selected for merge.

            // `c` is not present in the `current`, and not selected for merge,
            // so it won't be included in the final event.

            log
        };

        let incoming = {
            let mut log = LogEvent::default();

            log.insert("merge", "world"); // will be concatenated to the `merge` from `current`.
            log.insert("do_not_merge", "my_second_value"); // will be ignored, since it's not selected for merge.

            log.insert("merge_b", 456); // will be merged in as `456`.
            log.insert("merge_c", false); // will be merged in as `false`.

            // `a` will remain as-is, since it's not marked for merge and
            // neither is it specified in the `incoming` event.
            log.insert("b", 456); // `b` not marked for merge, will not change.
            log.insert("c", true); // `c` not marked for merge, will be ignored.

            log
        };

        let mut merged = current;
        merged.merge(incoming, &fields_to_merge);

        let expected = {
            let mut log = LogEvent::default();
            log.insert("merge", "hello world");
            log.insert("do_not_merge", "my_first_value");
            log.insert("a", true);
            log.insert("b", 123);
            log.insert("merge_a", true);
            log.insert("merge_b", 456);
            log.insert("merge_c", false);
            log
        };

        shared::assert_event_data_eq!(merged, expected);
    }
}
