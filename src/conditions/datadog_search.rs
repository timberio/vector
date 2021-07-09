use super::vrl::Vrl;
use crate::conditions::{Condition, ConditionConfig, ConditionDescription};

#[cfg(feature = "transforms-filter")]
use datadog_search_syntax::{build, compile, parse};
use serde::{Deserialize, Serialize};
use vrl::diagnostic::Formatter;

#[derive(Deserialize, Serialize, Debug, Default, Clone, PartialEq)]
pub struct DatadogSearchConfig {
    pub source: String,
}

inventory::submit! {
    ConditionDescription::new::<DatadogSearchConfig>("datadog_search")
}

impl_generate_config_from_default!(DatadogSearchConfig);

#[typetag::serde(name = "datadog_search")]
impl ConditionConfig for DatadogSearchConfig {
    fn build(&self) -> crate::Result<Box<dyn Condition>> {
        let query_node = parse(&self.source)?;

        let program = compile(build(&query_node)).map_err(|diagnostics| {
            Formatter::new(&self.source, diagnostics)
                .colored()
                .to_string()
        })?;

        Ok(Box::new(Vrl {
            program,
            source: self.source.clone(),
        }))
    }
}

//------------------------------------------------------------------------------

#[cfg(test)]
mod test {
    use super::*;
    use crate::log_event;
    use serde_json::json;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<DatadogSearchConfig>();
    }

    #[test]
    fn check_datadog() {
        let checks = vec![
            // Tag exists.
            (
                "_exists_:a",                        // Source
                log_event!["tags" => vec!["a:foo"]], // Pass
                log_event!["tags" => vec!["b:foo"]], // Fail
            ),
            // Tag exists (negate).
            (
                "NOT _exists_:a",
                log_event!["tags" => vec!["b:foo"]],
                log_event!("tags" => vec!["a:foo"]),
            ),
            // Tag exists (negate w/-).
            (
                "-_exists_:a",
                log_event!["tags" => vec!["b:foo"]],
                log_event!["tags" => vec!["a:foo"]],
            ),
            // Facet exists.
            (
                "_exists_:@b",
                log_event!["custom" => json!({"b": "foo"})],
                log_event!["custom" => json!({"a": "foo"})],
            ),
            // Facet exists (negate).
            (
                "NOT _exists_:@b",
                log_event!["custom" => json!({"a": "foo"})],
                log_event!["custom" => json!({"b": "foo"})],
            ),
            // Facet exists (negate w/-).
            (
                "-_exists_:@b",
                log_event!["custom" => json!({"a": "foo"})],
                log_event!["custom" => json!({"b": "foo"})],
            ),
            // Tag doesn't exist.
            (
                "_missing_:a",
                log_event![],
                log_event!["tags" => vec!["a:foo"]],
            ),
            // Tag doesn't exist (negate).
            (
                "NOT _missing_:a",
                log_event!["tags" => vec!["a:foo"]],
                log_event![],
            ),
            // Tag doesn't exist (negate w/-).
            (
                "-_missing_:a",
                log_event!["tags" => vec!["a:foo"]],
                log_event![],
            ),
            // Facet doesn't exist.
            (
                "_missing_:@b",
                log_event!["custom" => json!({"a": "foo"})],
                log_event!["custom" => json!({"b": "foo"})],
            ),
            // Facet doesn't exist (negate).
            (
                "NOT _missing_:@b",
                log_event!["custom" => json!({"b": "foo"})],
                log_event!["custom" => json!({"a": "foo"})],
            ),
            // Facet doesn't exist (negate w/-).
            (
                "-_missing_:@b",
                log_event!["custom" => json!({"b": "foo"})],
                log_event!["custom" => json!({"a": "foo"})],
            ),
            // Keyword.
            ("bla", log_event!["message" => "bla"], log_event![]),
            (
                "foo",
                log_event!["message" => r#"{"key": "foo"}"#],
                log_event![],
            ),
            (
                "bar",
                log_event!["message" => r#"{"nested": {"value": ["foo", "bar"]}}"#],
                log_event![],
            ),
            // Keyword (negate).
            (
                "NOT bla",
                log_event!["message" => "nothing"],
                log_event!["message" => "bla"],
            ),
            (
                "NOT foo",
                log_event![],
                log_event!["message" => r#"{"key": "foo"}"#],
            ),
            (
                "NOT bar",
                log_event![],
                log_event!["message" => r#"{"nested": {"value": ["foo", "bar"]}}"#],
            ),
            // Keyword (negate w/-).
            (
                "-bla",
                log_event!["message" => "nothing"],
                log_event!["message" => "bla"],
            ),
            (
                "-foo",
                log_event![],
                log_event!["message" => r#"{"key": "foo"}"#],
            ),
            (
                "-bar",
                log_event![],
                log_event!["message" => r#"{"nested": {"value": ["foo", "bar"]}}"#],
            ),
            // Quoted keyword.
            (r#""bla""#, log_event!["message" => "bla"], log_event![]),
            (
                r#""foo""#,
                log_event!["message" => r#"{"key": "foo"}"#],
                log_event![],
            ),
            (
                r#""bar""#,
                log_event!["message" => r#"{"nested": {"value": ["foo", "bar"]}}"#],
                log_event![],
            ),
            // Quoted keyword (negate).
            (r#"NOT "bla""#, log_event![], log_event!["message" => "bla"]),
            (
                r#"NOT "foo""#,
                log_event![],
                log_event!["message" => r#"{"key": "foo"}"#],
            ),
            (
                r#"NOT "bar""#,
                log_event![],
                log_event!["message" => r#"{"nested": {"value": ["foo", "bar"]}}"#],
            ),
            // Quoted keyword (negate w/-).
            (r#"-"bla""#, log_event![], log_event!["message" => "bla"]),
            (
                r#"NOT "foo""#,
                log_event![],
                log_event!["message" => r#"{"key": "foo"}"#],
            ),
            (
                r#"NOT "bar""#,
                log_event![],
                log_event!["message" => r#"{"nested": {"value": ["foo", "bar"]}}"#],
            ),
            // Tag match.
            (
                "a:bla",
                log_event!["tags" => vec!["a:bla"]],
                log_event!["tags" => vec!["b:bla"]],
            ),
            // Reserved tag match.
            (
                "host:foo",
                log_event!["host" => "foo"],
                log_event!["tags" => vec!["host:foo"]],
            ),
            (
                "host:foo",
                log_event!["host" => "foo"],
                log_event!["host" => "foobar"],
            ),
            (
                "host:foo",
                log_event!["host" => "foo"],
                log_event!["host" => r#"{"value": "foo"}"#],
            ),
            // Tag match (negate).
            (
                "NOT a:bla",
                log_event!["tags" => vec!["b:bla"]],
                log_event!["tags" => vec!["a:bla"]],
            ),
            // Reserved tag match (negate).
            (
                "NOT host:foo",
                log_event!["tags" => vec!["host:fo  o"]],
                log_event!["host" => "foo"],
            ),
            // Tag match (negate w/-).
            (
                "-a:bla",
                log_event!["tags" => vec!["b:bla"]],
                log_event!["tags" => vec!["a:bla"]],
            ),
            // Reserved tag match (negate w/-).
            (
                "-trace_id:foo",
                log_event![],
                log_event!["trace_id" => "foo"],
            ),
            // Quoted tag match.
            (
                r#"a:"bla""#,
                log_event!["tags" => vec!["a:bla"]],
                log_event!["custom" => json!({"a": "bla"})],
            ),
            // Quoted tag match (negate).
            (
                r#"NOT a:"bla""#,
                log_event!["custom" => json!({"a": "bla"})],
                log_event!["tags" => vec!["a:bla"]],
            ),
            // Quoted tag match (negate w/-).
            (
                r#"-a:"bla""#,
                log_event!["custom" => json!({"a": "bla"})],
                log_event!["tags" => vec!["a:bla"]],
            ),
            // Facet match.
            (
                "@a:bla",
                log_event!["custom" => json!({"a": "bla"})],
                log_event!["tags" => vec!["a:bla"]],
            ),
            // Facet match (negate).
            (
                "NOT @a:bla",
                log_event!["tags" => vec!["a:bla"]],
                log_event!["custom" => json!({"a": "bla"})],
            ),
            // Facet match (negate w/-).
            (
                "-@a:bla",
                log_event!["tags" => vec!["a:bla"]],
                log_event!["custom" => json!({"a": "bla"})],
            ),
            // Quoted facet match.
            (
                r#"@a:"bla""#,
                log_event!["custom" => json!({"a": "bla"})],
                log_event!["tags" => vec!["a:bla"]],
            ),
            // Quoted facet match (negate).
            (
                r#"NOT @a:"bla""#,
                log_event!["tags" => vec!["a:bla"]],
                log_event!["custom" => json!({"a": "bla"})],
            ),
            // Quoted facet match (negate w/-).
            (
                r#"-@a:"bla""#,
                log_event!["tags" => vec!["a:bla"]],
                log_event!["custom" => json!({"a": "bla"})],
            ),
            // Wildcard prefix.
            (
                "*bla",
                log_event!["message" => "foobla"],
                log_event!["message" => "blafoo"],
            ),
            // Wildcard prefix (negate).
            (
                "NOT *bla",
                log_event!["message" => "blafoo"],
                log_event!["message" => "foobla"],
            ),
            // Wildcard prefix (negate w/-).
            (
                "-*bla",
                log_event!["message" => "blafoo"],
                log_event!["message" => "foobla"],
            ),
            // Wildcard suffix.
            (
                "bla*",
                log_event!["message" => "blafoo"],
                log_event!["message" => "foobla"],
            ),
            // Wildcard suffix (negate).
            (
                "NOT bla*",
                log_event!["message" => "foobla"],
                log_event!["message" => "blafoo"],
            ),
            // Wildcard suffix (negate w/-).
            (
                "-bla*",
                log_event!["message" => "foobla"],
                log_event!["message" => "blafoo"],
            ),
            // Multiple wildcards.
            (
                "*b*la*",
                log_event!["custom" => json!({"title": "foobla"})],
                log_event![],
            ),
            // Multiple wildcards (negate).
            (
                "NOT *b*la*",
                log_event![],
                log_event!["custom" => json!({"title": "foobla"})],
            ),
            // Multiple wildcards (negate w/-).
            (
                "-*b*la*",
                log_event![],
                log_event!["custom" => json!({"title": "foobla"})],
            ),
            // Wildcard prefix - tag.
            (
                "a:*bla",
                log_event!["tags" => vec!["a:foobla"]],
                log_event!["tags" => vec!["a:blafoo"]],
            ),
            // Wildcard prefix - tag (negate).
            (
                "NOT a:*bla",
                log_event!["tags" => vec!["a:blafoo"]],
                log_event!["tags" => vec!["a:foobla"]],
            ),
            // Wildcard prefix - tag (negate w/-).
            (
                "-a:*bla",
                log_event!["tags" => vec!["a:blafoo"]],
                log_event!["tags" => vec!["a:foobla"]],
            ),
            // Wildcard suffix - tag.
            (
                "b:bla*",
                log_event!["tags" => vec!["b:blabop"]],
                log_event!["tags" => vec!["b:bopbla"]],
            ),
            // Wildcard suffix - tag (negate).
            (
                "NOT b:bla*",
                log_event!["tags" => vec!["b:bopbla"]],
                log_event!["tags" => vec!["b:blabop"]],
            ),
            // Wildcard suffix - tag (negate w/-).
            (
                "-b:bla*",
                log_event!["tags" => vec!["b:bopbla"]],
                log_event!["tags" => vec!["b:blabop"]],
            ),
            // Multiple wildcards - tag.
            (
                "c:*b*la*",
                log_event!["tags" => vec!["c:foobla"]],
                log_event!["custom" => r#"{"title": "foobla"}"#],
            ),
            // Multiple wildcards - tag (negate).
            (
                "NOT c:*b*la*",
                log_event!["custom" => r#"{"title": "foobla"}"#],
                log_event!["tags" => vec!["c:foobla"]],
            ),
            // Multiple wildcards - tag (negate w/-).
            (
                "-c:*b*la*",
                log_event!["custom" => r#"{"title": "foobla"}"#],
                log_event!["tags" => vec!["c:foobla"]],
            ),
            // Wildcard prefix - facet.
            (
                "@a:*bla",
                log_event!["custom" => json!({"a": "foobla"})],
                log_event!["tags" => vec!["a:foobla"]],
            ),
            // Wildcard prefix - facet (negate).
            (
                "NOT @a:*bla",
                log_event!["tags" => vec!["a:foobla"]],
                log_event!["custom" => json!({"a": "foobla"})],
            ),
            // Wildcard prefix - facet (negate w/-).
            (
                "-@a:*bla",
                log_event!["tags" => vec!["a:foobla"]],
                log_event!["custom" => json!({"a": "foobla"})],
            ),
            // Wildcard suffix - facet.
            (
                "@b:bla*",
                log_event!["custom" => json!({"b": "blabop"})],
                log_event!["tags" => vec!["b:blabop"]],
            ),
            // Wildcard suffix - facet (negate).
            (
                "NOT @b:bla*",
                log_event!["tags" => vec!["b:blabop"]],
                log_event!["custom" => json!({"b": "blabop"})],
            ),
            // Wildcard suffix - facet (negate w/-).
            (
                "-@b:bla*",
                log_event!["tags" => vec!["b:blabop"]],
                log_event!["custom" => json!({"b": "blabop"})],
            ),
            // Multiple wildcards - facet.
            (
                "@c:*b*la*",
                log_event!["custom" => json!({"c": "foobla"})],
                log_event!["tags" => vec!["c:foobla"]],
            ),
            // Multiple wildcards - facet (negate).
            (
                "NOT @c:*b*la*",
                log_event!["tags" => vec!["c:foobla"]],
                log_event!["custom" => json!({"c": "foobla"})],
            ),
            // Multiple wildcards - facet (negate w/-).
            (
                "-@c:*b*la*",
                log_event!["tags" => vec!["c:foobla"]],
                log_event!["custom" => json!({"c": "foobla"})],
            ),
            // Special case for tags.
            (
                "tags:a",
                log_event!["tags" => vec!["a", "b", "c"]],
                log_event!["tags" => vec!["d", "e", "f"]],
            ),
            // Special case for tags (negate).
            (
                "NOT tags:a",
                log_event!["tags" => vec!["d", "e", "f"]],
                log_event!["tags" => vec!["a", "b", "c"]],
            ),
            // Special case for tags (negate w/-).
            (
                "-tags:a",
                log_event!["tags" => vec!["d", "e", "f"]],
                log_event!["tags" => vec!["a", "b", "c"]],
            ),
            // Range - numeric, inclusive.
            (
                "[1 TO 10]",
                log_event!["message" => "1"],
                log_event!["message" => "2"],
            ),
            // Range - numeric, inclusive (negate).
            (
                "NOT [1 TO 10]",
                log_event!["message" => "2"],
                log_event!["message" => "1"],
            ),
            // Range - numeric, inclusive (negate w/-).
            (
                "-[1 TO 10]",
                log_event!["message" => "2"],
                log_event!["message" => "1"],
            ),
            // Range - numeric, inclusive, unbounded (upper).
            (
                "[50 TO *]",
                log_event!["message" => "6"],
                log_event!["message" => "40"],
            ),
            // Range - numeric, inclusive, unbounded (upper) (negate).
            (
                "NOT [50 TO *]",
                log_event!["message" => "40"],
                log_event!["message" => "6"],
            ),
            // Range - numeric, inclusive, unbounded (upper) (negate w/-).
            (
                "-[50 TO *]",
                log_event!["message" => "40"],
                log_event!["message" => "6"],
            ),
            // Range - numeric, inclusive, unbounded (lower).
            (
                "[* TO 50]",
                log_event!["message" => "3"],
                log_event!["message" => "6"],
            ),
            // Range - numeric, inclusive, unbounded (lower) (negate).
            (
                "NOT [* TO 50]",
                log_event!["message" => "6"],
                log_event!["message" => "3"],
            ),
            // Range - numeric, inclusive, unbounded (lower) (negate w/-).
            (
                "-[* TO 50]",
                log_event!["message" => "6"],
                log_event!["message" => "3"],
            ),
            // Range - numeric, inclusive, unbounded (both).
            ("[* TO *]", log_event!["message" => "foo"], log_event![]),
            // Range - numeric, inclusive, unbounded (both) (negate).
            ("NOT [* TO *]", log_event![], log_event!["message" => "foo"]),
            // Range - numeric, inclusive, unbounded (both) (negate w/-i).
            ("-[* TO *]", log_event![], log_event!["message" => "foo"]),
            // Range - numeric, inclusive, tag.
            (
                "a:[1 TO 10]",
                log_event!["tags" => vec!["a:1"]],
                log_event!["tags" => vec!["a:2"]],
            ),
            // Range - numeric, inclusive, tag (negate).
            (
                "NOT a:[1 TO 10]",
                log_event!["tags" => vec!["a:2"]],
                log_event!["tags" => vec!["a:1"]],
            ),
            // Range - numeric, inclusive, tag (negate w/-).
            (
                "-a:[1 TO 10]",
                log_event!["tags" => vec!["a:2"]],
                log_event!["tags" => vec!["a:1"]],
            ),
            // Range - numeric, inclusive, unbounded (upper), tag.
            (
                "a:[50 TO *]",
                log_event!["tags" => vec!["a:6"]],
                log_event!["tags" => vec!["a:40"]],
            ),
            // Range - numeric, inclusive, unbounded (upper), tag (negate).
            (
                "NOT a:[50 TO *]",
                log_event!["tags" => vec!["a:40"]],
                log_event!["tags" => vec!["a:6"]],
            ),
            // Range - numeric, inclusive, unbounded (upper), tag (negate w/-).
            (
                "-a:[50 TO *]",
                log_event!["tags" => vec!["a:40"]],
                log_event!["tags" => vec!["a:6"]],
            ),
            // Range - numeric, inclusive, unbounded (lower), tag.
            (
                "a:[* TO 50]",
                log_event!["tags" => vec!["a:400"]],
                log_event!["tags" => vec!["a:600"]],
            ),
            // Range - numeric, inclusive, unbounded (lower), tag (negate).
            (
                "NOT a:[* TO 50]",
                log_event!["tags" => vec!["a:600"]],
                log_event!["tags" => vec!["a:400"]],
            ),
            // Range - numeric, inclusive, unbounded (lower), tag (negate w/-).
            (
                "-a:[* TO 50]",
                log_event!["tags" => vec!["a:600"]],
                log_event!["tags" => vec!["a:400"]],
            ),
            // Range - numeric, inclusive, unbounded (both), tag.
            (
                "a:[* TO *]",
                log_event!["tags" => vec!["a:test"]],
                log_event!["tags" => vec!["b:test"]],
            ),
            // Range - numeric, inclusive, unbounded (both), tag (negate).
            (
                "NOT a:[* TO *]",
                log_event!["tags" => vec!["b:test"]],
                log_event!["tags" => vec!["a:test"]],
            ),
            // Range - numeric, inclusive, unbounded (both), tag (negate w/-).
            (
                "-a:[* TO *]",
                log_event!["tags" => vec!["b:test"]],
                log_event!["tags" => vec!["a:test"]],
            ),
            // Range - numeric, inclusive, facet.
            (
                "@b:[1 TO 10]",
                log_event!["custom" => json!({"b": 5})],
                log_event!["custom" => json!({"b": 11})],
            ),
            (
                "@b:[1 TO 100]",
                log_event!["custom" => json!({"b": "10"})],
                log_event!["custom" => json!({"b": "2"})],
            ),
            // Range - numeric, inclusive, facet (negate).
            (
                "NOT @b:[1 TO 10]",
                log_event!["custom" => json!({"b": 11})],
                log_event!["custom" => json!({"b": 5})],
            ),
            (
                "NOT @b:[1 TO 100]",
                log_event!["custom" => json!({"b": "2"})],
                log_event!["custom" => json!({"b": "10"})],
            ),
            // Range - numeric, inclusive, facet (negate w/-).
            (
                "-@b:[1 TO 10]",
                log_event!["custom" => json!({"b": 11})],
                log_event!["custom" => json!({"b": 5})],
            ),
            (
                "NOT @b:[1 TO 100]",
                log_event!["custom" => json!({"b": "2"})],
                log_event!["custom" => json!({"b": "10"})],
            ),
            // Range - alpha, inclusive, facet.
            (
                "@b:[a TO z]",
                log_event!["custom" => json!({"b": "c"})],
                log_event!["custom" => json!({"b": 5})],
            ),
            // Range - alphanumeric, inclusive, facet.
            (
                r#"@b:["1" TO "100"]"#,
                log_event!["custom" => json!({"b": "10"})],
                log_event!["custom" => json!({"b": "2"})],
            ),
            // Range - alphanumeric, inclusive, facet (negate).
            (
                r#"NOT @b:["1" TO "100"]"#,
                log_event!["custom" => json!({"b": "2"})],
                log_event!["custom" => json!({"b": "10"})],
            ),
            // Range - alphanumeric, inclusive, facet (negate).
            (
                r#"-@b:["1" TO "100"]"#,
                log_event!["custom" => json!({"b": "2"})],
                log_event!["custom" => json!({"b": "10"})],
            ),
            // Range - tag, exclusive.
            (
                "f:{1 TO 100}",
                log_event!["tags" => vec!["f:10"]],
                log_event!["tags" => vec!["f:1"]],
            ),
            (
                "f:{1 TO 100}",
                log_event!["tags" => vec!["f:10"]],
                log_event!["tags" => vec!["f:100"]],
            ),
            // Range - tag, exclusive (negate).
            (
                "NOT f:{1 TO 100}",
                log_event!["tags" => vec!["f:1"]],
                log_event!["tags" => vec!["f:10"]],
            ),
            (
                "NOT f:{1 TO 100}",
                log_event!["tags" => vec!["f:100"]],
                log_event!["tags" => vec!["f:10"]],
            ),
            // Range - tag, exclusive (negate w/-).
            (
                "-f:{1 TO 100}",
                log_event!["tags" => vec!["f:1"]],
                log_event!["tags" => vec!["f:10"]],
            ),
            (
                "-f:{1 TO 100}",
                log_event!["tags" => vec!["f:100"]],
                log_event!["tags" => vec!["f:10"]],
            ),
            // Range - facet, exclusive.
            (
                "@f:{1 TO 100}",
                log_event!["custom" => json!({"f": 50})],
                log_event!["custom" => json!({"f": 1})],
            ),
            (
                "@f:{1 TO 100}",
                log_event!["custom" => json!({"f": 50})],
                log_event!["custom" => json!({"f": 100})],
            ),
            // Range - facet, exclusive (negate).
            (
                "NOT @f:{1 TO 100}",
                log_event!["custom" => json!({"f": 1})],
                log_event!["custom" => json!({"f": 50})],
            ),
            (
                "NOT @f:{1 TO 100}",
                log_event!["custom" => json!({"f": 100})],
                log_event!["custom" => json!({"f": 50})],
            ),
            // Range - facet, exclusive (negate w/-).
            (
                "-@f:{1 TO 100}",
                log_event!["custom" => json!({"f": 1})],
                log_event!["custom" => json!({"f": 50})],
            ),
            (
                "-@f:{1 TO 100}",
                log_event!["custom" => json!({"f": 100})],
                log_event!["custom" => json!({"f": 50})],
            ),
        ];

        for (source, pass, fail) in checks {
            let config = DatadogSearchConfig {
                source: source.to_owned(),
            };

            // Every query should build successfully.
            let cond = config
                .build()
                .unwrap_or_else(|_| panic!("build failed: {}", source));

            assert!(
                cond.check_with_context(&pass).is_ok(),
                "should pass: {}\nevent: {:?}",
                source,
                pass
            );
            assert!(
                cond.check_with_context(&fail).is_err(),
                "should fail: {}\nevent: {:?}",
                source,
                fail
            );
        }
    }
}
