package metadata

components: sinks: console: {
	title: "Console"

	classes: {
		commonly_used: false
		delivery:      "best_effort"
		development:   "stable"
		egress_method: "stream"
		service_providers: []
		stateful: false
	}

	features: {
		buffer: enabled:      false
		healthcheck: enabled: false
		send: {
			compression: enabled: false
			encoding: {
				enabled: true
				codec: {
					enabled: true
					default: null
					enum: ["json", "text"]
				}
			}
			request: enabled: false
			tls: enabled:     false
			to: {
				service: services.stdout
				interface: stdout: {}
			}
		}
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

	configuration: {
		target: {
			common:      true
			description: "The [standard stream](\(urls.standard_streams)) to write to."
			required:    false
			warnings: []
			type: string: {
				default: "stdout"
				enum: {
					stdout: "Output will be written to [STDOUT](\(urls.stdout))"
					stderr: "Output will be written to [STDERR](\(urls.stderr))"
				}
				syntax: "literal"
			}
		}
	}

	input: {
		logs: true
		metrics: {
			counter:      true
			distribution: true
			gauge:        true
			histogram:    true
			set:          true
			summary:      true
		}
	}

	telemetry: metrics: {
		processed_bytes_total:   components.sources.internal_metrics.output.metrics.processed_bytes_total
		processed_events_total:  components.sources.internal_metrics.output.metrics.processed_events_total
		processing_errors_total: components.sources.internal_metrics.output.metrics.processing_errors_total
	}
}
