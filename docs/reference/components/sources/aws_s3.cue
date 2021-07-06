package metadata

components: sources: aws_s3: components._aws & {
	title: "AWS S3"

	features: {
		multiline: enabled: true
		collect: {
			tls: enabled:        false
			checkpoint: enabled: false
			from: service:       services.aws_s3
		}
	}

	classes: {
		commonly_used: true
		deployment_roles: ["aggregator"]
		delivery:      "best_effort"
		development:   "beta"
		egress_method: "stream"
		stateful:      false
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
		requirements: [
			"""
				The AWS S3 source requires a SQS queue configured to receive S3
				bucket notifications for the desired S3 buckets.
				""",
		]
		warnings: []
		notices: []
	}

	installation: {
		platform_name: null
	}

	configuration: {
		strategy: {
			common:      false
			description: "The strategy to use to consume objects from AWS S3."
			required:    false
			type: string: {
				default: "sqs"
				enum: {
					sqs: "Consume S3 objects by polling for bucket notifications sent to an [AWS SQS queue](\(urls.aws_sqs))."
				}
				syntax: "literal"
			}
		}
		compression: {
			common:      false
			description: "The compression format of the S3 objects.."
			required:    false
			type: string: {
				default: "text"
				enum: {
					auto: "Vector will try to determine the compression format of the object from its: `Content-Encoding` metadata, `Content-Type` metadata, and key suffix (e.g. `.gz`). It will fallback to 'none' if it cannot determine the compression."
					gzip: "GZIP format."
					zstd: "ZSTD format."
					none: "Uncompressed."
				}
				syntax: "literal"
			}
		}
		sqs: {
			common:      true
			description: "SQS strategy options. Required if strategy=`sqs`."
			required:    false
			warnings: []
			type: object: {
				examples: []
				options: {
					poll_secs: {
						common:      true
						description: "How long to wait when polling SQS for new messages."
						required:    false
						warnings: []
						type: uint: {
							default: 15
							unit:    "seconds"
						}
					}
					visibility_timeout_secs: {
						common:      false
						description: "The visibility timeout to use for messages in secords. This controls how long a message is left unavailable when a Vector receives it. If a `vector` does not delete the message before the timeout expires, it will be made reavailable for another consumer; this can happen if, for example, the `vector` process crashes."
						required:    false
						warnings: ["Should be set higher than the length of time it takes to process an individual message to avoid that message being reprocessed."]
						type: uint: {
							default: 300
							unit:    "seconds"
						}
					}
					delete_message: {
						common:      true
						description: "Whether to delete the message once Vector processes it. It can be useful to set this to `false` to debug or during initial Vector setup."
						required:    false
						warnings: []
						type: bool: default: true
					}
					queue_url: {
						description: "The URL of the SQS queue to receieve bucket notifications from."
						required:    true
						warnings: []
						type: string: {
							examples: ["https://sqs.us-east-2.amazonaws.com/123456789012/MyQueue"]
							syntax: "literal"
						}
					}
				}
			}
		}
	}

	output: logs: object: {
		description: "A line from an S3 object."
		fields: {
			message: {
				description: "A line from the S3 object."
				required:    true
				type: string: {
					examples: ["53.126.150.246 - - [01/Oct/2020:11:25:58 -0400] \"GET /disintermediate HTTP/2.0\" 401 20308"]
					syntax: "literal"
				}
			}
			timestamp: fields._current_timestamp & {
				description: "The Last-Modified time of the object. Defaults the current timestamp if this information is missing."
			}
			bucket: {
				description: "The bucket of the object the line came from."
				required:    true
				type: string: {
					examples: ["my-bucket"]
					syntax: "literal"
				}
			}
			object: {
				description: "The object the line came from."
				required:    true
				type: string: {
					examples: ["AWSLogs/111111111111/vpcflowlogs/us-east-1/2020/10/26/111111111111_vpcflowlogs_us-east-1_fl-0c5605d9f1baf680d_20201026T1950Z_b1ea4a7a.log.gz"]
					syntax: "literal"
				}
			}
			region: {
				description: "The AWS region bucket is in."
				required:    true
				type: string: {
					examples: ["us-east-1"]
					syntax: "literal"
				}
			}
		}
	}

	how_it_works: {
		events: {
			title: "Handling events from the `aws_s3` source"
			body: """
				This source behaves very similarly to the `file` source in that
				it will output one event per line (unless the `multiline`
				configuration option is used).

				You will commonly want to use [transforms][urls.vector_transforms] to
				parse the data.  For example, to parse VPC flow logs sent to S3 you can
				chain the `tokenizer` transform:

				```toml
				[transforms.flow_logs]
					type = "tokenizer" # required
					inputs = ["s3"]
					field_names = ["version", "account_id", "interface_id", "srcaddr", "dstaddr", "srcport", "dstport", "protocol", "packets", "bytes", "start", "end", "action", "log_status"]

					types.srcport = "int"
					types.dstport = "int"
					types.packets = "int"
					types.bytes = "int"
					types.start = "timestamp|%s"
					types.end = "timestamp|%s"
				```

				To parse AWS load balancer logs, the `regex_parser` transform can be used:

				```toml
				[transforms.elasticloadbalancing_fields_parsed]
					type = "regex_parser"
					inputs = ["s3"]
					regex = '(?x)^
							(?P<type>[\\w]+)[ ]
							(?P<timestamp>[\\w:.-]+)[ ]
							(?P<elb>[^\\s]+)[ ]
							(?P<client_host>[\\d.:-]+)[ ]
							(?P<target_host>[\\d.:-]+)[ ]
							(?P<request_processing_time>[\\d.-]+)[ ]
							(?P<target_processing_time>[\\d.-]+)[ ]
							(?P<response_processing_time>[\\d.-]+)[ ]
							(?P<elb_status_code>[\\d-]+)[ ]
							(?P<target_status_code>[\\d-]+)[ ]
							(?P<received_bytes>[\\d-]+)[ ]
							(?P<sent_bytes>[\\d-]+)[ ]
							"(?P<request_method>[\\w-]+)[ ]
							(?P<request_url>[^\\s]+)[ ]
							(?P<request_protocol>[^"\\s]+)"[ ]
							"(?P<user_agent>[^"]+)"[ ]
							(?P<ssl_cipher>[^\\s]+)[ ]
							(?P<ssl_protocol>[^\\s]+)[ ]
							(?P<target_group_arn>[\\w.:/-]+)[ ]
							"(?P<trace_id>[^\\s"]+)"[ ]
							"(?P<domain_name>[^\\s"]+)"[ ]
							"(?P<chosen_cert_arn>[\\w:./-]+)"[ ]
							(?P<matched_rule_priority>[\\d-]+)[ ]
							(?P<request_creation_time>[\\w.:-]+)[ ]
							"(?P<actions_executed>[\\w,-]+)"[ ]
							"(?P<redirect_url>[^"]+)"[ ]
							"(?P<error_reason>[^"]+)"'
					field = "message"
					drop_failed = false

					types.received_bytes = "int"
					types.request_processing_time = "float"
					types.sent_bytes = "int"
					types.target_processing_time = "float"
					types.response_processing_time = "float"

				[transforms.elasticloadbalancing_url_parsed]
					type = "regex_parser"
					inputs = ["elasticloadbalancing_fields_parsed"]
					regex = '^(?P<url_scheme>[\\w]+)://(?P<url_hostname>[^\\s:/?#]+)(?::(?P<request_port>[\\d-]+))?-?(?:/(?P<url_path>[^\\s?#]*))?(?P<request_url_query>\\?[^\\s#]+)?'
					field = "request_url"
					drop_failed = false
				```
				"""
		}
	}

	permissions: iam: [
		{
			platform: "aws"
			_service: "s3"

			policies: [
				{
					_action: "GetObject"
				},
			]
		},
		{
			platform:  "aws"
			_service:  "sqs"
			_docs_tag: "AWSSimpleQueueService"

			policies: [
				{
					_action:       "ReceiveMessage"
					required_when: "[`strategy`](#strategy) is set to `sqs`"
				},
				{
					_action:       "DeleteMessage"
					required_when: "[`strategy`](#strategy) is set to `sqs` and [`delete_message`](#delete_message) is set to `true`"
				},
			]
		},
	]

	telemetry: metrics: {
		events_in_total:                        components.sources.internal_metrics.output.metrics.events_in_total
		processed_bytes_total:                  components.sources.internal_metrics.output.metrics.processed_bytes_total
		sqs_message_delete_failed_total:        components.sources.internal_metrics.output.metrics.sqs_message_delete_failed_total
		sqs_message_delete_succeeded_total:     components.sources.internal_metrics.output.metrics.sqs_message_delete_succeeded_total
		sqs_message_processing_failed_total:    components.sources.internal_metrics.output.metrics.sqs_message_processing_failed_total
		sqs_message_processing_succeeded_total: components.sources.internal_metrics.output.metrics.sqs_message_processing_succeeded_total
		sqs_message_receive_failed_total:       components.sources.internal_metrics.output.metrics.sqs_message_receive_failed_total
		sqs_message_receive_succeeded_total:    components.sources.internal_metrics.output.metrics.sqs_message_receive_succeeded_total
		sqs_message_received_messages_total:    components.sources.internal_metrics.output.metrics.sqs_message_received_messages_total
		sqs_s3_event_record_ignored_total:      components.sources.internal_metrics.output.metrics.sqs_s3_event_record_ignored_total
	}
}
