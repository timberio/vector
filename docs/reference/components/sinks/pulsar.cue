package metadata

components: sinks: pulsar: {
	title: "Apache Pulsar"

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
			compression: enabled: false
			encoding: {
				enabled: true
				codec: {
					enabled: true
					default: null
					enum: ["text", "json"]
				}
			}
			request: enabled: false
			tls: enabled:     false
			to: {
				service: services.pulsar

				interface: {
					socket: {
						api: {
							title: "Pulsar protocol"
							url:   urls.pulsar_protocol
						}
						direction: "outgoing"
						protocols: ["http"]
						ssl: "disabled"
					}
				}
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
		auth: {
			common:      false
			description: "Options for the authentication strategy."
			required:    false
			warnings: []
			type: object: {
				examples: []
				options: {
					name: {
						common:      false
						description: "The basic authentication name."
						required:    false
						warnings: []
						type: string: {
							default: null
							examples: ["${PULSAR_NAME}", "name123"]
							syntax: "literal"
						}
					}
					token: {
						common:      false
						description: "The basic authentication password."
						required:    false
						warnings: []
						type: string: {
							default: null
							examples: ["${PULSAR_TOKEN}", "123456789"]
							syntax: "literal"
						}
					}
				}
			}
		}
		endpoint: {
			description: "Endpoint to which the pulsar client should connect to."
			required:    true
			type: string: {
				examples: ["pulsar://127.0.0.1:6650"]
				syntax: "literal"
			}
		}
		topic: {
			description: "The Pulsar topic name to write events to."
			required:    true
			warnings: []
			type: string: {
				examples: ["topic-1234"]
				syntax: "literal"
			}
		}
	}

	input: {
		logs:    true
		metrics: null
	}

	telemetry: metrics: {
		encode_errors_total: components.sources.internal_metrics.output.metrics.encode_errors_total
	}
}
