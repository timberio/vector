use crate::{
    event::{self, Event, LogEvent, Value},
    transforms::{
        regex_parser::{RegexParser, RegexParserConfig},
        FunctionTransform,
    },
};
use derivative::Derivative;
use shared::TimeZone;
use snafu::{OptionExt, Snafu};

pub const MULTILINE_TAG: &str = "multiline_tag";

/// Parser for the CRI log format.
///
/// Expects logs to arrive in a CRI log format.
///
/// CRI log format ([documentation][cri_log_format]) is a simple
/// newline-separated text format. We rely on regular expressions to parse it.
///
/// Normalizes parsed data for consistency.
///
/// [cri_log_format]: https://github.com/kubernetes/community/blob/ee2abbf9dbfa4523b414f99a04ddc97bd38c74b2/contributors/design-proposals/node/kubelet-cri-logging.md
#[derive(Clone, Derivative)]
#[derivative(Debug)]
pub struct Cri {
    #[derivative(Debug = "ignore")]
    regex_parser: Box<dyn FunctionTransform<Event>>,
}

impl Cri {
    /// Create a new [`Cri`] parser.
    pub fn new(timezone: TimeZone) -> Self {
        let regex_parser = {
            let mut rp_config = RegexParserConfig::default();

            let pattern = r"^(?P<timestamp>.*) (?P<stream>(stdout|stderr)) (?P<multiline_tag>(P|F)) (?P<message>.*)$";
            rp_config.patterns = vec![pattern.to_owned()];

            rp_config.types.insert(
                crate::config::log_schema().timestamp_key().to_string(),
                "timestamp|%+".to_owned(),
            );

            let parser = RegexParser::build(&rp_config, timezone)
                .expect("regexp patterns are static, should never fail");
            parser.into_function()
        };

        Self { regex_parser }
    }
}

impl FunctionTransform<Event> for Cri {
    fn transform(&mut self, output: &mut Vec<Event>, event: Event) {
        let mut buf = Vec::with_capacity(1);
        self.regex_parser.transform(&mut buf, event);
        if let Some(mut event) = buf.into_iter().next() {
            if normalize_event(event.as_mut_log()).ok().is_none() {
                return;
            } else {
                output.push(event);
            }
        }
    }
}

fn normalize_event(log: &mut LogEvent) -> Result<(), NormalizationError> {
    // Detect if this is a partial event.
    let multiline_tag = log
        .remove(MULTILINE_TAG)
        .context(MultilineTagFieldMissing)?;
    let multiline_tag = match multiline_tag {
        Value::Bytes(val) => val,
        _ => return Err(NormalizationError::MultilineTagValueUnexpectedType),
    };

    let is_partial = multiline_tag[0] == b'P';

    // For partial messages add a partial event indicator.
    if is_partial {
        log.insert(event::PARTIAL, true);
    }

    Ok(())
}

#[derive(Debug, Snafu)]
enum NormalizationError {
    MultilineTagFieldMissing,
    MultilineTagValueUnexpectedType,
}

#[cfg(test)]
pub mod tests {
    use super::super::test_util;
    use super::*;
    use crate::{event::LogEvent, test_util::trace_init, transforms::Transform};

    fn make_long_string(base: &str, len: usize) -> String {
        base.chars().cycle().take(len).collect()
    }

    /// Shared test cases.
    pub fn cases() -> Vec<(String, Vec<LogEvent>)> {
        vec![
            (
                "2016-10-06T00:17:09.669794202Z stdout F The content of the log entry 1".into(),
                vec![test_util::make_log_event(
                    "The content of the log entry 1",
                    "2016-10-06T00:17:09.669794202Z",
                    "stdout",
                    false,
                )],
            ),
            (
                "2016-10-06T00:17:09.669794202Z stdout P First line of log entry 2".into(),
                vec![test_util::make_log_event(
                    "First line of log entry 2",
                    "2016-10-06T00:17:09.669794202Z",
                    "stdout",
                    true,
                )],
            ),
            (
                "2016-10-06T00:17:09.669794202Z stdout P Second line of the log entry 2".into(),
                vec![test_util::make_log_event(
                    "Second line of the log entry 2",
                    "2016-10-06T00:17:09.669794202Z",
                    "stdout",
                    true,
                )],
            ),
            (
                "2016-10-06T00:17:10.113242941Z stderr F Last line of the log entry 2".into(),
                vec![test_util::make_log_event(
                    "Last line of the log entry 2",
                    "2016-10-06T00:17:10.113242941Z",
                    "stderr",
                    false,
                )],
            ),
            // A part of the partial message with a realistic length.
            (
                [
                    r#"2016-10-06T00:17:10.113242941Z stdout P "#,
                    make_long_string("very long message ", 16 * 1024).as_str(),
                ]
                .join(""),
                vec![test_util::make_log_event(
                    make_long_string("very long message ", 16 * 1024).as_str(),
                    "2016-10-06T00:17:10.113242941Z",
                    "stdout",
                    true,
                )],
            ),
        ]
    }

    #[test]
    fn test_parsing() {
        trace_init();
        test_util::test_parser(|| Transform::function(Cri::new(TimeZone::Local)), cases());
    }
}
