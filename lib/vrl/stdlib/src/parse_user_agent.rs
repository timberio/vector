use lazy_static::lazy_static;
use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use uaparser::UserAgentParser as UAParser;
use vrl::prelude::*;
use woothee::parser::Parser as WootheeParser;

lazy_static! {
    static ref UA_PARSER: UAParser = {
        let regexes = include_bytes!("./../data/user_agent_regexes.yaml");
        UAParser::from_bytes(regexes).expect("Regex file is not valid.")
    };
}

#[derive(Clone, Copy, Debug)]
pub struct ParseUserAgent;

impl Function for ParseUserAgent {
    fn identifier(&self) -> &'static str {
        "parse_user_agent"
    }

    fn summary(&self) -> &'static str {
        "parse a user agent string"
    }

    fn usage(&self) -> &'static str {
        indoc! {r#"
            Parses the provided `value` as a user agent.

            Parses on the basis of best effort. Returned schema depends only on the configured `mode`,
            so if the function fails to parse a field it will set it to `null`.
        "#}
    }

    fn parameters(&self) -> &'static [Parameter] {
        &[
            Parameter {
                keyword: "value",
                kind: kind::BYTES,
                required: true,
            },
            Parameter {
                keyword: "mode",
                kind: kind::BYTES,
                required: false,
            },
        ]
    }

    fn examples(&self) -> &'static [Example] {
        &[
            Example {
                title: "fast mode",
                source: r#"parse_user_agent!("Mozilla Firefox 1.0.1 Mozilla/5.0 (X11; U; Linux i686; de-DE; rv:1.7.6) Gecko/20050223 Firefox/1.0.1")"#,
                result: Ok(
                    r#"{ "browser": { "family": "Firefox", "version": "1.0.1" }, "device": { "category": "pc" }, "os": { "family": "Linux", "version": null } }"#,
                ),
            },
            Example {
                title: "reliable mode",
                source: r#"parse_user_agent!("Mozilla/4.0 (compatible; MSIE 7.66; Windows NT 5.1; SV1; .NET CLR 1.1.4322)", mode: "reliable"")"#,
                result: Ok(
                    r#"{ "browser": { "family": "Internet Explorer", "version": "7.66" }, "device": { "category": "pc" }, "os": { "family": "Windows XP", "version": "NT 5.1" } }"#,
                ),
            },
            Example {
                title: "enriched mode",
                source: r#"parse_user_agent!("Opera/9.80 (J2ME/MIDP; Opera Mini/4.3.24214; iPhone; CPU iPhone OS 4_2_1 like Mac OS X; AppleWebKit/24.783; U; en) Presto/2.5.25 Version/10.54", mode: "enriched"")"#,
                result: Ok(
                    r#"{ "browser": { "family": "Opera Mini", "major": "4", "minor": "3", "patch": "24214", "version": "10.54" }, "device": { "brand": "Apple", "category": "smartphone", "family": "iPhone", "model": "iPhone" }, "os": { "family": "iOS", "major": "4", "minor": "2", "patch": "1", "patch_minor": null, "version": "4.2.1" } }"#,
                ),
            },
            Example {
                title: "device",
                source: r#"parse_user_agent!("Mozilla/5.0 (Linux; Android 4.4.4; HP Slate 17 Build/KTU84P) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/33.0.0.0 Safari/537.36ESPN APP", mode: "enriched"")"#,
                result: Ok(
                    r#"{ "browser": { "family": "ESPN", "major": null, "minor": null, "patch": null, "version": "33.0.0.0" }, "device": { "brand": "HP", "category": "smartphone", "family": "HP Slate 17", "model": "Slate 17" }, "os": { "family": "Android", "major": "4", "minor": "4", "patch": "4", "patch_minor": null, "version": "4.4.4" } }"#,
                ),
            },
        ]
    }

    fn compile(&self, mut arguments: ArgumentList) -> Compiled {
        let value = arguments.required("value");

        let mode = arguments
            .optional_enum("mode", &Mode::all_value())?
            .map(|s| {
                Mode::from_str(&s.try_bytes_utf8_lossy().expect("mode not bytes"))
                    .expect("validated enum")
            })
            .unwrap_or_default();

        let parser = match mode {
            Mode::Fast => {
                let parser = WootheeParser::new();

                Arc::new(move |s: &str| parser.parse_user_agent(s).partial_schema()) as Arc<_>
            }
            Mode::Reliable => {
                let fast = WootheeParser::new();
                let slow = &UA_PARSER;

                Arc::new(move |s: &str| {
                    let ua = fast.parse_user_agent(s);
                    let ua = if ua.browser.family.is_none() || ua.os.family.is_none() {
                        let better_ua = slow.parse_user_agent(s);
                        better_ua.or(ua)
                    } else {
                        ua
                    };
                    ua.partial_schema()
                }) as Arc<_>
            }
            Mode::Enriched => {
                let fast = WootheeParser::new();
                let slow = &UA_PARSER;

                Arc::new(move |s: &str| {
                    slow.parse_user_agent(s)
                        .or(fast.parse_user_agent(s))
                        .full_schema()
                }) as Arc<_>
            }
        };

        Ok(Box::new(ParseUserAgentFn {
            value,
            mode,
            parser,
        }))
    }
}

