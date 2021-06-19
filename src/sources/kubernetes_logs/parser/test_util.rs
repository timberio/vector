#![cfg(test)]

use crate::{
    event::{Event, LogEvent},
    transforms::Transform,
};
use chrono::{DateTime, Utc};

/// Build a log event for test purposes.
///
/// The implementation is shared, and therefore consistent across all
/// the parsers.
pub fn make_log_event(message: &str, timestamp: &str, stream: &str, is_partial: bool) -> LogEvent {
    let mut log = LogEvent::default();

    log.insert("message", message);

    let timestamp = DateTime::parse_from_rfc3339(timestamp)
        .expect("invalid test case")
        .with_timezone(&Utc);
    log.insert("timestamp", timestamp);

    log.insert("stream", stream);

    if is_partial {
        log.insert("_partial", true);
    }
    log
}

/// Shared logic for testing parsers.
///
/// Takes a parser builder and a list of test cases.
pub fn test_parser<B>(builder: B, cases: Vec<(String, Vec<LogEvent>)>)
where
    B: Fn() -> Transform<Event>,
{
    for (message, expected) in cases {
        let input = Event::from(message);
        let mut parser = (builder)();
        let parser = parser.as_function();

        let mut output = Vec::new();
        parser.transform(&mut output, input);
        shared::assert_event_data_eq!(
            expected.into_iter().map(Event::Log).collect::<Vec<_>>(),
            output,
            "expected left, actual right"
        );
    }
}
