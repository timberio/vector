use remap::prelude::*;

#[derive(Clone, Copy, Debug)]
pub struct Replace;

impl Function for Replace {
    fn identifier(&self) -> &'static str {
        "replace"
    }

    fn parameters(&self) -> &'static [Parameter] {
        &[
            Parameter {
                keyword: "value",
                accepts: |v| matches!(v, Value::String(_)),
                required: true,
            },
            Parameter {
                keyword: "pattern",
                accepts: |v| matches!(v, Value::String(_)),
                required: true,
            },
            Parameter {
                keyword: "with",
                accepts: |v| matches!(v, Value::String(_)),
                required: true,
            },
            Parameter {
                keyword: "count",
                accepts: |v| matches!(v, Value::Integer(_)),
                required: false,
            },
        ]
    }

    fn compile(&self, mut arguments: ArgumentList) -> Result<Box<dyn Expression>> {
        let value = arguments.required_expr("value")?;
        let pattern = arguments.required("pattern")?;
        let with = arguments.required_expr("with")?;
        let count = arguments.optional_expr("count")?;

        Ok(Box::new(ReplaceFn {
            value,
            pattern,
            with,
            count,
        }))
    }
}

#[derive(Debug, Clone)]
struct ReplaceFn {
    value: Box<dyn Expression>,
    pattern: Argument,
    with: Box<dyn Expression>,
    count: Option<Box<dyn Expression>>,
}

impl ReplaceFn {
    #[cfg(test)]
    fn new(value: Box<dyn Expression>, pattern: Argument, with: &str, count: Option<i32>) -> Self {
        let with = Box::new(Literal::from(Value::from(with)));
        let count = count.map(Literal::from).map(|v| Box::new(v) as _);

        ReplaceFn {
            value,
            pattern,
            with,
            count,
        }
    }
}

impl Expression for ReplaceFn {
    fn execute(
        &self,
        state: &mut state::Program,
        object: &mut dyn Object,
    ) -> Result<Option<Value>> {
        let value = required!(state, object, self.value, Value::String(b) => String::from_utf8_lossy(&b).into_owned());
        let with = required!(state, object, self.with, Value::String(b) => String::from_utf8_lossy(&b).into_owned());
        let count = optional!(state, object, self.count, Value::Integer(v) => v).unwrap_or(-1);

        match &self.pattern {
            Argument::Expression(expr) => {
                let pattern = required!(state, object, expr, Value::String(b) => String::from_utf8_lossy(&b).into_owned());
                let replaced = match count {
                    i if i > 0 => value.replacen(&pattern, &with, i as usize),
                    i if i < 0 => value.replace(&pattern, &with),
                    _ => value,
                };

                Ok(Some(replaced.into()))
            }
            Argument::Regex(regex) => {
                let replaced = match count {
                    i if i > 0 => regex
                        .replacen(&value, i as usize, with.as_str())
                        .as_bytes()
                        .into(),
                    i if i < 0 => regex.replace_all(&value, with.as_str()).as_bytes().into(),
                    _ => value.into(),
                };

                Ok(Some(replaced))
            }
        }
    }