#[derive(Clone)]
struct ParseUserAgentFn {
    value: Box<dyn Expression>,
    mode: Mode,
    parser: Arc<dyn Fn(&str) -> Value + Send + Sync>,
}

impl Expression for ParseUserAgentFn {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        let value = self.value.resolve(ctx)?;
        let string = value.try_bytes_utf8_lossy()?;

        Ok((self.parser)(&string))
    }

    fn type_def(&self, _: &state::Compiler) -> TypeDef {
        self.mode.type_def()
    }
}

impl fmt::Debug for ParseUserAgentFn {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "ParseUserAgentFn{{ value: {:?}, mode: {:?}}}",
            self.value, self.mode
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Fast,
    Reliable,
    Enriched,
}

impl Mode {
    fn all_value() -> Vec<Value> {
        use Mode::*;

        vec![Fast, Reliable, Enriched]
            .into_iter()
            .map(|u| u.as_str().into())
            .collect::<Vec<_>>()
    }

    const fn as_str(self) -> &'static str {
        use Mode::*;

        match self {
            Fast => "fast",
            Reliable => "reliable",
            Enriched => "enriched",
        }
    }

    fn type_def(self) -> TypeDef {
        match self {
            Mode::Fast | Mode::Reliable => TypeDef::new()
                .infallible()
                .object::<&'static str, TypeDef>(map! {
                    "browser": TypeDef::new().infallible().object::<&'static str,Kind>(map!{
                        "family": Kind::Bytes | Kind::Null,
                        "version": Kind::Bytes | Kind::Null,
                    }),
                    "os": TypeDef::new().infallible().object::<&'static str,Kind>(map!{
                        "family": Kind::Bytes | Kind::Null,
                        "version": Kind::Bytes | Kind::Null,
                    }),
                    "device": TypeDef::new().infallible().object::<&'static str,Kind>(map!{
                        "category": Kind::Bytes | Kind::Null,
                    }),
                }),
            Mode::Enriched => TypeDef::new()
                .infallible()
                .object::<&'static str, TypeDef>(map! {
                    "browser": TypeDef::new().infallible().object::<&'static str,Kind>(map!{
                        "family": Kind::Bytes | Kind::Null,
                        "version": Kind::Bytes | Kind::Null,
                        "major": Kind::Bytes | Kind::Null,
                        "minor": Kind::Bytes | Kind::Null,
                        "patch": Kind::Bytes | Kind::Null,
                    }),
                    "os": TypeDef::new().infallible().object::<&'static str,Kind>(map!{
                        "family": Kind::Bytes | Kind::Null,
                        "version": Kind::Bytes | Kind::Null,
                        "major": Kind::Bytes | Kind::Null,
                        "minor": Kind::Bytes | Kind::Null,
                        "patch": Kind::Bytes | Kind::Null,
                        "patch_minor":  Kind::Bytes | Kind::Null,
                    }),
                    "device": TypeDef::new().infallible().object::<&'static str,Kind>(map!{
                        "family": Kind::Bytes | Kind::Null,
                        "category": Kind::Bytes | Kind::Null,
                        "brand": Kind::Bytes | Kind::Null,
                        "model": Kind::Bytes | Kind::Null,
                    }),
                }),
        }
    }
}

