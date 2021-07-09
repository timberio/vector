use datadog_grok::parse_grok_rules::Error as GrokError;
use datadog_grok::parse_grok_rules::GrokRule;
use datadog_grok::{parse_grok, parse_grok_rules};
use std::fmt;
use vrl::{
    diagnostic::{Label, Span},
    prelude::*,
};

#[derive(Debug)]
pub enum Error {
    GrokParsingError(GrokError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::GrokParsingError(err) => write!(f, "{}", err.to_string()),
        }
    }
}

impl std::error::Error for Error {}

impl DiagnosticError for Error {
    fn code(&self) -> usize {
        109
    }

    fn labels(&self) -> Vec<Label> {
        match self {
            Error::GrokParsingError(err) => {
                vec![Label::primary(
                    format!("grok pattern error: {}", err.to_string()),
                    Span::default(),
                )]
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ParseDatadogGrok;

impl Function for ParseDatadogGrok {
    fn identifier(&self) -> &'static str {
        "parse_datadog_grok"
    }

    fn examples(&self) -> &'static [Example] {
        &[Example {
            title: "parses DataDog grok rules",
            source: indoc! {r#"
                value = s'127.0.0.1 - frank [13/Jul/2016:10:55:36 +0000] "GET /apache_pb.gif HTTP/1.0" 200 2326 0.202 "http://www.perdu.com/" "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/55.0.2883.87 Safari/537.36" "-"'

                parse_datadog_grok!(
                    value,
                    parsing_rules : [
                        s'access.common %{_client_ip} %{_ident} %{_auth} \[%{_date_access}\] "(?>%{_method} |)%{_url}(?> %{_version}|)" %{_status_code} (?>%{_bytes_written}|-)',
                        s'access.combined %{access.common} (%{number:duration:scale(1000000000)} )?"%{_referer}" "%{_user_agent}"( "%{_x_forwarded_for}")?.*'
                    ],
                    helper_rules : [
                        s'_auth %{notSpace:http.auth:nullIf("-")}',
                        s'_bytes_written %{integer:network.bytes_written}',
                        s'_client_ip %{ipOrHost:network.client.ip}',
                        s'_version HTTP\/(?<http.version>\d+\.\d+)',
                        s'_url %{notSpace:http.url}',
                        s'_ident %{notSpace:http.ident}',
                        s'_user_agent %{regex("[^\\\"]*"):http.useragent}',
                        s'_referer %{notSpace:http.referer}',
                        s'_status_code %{integer:http.status_code}',
                        s'_method %{word:http.method}',
                        s'_date_access %{date("dd/MMM/yyyy:HH:mm:ss Z"):date_access}',
                        s'_x_forwarded_for %{regex("[^\\\"]*"):http._x_forwarded_for:nullIf("-")}'
                    ]
                )
            "#},
            result: Ok(indoc! {r#"
            {
              "date_access": 1468407336000,
              "duration": 202000000.0,
              "http": {
                "_x_forwarded_for": null,
                "auth": "frank",
                "ident": "-",
                "method": "GET",
                "referer": "http://www.perdu.com/",
                "status_code": 200,
                "url": "/apache_pb.gif",
                "useragent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/55.0.2883.87 Safari/537.36",
                "version": "1.0"
              },
              "network": {
                "bytes_written": 2326,
                "client": {
                  "ip": "127.0.0.1"
                }
              }
            }
            "#}),
        }]
    }

    fn compile(&self, mut arguments: ArgumentList) -> Compiled {
        let value = arguments.required("value");

        let parsing_rules = arguments
            .required_array("parsing_rules")?
            .into_iter()
            .map(|expr| {
                expr.as_value()
                    .ok_or(vrl::function::Error::ExpectedStaticExpression {
                        keyword: "parsing_rules",
                        expr,
                    })
                    .map(|e| {
                        e.try_bytes_utf8_lossy()
                            .expect("should be string")
                            .into_owned()
                    })
            })
            .collect::<std::result::Result<Vec<String>, _>>()?;
        let helper_rules = arguments
            .optional_array("helper_rules")?
            .unwrap_or_default()
            .into_iter()
            .map(|expr| {
                expr.as_value()
                    .ok_or(vrl::function::Error::ExpectedStaticExpression {
                        keyword: "helper_rules",
                        expr,
                    })
                    .map(|e| {
                        e.try_bytes_utf8_lossy()
                            .expect("should be string")
                            .into_owned()
                    })
            })
            .collect::<std::result::Result<Vec<String>, _>>()?;

        let grok_rules = parse_grok_rules::parse_grok_rules(&helper_rules, &parsing_rules)
            .map_err(|e| Box::new(Error::GrokParsingError(e)) as Box<dyn DiagnosticError>)?;

        Ok(Box::new(ParseDatadogGrokFn { value, grok_rules }))
    }

    fn parameters(&self) -> &'static [Parameter] {
        &[
            Parameter {
                keyword: "value",
                kind: kind::BYTES,
                required: true,
            },
            Parameter {
                keyword: "parsing_rules",
                kind: kind::ARRAY,
                required: true,
            },
            Parameter {
                keyword: "helper_rules",
                kind: kind::ARRAY,
                required: false,
            },
        ]
    }
}

#[derive(Clone, Debug)]
struct ParseDatadogGrokFn {
    value: Box<dyn Expression>,
    grok_rules: Vec<GrokRule>,
}

impl Expression for ParseDatadogGrokFn {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        let value = self.value.resolve(ctx)?;
        let bytes = value.try_bytes_utf8_lossy()?;

        let v = parse_grok::parse_grok(bytes.as_ref(), &self.grok_rules)
            .map_err(|e| format!("unable to parse grok: {}", e.to_string()))?;

        Ok(v.into())
    }

    fn type_def(&self, _: &state::Compiler) -> TypeDef {
        TypeDef::new().fallible().object::<(), Kind>(map! {
            (): Kind::all(),
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use shared::btreemap;

    test_function![
        parse_grok => ParseDatadogGrok;

        parses_simple_grok {
            args: func_args![ value: "2020-10-02T23:22:12.223222Z info Hello world",
                              parsing_rules: vec!["simple %{TIMESTAMP_ISO8601:timestamp} %{LOGLEVEL:level} %{GREEDYDATA:message}"]],
            want: Ok(Value::from(btreemap! {
                "timestamp" => "2020-10-02T23:22:12.223222Z",
                "level" => "info",
                "message" => "Hello world",
            })),
            tdef: TypeDef::new().fallible().object::<(), Kind>(map! {
                (): Kind::all(),
            }),
        }

        parses_nginx {
            args: func_args![
                value: r##"127.0.0.1 - frank [13/Jul/2016:10:55:36 +0000] "GET /apache_pb.gif HTTP/1.0" 200 2326 0.202 "http://www.perdu.com/" "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/55.0.2883.87 Safari/537.36" "-""##,
                helper_rules: vec![
                    r#"_auth %{notSpace:http.auth:nullIf("-")}"#,
                    r#"_bytes_written %{integer:network.bytes_written}"#,
                    r#"_client_ip %{ipOrHost:network.client.ip}"#,
                    r#"_version HTTP\/(?<http.version>\d+\.\d+)"#,
                    r#"_url %{notSpace:http.url}"#,
                    r#"_ident %{notSpace:http.ident}"#,
                    r#"_user_agent %{regex("[^\\\"]*"):http.useragent}"#,
                    r#"_referer %{notSpace:http.referer}"#,
                    r#"_status_code %{integer:http.status_code}"#,
                    r#"_method %{word:http.method}"#,
                    r#"_date_access %{date("dd/MMM/yyyy:HH:mm:ss Z"):date_access}"#,
                    r#"_x_forwarded_for %{regex("[^\\\"]*"):http._x_forwarded_for:nullIf("-")}"#],
                parsing_rules: vec![
                    r#"access.common %{_client_ip} %{_ident} %{_auth} \[%{_date_access}\] "(?>%{_method} |)%{_url}(?> %{_version}|)" %{_status_code} (?>%{_bytes_written}|-)"#,
                    r#"access.combined %{access.common} (%{number:duration:scale(1000000000)} )?"%{_referer}" "%{_user_agent}"( "%{_x_forwarded_for}")?.*"#
                ]],
            want: Ok(Value::from(btreemap! {
                "date_access" => "13/Jul/2016:10:55:36 +0000",
                "duration" => 202000000.0,
                "http" => btreemap! {
                    "auth" => "frank",
                    "ident" => "-",
                    "method" => "GET",
                    "status_code" => 200,
                    "url" => "/apache_pb.gif",
                    "version" => "1.0",
                    "referer" => "http://www.perdu.com/",
                    "useragent" => "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/55.0.2883.87 Safari/537.36",
                    "_x_forwarded_for" => Value::Null,
                },
                "network" => btreemap! {
                    "bytes_written" => 2326,
                    "client" => btreemap! {
                        "ip" => "127.0.0.1"
                    }
                }
            })),
            tdef: TypeDef::new().fallible().object::<(), Kind>(map! {
                (): Kind::all(),
            }),
        }

        invalid_rule_format {
            args: func_args![ value: "foo",
                              parsing_rules: vec!["%{data}"]],
            want: Err("failed to parse grok expression '%{data}': format must be: 'ruleName definition'"),
            tdef: TypeDef::new().fallible().object::<(), Kind>(map! {
                (): Kind::all(),
            }),
        }

        unknown_pattern_definition {
            args: func_args![ value: "foo",
                              parsing_rules: vec!["test %{unknown}"]],
            want: Err(r#"failed to parse grok expression '^%{unknown}$': The given pattern definition name "unknown" could not be found in the definition map"#),
            tdef: TypeDef::new().fallible().object::<(), Kind>(map! {
                (): Kind::all(),
            }),
        }

        unknown_filter {
            args: func_args![ value: "foo",
                              parsing_rules: vec!["test %{data:field:unknownFilter}"]],
            want: Err(r#"unknown filter 'unknownFilter'"#),
            tdef: TypeDef::new().fallible().object::<(), Kind>(map! {
                (): Kind::all(),
            }),
        }

        invalid_matcher_parameter {
            args: func_args![ value: "test",
                              parsing_rules: vec!["test_rule %{regex(1):field}"]],
            want: Err(r#"invalid arguments for the function 'regex'"#),
            tdef: TypeDef::new().fallible().object::<(), Kind>(map! {
                (): Kind::all(),
            }),
        }

        invalid_filter_parameter {
            args: func_args![ value: "test",
                              parsing_rules: vec!["test_rule %{data:field:scale()}"]],
            want: Err(r#"invalid arguments for the function 'scale'"#),
            tdef: TypeDef::new().fallible().object::<(), Kind>(map! {
                (): Kind::all(),
            }),
        }

        filter_runtime_error {
            args: func_args![ value: "not a number",
                              parsing_rules: vec!["test_rule %{data:field:number}"]],
            want: Ok(Value::from(btreemap! {
                "field" => Value::Null,
            })),
            tdef: TypeDef::new().fallible().object::<(), Kind>(map! {
                (): Kind::all(),
            }),
        }

        no_match {
            args: func_args![ value: "an ungrokkable message",
                              parsing_rules: vec!["test_rule %{TIMESTAMP_ISO8601:timestamp} %{LOGLEVEL:level} %{GREEDYDATA:message}"]],
            want: Err("unable to parse grok: value does not match any rule"),
            tdef: TypeDef::new().fallible().object::<(), Kind>(map! {
                (): Kind::all(),
            }),
        }

    ];
}
