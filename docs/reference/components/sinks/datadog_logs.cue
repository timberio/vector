package metadata

components: sinks: datadog_logs: {
	title: "Datadog Logs"

	classes: sinks._datadog.classes & {
		delivery: "at_least_once"
	}

	features: {
		buffer: enabled:      true
		healthcheck: enabled: true
		send: {
			batch: {
				enabled:      true
				common:       false
				max_bytes:    1049000
				timeout_secs: 1
			}
			compression: {
				enabled: true
				default: "gzip"
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
			request: enabled: false
			tls: {
				enabled:                true
				can_enable:             true
				can_verify_certificate: true
				can_verify_hostname:    true
				enabled_default:        true
			}
			to: {
				service: services.datadog_logs

				interface: {
					socket: {
						api: {
							title: "Datadog logs API"
							url:   urls.datadog_logs_endpoints
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
		default_api_key: {
			description: "Default Datadog [API key](https://docs.datadoghq.com/api/?lang=bash#authentication), if an event has a key set in its metadata it will prevail over the one set here."
			required:    true
			warnings: []
			type: string: {
				examples: ["${DATADOG_API_KEY_ENV_VAR}", "ef8d5de700e7989468166c40fc8a0ccd"]
				syntax: "literal"
			}
		}
		endpoint: sinks._datadog.configuration.endpoint
		region:   sinks._datadog.configuration.region
		site:     sinks._datadog.configuration.site
	}

	input: {
		logs:    true
		metrics: null
	}
}
