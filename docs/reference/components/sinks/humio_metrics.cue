package metadata

components: sinks: humio_metrics: {
	title: "Humio Metrics"

	classes: sinks._humio.classes & {
		delivery: "best_effort"
	}
	features:      sinks._humio.features
	support:       sinks._humio.support
	configuration: sinks._humio.configuration

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

	how_it_works: {
		metrics: {
			title: "Metrics"
			body: """
				Metrics are converted to log events via the `log_to_event` transform prior to sending to humio.
				"""
		}
	}
}
