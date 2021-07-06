package metadata

components: sources: exec: {
	title: "Exec"

	classes: {
		commonly_used: false
		delivery:      "best_effort"
		deployment_roles: ["sidecar"]
		development:   "beta"
		egress_method: "stream"
		stateful:      false
	}

	features: {
		multiline: enabled: false
		receive: {
			from: {
				service: services.exec
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
		mode: {
			description: "The type of exec mechanism."
			required:    true
			type: string: {
				enum: {
					scheduled: "Scheduled exec mechanism."
					streaming: "Streaming exec mechanism."
				}
				syntax: "literal"
			}
		}
		command: {
			required:    true
			description: "The command to be run, plus any arguments required."
			type: array: {
				examples: [["echo", "Hello World!"], ["ls", "-la"]]
				items: type: string: {
					syntax: "literal"
				}
			}
		}
		working_directory: {
			common:      false
			required:    false
			description: "The directory in which to run the command."
			warnings: []
			type: string: {
				default: null
				syntax:  "literal"
			}
		}
		include_stderr: {
			common:      false
			description: "Include the output of stderr when generating events."
			required:    false
			type: bool: default: true
		}
		event_per_line: {
			common:      false
			description: "Determine if events should be generated per line or buffered and output as a single event when script execution finishes."
			required:    false
			type: bool: default: true
		}
		maximum_buffer_size_bytes: {
			common:      false
			description: "The maximum buffer size allowed before a log event will be generated."
			required:    false
			type: uint: {
				default: 1000000
				unit:    "bytes"
			}
		}
		scheduled: {
			common:      true
			description: "The scheduled options."
			required:    false
			warnings: []
			type: object: {
				examples: []
				options: {
					exec_interval_secs: {
						common:        true
						description:   "The interval in seconds between scheduled command runs. The command will be killed if it takes longer than exec_interval_secs to run."
						relevant_when: "mode = `scheduled`"
						required:      false
						type: uint: {
							default: 60
							unit:    "seconds"
						}
					}
				}
			}
		}
		streaming: {
			common:      true
			description: "The streaming options."
			required:    false
			warnings: []
			type: object: {
				examples: []
				options: {
					respawn_on_exit: {
						common:        true
						description:   "Determine if a streaming command should be restarted if it exits."
						relevant_when: "mode = `streaming`"
						required:      false
						type: bool: default: true
					}
					respawn_interval_secs: {
						common:        false
						description:   "The interval in seconds between restarting streaming commands if needed."
						relevant_when: "mode = `streaming`"
						required:      false
						warnings: []
						type: uint: {
							default: 5
							unit:    "seconds"
						}
					}
				}
			}
		}
	}

	output: logs: line: {
		description: "An individual event from exec."
		fields: {
			host:      fields._local_host
			message:   fields._raw_line
			timestamp: fields._current_timestamp
			data_stream: {
				common:      true
				description: "The data stream from which the event originated."
				required:    false
				type: string: {
					examples: ["stdout", "stderr"]
					default: null
					syntax:  "literal"
				}
			}
			pid: {
				description: "The process ID of the command."
				required:    true
				type: uint: {
					examples: [60085, 668]
					unit: null
				}
			}
			command: {
				required:    true
				description: "The command that was run to generate this event."
				type: array: {
					items: type: string: {
						examples: ["echo", "Hello World!", "ls", "-la"]
						syntax: "literal"
					}
				}
			}
		}
	}

	examples: [
		{
			_line:      "64 bytes from 127.0.0.1: icmp_seq=0 ttl=64 time=0.060 ms"
			_timestamp: "2020-03-13T20:45:38.119Z"
			title:      "Exec line"
			configuration: {}
			input: """
				```text
				(_message)
				```
				"""
			output: log: {
				data_stream: "stdout"
				pid:         5678
				timestamp:   _timestamp
				host:        _values.local_host
				message:     _line
			}
		},
	]

	how_it_works: {
		line_delimiters: {
			title: "Line Delimiters"
			body: """
				Each line is read until a new line delimiter, the `0xA` byte, is found or the end of the
				maximum_buffer_size is reached.
				"""
		}
	}

	telemetry: metrics: {
		events_in_total:                    components.sources.internal_metrics.output.metrics.events_in_total
		processed_bytes_total:              components.sources.internal_metrics.output.metrics.processed_bytes_total
		processed_events_total:             components.sources.internal_metrics.output.metrics.processed_events_total
		processing_errors_total:            components.sources.internal_metrics.output.metrics.processing_errors_total
		command_executed_total:             components.sources.internal_metrics.output.metrics.command_executed_total
		command_execution_duration_seconds: components.sources.internal_metrics.output.metrics.command_execution_duration_seconds
	}
}
