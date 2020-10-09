package metadata

components: sinks: sematext_metrics: {
	title:             "Sematext Metrics"
	short_description: ""
	long_description:  "[Sematext][urls.sematext] is a hosted monitoring platform for metrics based on InfluxDB. Providing powerful monitoring and management solutions to monitor and observe your apps in real-time."

	classes: {
		commonly_used: false
		function:      "transmit"
		service_proveders: ["Sematext"]
	}

	features: {
		batch: {
			enabled:      true
			common:       false
			max_bytes:    30000000
			max_events:   null
			timeout_secs: 1
		}
		buffer: enabled:      true
		compression: enabled: false
		healthcheck: enabled: true
		request: enabled:     false
		encoding: enabled:    false
		tls: enabled:         false
	}

	statuses: {
		delivery:    "at_least_once"
		development: "beta"
	}

	support: {
		input_types: ["metrics"]

		platforms: {
			"aarch64-unknown-linux-gnu":  true
			"aarch64-unknown-linux-musl": true
			"x86_64-apple-darwin":        true
			"x86_64-pc-windows-msv":      true
			"x86_64-unknown-linux-gnu":   true
			"x86_64-unknown-linux-musl":  true
		}

		requirements: []
		warnings: []
		notices: []
	}

	configuration: {
		region: {
			common:        true
			description:   "The region destination to send metrics to. This option is required if `endpoint` is not set."
			required:      false
			relevant_when: "`endpoint` is not set"
			warnings: []
			type: string: {
				enum: ["us", "eu"]
			}
		}
		endpoint: {
			common:        false
			description:   "The endpoint that will be used to send metrics to. This option is required if `region` is not set."
			required:      false
			relevant_when: "`region` is not set"
			warnings: []
			type: string: {
				examples: ["https://spm-receiver.sematext.com", "https://spm-receiver.eu.sematext.com"]
			}
		}
		token: {
			required:    true
			description: "The api token for the app in Sematext to send the metrics to."
			warnings: []
			type: string: {
				examples: ["${SEMATEXT_TOKEN}", "some-sematext-token"]
			}
		}
	}
	how_it_works: {
		metric_types: {
			title: "Metric Types"
			body: #"""
                               [Sematext monitoring](https://sematext.com/docs/monitoring/) accepts metrics which contain a single value. 
                               These are the Counter and Gauge Vector metric types.

                               <Alert type="info">
                               Other metric types are not supported. The following metric types will not be sent to Sematext: 

                               `aggregated_histogram`, `aggregated_summary`, `distribution`, `set`
                               </Alert>

                               All metrics are sent with a namespace. If no namespace is included with the metric, the metric name becomes
                               the namespace and the metric is named `value`.
                               """#
		}
	}
}
