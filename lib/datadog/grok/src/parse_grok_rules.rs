use std::collections::HashMap;
use std::convert::TryFrom;
use std::fmt::Write;
use std::sync::Arc;

use grok::Grok;
use lazy_static::lazy_static;
use regex::Regex;

use lookup::LookupBuf;
use parsing::value::Value;

use crate::ast::{self, Destination, GrokPattern};
use crate::date;
use crate::grok_filter::GrokFilter;
use crate::parse_grok_pattern::parse_grok_pattern;
use itertools::{Itertools, Position};
use tracing::error;

#[derive(Debug, Clone)]
pub struct GrokRule {
    pub pattern: Arc<grok::Pattern>,
    pub filters: HashMap<LookupBuf, Vec<GrokFilter>>,
}

#[derive(thiserror::Error, Debug, PartialEq)]
pub enum Error {
    #[error("failed to parse grok expression '{}': {}", .0, .1)]
    InvalidGrokExpression(String, String),
    #[error("invalid arguments for the function '{}'", .0)]
    InvalidFunctionArguments(String),
    #[error("unknown filter '{}'", .0)]
    UnknownFilter(String),
}

/**
Parses DD grok rules.

Here is an example:
access.common %{_client_ip} %{_ident} %{_auth} \[%{_date_access}\] "(?>%{_method} |)%{_url}(?> %{_version}|)" %{_status_code} (?>%{_bytes_written}|-)
access.combined %{access.common} (%{number:duration:scale(1000000000)} )?"%{_referer}" "%{_user_agent}"( "%{_x_forwarded_for}")?.*"#

You can write parsing rules with the %{MATCHER:EXTRACT:FILTER} syntax:
- Matcher: A rule (possibly a reference to another token rule) that describes what to expect (number, word, notSpace, etc.)
- Extract (optional): An identifier representing the capture destination for the piece of text matched by the Matcher.
- Filter (optional): A post-processor of the match to transform it.

Each rule can reference parsing rules defined above itself in the list.
Only one can match any given log. The first one that matches, from top to bottom, is the one that does the parsing.
For further documentation and the full list of available matcher and filters check out https://docs.datadoghq.com/logs/processing/parsing
*/
pub fn parse_grok_rules(
    helper_rules: &[String],
    parsing_rules: &[String],
) -> Result<Vec<GrokRule>, Error> {
    let mut prev_rule_definitions: HashMap<&str, String> = HashMap::new();
    let mut prev_rule_filters: HashMap<&str, HashMap<LookupBuf, Vec<GrokFilter>>> = HashMap::new();
    let mut grok = initialize_grok();

    // parse helper rules to reference them later in the match rules
    parse_rules(
        helper_rules,
        &mut prev_rule_definitions,
        &mut prev_rule_filters,
        &mut grok,
    )?;
    // parse match rules and return them
    parse_rules(
        parsing_rules,
        &mut prev_rule_definitions,
        &mut prev_rule_filters,
        &mut grok,
    )
}

fn parse_rules<'a>(
    parsing_rules: &'a [String],
    mut prev_rule_definitions: &mut HashMap<&'a str, String>,
    mut prev_rule_filters: &mut HashMap<&'a str, HashMap<LookupBuf, Vec<GrokFilter>>>,
    mut grok: &mut Grok,
) -> Result<Vec<GrokRule>, Error> {
    parsing_rules
        .iter()
        .filter(|&r| !r.is_empty())
        .map(|r| {
            parse_grok_rule(
                r,
                &mut prev_rule_definitions,
                &mut prev_rule_filters,
                &mut grok,
            )
        })
        .collect::<Result<Vec<GrokRule>, Error>>()
}

