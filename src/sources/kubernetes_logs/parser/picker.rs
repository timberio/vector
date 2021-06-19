use super::{cri::Cri, docker::Docker};
use crate::{
    event::{Event, Value},
    internal_events::KubernetesLogsFormatPickerEdgeCase,
    transforms::FunctionTransform,
};
use shared::TimeZone;

#[derive(Clone, Debug)]
enum PickerState {
    Init,
    Docker(Docker),
    Cri(Cri),
}

#[derive(Clone, Debug)]
pub(crate) struct Picker {
    timezone: TimeZone,
    state: PickerState,
}

impl Picker {
    pub(crate) fn new(timezone: TimeZone) -> Self {
        let state = PickerState::Init;
        Self { timezone, state }
    }
}

impl FunctionTransform<Event> for Picker {
    fn transform(&mut self, output: &mut Vec<Event>, event: Event) {
        match &mut self.state {
            PickerState::Init => {
                let message = match event
                    .as_log()
                    .get(crate::config::log_schema().message_key())
                {
                    Some(message) => message,
                    None => {
                        emit!(KubernetesLogsFormatPickerEdgeCase {
                            what: "got an event with no message field"
                        });
                        return;
                    }
                };

                let bytes = match message {
                    Value::Bytes(bytes) => bytes,
                    _ => {
                        emit!(KubernetesLogsFormatPickerEdgeCase {
                            what: "got an event with non-bytes message field"
                        });
                        return;
                    }
                };

                self.state = if bytes.len() > 1 && bytes[0] == b'{' {
                    PickerState::Docker(Docker)
                } else {
                    PickerState::Cri(Cri::new(self.timezone))
                };
                self.transform(output, event)
            }
            PickerState::Docker(t) => t.transform(output, event),
            PickerState::Cri(t) => t.transform(output, event),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{cri, docker, test_util};
    use super::*;
    use crate::{
        event::{Event, LogEvent},
        test_util::trace_init,
        transforms::Transform,
    };

    /// Picker has to work for all test cases for underlying parsers.
    fn cases() -> Vec<(String, Vec<LogEvent>)> {
        let mut cases = vec![];
        cases.extend(docker::tests::cases());
        cases.extend(cri::tests::cases());
        cases
    }

    #[test]
    fn test_parsing() {
        trace_init();
        test_util::test_parser(
            || Transform::function(Picker::new(TimeZone::Local)),
            cases(),
        );
    }

    #[test]
    fn test_parsing_invalid() {
        trace_init();

        let cases = vec!["", "qwe", "{"];

        for message in cases {
            let input = Event::from(message);
            let mut picker = Picker::new(TimeZone::Local);
            let mut output = Vec::new();
            picker.transform(&mut output, input);
            assert!(output.is_empty(), "Expected no events: {:?}", output);
        }
    }

    #[test]
    fn test_parsing_invalid_non_standard_events() {
        trace_init();

        let cases = vec![
            // No `message` field.
            Event::new_empty_log(),
            // Non-bytes `message` field.
            {
                let mut input = Event::new_empty_log();
                input.as_mut_log().insert("message", 123);
                input
            },
        ];

        for input in cases {
            let mut picker = Picker::new(TimeZone::Local);
            let mut output = Vec::new();
            picker.transform(&mut output, input);
            assert!(output.is_empty(), "Expected no events: {:?}", output);
        }
    }
}
