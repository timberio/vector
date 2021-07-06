package metadata

components: sources: prometheus_scrape: {
	title: "Prometheus Scrape"
	alias: "prometheus"

	classes: {
		commonly_used: false
		delivery:      "best_effort"
		deployment_roles: ["daemon", "sidecar"]
		development:   "beta"
		egress_method: "batch"
		stateful:      false
	}

	features: {
		collect: {
			checkpoint: enabled: false
			from: {
				service: services.prometheus_client

				interface: socket: {
					api: {
						title: "Prometheus"
						url:   urls.prometheus_text_based_exposition_format
					}
					direction: "outgoing"
					protocols: ["http"]
					ssl: "optional"
				}
			}
			tls: {
				enabled:                true
				can_enable:             false
				can_verify_certificate: true
				can_verify_hostname:    true
				enabled_default:        false
			}
		}
		multiline: enabled: false
	}

	support: {
		targets: {
			"aarch64-unknown-linux-gnu":      true
			"aarch64-unknown-linux-musl":     true
			"armv7-unknown-linux-gnueabihf":  true
			"armv7-unknown-linux-musleabihf": true
			"x86_64-apple-darwin":            true
			"x86_64-pc-windows-msv":          true
			"x86_64-unknown-linux-gnu":       true
			"x86_64-unknown-linux-musl":      true
		}
		requirements: []
		warnings: []
		notices: []
	}

	installation: {
		platform_name: null
	}

	configuration: {
		endpoints: {
			description: "Endpoints to scrape metrics from."
			required:    true
			warnings: ["You must explicitly add the path to your endpoints. Vector will _not_ automatically add `/metics`."]
			type: array: {
				items: type: string: {
					examples: ["http://localhost:9090/metrics"]
					syntax: "literal"
				}
			}
		}
		scrape_interval_secs: {
			common:      true
			description: "The interval between scrapes, in seconds."
			required:    false
			warnings: []
			type: uint: {
				default: 15
				unit:    "seconds"
			}
		}
		auth: configuration._http_auth & {_args: {
			password_example: "${PROMETHEUS_PASSWORD}"
			username_example: "${PROMETHEUS_USERNAME}"
		}}
	}

	output: metrics: {
		counter:   output._passthrough_counter
		gauge:     output._passthrough_gauge
		histogram: output._passthrough_histogram
		summary:   output._passthrough_summary
	}

	telemetry: metrics: {
		events_in_total:           components.sources.internal_metrics.output.metrics.events_in_total
		http_error_response_total: components.sources.internal_metrics.output.metrics.http_error_response_total
		http_request_errors_total: components.sources.internal_metrics.output.metrics.http_request_errors_total
		parse_errors_total:        components.sources.internal_metrics.output.metrics.parse_errors_total
		processed_bytes_total:     components.sources.internal_metrics.output.metrics.processed_bytes_total
		processed_events_total:    components.sources.internal_metrics.output.metrics.processed_events_total
		requests_completed_total:  components.sources.internal_metrics.output.metrics.requests_completed_total
		request_duration_seconds:  components.sources.internal_metrics.output.metrics.request_duration_seconds
	}
}
