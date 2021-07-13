use lookup::{Field, FieldBuf, Lookup, LookupBuf, Segment, SegmentBuf};
use parsing::value::Value;
use snafu::Snafu;
use std::collections::BTreeMap;
use std::fmt::Debug;

pub fn get_field<'a>(
    source: &Value,
    lookup: impl Into<Lookup<'a>> + Debug,
) -> std::result::Result<Option<&Value>, Error> {
    let mut working_lookup = lookup.into();

    let this_segment = working_lookup.pop_front();
    match (this_segment, source) {
        // We've met an end and found our value.
        (None, item) => Ok(Some(item)),
        // Descend into a map
        (Some(Segment::Field(Field { name, .. })), Value::Map(map)) => match map.get(name) {
            Some(inner) => get_field(inner, working_lookup.clone()),
            None => Ok(None),
        },
        // Descend into an array
        (Some(Segment::Index(i)), Value::Array(array)) => match array.get(i as usize) {
            Some(inner) => get_field(inner, working_lookup.clone()),
            None => Ok(None),
        },
        // anything else is not allowed
        _ => Err(Error::LookupFailed {
            lookup: working_lookup.into_buf(),
        }),
    }
}

pub fn insert_field(
    source: &mut Value,
    lookup: impl Into<LookupBuf> + Debug,
    value: Value,
) -> std::result::Result<Option<Value>, Error> {
    let mut working_lookup: LookupBuf = lookup.into();
    let this_segment = working_lookup.pop_front();
    match (this_segment, source) {
        // We've met an end and found our value.
        (None, item) => {
            let mut value = value;
            core::mem::swap(&mut value, item);
            Ok(Some(value))
        }
        // descend into an array
        (Some(SegmentBuf::Index(i)), ref mut item) => {
            let i = i as usize;
            match item {
                Value::Array(ref mut values) => {
                    if i >= values.len() {
                        values.resize(i + 1, Value::Null);
                    }
                    values[i] = value.clone();
                }
                single_value => {
                    let mut values = Vec::with_capacity(i + 1);
                    if i >= values.len() {
                        values.resize(i + 1, Value::Null);
                    }
                    values[i] = value.clone();
                    let mut value = Value::Array(values);
                    core::mem::swap(&mut value, single_value);
                }
            }
            Ok(Some(value))
        }
        (Some(segment), Value::Boolean(_))
        | (Some(segment), Value::Bytes(_))
        | (Some(segment), Value::Float(_))
        | (Some(segment), Value::Integer(_))
        | (Some(segment), Value::Null)
        | (Some(segment), Value::Array(_)) => Err(Error::InsertionFailed {
            at: segment,
            original_target: working_lookup,
        }),
        // Descend into a map
        (Some(SegmentBuf::Field(FieldBuf { ref name, .. })), Value::Map(ref mut map)) => {
            insert_map(name, working_lookup, map, value)
        }
        (Some(segment), _) => Err(Error::InsertionFailed {
            at: segment,
            original_target: working_lookup,
        }),
    }
}

fn insert_map(
    name: &str,
    mut working_lookup: LookupBuf,
    map: &mut BTreeMap<String, Value>,
    value: Value,
) -> Result<Option<Value>, Error> {
    match working_lookup.get(0) {
        Some(_) => insert_field(
            map.entry(name.to_string())
                .or_insert_with(|| Value::Map(Default::default())),
            working_lookup,
            value,
        ),
        None => Ok(map.insert(name.to_string(), value)),
    }
}

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display(
        "Cannot insert nested value at {}. {} was the original target.",
        at,
        original_target
    ))]
    InsertionFailed {
        at: SegmentBuf,
        original_target: LookupBuf,
    },
    #[snafu(display("Lookup Error at: {}", lookup))]
    LookupFailed { lookup: LookupBuf },
}
