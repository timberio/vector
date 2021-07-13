use super::InternalEvent;
use metrics::{counter, gauge};
use vector_core::update_counter;

#[derive(Debug)]
pub struct KafkaEventReceived {
    pub byte_size: usize,
}

impl InternalEvent for KafkaEventReceived {
    fn emit_logs(&self) {
        trace!(message = "Received one event.", internal_log_rate_secs = 10);
    }

    fn emit_metrics(&self) {
        counter!("events_in_total", 1);
        counter!("processed_bytes_total", self.byte_size as u64);
    }
}

#[derive(Debug)]
pub struct KafkaOffsetUpdateFailed {
    pub error: rdkafka::error::KafkaError,
}

impl InternalEvent for KafkaOffsetUpdateFailed {
    fn emit_logs(&self) {
        error!(message = "Unable to update consumer offset.", error = ?self.error);
    }

    fn emit_metrics(&self) {
        counter!("consumer_offset_updates_failed_total", 1);
    }
}

#[derive(Debug)]
pub struct KafkaEventFailed {
    pub error: rdkafka::error::KafkaError,
}

impl InternalEvent for KafkaEventFailed {
    fn emit_logs(&self) {
        error!(message = "Failed to read message.", error = ?self.error);
    }

    fn emit_metrics(&self) {
        counter!("events_failed_total", 1);
    }
}

#[derive(Debug)]
pub struct KafkaKeyExtractionFailed<'a> {
    pub key_field: &'a str,
}

impl InternalEvent for KafkaKeyExtractionFailed<'_> {
    fn emit_logs(&self) {
        error!(message = "Failed to extract key.", key_field = %self.key_field);
    }
}

#[derive(Debug)]
pub struct KafkaStatisticsReceived<'a> {
    pub statistics: &'a rdkafka::Statistics,
}

impl InternalEvent for KafkaStatisticsReceived<'_> {
    fn emit_metrics(&self) {
        gauge!("kafka_queue_messages", self.statistics.msg_cnt as f64);
        gauge!(
            "kafka_queue_messages_bytes",
            self.statistics.msg_size as f64
        );
        update_counter!("kafka_requests_total", self.statistics.tx as u64);
        update_counter!(
            "kafka_requests_bytes_total",
            self.statistics.tx_bytes as u64
        );
        update_counter!("kafka_responses_total", self.statistics.rx as u64);
        update_counter!(
            "kafka_responses_bytes_total",
            self.statistics.rx_bytes as u64
        );
        update_counter!(
            "kafka_produced_messages_total",
            self.statistics.txmsgs as u64
        );
        update_counter!(
            "kafka_produced_messages_bytes_total",
            self.statistics.txmsg_bytes as u64
        );
        update_counter!(
            "kafka_consumed_messages_total",
            self.statistics.rxmsgs as u64
        );
        update_counter!(
            "kafka_consumed_messages_bytes_total",
            self.statistics.rxmsg_bytes as u64
        );
    }
}

#[derive(Debug)]
pub struct KafkaDecodeMessageFailed {
    pub error: crate::Error,
}

impl InternalEvent for KafkaDecodeMessageFailed {
    fn emit_logs(&self) {
        error!(message = "Failed to decode message.", error = %self.error);
    }

    fn emit_metrics(&self) {
        counter!("decode_failed_messages_total", 1);
    }
}
