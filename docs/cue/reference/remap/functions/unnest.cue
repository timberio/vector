package metadata

remap: functions: unnest: {
	category:    "Object"
	description: """
		Unnest an array field from an object to create an array of objects using that field; keeping all other fields.

		Assigning the array result of this to `.` will result in multiple events being emitted from `remap`. See the
		[`remap` transform docs](\(urls.vector_remap_transform_multiple)) for more details.

		This is also referred to as `explode`ing in some languages.
		"""

	arguments: [
		{
			name:        "path"
			description: "The path of the field to unnest."
			required:    true
			type: ["path"]
		},
	]
	internal_failure_reasons: [
		"Field path refers to is not an array",
	]
	notices: []
	return: {
		types: ["array"]
		rules: [
			"Returns an array of objects that matches the original object, but each with the specified path replaced with a single element from the original path.",
		]
	}

	examples: [
		{
			title: "Unnest an array field"
			input: log: {
				hostname: "localhost"
				messages: [
					"message 1",
					"message 2",
				]
			}
			source: ". = unnest!(.messages)"
			output: [
				{log: {
					hostname: "localhost"
					messages: "message 1"
				}},
				{log: {
					hostname: "localhost"
					messages: "message 2"
				}},
			]
		},
		{
			title: "Unnest nested an array field"
			input: log: {
				hostname: "localhost"
				event: {
					messages: [
						"message 1",
						"message 2",
					]
				}
			}
			source: ". = unnest!(.event.messages)"
			output: [
				{log: {
					hostname: "localhost"
					event: messages: "message 1"
				}},
				{log: {
					hostname: "localhost"
					event: messages: "message 2"
				}},
			]
		},
	]
}
