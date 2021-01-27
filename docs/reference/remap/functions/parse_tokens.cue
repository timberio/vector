package metadata

remap: functions: parse_tokens: {
	category: "Parse"
	description: #"""
		Parses the `value` in "token" format.

		A token is considered to be:

		* A word surrounded by whitespace.
		* Text delimited by double quotes: `".."`. Quotes can be included in the token if they are escaped by a backslash (`\`).
		* Text delimited by square brackets: `[..]`. Closing square brackets can be included in the token if they are escaped by a backslash (`\`).
		"""#

	arguments: [
		{
			name:        "value"
			description: "The string to tokenize."
			required:    true
			type: ["string"]
		},
	]
	internal_failure_reasons: [
		"`value` is not a properly formatted tokenized string",
	]
	return: types: ["array"]

	examples: [
		{
			title: "Parse tokens"
			source: #"""
				parse_tokens(
					"A sentence \"with \\"a\\" sentence inside\" and [some brackets]"
				)
				"""#
			return: ["A", "sentence", #"with \"a\" sentence inside"#, "and", "some brackets"]
		},
	]
}
