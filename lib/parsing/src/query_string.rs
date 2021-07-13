use crate::value::Value;
use bytes::Bytes;
use std::collections::BTreeMap;

pub fn parse(bytes: &Bytes) -> Result<Value, String> {
    let mut query_string = bytes.as_ref();
    if !query_string.is_empty() && query_string[0] == b'?' {
        query_string = &query_string[1..];
    }

    let mut result = BTreeMap::new();
    let parsed = form_urlencoded::parse(query_string);
    for (k, value) in parsed {
        let value = value.as_ref();
        result
            .entry(k.into_owned())
            .and_modify(|v| {
                match v {
                    Value::Array(v) => {
                        v.push(value.into());
                    }
                    v => {
                        *v = Value::Array(vec![v.to_owned(), value.into()]);
                    }
                };
            })
            .or_insert_with(|| value.into());
    }
    Ok(result.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::btreemap;

    #[test]
    fn test_parses_complete() {
        let result = parse(&"foo=%2B1&bar=2&xyz=&abc".into()).unwrap();
        assert_eq!(
            result,
            Value::from(btreemap! {
                "foo" => "+1",
                "bar" => "2",
                "xyz" => "",
                "abc" => "",
            })
        );
    }

    #[test]
    fn test_parses_multiple_values() {
        let result = parse(&"foo=bar&foo=xyz".into()).unwrap();
        assert_eq!(
            result,
            Value::from(btreemap! {
                "foo" => vec!["bar", "xyz"],
            })
        );
    }

    #[test]
    fn test_parses_ruby_on_rails_multiple_values() {
        let result = parse(&"?foo%5b%5d=bar&foo%5b%5d=xyz".into()).unwrap();
        assert_eq!(
            result,
            Value::from(btreemap! {
                "foo[]" =>  vec!["bar", "xyz"],
            })
        );
    }

    #[test]
    fn test_parses_empty_key() {
        let result = parse(&"=&=".into()).unwrap();
        assert_eq!(
            result,
            Value::from(btreemap! {
                "" => vec!["", ""],
            })
        );
    }

    #[test]
    fn test_parses_single_key() {
        let result = parse(&"foo".into()).unwrap();
        assert_eq!(
            result,
            Value::from(btreemap! {
                "foo" => "",
            })
        );
    }

    #[test]
    fn test_parses_empty_string() {
        let result = parse(&"".into()).unwrap();
        assert_eq!(result, Value::from(btreemap! {}));
    }

    #[test]
    fn test_parses_if_starts_with_question_mark() {
        let result = parse(&"?foo=bar".into()).unwrap();
        assert_eq!(
            result,
            Value::from(btreemap! {
                "foo" => "bar",
            })
        );
    }
}