fn parse_grok_rule<'a>(
    rule: &'a str,
    mut prev_rule_definitions: &mut HashMap<&'a str, String>,
    mut prev_rule_filters: &mut HashMap<&'a str, HashMap<LookupBuf, Vec<GrokFilter>>>,
    grok: &mut Grok,
) -> Result<GrokRule, Error> {
    let mut split_whitespace = rule.splitn(2, ' ');
    let split = split_whitespace.by_ref();
    let rule_name = split.next().ok_or_else(|| {
        Error::InvalidGrokExpression(
            rule.to_string(),
            "format must be: 'ruleName definition'".into(),
        )
    })?;
    let mut rule_def = split
        .next()
        .ok_or_else(|| {
            Error::InvalidGrokExpression(
                rule.to_string(),
                "format must be: 'ruleName definition'".into(),
            )
        })?
        .to_string();

    let rule_def_cloned = rule_def.clone();
    lazy_static! {
        static ref GROK_PATTER_RE: Regex = Regex::new(r"%\{.+?\}").unwrap();
    }
    // find all patterns %{}
    let raw_grok_patterns = GROK_PATTER_RE
        .find_iter(rule_def_cloned.as_str())
        .map(|rule| rule.as_str())
        .collect::<Vec<&str>>();
    // parse them
    let mut grok_patterns = raw_grok_patterns
        .iter()
        .map(|pattern| {
            parse_grok_pattern(pattern)
                .map_err(|e| Error::InvalidGrokExpression(pattern.to_string(), e.to_string()))
        })
        .collect::<Result<Vec<GrokPattern>, Error>>()?;

    grok_patterns = index_repeated_fields(grok_patterns);

    let mut filters: HashMap<LookupBuf, Vec<GrokFilter>> = HashMap::new();
    let pure_grok_patterns: Vec<String> = grok_patterns
        .iter()
        .map(|pattern| {
            purify_grok_pattern(
                &pattern,
                &mut filters,
                &mut prev_rule_filters,
                &mut prev_rule_definitions,
            )
        })
        .collect::<Result<Vec<String>, Error>>()?;

    // replace grok patterns with "purified" ones
    let mut pure_pattern_it = pure_grok_patterns.iter();
    for r in raw_grok_patterns {
        rule_def = rule_def.replace(r, pure_pattern_it.next().unwrap().as_str());
    }

    // collect all filters to apply later
    grok_patterns
        .iter()
        .filter(|&rule| {
            rule.destination.is_some() && rule.destination.as_ref().unwrap().filter_fn.is_some()
        })
        .map(|rule| {
            let dest = rule.destination.as_ref().unwrap();
            let filter = GrokFilter::try_from(dest.filter_fn.as_ref().unwrap())?;
            Ok((dest, filter))
        })
        .collect::<Result<Vec<_>, Error>>()?
        .iter()
        .for_each(|(dest, filter)| {
            filters
                .entry(dest.path.clone())
                .and_modify(|v| v.push(filter.clone()))
                .or_insert_with(|| vec![filter.clone()]);
        });

    let mut pattern = String::new();
    pattern.push('^');
    pattern.push_str(rule_def.as_str());
    pattern.push('$');

    // store rule definitions and filters in case this rule is referenced in the next rules
    prev_rule_definitions.insert(rule_name, rule_def);
    prev_rule_filters.insert(rule_name, filters.clone());

    let pattern = Arc::new(
        grok.compile(&pattern, true)
            .map_err(|e| Error::InvalidGrokExpression(pattern, e.to_string()))?,
    );

    Ok(GrokRule { pattern, filters })
}

/// replaces repeated field names with indexed versions, e.g. : field.name, field.name -> field.name.0, field.name.1 to avoid collisions in grok
fn index_repeated_fields(grok_patterns: Vec<GrokPattern>) -> Vec<GrokPattern> {
    grok_patterns
        .iter()
        // group-by is a bit suboptimal with extra cloning, but acceptable since parsing usually happens only once
        .group_by(|pattern| pattern.destination.as_ref().map(|d| d.path.clone()))
        .into_iter()
        .flat_map(|(path, patterns)| match path {
            Some(path) => patterns
                .with_position()
                .enumerate()
                .map(|(i, pattern)| match pattern {
                    Position::First(pattern)
                    | Position::Middle(pattern)
                    | Position::Last(pattern) => {
                        let mut indexed_path = path.clone();
                        indexed_path.push_back(i as isize);
                        GrokPattern {
                            match_fn: pattern.match_fn.clone(),
                            destination: Some(Destination {
                                path: indexed_path,
                                filter_fn: pattern.destination.as_ref().unwrap().filter_fn.clone(),
                            }),
                        }
                    }
                    Position::Only(pattern) => pattern.to_owned(),
                })
                .collect::<Vec<_>>(),
            None => patterns.map(|p| p.to_owned()).collect::<Vec<_>>(),
        })
        .collect::<Vec<_>>()
}

