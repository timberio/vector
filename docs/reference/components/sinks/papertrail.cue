package metadata

components: sinks: papertrail: {
	title: "Papertrail"

	classes: {
		commonly_used: false
		delivery:      "best_effort"
		development:   "stable"
		egress_method: "stream"
		service_providers: ["Papertrail"]
		stateful: false
	}

	features: {
		buffer: enabled:      true
		healthcheck: enabled: true
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
			send_buffer_bytes: enabled: true
			keepalive: enabled:         true
			request: enabled:           false
			tls: {
				enabled:                true
				can_enable:             true
				can_verify_certificate: true
				can_verify_hostname:    true
				enabled_default:        true
			}
			to: {
				service: services.papertrail

				interface: {
					socket: {
						api: {
							title: "Syslog"
							url:   urls.syslog
						}
						direction: "outgoing"
						protocols: ["tcp"]
						ssl: "required"
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
		endpoint: {
			description: "The endpoint to send logs to."
			required:    true
			type: string: {
				examples: ["logs.papertrailapp.com:12345"]
				syntax: "literal"
			}
		}
	}

	input: {
		logs:    true
		metrics: null
	}

	how_it_works: {
		setup: {
			title: "Setup"
			body: """
				1. Register for a free account at [Papertrailapp.com](https://papertrailapp.com/signup?plan=free)

				2. [Create a Log Destination](https://papertrailapp.com/destinations/new) to get a Log Destination
				and ensure that TCP is enabled.

				3. Set the log destination as the `endpoint` option and start shipping your logs!
				"""
		}
	}
}
