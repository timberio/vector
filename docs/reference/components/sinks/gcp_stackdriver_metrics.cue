package metadata

components: sinks: gcp_stackdriver_metrics: {
	title: "GCP Cloud Monitoring (formerly Stackdrive) Metrics"

	classes: {
		commonly_used: true
		delivery:      "best_effort"
		development:   "beta"
		egress_method: "batch"
		service_providers: ["GCP"]
		stateful: false
	}

	features: {
		buffer: enabled:      true
		healthcheck: enabled: false
		send: {
			batch: {
				enabled:      true
				common:       false
				max_events:   1
				timeout_secs: 1
			}
			compression: enabled: false
			encoding: {
				enabled: true
				codec: enabled: false
			}
			request: {
				enabled:                    true
				concurrency:                5
				rate_limit_duration_secs:   1
				rate_limit_num:             1000
				retry_initial_backoff_secs: 1
				retry_max_duration_secs:    10
				timeout_secs:               60
				headers:                    false
			}
			tls: {
				enabled:                true
				can_enable:             false
				can_verify_certificate: true
				can_verify_hostname:    true
				enabled_default:        true
			}
			to: {
				service: services.gcp_cloud_monitoring

				interface: {
					socket: {
						api: {
							title: "REST Interface"
							url:   urls.gcp_stackdriver_metrics_rest
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
		credentials_path: {
			common:      true
			description: "The filename for a Google Cloud service account credentials JSON file used to authenticate access to the Stackdriver Logging API. If this is unset, Vector checks the `GOOGLE_APPLICATION_CREDENTIALS` environment variable for a filename.\n\nIf no filename is named, Vector will attempt to fetch an instance service account for the compute instance the program is running on. If Vector is not running on a GCE instance, you must define a credentials file as above."
			required:    false
			warnings: []
			type: string: {
				default: null
				examples: ["/path/to/credentials.json"]
				syntax: "literal"
			}
		}
		project_id: {
			description: "The project ID to which to publish logs. See the [Google Cloud Platform project management documentation][urls.gcp_projects] for more details.\n\nExactly one of `billing_account_id`, `folder_id`, `organization_id`, or `project_id` must be set."
			required:    true
			warnings: []
			type: string: {
				examples: ["vector-123456"]
				syntax: "literal"
			}
		}
		default_namespace: {
			common:      false
			description: "The namespace used if the metric we are going to send to GCP has no namespace."
			required:    false
			warnings: []
			type: string: {
				examples: ["vector-123456"]
				default: "namespace"
				syntax:  "literal"
			}
		}
		resource: {
			description: "Options for describing the logging resource."
			required:    true
			warnings: []
			type: object: {
				examples: [
					{
						type:       "global"
						projectId:  "vector-123456"
						instanceId: "Twilight"
						zone:       "us-central1-a"
					},
				]
				options: {
					type: {
						description: "The monitored resource type. For example, the type of a Compute Engine VM instance is gce_instance.\n\nSee the [Google Cloud Platform monitored resource documentation][urls.gcp_resources] for more details."
						required:    true
						warnings: []
						type: string: {
							examples: ["global", "gce_instance"]
							syntax: "literal"
						}
					}
					"*": {
						common:      false
						description: "Values for all of the labels listed in the associated monitored resource descriptor.\n\nFor example, Compute Engine VM instances use the labels `projectId`, `instanceId`, and `zone`."
						required:    false
						warnings: []
						type: string: {
							default: null
							examples: ["vector-123456", "Twilight"]
							syntax: "literal"
						}
					}
				}
			}
		}
	}

	input: {
		logs: false
		metrics: {
			counter:      true
			distribution: false
			gauge:        true
			histogram:    false
			set:          false
			summary:      false
		}
	}

	how_it_works: {
	}

	permissions: iam: [
		{
			platform: "gcp"
			_service: "monitoring"

			policies: [
				{
					_action: "timeSeries.create"
					required_for: ["healthcheck", "write"]
				},
			]
		},
	]
}
