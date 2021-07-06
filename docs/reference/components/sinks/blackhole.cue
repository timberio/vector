package metadata

components: sinks: blackhole: {
	title: "Blackhole"

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
			encoding: enabled:    false
			request: enabled:     false
			tls: enabled:         false
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
		print_amount: {
			common:      false
			description: "The number of events that must be received in order to print a summary of activity."
			required:    false
			warnings: []
			type: uint: {
				default: 1000
				examples: [1000]
				unit: null
			}
		}
		rate: {
			common:      false
			description: "Rates the amount of events that the sink can consume per second."
			required:    false
			warnings: []
			type: uint: {
				default: null
				examples: [1000]
				unit: null
			}
		}
	}

	input: {
		logs:    true
		metrics: null
	}

	telemetry: metrics: {
		processed_bytes_total:  components.sources.internal_metrics.output.metrics.processed_bytes_total
		processed_events_total: components.sources.internal_metrics.output.metrics.processed_events_total
	}
}
