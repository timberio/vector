use super::InternalEvent;
use metrics::counter;

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // some features only use some variants
pub(crate) enum SocketMode {
    Tcp,
    Udp,
    Unix,
}

impl SocketMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
            Self::Unix => "unix",
        }
    }
}

#[derive(Debug)]
pub(crate) struct SocketEventReceived {
    pub mode: SocketMode,
    pub byte_size: usize,
}

impl InternalEvent for SocketEventReceived {
    fn emit_logs(&self) {
        trace!(message = "Received one event.", byte_size = %self.byte_size, mode = self.mode.as_str());
    }

    fn emit_metrics(&self) {
        counter!("events_in_total", 1, "mode" => self.mode.as_str());
        counter!("processed_bytes_total", self.byte_size as u64, "mode" => self.mode.as_str());
    }
}

#[derive(Debug)]
pub(crate) struct SocketEventsSent {
    pub mode: SocketMode,
    pub count: u64,
    pub byte_size: usize,
}

impl InternalEvent for SocketEventsSent {
    fn emit_logs(&self) {
        trace!(message = "Events sent.", count = %self.count, byte_size = %self.byte_size);
    }

    fn emit_metrics(&self) {
        counter!("processed_bytes_total", self.byte_size as u64, "mode" => self.mode.as_str());
    }
}

#[derive(Debug)]
pub(crate) struct SocketReceiveError {
    pub mode: SocketMode,
    pub error: std::io::Error,
}

impl InternalEvent for SocketReceiveError {
    fn emit_logs(&self) {
        error!(message = "Error receiving data.", error = ?self.error, mode = %self.mode.as_str());
    }

    fn emit_metrics(&self) {
        counter!("connection_errors_total", 1, "mode" => self.mode.as_str());
    }
}
