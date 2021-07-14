package metadata

components: sources: nats: {
	title: "NATS"

	features: {
		collect: {
			checkpoint: enabled: false
			from: components._nats.features.collect.from
		}
		multiline: enabled: false
	}

	classes: {
		commonly_used: true
		deployment_roles: ["aggregator"]
		delivery:      "best_effort"
		development:   "beta"
		egress_method: "stream"
		stateful:      false
	}

	support: components._nats.support

	installation: {
		platform_name: null
	}

	configuration: components._nats.configuration & {
		queue: {
			common:      false
			description: "NATS Queue Group to join"
			required:    false
			type: string: {
				default: "vector"
				examples: ["foo", "API Name Option Example"]
				syntax: "literal"
			}
		}
	}

	output: logs: record: {
		description: "An individual NATS record"
		fields: {
			message: {
				description: "The raw line from the NATS message."
				required:    true
				type: string: {
					examples: ["53.126.150.246 - - [01/Oct/2020:11:25:58 -0400] \"GET /disintermediate HTTP/2.0\" 401 20308"]
					syntax: "literal"
				}
			}
		}
	}

	telemetry: metrics: {
		processed_bytes_total:  components.sources.internal_metrics.output.metrics.processed_bytes_total
		processed_events_total: components.sources.internal_metrics.output.metrics.processed_events_total
	}

	how_it_works: components._nats.how_it_works
}
