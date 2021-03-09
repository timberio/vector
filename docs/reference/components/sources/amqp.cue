package metadata

components: sources: amqp: {
	title: "Amqp"

	features: {
	    collect: from: {
            service: services.amqp
            interface: {
                socket: {
                    api: {
                        title: "Amqp protocol"
                        url:   urls.amqp_protocol
                    }
                    direction: "incoming"
                    port:      5672
                    protocols: ["tcp"]
                    ssl: "optional"
                }
            }
        }
	}

	classes: {
		commonly_used: true
		deployment_roles: ["aggregator"]
		delivery:      "at_least_once"
		development:   "beta"
		egress_method: "stream"
		stateful:      false
	}

	support: components._amqp.support

	installation: {
		platform_name: null
	}

	configuration: {
	    connection: {
            common: true
            description: "Connection options for Amqp source"
            required: true
            warnings: []
            type: object: {
                user: components._amqp.configuration.user
                password: components._amqp.configuration.password
                host: components._amqp.configuration.host
                port: components._amqp.configuration.port
                connection_timeout: components._amqp.configuration.connection_timeout
                vhost: components._amqp.configuration.vhost
            }
        }
        group_id: {
            description: "The consumer group name to be used to consume events from Amqp.\n"
            required:    true
            warnings: []
            type: string: {
                examples: ["consumer-group-name"]
                syntax: "literal"
            }
        }
		routing_key: {
			common:      true
			description: "The log field name to use for the Amqp routing key."
			required:    false
			warnings: []
			type: string: {
				default: "message_key"
				examples: ["message_key"]
				syntax: "literal"
			}
		}
		exchange_key: {
            common:      true
            description: "The log field name to use for the Amqp exchange key."
            required:    false
            warnings: []
            type: string: {
                default: "message_key"
                examples: ["message_key"]
                syntax: "literal"
            }
        }
		offset_key: {
            common:      true
            description: "The log field name to use for the Amqp offset key."
            required:    false
            warnings: []
            type: string: {
                default: "message_key"
                examples: ["message_key"]
                syntax: "literal"
            }
        }
	}

	output: logs: record: {
		description: "An individual Amqp record"
		fields: {
			message: {
				description: "The raw line from the Amqp record."
				required:    true
				type: string: {
					examples: ["53.126.150.246 - - [01/Oct/2020:11:25:58 -0400] \"GET /disintermediate HTTP/2.0\" 401 20308"]
					syntax: "literal"
				}
			}
			offset: {
				description: "The Amqp offset at the time the record was retrieved."
				required:    true
				type: uint: {
					examples: [100]
					unit: null
				}
			}
			timestamp: fields._current_timestamp & {
				description: "The timestamp encoded in the Amqp message or the current time if it cannot be fetched."
			}
			exchange: {
				description: "The Amqp exchange that the record came from."
				required:    true
				type: string: {
					examples: ["topic"]
					syntax: "literal"
				}
			}
		}
	}

	telemetry: metrics: {
		events_in_total:                      components.sources.internal_metrics.output.metrics.events_in_total
		consumer_offset_updates_failed_total: components.sources.internal_metrics.output.metrics.consumer_offset_updates_failed_total
		events_failed_total:                  components.sources.internal_metrics.output.metrics.events_failed_total
		processed_bytes_total:                components.sources.internal_metrics.output.metrics.processed_bytes_total
		processed_events_total:               components.sources.internal_metrics.output.metrics.processed_events_total
	}

	how_it_works: components._amqp.how_it_works
}
