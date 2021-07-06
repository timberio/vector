package metadata

components: sinks: aws_cloudwatch_metrics: components._aws & {
	title: "AWS Cloudwatch Metrics"

	classes: {
		commonly_used: false
		delivery:      "best_effort"
		development:   "stable"
		egress_method: "batch"
		service_providers: ["AWS"]
		stateful: false
	}

	features: {
		buffer: enabled:      false
		healthcheck: enabled: true
		send: {
			batch: {
				enabled:      true
				common:       false
				max_events:   20
				timeout_secs: 1
			}
			compression: {
				enabled: true
				default: "none"
				algorithms: ["none", "gzip"]
				levels: ["none", "fast", "default", "best", 0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
			}
			encoding: enabled: false
			request: enabled:  false
			tls: enabled:      false
			to: {
				service: services.aws_cloudwatch_metrics

				interface: {
					socket: {
						api: {
							title: "AWS Cloudwatch metrics API"
							url:   urls.aws_cloudwatch_metrics_api
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
		warnings: [
			"""
				Gauge values are persisted between flushes. On Vector start up each
				gauge is assumed to have zero, 0.0, value, that can be updated
				explicitly by the consequent absolute, not delta, gauge observation,
				or by delta increments/decrements. Delta gauges are considered an
				advanced feature useful in a distributed setting, however they
				should be used with care.
				""",
		]
		notices: [
			"""
				CloudWatch Metrics types are organized not by their semantics, but
				by storage properties:

				* Statistic Sets
				* Data Points

				In Vector only the latter is used to allow lossless statistics
				calculations on CloudWatch side.
				""",
		]
	}

	configuration: {
		default_namespace: {
			description: """
				A [namespace](https://docs.aws.amazon.com/AmazonCloudWatch/latest/monitoring/cloudwatch_concepts.html#Namespace) that will isolate different metrics from each other.
				Used as a namespace for metrics that don't have it.
				"""
			required: true
			warnings: []
			type: string: {
				examples: ["service"]
				syntax: "literal"
			}
		}
	}

	input: {
		logs: false
		metrics: {
			counter:      true
			distribution: true
			gauge:        true
			histogram:    false
			set:          false
			summary:      false
		}
	}

	permissions: iam: [
		{
			platform:  "aws"
			_service:  "cloudwatch"
			_docs_tag: "AmazonCloudWatch"

			policies: [
				{
					_action: "PutMetricData"
					required_for: ["healthcheck", "write"]
				},
			]
		},
	]
}
