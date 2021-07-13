use super::InternalEvent;
use metrics::counter;

#[derive(Debug)]
pub struct DecoderParseFailed {
    pub error: crate::Error,
}

impl InternalEvent for DecoderParseFailed {
    fn emit_logs(&self) {
        warn!(message = "Failed parsing frame.", error = %self.error, internal_log_rate_secs = 10);
    }

    fn emit_metrics(&self) {
        counter!("decoder_parse_errors_total", 1);
    }
}
