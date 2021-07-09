use crate::ast::GrokPattern;
use crate::lexer::Lexer;
use lalrpop_util::lalrpop_mod;

lalrpop_mod!(
    #[allow(clippy::all)]
    #[allow(unused)]
    parser
);

/// Parses grok patterns as %{MATCHER:FIELD:FILTER}
pub fn parse_grok_pattern(input: &str) -> Result<GrokPattern, &str> {
    let lexer = Lexer::new(input);
    parser::GrokFilterParser::new()
        .parse(input, lexer)
        .map_err(|_| input) // just return the original string containing an error
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::FunctionArgument;

    use lookup::{LookupBuf, SegmentBuf};
    use parsing::value::Value;

    fn from_path_segments(path_segments: Vec<&str>) -> LookupBuf {
        LookupBuf::from_segments(
            path_segments
                .iter()
                .map(|&s| s.into())
                .collect::<Vec<SegmentBuf>>(),
        )
    }

    #[test]
    fn parse_grok_filter() {
        let input = r#"%{date:e-http.status.abc[".\""]:integer("a. df",.123,1.23e-32, true, null, 123e-5)}"#;
        let lexer = Lexer::new(input);
        let parsed = parser::GrokFilterParser::new()
            .parse(input, lexer)
            .unwrap_or_else(|error| {
                panic!("Problem parsing grok: {:?}", error);
            });
        assert_eq!(parsed.match_fn.name, "date");
        let destination = parsed.destination.unwrap();
        assert_eq!(
            destination.path,
            from_path_segments(vec!["e-http", "status", "abc", r#".""#])
        );
        let filter = destination.filter_fn.unwrap();
        assert_eq!(filter.name, "integer");
        let args = filter.args.unwrap();
        let expected_args = vec![
            "a. df".into(),
            0.123.into(),
            1.23e-32_f64.into(),
            true.into(),
            Value::Null,
            123e-5.into(),
        ];
        for (i, arg) in args.iter().enumerate() {
            match arg {
                FunctionArgument::Arg(arg) => assert_eq!(arg, expected_args.get(i).unwrap()),
                _ => panic!("failed to parse arguments"),
            };
        }
    }
}
