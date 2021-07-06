package metadata

components: sinks: aws_kinesis_firehose: components._aws & {
	title: "AWS Kinesis Firehose"

	classes: {
		commonly_used: false
		delivery:      "best_effort"
		development:   "stable"
		egress_method: "batch"
		service_providers: ["AWS"]
		stateful: false
	}

	features: {
		buffer: enabled:      true
		healthcheck: enabled: true
		send: {
			batch: {
				enabled:      true
				common:       false
				max_bytes:    4000000
				max_events:   500
				timeout_secs: 1
			}
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
				service: services.aws_kinesis_firehose

				interface: {
					socket: {
						api: {
							title: "AWS Kinesis Firehose API"
							url:   urls.aws_kinesis_firehose_api
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
		stream_name: {
			description: "The [stream name](\(urls.aws_cloudwatch_logs_stream_name)) of the target Kinesis Firehose delivery stream."
			required:    true
			warnings: []
			type: string: {
				examples: ["my-stream"]
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
			platform: "aws"
			_service: "firehose"

			policies: [
				{
					_action: "DescribeDeliveryStream"
					required_for: ["healthcheck"]
				},
				{
					_action: "PutRecordBatch"
				},
			]
		},
	]
}
