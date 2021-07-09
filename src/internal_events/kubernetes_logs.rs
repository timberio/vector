use super::InternalEvent;
use crate::event::Event;
use metrics::counter;

#[derive(Debug)]
pub struct KubernetesLogsEventReceived<'a> {
    pub file: &'a str,
    pub byte_size: usize,
    pub pod_name: Option<&'a str>,
}

impl InternalEvent for KubernetesLogsEventReceived<'_> {
    fn emit_logs(&self) {
        trace!(
            message = "Received one event.",
            file = %self.file
        );
    }

    fn emit_metrics(&self) {
        match self.pod_name {
            Some(name) => {
                counter!("events_in_total", 1, "pod_name" => name.to_owned());
                counter!(
                    "processed_bytes_total", self.byte_size as u64,
                    "pod_name" => name.to_owned()
                );
            }
            None => {
                counter!("events_in_total", 1);
                counter!("processed_bytes_total", self.byte_size as u64);
            }
        }
    }
}

#[derive(Debug)]
pub struct KubernetesLogsEventAnnotationFailed<'a> {
    pub event: &'a Event,
}

impl InternalEvent for KubernetesLogsEventAnnotationFailed<'_> {
    fn emit_logs(&self) {
        warn!(
            message = "Failed to annotate event with pod metadata.",
            event = ?self.event
        );
    }

    fn emit_metrics(&self) {
        counter!("k8s_event_annotation_failures_total", 1);
    }
}

#[derive(Debug)]
pub struct KubernetesLogsEventNamespaceAnnotationFailed<'a> {
    pub event: &'a Event,
}

impl InternalEvent for KubernetesLogsEventNamespaceAnnotationFailed<'_> {
    fn emit_logs(&self) {
        warn!(
            message = "Failed to annotate event with namespace metadata.",
            event = ?self.event
        );
    }

    fn emit_metrics(&self) {
        counter!("k8s_event_namespace_annotation_failures_total", 1);
    }
}

#[derive(Debug)]
pub struct KubernetesLogsFormatPickerEdgeCase {
    pub what: &'static str,
}

impl InternalEvent for KubernetesLogsFormatPickerEdgeCase {
    fn emit_logs(&self) {
        warn!(
            message = "Encountered format picker edge case.",
            what = %self.what,
        );
    }

    fn emit_metrics(&self) {
        counter!("k8s_format_picker_edge_cases_total", 1);
    }
}

#[derive(Debug)]
pub struct KubernetesLogsDockerFormatParseFailed<'a> {
    pub error: &'a dyn std::error::Error,
}

impl InternalEvent for KubernetesLogsDockerFormatParseFailed<'_> {
    fn emit_logs(&self) {
        warn!(
            message = "Failed to parse log line in docker format.",
            error = %self.error,
        );
    }

    fn emit_metrics(&self) {
        counter!("k8s_docker_format_parse_failures_total", 1);
    }
}
