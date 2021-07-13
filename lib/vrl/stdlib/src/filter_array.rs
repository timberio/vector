use vrl::prelude::*;

#[derive(Clone, Copy, Debug)]
pub struct FilterArray;

impl Function for FilterArray {
    fn identifier(&self) -> &'static str {
        "filter_array"
    }

    fn examples(&self) -> &'static [Example] {
        &[
            Example {
                title: "some item match",
                source: r#"filter_array(["foobar", "bazqux"], r'foo')"#,
                result: Ok(r#"["foobar"]"#),
            },
            Example {
                title: "no match",
                source: r#"filter_array(["bazqux", "xyz"], r'foo')"#,
                result: Ok(r#"[]"#),
            },
        ]
    }

    fn compile(&self, mut arguments: ArgumentList) -> Compiled {
        let value = arguments.required("value");
        let pattern = arguments.required("pattern");

        Ok(Box::new(FilterArrayFn { value, pattern }))
    }

    fn parameters(&self) -> &'static [Parameter] {
        &[
            Parameter {
                keyword: "value",
                kind: kind::ARRAY,
                required: true,
            },
            Parameter {
                keyword: "pattern",
                kind: kind::REGEX,
                required: true,
            },
        ]
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FilterArrayFn {
    value: Box<dyn Expression>,
    pattern: Box<dyn Expression>,
}

impl Expression for FilterArrayFn {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        let mut list = self.value.resolve(ctx)?.try_array()?;
        let pattern = self.pattern.resolve(ctx)?.try_regex()?;

        let matcher = |i: &Value| match i.try_bytes_utf8_lossy() {
            Ok(v) => pattern.is_match(&v),
            _ => false,
        };
        list.retain(matcher);
        Ok(list.into())
    }

    fn type_def(&self, _: &state::Compiler) -> TypeDef {
        TypeDef::new()
            .infallible()
            .array_mapped::<(), Kind>(map! {(): Kind::Bytes})
    }
}

#[cfg(test)]
#[allow(clippy::trivial_regex)]
mod tests {
    use super::*;
    use regex::Regex;

    test_function![
        filter_array => FilterArray;

        all {
            args: func_args![
                value: value!(["foo", "foobar", "barfoo"]),
                pattern: Value::Regex(Regex::new("foo").unwrap().into())
            ],
            want: Ok(value!(["foo", "foobar", "barfoo"])),
            tdef: TypeDef::new()
                .infallible()
                .array_mapped::<(), Kind>(map! {(): Kind::Bytes}),
        }

        partial {
            args: func_args![
                value: value!(["foo", "foobar", "baz"]),
                pattern: Value::Regex(Regex::new("foo").unwrap().into()),
                all: value!(true),
            ],
            want: Ok(value!(["foo", "foobar"])),
            tdef: TypeDef::new()
                .infallible()
                .array_mapped::<(), Kind>(map! {(): Kind::Bytes}),
        }

        none {
            args: func_args![
                value: value!(["yyz", "yvr", "yul"]),
                pattern: Value::Regex(Regex::new("foo").unwrap().into()),
                all: value!(true),
            ],
            want: Ok(value!([])),
            tdef: TypeDef::new()
                .infallible()
                .array_mapped::<(), Kind>(map! {(): Kind::Bytes}),
        }

        mixed_values {
            args: func_args![
                value: value!(["foo", "123abc", 1, true, [1,2,3]]),
                pattern: Value::Regex(Regex::new("abc").unwrap().into())
            ],
            want: Ok(value!(["123abc"])),
            tdef: TypeDef::new()
                .infallible()
                .array_mapped::<(), Kind>(map! {(): Kind::Bytes}),
        }

        mixed_values_no_match {
            args: func_args![
                value: value!(["foo", "123abc", 1, true, [1,2,3]]),
                pattern: Value::Regex(Regex::new("xyz").unwrap().into()),
            ],
            want: Ok(value!([])),
            tdef: TypeDef::new()
                .infallible()
                .array_mapped::<(), Kind>(map! {(): Kind::Bytes}),
        }
    ];
}
