package metadata

components: sources: redis: {
	title: "Redis"

	features: {
		collect: {
			checkpoint: enabled: false
			tls: {
				enabled:                true
				can_enable:             true
				can_verify_certificate: false
				can_verify_hostname:    false
				enabled_default:        false
			}
			from: {
				service: services.redis
				interface: {
					socket: {
						direction: "incoming"
						port:      6379
						protocols: ["tcp"]
						ssl: "disabled"
					}
				}
			}
		}
		multiline: enabled: false
	}

	classes: {
		commonly_used: false
		delivery:      "best_effort"
		deployment_roles: ["daemon", "sidecar"]
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

		requirements: []
		warnings: []
		notices: []
	}

	installation: {
		platform_name: null
	}

	configuration: {
		url: {
			description: "The Redis URL to connect to. The url _must_ take the form of `redis://server:port/db`."
			groups: ["tcp"]
			required: true
			warnings: []
			type: string: {
				examples: ["redis://127.0.0.1:6379/0"]
				syntax: "literal"
			}
		}
		key: {
			description: "The Redis key to read messages from."
			required:    true
			warnings: []
			type: string: {
				examples: ["vector"]
				syntax: "literal"
			}
		}
		data_type: {
			common:      false
			description: "The Redis Data Type(list or channel) to use."
			required:    false
			type: string: {
				default: "list"
				enum: {
					list:    "Use list"
					channel: "Use channel"
				}
				syntax: "literal"
			}
		}
		list: {
			common:      false
			description: "Options for the Redis `list` data type."
			required:    false
			warnings: []
			type: object: {
				examples: []
				options: {
					method: {
						common:      false
						description: "The Method(brpop or blpop) to pop messages when data_type is list."
						required:    false
						type: string: {
							default: "blpop"
							enum: {
								blpop: "Use the `blpop` method to pop messages."
								brpop: "Use the `brpop` method to pop messages."
							}
							syntax: "literal"
						}
					}
				}
			}
		}
		redis_key: {
			common:      false
			description: "The log field name to use for the redis key. If unspecified, the key would not be added to the log event."
			required:    false
			warnings: []
			type: string: {
				default: "redis_key"
				examples: ["redis_key"]
				syntax: "literal"
			}
		}
	}

	output: logs: record: {
		description: "An individual Redis record"
		fields: {
			host:      fields._local_host
			message:   fields._raw_line
			timestamp: fields._current_timestamp
		}
	}

	how_it_works: {
		redis_rs: {
			title: "redis-rs"
			body:  """
				The `redis` source uses [`redis-rs`](\(urls.redis_rs)) under the hood. This
				is a is a high level redis library for Rust. It provides convenient access to all Redis functionality through a very flexible but low-level API.
				It uses a customizable type conversion trait so that any operation can return results in just the type you are expecting.
				This makes for a very pleasant development experience.
				"""
		}
	}

	telemetry: metrics: {
		events_in_total:         components.sources.internal_metrics.output.metrics.events_in_total
		events_out_total:        components.sources.internal_metrics.output.metrics.events_out_total
		processed_bytes_total:   components.sources.internal_metrics.output.metrics.processed_bytes_total
		processed_bytes_total:   components.sources.internal_metrics.output.metrics.processed_bytes_total
		processing_errors_total: components.sources.internal_metrics.output.metrics.processing_errors_total
	}
}
