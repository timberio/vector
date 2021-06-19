use crate::{
    config::log_schema,
    event::{self, Event, LogEvent, Value},
    internal_events::KubernetesLogsDockerFormatParseFailed,
    transforms::FunctionTransform,
};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use snafu::{OptionExt, ResultExt, Snafu};

pub const TIME: &str = "time";
pub const LOG: &str = "log";

/// Parser for the docker log format.
///
/// Expects logs to arrive in a JSONLines format with the fields names and
/// contents specific to the implementation of the Docker `json` log driver.
///
/// Normalizes parsed data for consistency.
#[derive(Clone, Debug)]
pub struct Docker;

impl FunctionTransform<Event> for Docker {
    fn transform(&mut self, output: &mut Vec<Event>, mut event: Event) {
        let log = event.as_mut_log();
        if let Err(err) = parse_json(log) {
            emit!(KubernetesLogsDockerFormatParseFailed { error: &err });
            return;
        }
        if let Err(err) = normalize_event(log) {
            emit!(KubernetesLogsDockerFormatParseFailed { error: &err });
            return;
        }
        output.push(event);
    }
}

/// Parses `message` as json object and removes it.
fn parse_json(log: &mut LogEvent) -> Result<(), ParsingError> {
    let message = log
        .remove(log_schema().message_key())
        .ok_or(ParsingError::NoMessageField)?;

    let bytes = match message {
        Value::Bytes(bytes) => bytes,
        _ => return Err(ParsingError::MessageFieldNotInBytes),
    };

    match serde_json::from_slice(bytes.as_ref()) {
        Ok(JsonValue::Object(object)) => {
            for (key, value) in object {
                log.insert_flat(key, value);
            }
            Ok(())
        }
        Ok(_) => Err(ParsingError::NotAnObject { message: bytes }),
        Err(err) => Err(ParsingError::InvalidJson {
            source: err,
            message: bytes,
        }),
    }
}

const DOCKER_MESSAGE_SPLIT_THRESHOLD: usize = 16 * 1024; // 16 Kib

fn normalize_event(log: &mut LogEvent) -> Result<(), NormalizationError> {
    // Parse and rename timestamp.
    let time = log.remove(&*TIME).context(TimeFieldMissing)?;
    let time = match time {
        Value::Bytes(val) => val,
        _ => return Err(NormalizationError::TimeValueUnexpectedType),
    };
    let time = DateTime::parse_from_rfc3339(String::from_utf8_lossy(time.as_ref()).as_ref())
        .context(TimeParsing)?;
    log.insert(log_schema().timestamp_key(), time.with_timezone(&Utc));

    // Parse message, remove trailing newline and detect if it's partial.
    let message = log.remove(&*LOG).context(LogFieldMissing)?;
    let mut message = match message {
        Value::Bytes(val) => val,
        _ => return Err(NormalizationError::LogValueUnexpectedType),
    };
    // Here we apply out heuristics to detect if message is partial.
    // Partial messages are only split in docker at the maximum message length
    // (`DOCKER_MESSAGE_SPLIT_THRESHOLD`).
    // Thus, for a message to be partial it also has to have exactly that
    // length.
    // Now, whether that message will or won't actually be partial if it has
    // exactly the max length is unknown. We consider all messages with the
    // exact length of `DOCKER_MESSAGE_SPLIT_THRESHOLD` bytes partial
    // by default, and then, if they end with newline - consider that
    // an exception and make them non-partial.
    // This is still not ideal, and can potentially be improved.
    let mut is_partial = message.len() == DOCKER_MESSAGE_SPLIT_THRESHOLD;
    if message.last().map(|&b| b as char == '\n').unwrap_or(false) {
        message.truncate(message.len() - 1);
        is_partial = false;
    };
    log.insert(log_schema().message_key(), message);

    // For partial messages add a partial event indicator.
    if is_partial {
        log.insert(&*event::PARTIAL, true);
    }

    Ok(())
}

