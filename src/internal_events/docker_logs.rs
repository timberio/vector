use super::InternalEvent;
use bollard::errors::Error;
use chrono::ParseError;
use metrics::counter;

#[derive(Debug)]
pub struct DockerLogsEventReceived<'a> {
    pub byte_size: usize,
    pub container_id: &'a str,
    pub container_name: &'a str,
}

impl<'a> InternalEvent for DockerLogsEventReceived<'a> {
    fn emit_logs(&self) {
        trace!(
            message = "Received one event.",
            byte_size = %self.byte_size,
            container_id = %self.container_id
        );
    }

    fn emit_metrics(&self) {
        counter!(
            "events_in_total", 1,
            "container_name" => self.container_name.to_owned()
        );
        counter!(
            "processed_bytes_total", self.byte_size as u64,
            "container_name" => self.container_name.to_owned()
        );
    }
}

#[derive(Debug)]
pub struct DockerLogsContainerEventReceived<'a> {
    pub container_id: &'a str,
    pub action: &'a str,
}

impl<'a> InternalEvent for DockerLogsContainerEventReceived<'a> {
    fn emit_logs(&self) {
        debug!(
            message = "Received one container event.",
            container_id = %self.container_id,
            action = %self.action
        );
    }

    fn emit_metrics(&self) {
        counter!("container_processed_events_total", 1);
    }
}

#[derive(Debug)]
pub struct DockerLogsContainerWatch<'a> {
    pub container_id: &'a str,
}

impl<'a> InternalEvent for DockerLogsContainerWatch<'a> {
    fn emit_logs(&self) {
        info!(
            message = "Started watching for container logs.",
            container_id = %self.container_id,
        );
    }

    fn emit_metrics(&self) {
        counter!("containers_watched_total", 1);
    }
}

#[derive(Debug)]
pub struct DockerLogsContainerUnwatch<'a> {
    pub container_id: &'a str,
}

impl<'a> InternalEvent for DockerLogsContainerUnwatch<'a> {
    fn emit_logs(&self) {
        info!(
            message = "Stopped watching for container logs.",
            container_id = %self.container_id,
        );
    }

    fn emit_metrics(&self) {
        counter!("containers_unwatched_total", 1);
    }
}

#[derive(Debug)]
pub struct DockerLogsCommunicationError<'a> {
    pub error: Error,
    pub container_id: Option<&'a str>,
}

impl<'a> InternalEvent for DockerLogsCommunicationError<'a> {
    fn emit_logs(&self) {
        error!(
            message = "Error in communication with Docker daemon.",
            error = ?self.error,
            container_id = ?self.container_id,
            internal_log_rate_secs = 10
        );
    }

    fn emit_metrics(&self) {
        counter!("communication_errors_total", 1);
    }
}

#[derive(Debug)]
pub struct DockerLogsContainerMetadataFetchFailed<'a> {
    pub error: Error,
    pub container_id: &'a str,
}

impl<'a> InternalEvent for DockerLogsContainerMetadataFetchFailed<'a> {
    fn emit_logs(&self) {
        error!(
            message = "Failed to fetch container metadata.",
            error = ?self.error,
            container_id = ?self.container_id,
            internal_log_rate_secs = 10
        );
    }

    fn emit_metrics(&self) {
        counter!("container_metadata_fetch_errors_total", 1);
    }
}

#[derive(Debug)]
pub struct DockerLogsTimestampParseFailed<'a> {
    pub error: ParseError,
    pub container_id: &'a str,
}

impl<'a> InternalEvent for DockerLogsTimestampParseFailed<'a> {
    fn emit_logs(&self) {
        error!(
            message = "Failed to parse timestamp as RFC3339 timestamp.",
            error = ?self.error,
            container_id = ?self.container_id,
            internal_log_rate_secs = 10
        );
    }

    fn emit_metrics(&self) {
        counter!("timestamp_parse_errors_total", 1);
    }
}

#[derive(Debug)]
pub struct DockerLogsLoggingDriverUnsupported<'a> {
    pub container_id: &'a str,
    pub error: Error,
}

impl<'a> InternalEvent for DockerLogsLoggingDriverUnsupported<'a> {
    fn emit_logs(&self) {
        error!(
            message = r#"
                Docker engine is not using either the `jsonfile` or `journald`
                logging driver. Please enable one of these logging drivers
                to get logs from the Docker daemon."#,
            error = ?self.error,
            container_id = ?self.container_id,
        );
    }

    fn emit_metrics(&self) {
        counter!("logging_driver_errors_total", 1);
    }
}
