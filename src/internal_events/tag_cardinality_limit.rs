use super::InternalEvent;
use metrics::counter;

pub(crate) struct TagCardinalityLimitEventProcessed;

impl InternalEvent for TagCardinalityLimitEventProcessed {
    fn emit_logs(&self) {
        trace!(message = "Processed one event.");
    }

    fn emit_metrics(&self) {
        counter!("events_processed", 1,
            "component_kind" => "transform",
            "component_type" => "regex_parser",
        );
    }
}

pub(crate) struct TagCardinalityLimitRejectingEvent<'a> {
    pub tag_key: &'a str,
    pub tag_value: &'a str,
}

impl<'a> InternalEvent for TagCardinalityLimitRejectingEvent<'a> {
    fn emit_logs(&self) {
        error!(
            message = "Event containing tag with new value after hitting configured 'value_limit'; discarding event",
            tag_key = self.tag_key,
            tag_value = self.tag_value,
            rate_limit_secs = 10,
        );
    }

    fn emit_metrics(&self) {}
}

pub(crate) struct TagCardinalityLimitRejectingTag<'a> {
    pub tag_key: &'a str,
    pub tag_value: &'a str,
}

impl<'a> InternalEvent for TagCardinalityLimitRejectingTag<'a> {
    fn emit_logs(&self) {
        warn!(
            message = "Rejecting tag after hitting configured 'value_limit'",
            tag_key = self.tag_key,
            tag_value = self.tag_value,
            rate_limit_secs = 10,
        );
    }

    fn emit_metrics(&self) {}
}

pub(crate) struct TagCardinalityLimitReachedLimit<'a> {
    pub key: &'a str,
}

impl<'a> InternalEvent for TagCardinalityLimitReachedLimit<'a> {
    fn emit_logs(&self) {
        warn!(
            "value_limit reached for key {}. New values for this key will be rejected",
            key = self.key,
        );
    }
}