/// Converts each rule to a pure grok rule:
///  - strips filters and "remembers" them to apply later
///  - replaces references to previous rules with actual definitions
///  - replaces match functions with corresponding regex groups
fn purify_grok_pattern(
    pattern: &GrokPattern,
    mut filters: &mut HashMap<LookupBuf, Vec<GrokFilter>>,
    prev_rule_filters: &mut HashMap<&str, HashMap<LookupBuf, Vec<GrokFilter>>>,
    prev_rule_definitions: &mut HashMap<&str, String>,
) -> Result<String, Error> {
    let mut res = String::new();
    if prev_rule_definitions.contains_key(pattern.match_fn.name.as_str()) {
        // this is a reference to a previous rule - replace it and copy all destinations from the prev rule
        res.push_str(
            prev_rule_definitions
                .get(pattern.match_fn.name.as_str())
                .unwrap(),
        );
        if let Some(d) = prev_rule_filters.get(pattern.match_fn.name.as_str()) {
            d.iter().for_each(|(path, function)| {
                filters.insert(path.to_owned(), function.to_owned());
            });
        }
    } else if pattern.match_fn.name == "regex"
        || pattern.match_fn.name == "date"
        || pattern.match_fn.name == "boolean"
    {
        // these patterns will be converted to named capture groups e.g. (?<http.status_code>[0-9]{3})
        res.push_str("(?<");
        if let Some(destination) = &pattern.destination {
            res.push_str(destination.path.to_string().as_str());
        }
        res.push('>');
        res.push_str(process_match_function(&mut filters, &pattern)?.as_str());
        res.push(')');
    } else {
        // these will be converted to "pure" grok patterns %{PATTERN:DESTINATION} but without filters
        res.push_str("%{");

        res.push_str(process_match_function(&mut filters, &pattern)?.as_str());

        if let Some(destination) = &pattern.destination {
            write!(res, ":{}", destination.path).unwrap();
        }
        res.push('}');
    }
    Ok(res)
}

