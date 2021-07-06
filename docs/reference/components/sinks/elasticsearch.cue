package metadata

components: sinks: elasticsearch: {
	title: "Elasticsearch"

	classes: {
		commonly_used: true
		delivery:      "best_effort"
		development:   "stable"
		egress_method: "batch"
		service_providers: ["AWS", "Azure", "Elastic", "GCP"]
		stateful: false
	}

	features: {
		buffer: enabled:      true
		healthcheck: enabled: true
		send: {
			batch: {
				enabled:      true
				common:       false
				max_bytes:    10490000
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
				codec: enabled: false
			}
			request: {
				enabled:                    true
				concurrency:                5
				rate_limit_duration_secs:   1
				rate_limit_num:             5
				retry_initial_backoff_secs: 1
				retry_max_duration_secs:    10
				timeout_secs:               60
				headers:                    true
			}
			tls: {
				enabled:                true
				can_enable:             false
				can_verify_certificate: true
				can_verify_hostname:    true
				enabled_default:        false
			}
			to: {
				service: services.elasticsearch

				interface: {
					socket: {
						api: {
							title: "Elasticsearch bulk API"
							url:   urls.elasticsearch_bulk
						}
						direction: "outgoing"
						protocols: ["http"]
						ssl: "optional"
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
		requirements: [
			#"""
				Elasticsearch's Data streams feature requires Vector to be configured with the `create` `bulk_action`. *This is not enabled by default.*
				"""#,
		]
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
				options: components._aws.configuration.auth.type.object.options & {
					password: {
						description: "The basic authentication password."
						required:    true
						warnings: []
						type: string: {
							examples: ["${ELASTICSEARCH_PASSWORD}", "password"]
							syntax: "literal"
						}
					}
					strategy: {
						description: "The authentication strategy to use."
						required:    true
						warnings: []
						type: string: {
							enum: {
								aws:   "Authentication strategy used for [AWS' hosted Elasticsearch service](\(urls.aws_elasticsearch))."
								basic: "The [basic authentication strategy](\(urls.basic_auth))."
							}
							syntax: "literal"
						}
					}
					user: {
						description: "The basic authentication user name."
						required:    true
						warnings: []
						type: string: {
							examples: ["${ELASTICSEARCH_USERNAME}", "username"]
							syntax: "literal"
						}
					}
				}
			}
		}
		aws: {
			common:      false
			description: "Options for the AWS connections."
			required:    false
			warnings: []
			type: object: {
				examples: []
				options: {
					region: {
						common:      true
						description: "The [AWS region][urls.aws_regions] of the target service. This defaults to the region named in the endpoint parameter, or the value of the `$AWS_REGION` or `$AWS_DEFAULT_REGION` environment variables if that cannot be determined, or \"us-east-1\"."
						required:    false
						warnings: []
						type: string: {
							default: null
							examples: ["us-east-1"]
							syntax: "literal"
						}
					}
				}
			}
		}
		bulk_action: {
			common:      false
			description: "Action to use when making requests to the [Elasticsearch Bulk API](elasticsearch_bulk). Currently, Vector only supports `index` and `create` in the `regular` mode and `create` in `data_stream` mode. `update` and `delete` actions are not supported."
			required:    false
			warnings: []
			type: string: {
				default: "index"
				examples: ["index", "create", "{{ action }}"]
				syntax: "template"
			}
		}
		data_stream: {
			common:      false
			description: "Options for the data stream mode."
			required:    false
			warnings: []
			type: object: {
				examples: []
				options: {
					auto_routing: {
						common: false
						description: """
							Automatically routes events by deriving the data stream name using specific event fields with the `data_stream.type-data_stream.dataset-data_stream.namespace` format.

							If enabled, the data_stream.* event fields will take precedence over the data_stream.type, data_stream.dataset, and data_stream.namespace settings, but will fall back to them if any of the fields are missing from the event.
						"""
						required: false
						warnings: []
						type: bool: default: true
					}
					dataset: {
						common:      false
						description: "The data stream dataset used to construct the data stream at index time."
						required:    false
						warnings: []
						type: string: {
							default: "generic"
							examples: ["generic", "nginx", "{{ service }}"]
							syntax: "template"
						}
					}
					namespace: {
						common:      false
						description: "The data stream namespace used to construct the data stream at index time."
						required:    false
						warnings: []
						type: string: {
							default: "default"
							examples: ["default", "{{ environment }}"]
							syntax: "template"
						}
					}
					sync_fields: {
						common:      false
						description: "Automatically adds and syncs the data_stream.* event fields if they are missing from the event. This ensures that fields match the name of the data stream that is receiving events."
						required:    false
						warnings: []
						type: bool: default: true
					}
					type: {
						common:      false
						description: "The data stream type used to construct the data stream at index time."
						required:    false
						warnings: []
						type: string: {
							default: "logs"
							examples: ["logs", "metrics", "synthetics", "{{ type }}"]
							syntax: "template"
						}
					}
				}
			}
		}
		doc_type: {
			common:      false
			description: "The `doc_type` for your index data. This is only relevant for Elasticsearch <= 6.X. If you are using >= 7.0 you do not need to set this option since Elasticsearch has removed it."
			required:    false
			warnings: []
			type: string: {
				default: "_doc"
				syntax:  "literal"
			}
		}
		endpoint: {
			description: "The Elasticsearch endpoint to send logs to. This should be the full URL as shown in the example."
			required:    true
			warnings: []
			type: string: {
				examples: ["http://10.24.32.122:9000", "https://example.com", "https://user:password@example.com"]
				syntax: "literal"
			}
		}
		id_key: {
			common:      false
			description: "The name of the event key that should map to Elasticsearch's [`_id` field][urls.elasticsearch_id_field]. By default, Vector does not set the `_id` field, which allows Elasticsearch to set this automatically. You should think carefully about setting your own Elasticsearch IDs, since this can [hinder perofrmance][urls.elasticsearch_id_performance]."
			required:    false
			warnings: []
			type: string: {
				default: null
				examples: ["id", "_id"]
				syntax: "literal"
			}
		}
		index: {
			common:      true
			description: "Index name to write events to."
			required:    false
			warnings: []
			type: string: {
				default: "vector-%F"
				examples: ["application-{{ application_id }}-%Y-%m-%d", "vector-%Y-%m-%d"]
				syntax: "template"
			}
		}
		metrics: {
			common:      false
			description: "Options for metrics."
			required:    false
			warnings: []
			type: object: {
				examples: []
				options: {
					host_tag: {
						common:      false
						description: "Tag key that identifies the source host."
						required:    false
						warnings: []
						type: string: {
							default: "hostname"
							examples: ["host", "hostname"]
							syntax: "literal"
						}
					}
					timezone: configuration._timezone
				}
			}
		}
		mode: {
			common:      true
			description: "The type of index mechanism. If `data_stream` mode is enabled, the `bulk_action` is set to `create`."
			required:    false
			warnings: []
			type: string: {
				default: "normal"
				examples: ["normal", "data_stream"]
				syntax: "literal"
			}
		}
		pipeline: {
			common:      true
			description: "Name of the pipeline to apply."
			required:    false
			warnings: []
			type: string: {
				default: null
				examples: ["pipeline-name"]
				syntax: "literal"
			}
		}
		query: {
			common:      false
			description: "Custom parameters to Elasticsearch query string."
			required:    false
			warnings: []
			type: object: {
				examples: [{"X-Powered-By": "Vector"}]
				options: {}
			}
		}
	}

	input: {
		logs:    true
		metrics: null
	}

	how_it_works: {
		conflicts: {
			title: "Conflicts"
			body: """
				Vector [batches](#buffers--batches) data flushes it to Elasticsearch's
				[`_bulk` API endpoint][urls.elasticsearch_bulk]. By default, all events are
				inserted via the `index` action which will replace documents if an existing
				one has the same `id`. If `bulk_action` is configured with `create`, Elasticsearch
				will _not_ replace an existing document and instead return a conflict error.
				"""
		}

		data_streams: {
			title: "Data streams"
			body: """
				To use [Data streams][urls.elasticsearch_data_streams], set the `mode` to `data_stream`.
				The `index` is not used but use the combination of `data_stream.type`, `data_stream.dataset` and `data_stream.namespace`.
				"""
		}

		partial_failures: {
			title: "Partial Failures"
			body:  """
					By default, Elasticsearch will allow partial bulk ingestion
					failures. This is typically due to type Elasticsearch index
					mapping errors, where data keys are not consistently typed.
					To change this behavior please refer to the Elasticsearch
					[`ignore_malformed` setting](\(urls.elasticsearch_ignore_malformed)).
					"""
		}

		aws_authentication: components._aws.how_it_works.aws_authentication
	}

	telemetry: metrics: {
		events_discarded_total:  components.sources.internal_metrics.output.metrics.events_discarded_total
		processing_errors_total: components.sources.internal_metrics.output.metrics.processing_errors_total
	}
}