impl Default for Mode {
    fn default() -> Self {
        Self::Fast
    }
}

impl FromStr for Mode {
    type Err = &'static str;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        use Mode::*;

        match s {
            "fast" => Ok(Fast),
            "reliable" => Ok(Reliable),
            "enriched" => Ok(Enriched),
            _ => Err("unknown mode variant"),
        }
    }
}

#[derive(Default)]
struct UserAgent {
    browser: Browser,
    os: Os,
    device: Device,
}

impl UserAgent {
    fn partial_schema(self) -> Value {
        let Self {
            browser,
            os,
            device,
        } = self;

        IntoIterator::into_iter([
            ("browser", browser.partial_schema()),
            ("os", os.partial_schema()),
            ("device", device.partial_schema()),
        ])
        .map(|(name, value)| (name.to_string(), value))
        .collect()
    }

    fn full_schema(self) -> Value {
        let Self {
            browser,
            os,
            device,
        } = self;

        IntoIterator::into_iter([
            ("browser", browser.full_schema()),
            ("os", os.full_schema()),
            ("device", device.full_schema()),
        ])
        .map(|(name, value)| (name.to_string(), value))
        .collect()
    }

    fn or(self, other: Self) -> Self {
        Self {
            browser: self.browser.or(other.browser),
            os: self.os.or(other.os),
            device: self.device.or(other.device),
        }
    }
}

#[derive(Default)]
struct Browser {
    family: Option<String>,
    version: Option<String>,
    major: Option<String>,
    minor: Option<String>,
    patch: Option<String>,
}

impl Browser {
    fn partial_schema(self) -> Value {
        let Self {
            family, version, ..
        } = self;

        into_value([("family", family), ("version", version)])
    }

    fn full_schema(self) -> Value {
        let Self {
            family,
            version,
            major,
            minor,
            patch,
        } = self;

        into_value([
            ("family", family),
            ("version", version),
            ("major", major),
            ("minor", minor),
            ("patch", patch),
        ])
    }

    fn or(self, other: Self) -> Self {
        Self {
            family: self.family.or(other.family),
            version: self.version.or(other.version),
            major: self.major.or(other.major),
            minor: self.minor.or(other.minor),
            patch: self.patch.or(other.patch),
        }
    }
}

#[derive(Default)]
struct Os {
    family: Option<String>,
    version: Option<String>,
    major: Option<String>,
    minor: Option<String>,
    patch: Option<String>,
    patch_minor: Option<String>,
}

impl Os {
    fn partial_schema(self) -> Value {
        let Self {
            family, version, ..
        } = self;

        into_value([("family", family), ("version", version)])
    }

    fn full_schema(self) -> Value {
        let Self {
            family,
            version,
            major,
            minor,
            patch,
            patch_minor,
        } = self;

        into_value([
            ("family", family),
            ("version", version),
            ("major", major),
            ("minor", minor),
            ("patch", patch),
            ("patch_minor", patch_minor),
        ])
    }

    fn or(self, other: Self) -> Self {
        Self {
            family: self.family.or(other.family),
            version: self.version.or(other.version),
            major: self.major.or(other.major),
            minor: self.minor.or(other.minor),
            patch: self.patch.or(other.patch),
            patch_minor: self.patch_minor.or(other.patch_minor),
        }
    }
}

#[derive(Default)]
struct Device {
    family: Option<String>,
    category: Option<String>,
    brand: Option<String>,
    model: Option<String>,
}

