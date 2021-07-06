package metadata

components: sinks: humio_logs: {
	title: "Humio Logs"

	classes: sinks._humio.classes & {
		delivery: "best_effort"
	}
	features:      sinks._humio.features
	support:       sinks._humio.support
	configuration: sinks._humio.configuration

	input: {
		logs:    true
		metrics: null
	}
}