    fn type_def(&self, state: &state::Compiler) -> TypeDef {
        use value::Kind::*;

        let with_def = self.with.type_def(state).fallible_unless(String);

        let count_def = self
            .count
            .as_ref()
            .map(|count| count.type_def(state).fallible_unless(Integer));

        let pattern_def = match &self.pattern {
            Argument::Expression(expr) => Some(expr.type_def(state).fallible_unless(String)),
            Argument::Regex(_) => None, // regex is a concrete infallible type
        };

        self.value
            .type_def(state)
            .fallible_unless(String)
            .merge(with_def)
            .merge_optional(pattern_def)
            .merge_optional(count_def)
            .with_constraint(String)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::map;

    remap::test_type_def![
        infallible {
            expr: |_| ReplaceFn {
                value: Literal::from("foo").boxed(),
                pattern: regex::Regex::new("foo").unwrap().into(),
                with: Literal::from("foo").boxed(),
                count: None,
            },
            def: TypeDef {
                constraint: value::Kind::String.into(),
                ..Default::default()
            },
        }

        value_fallible {
            expr: |_| ReplaceFn {
                value: Literal::from(10).boxed(),
                pattern: regex::Regex::new("foo").unwrap().into(),
                with: Literal::from("foo").boxed(),
                count: None,
            },
            def: TypeDef {
                fallible: true,
                constraint: value::Kind::String.into(),
                ..Default::default()
            },
        }

        pattern_expression_infallible {
            expr: |_| ReplaceFn {
                value: Literal::from("foo").boxed(),
                pattern: Literal::from("foo").boxed().into(),
                with: Literal::from("foo").boxed(),
                count: None,
            },
            def: TypeDef {
                constraint: value::Kind::String.into(),
                ..Default::default()
            },
        }

        pattern_expression_fallible {
            expr: |_| ReplaceFn {
                value: Literal::from("foo").boxed(),
                pattern: Literal::from(10).boxed().into(),
                with: Literal::from("foo").boxed(),
                count: None,
            },
            def: TypeDef {
                fallible: true,
                constraint: value::Kind::String.into(),
                ..Default::default()
            },
        }

        with_fallible {
            expr: |_| ReplaceFn {
                value: Literal::from("foo").boxed(),
                pattern: regex::Regex::new("foo").unwrap().into(),
                with: Literal::from(10).boxed(),
                count: None,
            },
            def: TypeDef {
                fallible: true,
                constraint: value::Kind::String.into(),
                ..Default::default()
            },
        }

        count_infallible {
            expr: |_| ReplaceFn {
                value: Literal::from("foo").boxed(),
                pattern: regex::Regex::new("foo").unwrap().into(),
                with: Literal::from("foo").boxed(),
                count: Some(Literal::from(10).boxed()),
            },
            def: TypeDef {
                constraint: value::Kind::String.into(),
                ..Default::default()
            },
        }

        count_fallible {
            expr: |_| ReplaceFn {
                value: Literal::from("foo").boxed(),
                pattern: regex::Regex::new("foo").unwrap().into(),
                with: Literal::from("foo").boxed(),
                count: Some(Literal::from("foo").boxed()),
            },
            def: TypeDef {
                fallible: true,
                constraint: value::Kind::String.into(),
                ..Default::default()
            },
        }
    ];

    #[test]
    fn check_replace_string() {
        let cases = vec![
            (
                map![],
                Ok(Some("I like opples ond bononos".into())),
                ReplaceFn::new(
                    Box::new(Literal::from("I like apples and bananas")),
                    Box::new(Literal::from("a")).into(),
                    "o",
                    None,
                ),
            ),
            (
                map![],
                Ok(Some("I like opples ond bononos".into())),
                ReplaceFn::new(
                    Box::new(Literal::from("I like apples and bananas")),
                    Box::new(Literal::from("a")).into(),
                    "o",
                    Some(-1),
                ),
            ),
            (
                map![],
                Ok(Some("I like apples and bananas".into())),
                ReplaceFn::new(
                    Box::new(Literal::from("I like apples and bananas")),
                    Box::new(Literal::from("a")).into(),
                    "o",
                    Some(0),
                ),
            ),
            (
                map![],
                Ok(Some("I like opples and bananas".into())),
                ReplaceFn::new(
                    Box::new(Literal::from("I like apples and bananas")),
                    Box::new(Literal::from("a")).into(),
                    "o",
                    Some(1),
                ),
            ),
            (
                map![],
                Ok(Some("I like opples ond bananas".into())),
                ReplaceFn::new(
                    Box::new(Literal::from("I like apples and bananas")),
                    Box::new(Literal::from("a")).into(),
                    "o",
                    Some(2),
                ),
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

    #[test]
    fn check_replace_regex() {
        let cases = vec![
            (
                map![],
                Ok(Some("I like opples ond bononos".into())),
                ReplaceFn::new(
                    Box::new(Literal::from("I like apples and bananas")),
                    regex::Regex::new("a").unwrap().into(),
                    "o",
                    None,
                ),
            ),
            (
                map![],
                Ok(Some("I like opples ond bononos".into())),
                ReplaceFn::new(
                    Box::new(Literal::from("I like apples and bananas")),
                    regex::Regex::new("a").unwrap().into(),
                    "o",
                    Some(-1),
                ),
            ),
            (
                map![],
                Ok(Some("I like apples and bananas".into())),
                ReplaceFn::new(
                    Box::new(Literal::from("I like apples and bananas")),
                    regex::Regex::new("a").unwrap().into(),
                    "o",
                    Some(0),
                ),
            ),
            (
                map![],
                Ok(Some("I like opples and bananas".into())),
                ReplaceFn::new(
                    Box::new(Literal::from("I like apples and bananas")),
                    regex::Regex::new("a").unwrap().into(),
                    "o",
                    Some(1),
                ),
            ),
            (
                map![],
                Ok(Some("I like opples ond bananas".into())),
                ReplaceFn::new(
                    Box::new(Literal::from("I like apples and bananas")),
                    regex::Regex::new("a").unwrap().into(),
                    "o",
                    Some(2),
                ),
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

    #[test]
    fn check_replace_other() {
        let cases = vec![
            (
                map![],
                Ok(Some("I like biscuits and bananas".into())),
                ReplaceFn::new(
                    Box::new(Literal::from("I like apples and bananas")),
                    Box::new(Literal::from("apples")).into(),
                    "biscuits",
                    None,
                ),
            ),
            (
                map!["foo": "I like apples and bananas"],
                Ok(Some("I like opples and bananas".into())),
                ReplaceFn::new(
                    Box::new(Path::from("foo")),
                    regex::Regex::new("a").unwrap().into(),
                    "o",
                    Some(1),
                ),
            ),
            (
                map!["foo": "I like [apples] and bananas"],
                Ok(Some("I like biscuits and bananas".into())),
                ReplaceFn::new(
                    Box::new(Path::from("foo")),
                    regex::Regex::new("\\[apples\\]").unwrap().into(),
                    "biscuits",
                    None,
                ),
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