impl Device {
    fn partial_schema(self) -> Value {
        let Self { category, .. } = self;

        into_value([("category", category)])
    }

    fn full_schema(self) -> Value {
        let Self {
            category,
            family,
            brand,
            model,
        } = self;

        into_value([
            ("category", category),
            ("family", family),
            ("brand", brand),
            ("model", model),
        ])
    }

    fn or(self, other: Self) -> Self {
        Self {
            category: self.category.or(other.category),
            family: self.family.or(other.family),
            brand: self.brand.or(other.brand),
            model: self.model.or(other.model),
        }
    }
}

fn into_value<'a>(iter: impl IntoIterator<Item = (&'a str, Option<String>)>) -> Value {
    iter.into_iter()
        .map(|(name, value)| {
            (
                name.to_string(),
                value.map(|s| s.to_owned().into()).unwrap_or(Value::Null),
            )
        })
        .collect()
}

trait Parser {
    fn parse_user_agent(&self, user_agent: &str) -> UserAgent;
}

impl Parser for WootheeParser {
    fn parse_user_agent(&self, user_agent: &str) -> UserAgent {
        fn unknown_to_none<'a>(s: impl Into<Cow<'a, str>>) -> Option<String> {
            let cow = s.into();
            match cow.as_ref() {
                "" | woothee::woothee::VALUE_UNKNOWN => None,
                _ => Some(cow.into_owned()),
            }
        }

        let ua = self.parse(user_agent).unwrap_or_default();

        UserAgent {
            browser: Browser {
                family: unknown_to_none(ua.name),
                version: unknown_to_none(ua.version),
                ..Default::default()
            },
            os: Os {
                family: unknown_to_none(ua.os),
                version: unknown_to_none(ua.os_version),
                ..Default::default()
            },
            device: Device {
                category: unknown_to_none(ua.category),
                ..Default::default()
            },
        }
    }
}

impl Parser for UAParser {
    fn parse_user_agent(&self, user_agent: &str) -> UserAgent {
        fn unknown_to_none(s: impl Into<Option<String>>) -> Option<String> {
            let s = s.into()?;
            match s.as_str() {
                "" | "Other" => None,
                _ => Some(s),
            }
        }

        let ua = <UAParser as uaparser::Parser>::parse(self, user_agent);

        UserAgent {
            browser: Browser {
                family: unknown_to_none(ua.user_agent.family),
                major: unknown_to_none(ua.user_agent.major),
                minor: unknown_to_none(ua.user_agent.minor),
                patch: unknown_to_none(ua.user_agent.patch),
                ..Default::default()
            },
            os: Os {
                family: unknown_to_none(ua.os.family),
                major: unknown_to_none(ua.os.major),
                minor: unknown_to_none(ua.os.minor),
                patch: unknown_to_none(ua.os.patch),
                patch_minor: unknown_to_none(ua.os.patch_minor),
                ..Default::default()
            },
            device: Device {
                family: unknown_to_none(ua.device.family),
                brand: unknown_to_none(ua.device.brand),
                model: unknown_to_none(ua.device.model),
                ..Default::default()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    test_function![
        parse_user_agent => ParseUserAgent;

        parses {
            args: func_args![ value: r#"Mozilla/4.0 (compatible; MSIE 7.66; Windows NT 5.1; SV1)"# ],
            want: Ok(value!({ browser: { family: "Internet Explorer", version: "7.66" }, device: { category: "pc" }, os: { family: "Windows XP", version: "NT 5.1" } })),
            tdef: Mode::Fast.type_def(),
        }

        unknown_user_agent {
            args: func_args![ value: r#"w3m/0.3"#, mode: "enriched"],
            want: Ok(value!({ browser: { family: null, major: null, minor: null, patch: null, version: null }, device: { brand: null, category: null, family: null, model: null }, os: { family: null, major: null, minor: null, patch: null, patch_minor: null, version: null } })),
            tdef: Mode::Enriched.type_def(),
        }
    ];
}
