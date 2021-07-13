use super::InternalEvent;
use metrics::counter;

#[derive(Debug)]
pub struct StatsdEventReceived {
    pub byte_size: usize,
}

impl InternalEvent for StatsdEventReceived {
    fn emit_logs(&self) {
        trace!(message = "Received packet.", byte_size = %self.byte_size);
    }

    fn emit_metrics(&self) {
        counter!("events_in_total", 1);
        counter!("processed_bytes_total", self.byte_size as u64,);
    }
}

#[derive(Debug)]
pub struct StatsdInvalidRecord<'a> {
    pub error: crate::sources::statsd::parser::ParseError,
    pub text: &'a str,
}

impl InternalEvent for StatsdInvalidRecord<'_> {
    fn emit_logs(&self) {
        error!(message = "Invalid packet from statsd, discarding.", error = ?self.error, text = %self.text);
    }

    fn emit_metrics(&self) {
        counter!("invalid_record_total", 1,);
        counter!("invalid_record_bytes_total", self.text.len() as u64,);
    }
}

#[derive(Debug)]
enum StatsdSocketErrorType {
    Bind,
    Read,
}

#[derive(Debug)]
pub struct StatsdSocketError<T> {
    r#type: StatsdSocketErrorType,
    pub error: T,
}

impl<T> StatsdSocketError<T> {
    fn new(r#type: StatsdSocketErrorType, error: T) -> Self {
        Self { r#type, error }
    }

    pub fn bind(error: T) -> Self {
        Self::new(StatsdSocketErrorType::Bind, error)
    }

    pub fn read(error: T) -> Self {
        Self::new(StatsdSocketErrorType::Read, error)
    }
}

impl<T: std::fmt::Debug + std::fmt::Display> InternalEvent for StatsdSocketError<T> {
    fn emit_logs(&self) {
        let message = match self.r#type {
            StatsdSocketErrorType::Bind => "Failed to bind to UDP listener socket.",
            StatsdSocketErrorType::Read => "Failed to read UDP datagram.",
        };
        error!(message, error = ?self.error);
    }

    fn emit_metrics(&self) {
        counter!("connection_errors_total", 1);
    }
}

#[derive(Debug)]
pub(crate) struct StatsdInvalidUtf8FrameReceived {
    pub error: std::str::Utf8Error,
}

impl InternalEvent for StatsdInvalidUtf8FrameReceived {
    fn emit_logs(&self) {
        error!(message = "Received frame containing invalid UTF-8.", %self.error);
    }

    fn emit_metrics(&self) {
        counter!("invalid_utf8_frames_total", 1);
    }
}
