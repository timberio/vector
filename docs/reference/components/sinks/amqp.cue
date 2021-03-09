package metadata

components: sinks: amqp: {
	title: "Amqp"

	classes: {
		commonly_used: true
		delivery:      "at_least_once"
		development:   "beta"
		egress_method: "dynamic"
		service_providers: ["Amqp"]
		stateful: false
	}

	features: {
		buffer: enabled:      true
		healthcheck: enabled: true
		send: {
			batch: {
				enabled:      true
				common:       true
				max_bytes:    null
				max_events:   null
				timeout_secs: null
			}
			compression: {
				enabled: true
				default: "none"
				algorithms: ["none", "gzip", "lz4", "snappy", "zstd"]
				levels: ["none", "fast", "default", "best", 0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
			}
			encoding: {
				enabled: true
				codec: {
					enabled: true
					default: null
					enum: ["json", "text"]
				}
			}
			request: enabled: false
			tls: {
				enabled:                true
				can_enable:             true
				can_verify_certificate: false
				can_verify_hostname:    false
				enabled_default:        false
			}
			to: components._amqp.features.send.to
		}
	}

	support: components._amqp.support

	configuration: {
		connection: {
			common:      true
			description: "Connection options for Amqp sink"
			required:    true
			warnings: []
			type: object: {
				connection_string: components._amqp.configuration.connection_string
				tls: components._amqp.configuration.tls
			}
		}
		exchange: {
			description: "The exchange to publish messages to"
			required:    true
			warnings: []
			type: string: {
				examples: ["message_exchange"]
				syntax: "literal"
			}
		}
		routing_key: {
			description: "Template use to generate a routing key which corresponds to a queue binding"
			required:    false
			warnings: []
			type: string: {
				examples: ["{{ field_a }}-{{ field_b}}"]
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

	how_it_works: components._amqp.how_it_works

	telemetry: metrics: {
		events_discarded_total:  components.sources.internal_metrics.output.metrics.events_discarded_total
		processing_errors_total: components.sources.internal_metrics.output.metrics.processing_errors_total
	}
}
