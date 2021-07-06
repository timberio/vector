package metadata

components: sources: stdin: {
	title: "STDIN"

	classes: {
		commonly_used: false
		delivery:      "best_effort"
		deployment_roles: ["sidecar"]
		development:   "stable"
		egress_method: "stream"
		stateful:      false
	}

	features: {
		multiline: enabled: false
		receive: {
			from: {
				service: services.stdin
				interface: stdin: {}
			}

			tls: enabled: false
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

	installation: {
		platform_name: null
	}

	configuration: {
		host_key: {
			category:    "Context"
			common:      false
			description: "The key name added to each event representing the current host. This can also be globally set via the [global `host_key` option][docs.reference.configuration.global-options#host_key]."
			required:    false
			warnings: []
			type: string: {
				default: "host"
				syntax:  "literal"
			}
		}
		max_length: {
			common:      false
			description: "The maximum bytes size of a message before rest of it will be discarded."
			required:    false
			warnings: []
			type: uint: {
				default: 102400
				unit:    "bytes"
			}
		}
	}

	output: logs: line: {
		description: "An individual event from STDIN."
		fields: {
			host:      fields._local_host
			message:   fields._raw_line
			timestamp: fields._current_timestamp
		}
	}

	examples: [
		{
			_line: """
				2019-02-13T19:48:34+00:00 [info] Started GET "/" for 127.0.0.1
				"""
			title: "STDIN line"
			configuration: {}
			input: """
				```text
				\( _line )
				```
				"""
			output: log: {
				timestamp: _values.current_timestamp
				message:   _line
				host:      _values.local_host
			}
		},
	]

	how_it_works: {
		line_delimiters: {
			title: "Line Delimiters"
			body: """
				Each line is read until a new line delimiter, the `0xA` byte, is found.
				"""
		}
	}

	telemetry: metrics: {
		events_in_total:          components.sources.internal_metrics.output.metrics.events_in_total
		processed_bytes_total:    components.sources.internal_metrics.output.metrics.processed_bytes_total
		processed_events_total:   components.sources.internal_metrics.output.metrics.processed_events_total
		stdin_reads_failed_total: components.sources.internal_metrics.output.metrics.stdin_reads_failed_total
	}
}
