use parsing::ruby_hash;
use vrl::prelude::*;

#[derive(Clone, Copy, Debug)]
pub struct ParseRubyHash;

impl Function for ParseRubyHash {
    fn identifier(&self) -> &'static str {
        "parse_ruby_hash"
    }

    fn examples(&self) -> &'static [Example] {
        &[Example {
            title: "parse ruby hash",
            source: r#"parse_ruby_hash!(s'{ "test" => "value", "testNum" => 0.2, "testObj" => { "testBool" => true, "testNull" => nil } }')"#,
            result: Ok(r#"
                {
                    "test": "value",
                    "testNum": 0.2,
                    "testObj": {
                        "testBool": true,
                        "testNull": null
                    }
                }
            "#),
        }]
    }

    fn compile(&self, mut arguments: ArgumentList) -> Compiled {
        let value = arguments.required("value");
        Ok(Box::new(ParseRubyHashFn { value }))
    }

    fn parameters(&self) -> &'static [Parameter] {
        &[Parameter {
            keyword: "value",
            kind: kind::BYTES,
            required: true,
        }]
    }
}

#[derive(Debug, Clone)]
struct ParseRubyHashFn {
    value: Box<dyn Expression>,
}

impl Expression for ParseRubyHashFn {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        let value = self.value.resolve(ctx)?;
        let input = value.try_bytes_utf8_lossy()?;
        ruby_hash::parse(&input)
            .map(|v| v.into())
            .map_err(|e| e.into())
    }

    fn type_def(&self, _: &state::Compiler) -> TypeDef {
        type_def()
    }
}

fn kinds() -> Kind {
    Kind::Null | Kind::Bytes | Kind::Float | Kind::Boolean | Kind::Array | Kind::Object
}

fn type_def() -> TypeDef {
    TypeDef::new()
        .fallible()
        .add_object::<(), Kind>(map! { (): kinds() })
}

#[cfg(test)]
mod tests {
    use super::*;

    test_function![
        parse_ruby_hash => ParseRubyHash;

        complete {
            args: func_args![value: value!(r#"{ "test" => "value", "testNum" => 0.2, "testObj" => { "testBool" => true } }"#)],
            want: Ok(value!({
                test: "value",
                testNum: 0.2,
                testObj: {
                    testBool: true
                }
            })),
            tdef: type_def(),
        }
    ];
}
