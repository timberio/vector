package metadata

services: datadog_logs: {
	name:     "Datadog logs"
	thing:    "a \(name) index"
	url:      urls.datadog_logs
	versions: null
	drop_valid_api_key: False
	valid_api_keys: null

	description: services._datadog.description
}
