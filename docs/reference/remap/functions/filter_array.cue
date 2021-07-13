package metadata

remap: functions: filter_array: {
	category: "Array"
	description: """
		Filters elements from the `value` array. It returns an array containing the elements matching the `pattern`.
		"""

	arguments: [
		{
			name:        "value"
			description: "The array."
			required:    true
			type: ["array"]
		},
		{
			name:        "pattern"
			description: "The regular expression pattern to match against."
			required:    true
			type: ["regex"]
		},

	]
	internal_failure_reasons: []
	return: types: ["array"]

	examples: [
		{
			title: "Match at least one element"
			source: #"""
					filter_array(["foobar", "bazqux"], r'foo')
				"""#
			return: ["foobar"]
		},
		{
			title: "Match all elements"
			source: #"""
					filter_array(["foo", "foobar", "barfoo"], r'foo')
				"""#
			return: ["foo", "foobar", "barfoo"]
		},
		{
			title: "No matches"
			source: #"""
					filter_array(["bazqux", "xyz"], r'foo')
				"""#
			return: []
		},
	]
}