#[derive(Debug, Snafu)]
enum ParsingError {
    NoMessageField,
    MessageFieldNotInBytes,
    #[snafu(display(
        "Could not parse json: {} in message {:?}",
        source,
        String::from_utf8_lossy(message)
    ))]
    InvalidJson {
        source: serde_json::Error,
        message: Bytes,
    },
    #[snafu(display("Message was not an object: {:?}", String::from_utf8_lossy(message)))]
    NotAnObject {
        message: Bytes,
    },
}

#[derive(Debug, Snafu)]
enum NormalizationError {
    TimeFieldMissing,
    TimeValueUnexpectedType,
    TimeParsing { source: chrono::ParseError },
    LogFieldMissing,
    LogValueUnexpectedType,
}

#[cfg(test)]
pub mod tests {
    use super::{super::test_util, *};
    use crate::{test_util::trace_init, transforms::Transform};

    fn make_long_string(base: &str, len: usize) -> String {
        base.chars().cycle().take(len).collect()
    }

    /// Shared test cases.
    pub fn cases() -> Vec<(String, Vec<LogEvent>)> {
        vec![
            (
                r#"{"log": "The actual log line\n", "stream": "stderr", "time": "2016-10-05T00:00:30.082640485Z"}"#.into(),
                vec![test_util::make_log_event(
                    "The actual log line",
                    "2016-10-05T00:00:30.082640485Z",
                    "stderr",
                    false,
                )],
            ),
            (
                r#"{"log": "A line without newline chan at the end", "stream": "stdout", "time": "2016-10-05T00:00:30.082640485Z"}"#.into(),
                vec![test_util::make_log_event(
                    "A line without newline chan at the end",
                    "2016-10-05T00:00:30.082640485Z",
                    "stdout",
                    false,
                )],
            ),
            // Partial message due to message length.
            (
                [
                    r#"{"log": ""#,
                    make_long_string("partial ", 16 * 1024).as_str(),
                    r#"", "stream": "stdout", "time": "2016-10-05T00:00:30.082640485Z"}"#,
                ]
                .join(""),
                vec![test_util::make_log_event(
                    make_long_string("partial ",16 * 1024).as_str(),
                    "2016-10-05T00:00:30.082640485Z",
                    "stdout",
                    true,
                )],
            ),
            // Non-partial message, because message length matches but
            // the message also ends with newline.
            (
                [
                    r#"{"log": ""#,
                    make_long_string("non-partial ", 16 * 1024 - 1).as_str(),
                    r"\n",
                    r#"", "stream": "stdout", "time": "2016-10-05T00:00:30.082640485Z"}"#,
                ]
                .join(""),
                vec![test_util::make_log_event(
                    make_long_string("non-partial ", 16 * 1024 - 1).as_str(),
                    "2016-10-05T00:00:30.082640485Z",
                    "stdout",
                    false,
                )],
            ),
        ]
    }

    #[test]
    fn test_parsing() {
        trace_init();

        test_util::test_parser(|| Transform::function(Docker), cases());
    }

    #[test]
    fn test_parsing_invalid() {
        trace_init();

        let cases = vec![
            // Empty string.
            r#""#,
            // Incomplete.
            r#"{"#,
            // Random non-JSON text.
            r#"hello world"#,
            // Random JSON non-object.
            r#"123"#,
            // Empty JSON object.
            r#"{}"#,
            // No timestamp.
            r#"{"log": "Hello world", "stream": "stdout"}"#,
            // Timestamp not a string.
            r#"{"log": "Hello world", "stream": "stdout", "time": 123}"#,
            // Empty timestamp.
            r#"{"log": "Hello world", "stream": "stdout", "time": ""}"#,
            // Invalid timestamp.
            r#"{"log": "Hello world", "stream": "stdout", "time": "qwerty"}"#,
            // No log field.
            r#"{"stream": "stderr", "time": "2016-10-05T00:00:30.082640485Z"}"#,
            // Log is not a string.
            r#"{"log": 123, "stream": "stderr", "time": "2016-10-05T00:00:30.082640485Z"}"#,
        ];

        for message in cases {
            let input = Event::from(message);
            let mut output = Vec::new();
            Docker.transform(&mut output, input);
            assert!(output.is_empty(), "Expected no events: {:?}", output);
        }
    }
}