fn process_match_function(
    filters: &mut HashMap<LookupBuf, Vec<GrokFilter>>,
    pattern: &ast::GrokPattern,
) -> Result<String, Error> {
    let match_fn = &pattern.match_fn;
    let result = match match_fn.name.as_ref() {
        "regex" => {
            if match_fn.args.is_some() {
                if let ast::FunctionArgument::Arg(Value::Bytes(b)) =
                    &match_fn.args.as_ref().unwrap()[0]
                {
                    return Ok(String::from_utf8_lossy(&b).to_string());
                }
            }
            Err(Error::InvalidFunctionArguments(match_fn.name.clone()))
        }
        "date" => {
            let args_opt = match_fn.args.as_ref();
            if match_fn.args.is_some()
                && !args_opt.unwrap().is_empty()
                && args_opt.unwrap().len() <= 2
            {
                if let ast::FunctionArgument::Arg(Value::Bytes(b)) = &args_opt.unwrap()[0] {
                    let format = String::from_utf8_lossy(b);
                    let result = date::time_format_to_regex(&format, true)
                        .map_err(|_e| Error::InvalidFunctionArguments(match_fn.name.clone()))?;
                    let mut tz_regex_opt = None;
                    if result.tz_captured {
                        tz_regex_opt = Some(Regex::new(&result.regex).map_err(|error| {
                            error!(message = "Error compiling regex", regex = %result.regex, %error);
                            Error::InvalidFunctionArguments(match_fn.name.clone())
                        })?);
                    }
                    let strp_format = date::convert_time_format(&format).map_err(|error| {
                        error!(message = "Error compiling regex", regex = %result.regex, %error);
                        Error::InvalidFunctionArguments(match_fn.name.clone())
                    })?;
                    let mut tz = None;
                    if args_opt.unwrap().len() > 1 {
                        if let ast::FunctionArgument::Arg(Value::Bytes(b)) = &args_opt.unwrap()[1] {
                            tz = Some(String::from_utf8_lossy(b).to_string());
                        }
                    }
                    let filter = GrokFilter::Date(strp_format, tz_regex_opt, tz, result.with_tz);
                    let result = date::time_format_to_regex(&format, false).map_err(|error| {
                        error!(message = "Invalid time format", format = %format, %error);
                        Error::InvalidFunctionArguments(match_fn.name.clone())
                    })?;
                    return replace_with_pattern_and_add_as_filter(
                        &result.regex,
                        filter,
                        filters,
                        pattern,
                    );
                }
            }
            Err(Error::InvalidFunctionArguments(match_fn.name.clone()))
        }
        "integer" => replace_with_pattern_and_add_as_filter(
            "integerStr",
            GrokFilter::Integer,
            filters,
            pattern,
        ),
        "integerExt" => replace_with_pattern_and_add_as_filter(
            "integerExtStr",
            GrokFilter::IntegerExt,
            filters,
            pattern,
        ),
        "number" => replace_with_pattern_and_add_as_filter(
            "numberStr",
            GrokFilter::Number,
            filters,
            pattern,
        ),
        "numberExt" => replace_with_pattern_and_add_as_filter(
            "numberExtStr",
            GrokFilter::NumberExt,
            filters,
            pattern,
        ),
        "boolean" => {
            if match_fn.args.is_some() {
                let args = match_fn.args.as_ref().unwrap();
                if args.len() == 2 {
                    if let ast::FunctionArgument::Arg(true_pattern) = &args[0] {
                        if let ast::FunctionArgument::Arg(false_pattern) = &args[1] {
                            let regex = Regex::new(
                                format!("^{}$", true_pattern.to_string_lossy()).as_str(),
                            )
                            .unwrap();
                            return replace_with_pattern_and_add_as_filter(
                                format!(
                                    "{}|{}",
                                    true_pattern.to_string_lossy(),
                                    false_pattern.to_string_lossy()
                                )
                                .as_str(),
                                GrokFilter::Boolean(Some(regex)),
                                filters,
                                pattern,
                            );
                        }
                    }
                }
                Err(Error::InvalidFunctionArguments(match_fn.name.clone()))
            } else {
                replace_with_pattern_and_add_as_filter(
                    "[Tt][Rr][Uu][Ee]|[Ff][Aa][Ll][Ss][Ee]",
                    GrokFilter::Boolean(None),
                    filters,
                    pattern,
                )
            }
        }
        // otherwise just add it as is, it should be a known grok pattern
        grok_pattern_name => Ok(grok_pattern_name.to_string()),
    };
    result
}

fn replace_with_pattern_and_add_as_filter(
    new_pattern: &str,
    filter: GrokFilter,
    filters: &mut HashMap<LookupBuf, Vec<GrokFilter>>,
    pattern: &ast::GrokPattern,
) -> Result<String, Error> {
    if let Some(destination) = &pattern.destination {
        filters.insert(destination.path.clone(), vec![filter]);
    }
    Ok(new_pattern.to_string())
}

include!(concat!(env!("OUT_DIR"), "/patterns.rs"));
fn initialize_grok() -> Grok {
    let mut grok = grok::Grok::with_patterns();

    // insert DataDog grok patterns
    for &(key, value) in PATTERNS {
        grok.insert_definition(String::from(key), String::from(value));
    }
    grok
}
