package metadata

components: sources: generator: {
	title: "Generator"

	description: """
		Generates fakes events, useful for testing, benchmarking, and demoing.
		"""

	classes: {
		commonly_used: false
		delivery:      "best_effort"
		deployment_roles: ["daemon", "sidecar"]
		development:   "stable"
		egress_method: "stream"
		stateful:      false
	}

	features: {
		multiline: enabled: false
		generate: {}
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
		format: {
			description: "The format of the randomly generated output."
			required:    true
			warnings: []
			type: string: {
				enum: {
					"shuffle":       "Lines are chosen at random from the list specified using `lines`."
					"apache_common": "Randomly generated logs in [Apache common](\(urls.apache_common)) format."
					"apache_error":  "Randomly generated logs in [Apache error](\(urls.apache_error)) format."
					"syslog":        "Randomly generated logs in Syslog format ([RFC 5424](\(urls.syslog_5424)))."
					"bsd_syslog":    "Randomly generated logs in Syslog format ([RFC 3164](\(urls.syslog_3164)))."
					"json":          "Randomly generated HTTP server logs in [JSON](\(urls.json)) format."
				}
				syntax: "literal"
			}
		}
		interval: {
			common:      false
			description: "The amount of time, in seconds, to pause between each batch of output lines. If not set, there is no delay."
			required:    false
			warnings: []
			type: float: {
				default: null
				examples: [1.0, 0.1, 0.01]
			}
		}
		count: {
			common:      false
			description: "The total number of lines to output. By default the source continuously prints logs (infinitely)."
			required:    false
			warnings: []
			type: uint: {
				default: null
				unit:    null
			}
		}
		lines: {
			common:        false
			description:   "The list of lines to output."
			relevant_when: "`format` = `shuffle`"
			required:      false
			warnings: []
			type: array: {
				default: null
				items: type: string: {
					examples: ["Line 1", "Line 2"]
					syntax: "literal"
				}
			}
		}
		sequence: {
			common:        false
			relevant_when: "`format` = `shuffle`"
			description:   "If `true`, each output line starts with an increasing sequence number, beginning with 0."
			required:      false
			warnings: []
			type: bool: default: false
		}
	}

	output: logs: {}

	telemetry: metrics: {
		processed_events_total: components.sources.internal_metrics.output.metrics.processed_events_total
	}
}
