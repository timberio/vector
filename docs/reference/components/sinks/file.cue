package metadata

components: sinks: file: {
	title: "File"

	classes: {
		commonly_used: false
		delivery:      "best_effort"

		development:   "beta"
		egress_method: "stream"
		service_providers: []
		stateful: false
	}

	features: {
		buffer: enabled:      false
		healthcheck: enabled: true
		send: {
			compression: {
				enabled: true
				default: "none"
				algorithms: ["none", "gzip"]
				levels: ["none", "fast", "default", "best", 0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
			}
			encoding: {
				enabled: true
				codec: {
					enabled: true
					default: null
					enum: ["ndjson", "text"]
				}
			}
			request: enabled: false
			tls: enabled:     false
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
		idle_timeout_secs: {
			common:      false
			description: "The amount of time a file can be idle  and stay open. After not receiving any events for this timeout, the file will be flushed and closed.\n"
			required:    false
			warnings: []
			type: uint: {
				default: 30
				unit:    null
			}
		}
		path: {
			description: "File name to write events to."
			required:    true
			warnings: []
			type: string: {
				examples: ["/tmp/vector-%Y-%m-%d.log", "/tmp/application-{{ application_id }}-%Y-%m-%d.log"]
				syntax: "template"
			}
		}
	}

	input: {
		logs:    true
		metrics: null
	}

	how_it_works: {
		dir_and_file_creation: {
			title: "File & Directory Creation"
			body: """
				Vector will attempt to create the entire directory structure
				and the file when emitting events to the file sink. This
				requires that the Vector agent have the correct permissions
				to create and write to files in the specified directories.
				"""
		}
	}

	telemetry: metrics: {
		events_discarded_total:  components.sources.internal_metrics.output.metrics.events_discarded_total
		processing_errors_total: components.sources.internal_metrics.output.metrics.processing_errors_total
	}
}
