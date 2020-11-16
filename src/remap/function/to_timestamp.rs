use crate::types::Conversion;
use chrono::{TimeZone, Utc};
use remap::prelude::*;

#[derive(Clone, Copy, Debug)]
pub struct ToTimestamp;

impl Function for ToTimestamp {
    fn identifier(&self) -> &'static str {
        "to_timestamp"
    }

    fn parameters(&self) -> &'static [Parameter] {
        &[
            Parameter {
                keyword: "value",
                accepts: |v| {
                    matches!(
                        v,
                        Value::Integer(_) |
                        Value::Float(_) |
                        Value::String(_) |
                        Value::Timestamp(_)
                    )
                },
                required: true,
            },
            Parameter {
                keyword: "default",
                accepts: |v| {
                    matches!(
                        v,
                        Value::Integer(_) |
                        Value::Float(_) |
                        Value::String(_) |
                        Value::Timestamp(_)
                    )
                },
                required: false,
            },
        ]
    }

    fn compile(&self, mut arguments: ArgumentList) -> Result<Box<dyn Expression>> {
        let value = arguments.required_expr("value")?;
        let default = arguments.optional_expr("default")?;

        Ok(Box::new(ToTimestampFn { value, default }))
    }
}

#[derive(Debug, Clone)]
struct ToTimestampFn {
    value: Box<dyn Expression>,
    default: Option<Box<dyn Expression>>,
}

impl ToTimestampFn {
    #[cfg(test)]
    fn new(value: Box<dyn Expression>, default: Option<Value>) -> Self {
        let default = default.map(|v| Box::new(Literal::from(v)) as _);
        Self { value, default }
    }
}

impl Expression for ToTimestampFn {
    fn execute(
        &self,
        state: &mut state::Program,
        object: &mut dyn Object,
    ) -> Result<Option<Value>> {
        use Value::*;

        let to_timestamp = |value| match value {
            Timestamp(_) => Ok(value),
            Integer(v) => Ok(Timestamp(Utc.timestamp(v, 0))),
            Float(v) => Ok(Timestamp(Utc.timestamp(v.round() as i64, 0))),
            String(_) => Conversion::Timestamp
                .convert(value.into())
                .map(Into::into)
                .map_err(|e| e.to_string().into()),
            Boolean(_) | Array(_) | Map(_) | Null => {
                Err("unable to convert value to timestamp".into())
            }
        };

        super::convert_value_or_default(
            self.value.execute(state, object),
            self.default.as_ref().map(|v| v.execute(state, object)),
            to_timestamp,
        )
    }

    fn type_def(&self, state: &state::Compiler) -> TypeDef {
        use value::Kind::*;

        self.value
            .type_def(state)
            .fallible_unless(vec![Timestamp, Integer, Float])
            .merge_with_default_optional(self.default.as_ref().map(|default| {
                default
                    .type_def(state)
                    .fallible_unless(vec![Timestamp, Integer, Float])
            }))
            .with_constraint(Timestamp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map;
    use std::collections::BTreeMap;
    use value::Kind::*;

    remap::test_type_def![
        timestamp_infallible {
            expr: |_| ToTimestampFn { value: Literal::from(chrono::Utc::now()).boxed(), default: None},
            def: TypeDef { constraint: Timestamp.into(), ..Default::default() },
        }

        integer_infallible {
            expr: |_| ToTimestampFn { value: Literal::from(1).boxed(), default: None},
            def: TypeDef { constraint: Timestamp.into(), ..Default::default() },
        }

        float_infallible {
            expr: |_| ToTimestampFn { value: Literal::from(1.0).boxed(), default: None},
            def: TypeDef { constraint: Timestamp.into(), ..Default::default() },
        }

        null_fallible {
            expr: |_| ToTimestampFn { value: Literal::from(()).boxed(), default: None},
            def: TypeDef { fallible: true, constraint: Timestamp.into(), ..Default::default() },
        }

        string_fallible {
            expr: |_| ToTimestampFn { value: Literal::from("foo").boxed(), default: None},
            def: TypeDef { fallible: true, constraint: Timestamp.into(), ..Default::default() },
        }

        map_fallible {
            expr: |_| ToTimestampFn { value: Literal::from(BTreeMap::new()).boxed(), default: None},
            def: TypeDef { fallible: true, constraint: Timestamp.into(), ..Default::default() },
        }

        array_fallible {
            expr: |_| ToTimestampFn { value: Literal::from(vec![0]).boxed(), default: None},
            def: TypeDef { fallible: true, constraint: Timestamp.into(), ..Default::default() },
        }

        boolean_fallible {
            expr: |_| ToTimestampFn { value: Literal::from(true).boxed(), default: None},
            def: TypeDef { fallible: true, constraint: Timestamp.into(), ..Default::default() },
        }

        fallible_value_without_default {
            expr: |_| ToTimestampFn { value: Variable::new("foo".to_owned()).boxed(), default: None},
            def: TypeDef {
                fallible: true,
                optional: false,
                constraint: Timestamp.into(),
            },
        }

       fallible_value_with_fallible_default {
            expr: |_| ToTimestampFn {
                value: Literal::from(vec![0]).boxed(),
                default: Some(Literal::from(vec![0]).boxed()),
            },
            def: TypeDef {
                fallible: true,
                optional: false,
                constraint: Timestamp.into(),
            },
        }

       fallible_value_with_infallible_default {
            expr: |_| ToTimestampFn {
                value: Literal::from(vec![0]).boxed(),
                default: Some(Literal::from(1).boxed()),
            },
            def: TypeDef {
                fallible: false,
                optional: false,
                constraint: Timestamp.into(),
            },
        }

        infallible_value_with_fallible_default {
            expr: |_| ToTimestampFn {
                value: Literal::from(1).boxed(),
                default: Some(Literal::from(vec![0]).boxed()),
            },
            def: TypeDef {
                fallible: false,
                optional: false,
                constraint: Timestamp.into(),
            },
        }

        infallible_value_with_infallible_default {
            expr: |_| ToTimestampFn {
                value: Literal::from(1).boxed(),
                default: Some(Literal::from(1).boxed()),
            },
            def: TypeDef {
                fallible: false,
                optional: false,
                constraint: Timestamp.into(),
            },
        }
    ];

    #[test]
    fn to_timestamp() {
        let cases = vec![
            (
                map![],
                Err("path error: missing path: foo".into()),
                ToTimestampFn::new(Box::new(Path::from("foo")), None),
            ),
            (
                map![],
                Ok(Some(Utc.timestamp(10, 0).into())),
                ToTimestampFn::new(Box::new(Path::from("foo")), Some(10.into())),
            ),
            (
                map![],
                Ok(Some(Utc.timestamp(10, 0).into())),
                ToTimestampFn::new(
                    Box::new(Path::from("foo")),
                    Some(Utc.timestamp(10, 0).into()),
                ),
            ),
            (
                map![],
                Ok(Some(Value::Timestamp(Utc.timestamp(10, 0)))),
                ToTimestampFn::new(Box::new(Path::from("foo")), Some("10".into())),
            ),
            (
                map!["foo": Utc.timestamp(10, 0)],
                Ok(Some(Value::Timestamp(Utc.timestamp(10, 0)))),
                ToTimestampFn::new(Box::new(Path::from("foo")), None),
            ),
        ];

        let mut state = state::Program::default();

        for (mut object, exp, func) in cases {
            let got = func
                .execute(&mut state, &mut object)
                .map_err(|e| format!("{:#}", anyhow::anyhow!(e)));

            assert_eq!(got, exp);
        }
    }
}
