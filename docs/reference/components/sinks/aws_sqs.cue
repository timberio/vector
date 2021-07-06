package metadata

components: sinks: aws_sqs: components._aws & {
	title: "Amazon Simple Queue Service (SQS)"

	classes: {
		commonly_used: false
		delivery:      "best_effort"
		development:   "beta"
		egress_method: "stream"
		service_providers: ["AWS"]
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
			request: {
				enabled:                    true
				concurrency:                5
				rate_limit_duration_secs:   1
				rate_limit_num:             5
				retry_initial_backoff_secs: 1
				retry_max_duration_secs:    10
				timeout_secs:               30
				headers:                    false
			}
			tls: enabled: false
			to: {
				service: services.aws_sqs

				interface: {
					socket: {
						api: {
							title: "Amazon Simple Queue Service API"
							url:   urls.aws_sqs_api
						}
						direction: "outgoing"
						protocols: ["http"]
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
		queue_url: {
			description: "The URL of the Amazon SQS queue to which messages are sent."
			required:    true
			warnings: []
			type: string: {
				examples: ["https://sqs.us-east-2.amazonaws.com/123456789012/MyQueue"]
				syntax: "literal"
			}
		}
		message_group_id: {
			common:      false
			description: "The tag that specifies that a message belongs to a specific message group. Can be applied only to FIFO queues."
			required:    false
			warnings: []
			type: string: {
				default: null
				examples: ["vector", "vector-%Y-%m-%d"]
				syntax: "literal"
			}
		}
	}

	input: {
		logs:    true
		metrics: null
	}

	permissions: iam: [
		{
			platform:  "aws"
			_service:  "sqs"
			_docs_tag: "AWSSimpleQueueService"

			policies: [
				{
					_action: "GetQueueAttributes"
					required_for: ["healthcheck"]
				},
				{
					_action: "SendMessage"
				},
			]
		},
	]

	telemetry: metrics: {
		events_discarded_total:  components.sources.internal_metrics.output.metrics.events_discarded_total
		processed_bytes_total:   components.sources.internal_metrics.output.metrics.processed_bytes_total
		processed_events_total:  components.sources.internal_metrics.output.metrics.processed_events_total
		processing_errors_total: components.sources.internal_metrics.output.metrics.processing_errors_total
	}
}
