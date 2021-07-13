use super::{InternalEvent, SocketMode};
use metrics::counter;

#[derive(Debug)]
pub struct SyslogEventReceived {
    pub byte_size: usize,
}

impl InternalEvent for SyslogEventReceived {
    fn emit_logs(&self) {
        trace!(message = "Received line.", byte_size = %self.byte_size);
    }

    fn emit_metrics(&self) {
        counter!("events_in_total", 1);
        counter!("processed_bytes_total", self.byte_size as u64);
    }
}

#[derive(Debug)]
pub struct SyslogUdpReadError {
    pub error: std::io::Error,
}

impl InternalEvent for SyslogUdpReadError {
    fn emit_logs(&self) {
        error!(message = "Error reading datagram.", error = ?self.error, internal_log_rate_secs = 10);
    }

    fn emit_metrics(&self) {
        counter!("connection_read_errors_total", 1, "mode" => "udp");
    }
}

#[derive(Debug)]
pub(crate) struct SyslogInvalidUtf8FrameReceived {
    pub mode: SocketMode,
    pub error: std::str::Utf8Error,
}

impl InternalEvent for SyslogInvalidUtf8FrameReceived {
    fn emit_logs(&self) {
        error!(message = "Received frame containing invalid UTF-8.", error = %self.error, internal_log_rate_secs = 10);
    }

    fn emit_metrics(&self) {
        counter!("invalid_utf8_frames_total", 1, "mode" => self.mode.as_str());
    }
}
