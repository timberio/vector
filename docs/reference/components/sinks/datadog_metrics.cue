package metadata

components: sinks: datadog_metrics: {
	title: "Datadog Metrics"

	classes: sinks._datadog.classes & {
		delivery: "at_least_once"
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
			compression: enabled: false
			encoding: enabled:    false
			request: {
				enabled:                    true
				concurrency:                5
				rate_limit_duration_secs:   1
				rate_limit_num:             5
				retry_initial_backoff_secs: 1
				retry_max_duration_secs:    10
				timeout_secs:               60
				headers:                    false
			}
			tls: enabled: false
			to: {
				service: services.datadog_metrics

				interface: {
					socket: {
						api: {
							title: "Datadog metrics API"
							url:   urls.datadog_metrics_endpoints
						}
						direction: "outgoing"
						protocols: ["http"]
						ssl: "required"
					}
				}
			}
		}
	}

	support: sinks._datadog.support

	configuration: {
		api_key:  sinks._datadog.configuration.api_key
		endpoint: sinks._datadog.configuration.endpoint
		region:   sinks._datadog.configuration.region
		default_namespace: {
			common: true
			description: """
				Used as a namespace for metrics that don't have it.
				A namespace will be prefixed to a metric's name.
				"""
			required: false
			warnings: []
			type: string: {
				default: null
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
}
