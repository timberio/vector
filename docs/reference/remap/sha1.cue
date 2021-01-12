package metadata

remap: functions: sha1: {
	fallible: true
	arguments: [
		{
			name:        "value"
			description: "The string to calculate the hash for."
			required:    true
			type: ["string"]
		},
	]
	return: ["string"]
	category: "Hash"
	description: #"""
		Calculates a sha1 hash of a given string.
		"""#
	examples: [
		{
			title: "Success"
			input: log: text: #"foo"#
			source: #"""
				.hash = sha1(.text)
				"""#
			output: input & {log: hash: "0beec7b5ea3f0fdbc95d0dd47f3c5bc275da8a33"}
		},
	]
}
