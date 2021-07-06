package metadata

components: sinks: influxdb_metrics: {
	title: "InfluxDB Metrics"

	classes: {
		commonly_used: false
		delivery:      "best_effort"
		development:   "stable"
		egress_method: "batch"
		service_providers: ["InfluxData"]
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
			compression: enabled: false
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
				headers:                    false
			}
			tls: sinks._influxdb.features.send.tls
			to:  sinks._influxdb.features.send.to
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

	configuration: sinks._influxdb.configuration & {
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
			histogram:    true
			set:          true
			summary:      true
		}
	}

	examples: [
		{
			_host:  _values.local_host
			_name:  "logins"
			_value: 1.5
			title:  "Counter"
			configuration: {
				default_namespace: "service"
			}
			input: metric: {
				kind: "incremental"
				name: _name
				counter: {
					value: _value
				}
				tags: {
					host: _host
				}
			}
			output: "service.\(_name),metric_type=counter,host=\(_host) value=\(_value) 1542182950000000011"
		},
		{
			_host: _values.local_host
			_name: "sparse_stats"
			title: "Distribution"
			notes: "For distributions with histogram, summary is computed."
			configuration: {}
			input: metric: {
				kind:      "incremental"
				name:      _name
				namespace: "app"
				distribution: {
					samples: [
						{value: 1.0, rate: 1},
						{value: 5.0, rate: 2},
						{value: 3.0, rate: 3},
					]
					statistic: "histogram"
				}
				tags: {
					host: _host
				}
			}
			output: "app.\(_name),metric_type=distribution,host=\(_host) avg=3.333333,count=6,max=5,median=3,min=1,quantile_0.95=5,sum=20 1542182950000000011"
		},
		{
			_host:  _values.local_host
			_name:  "memory_rss"
			_value: 1.5
			title:  "Gauge"
			configuration: {
				default_namespace: "service"
			}
			input: metric: {
				kind:      "absolute"
				name:      _name
				namespace: "app"
				gauge: {
					value: _value
				}
				tags: {
					host: _host
				}
			}
			output: "app.\(_name),metric_type=gauge,host=\(_host) value=\(_value) 1542182950000000011"
		},
		{
			_host: _values.local_host
			_name: "requests"
			title: "Histogram"
			configuration: {}
			input: metric: {
				kind: "absolute"
				name: _name
				histogram: {
					buckets: [
						{upper_limit: 1.0, count: 2},
						{upper_limit: 2.1, count: 5},
						{upper_limit: 3.0, count: 10},
					]
					count: 17
					sum:   46.2
				}
				tags: {
					host: _host
				}
			}
			output: "\(_name),metric_type=histogram,host=\(_host) bucket_1=2i,bucket_2.1=5i,bucket_3=10i,count=17i,sum=46.2 1542182950000000011"
		},
		{
			_host:  _values.local_host
			_name:  "users"
			_value: 1.5
			title:  "Set"
			configuration: {}
			input: metric: {
				kind: "incremental"
				name: _name
				set: {
					values: ["first", "another", "last"]
				}
				tags: {
					host: _host
				}
			}
			output: "\(_name),metric_type=set,host=\(_host) value=3 154218295000000001"
		},
		{
			_host: _values.local_host
			_name: "requests"
			title: "Summary"
			configuration: {}
			input: metric: {
				kind: "absolute"
				name: _name
				summary: {
					quantiles: [
						{upper_limit: 0.01, value: 1.5},
						{upper_limit: 0.5, value:  2.0},
						{upper_limit: 0.99, value: 3.0},
					]
					count: 6
					sum:   12.1
				}
				tags: {
					host: _host
				}
			}
			output: "\(_name),metric_type=summary,host=\(_host) count=6i,quantile_0.01=1.5,quantile_0.5=2,quantile_0.99=3,sum=12.1 1542182950000000011"
		},
	]
}
