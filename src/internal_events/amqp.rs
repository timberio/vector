#[cfg(feature = "sources-amqp")]
pub mod source {
    use crate::internal_events::InternalEvent;
    use metrics::counter;
    use crate::pipeline::ClosedError;

    #[derive(Debug)]
    pub struct AmqpEventReceived {
        pub byte_size: usize,
    }

    impl InternalEvent for AmqpEventReceived {
        fn emit_logs(&self) {
            trace!(message = "Received one event.", internal_log_rate_secs = 10);
        }

        fn emit_metrics(&self) {
            counter!("processed_events_total", 1);
            counter!("processed_bytes_total", self.byte_size as u64);
        }
    }

    #[derive(Debug)]
    pub struct AmqpConsumerFailed {
        pub error: lapin::Error,
    }

    impl InternalEvent for AmqpConsumerFailed {
        fn emit_logs(&self) {
            error!(message = "Failed to consume.", error = ?self.error, internal_log_rate_secs = 10);
        }

        fn emit_metrics(&self) {
            counter!("events_consume_failed_total", 1);
        }
    }

    #[derive(Debug)]
    pub struct AmqpEventFailed {
        pub error: lapin::Error,
    }

    impl InternalEvent for AmqpEventFailed {
        fn emit_logs(&self) {
            error!(message = "Failed to read message.", error = ?self.error, internal_log_rate_secs = 10);
        }

        fn emit_metrics(&self) {
            counter!("events_failed_total", 1);
        }
    }

    #[derive(Debug)]
    pub struct AmqpKeyExtractionFailed<'a> {
        pub key_field: &'a str,
    }

    impl InternalEvent for AmqpKeyExtractionFailed<'_> {
        fn emit_logs(&self) {
            error!(message = "Failed to extract key.", key_field = %self.key_field, internal_log_rate_secs = 10);
        }
    }

    #[derive(Debug)]
    pub struct AmqpDeliveryFailed {
        pub error: ClosedError,
    }

    impl InternalEvent for AmqpDeliveryFailed {
        fn emit_logs(&self) {
            error!(message = "Unable to deliver", error = ?self.error, internal_log_rate_secs = 10);
        }

        fn emit_metrics(&self) {
            counter!("consumer_delivery_failed_total", 1);
        }
    }

    #[derive(Debug)]
    pub struct AmqpCommitFailed {
        pub error: lapin::Error,
    }

    impl InternalEvent for AmqpCommitFailed {
        fn emit_logs(&self) {
            error!(message = "Unable to ack", error = ?self.error, internal_log_rate_secs = 10);
        }

        fn emit_metrics(&self) {
            counter!("consumer_ack_failed_total", 1);
        }
    }
}

#[cfg(feature = "sinks-amqp")]
pub mod sink {
    use crate::internal_events::InternalEvent;
    use metrics::counter;

    #[derive(Debug)]
    pub struct AmqpDeliveryFailed {
        pub error: lapin::Error,
    }

    impl InternalEvent for AmqpDeliveryFailed {
        fn emit_logs(&self) {
            error!(message = "Unable to deliver", error = ?self.error, internal_log_rate_secs = 10);
        }

        fn emit_metrics(&self) {
            counter!("events_deliver_failed_total", 1);
        }
    }

    #[derive(Debug)]
    pub struct AmqpAcknowledgementFailed {
        pub error: lapin::Error,
    }

    impl InternalEvent for AmqpAcknowledgementFailed {
        fn emit_logs(&self) {
            error!(message = "Acknowledgement failed", error = ?self.error, internal_log_rate_secs = 10);
        }

        fn emit_metrics(&self) {
            counter!("events_acknowledgement_failed_total", 1);
        }
    }

    #[derive(Debug, Default)]
    pub struct AmqpNoAcknowledgement;

    impl InternalEvent for AmqpNoAcknowledgement {
        fn emit_logs(&self) {
            error!(message = "No acknowledgement", internal_log_rate_secs = 10);
        }

        fn emit_metrics(&self) {
            counter!("events_acknowledgement_failed_total", 1);
        }
    }
}
