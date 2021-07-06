package metadata

components: sinks: aws_kinesis_streams: components._aws & {
	title: "AWS Kinesis Data Streams"

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
				max_bytes:    5000000
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
				service: services.aws_kinesis_data_streams

				interface: {
					socket: {
						api: {
							title: "AWS Kinesis Data Streams API"
							url:   urls.aws_kinesis_streams_api
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
		partition_key_field: {
			common:      true
			description: "The log field used as the Kinesis record's partition key value."
			required:    false
			warnings: []
			type: string: {
				default: null
				examples: ["user_id"]
				syntax: "literal"
			}
		}
		stream_name: {
			description: "The [stream name](\(urls.aws_cloudwatch_logs_stream_name)) of the target Kinesis Logs stream."
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

	how_it_works: {
		partitioning: {
			title: "Partitioning"
			body:  """
				By default, Vector issues random 16 byte values for each
				[Kinesis record's partition key](\(urls.aws_kinesis_partition_key)), evenly
				distributing records across your Kinesis partitions. Depending on your use case
				this might not be sufficient since random distribution does not preserve order.
				To override this, you can supply the `partition_key_field` option. This option
				presents an alternate field on your event to use as the partition key value instead.
				This is useful if you have a field already on your event, and it also pairs
				nicely with the [`add_fields` transform][docs.transforms.add_fields].
				"""
			sub_sections: [
				{
					title: "Missing partition keys"
					body: """
						Kenesis requires a value for the partition key and therefore if the key is
						missing or the value is blank the event will be dropped and a
						[`warning` level log event][docs.monitoring#logs] will be logged. As such,
						the field specified in the `partition_key_field` option should always contain
						a value.
						"""
				},
				{
					title: "Partition keys that exceed 256 characters"
					body: """
						If the value provided exceeds the maximum allowed length of 256 characters
						Vector will slice the value and use the first 256 characters.
						"""
				},
				{
					title: "Non-string partition keys"
					body: """
						Vector will coerce the value into a string.
						"""
				},
			]
		}
	}

	permissions: iam: [
		{
			platform: "aws"
			_service: "kinesis"

			policies: [
				{
					_action: "DescribeStream"
					required_for: ["healthcheck"]
				},
				{
					_action: "PutRecords"
				},
			]
		},
	]

	telemetry: metrics: {
		processed_bytes_total:  components.sources.internal_metrics.output.metrics.processed_bytes_total
		processed_events_total: components.sources.internal_metrics.output.metrics.processed_events_total
	}
}
