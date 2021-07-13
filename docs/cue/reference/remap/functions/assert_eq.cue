package metadata

remap: functions: assert_eq: {
	category: "Debug"

	description: """
		Asserts that two expressions, `left` and `right`, have the same value. If the values are
		equivalent, `true` is returned, otherwise `false` is returned. You can specify an optional
		`message` to append to the error output.
		"""

	arguments: [
		{
			name: "left"
			description: "The value to check for equality against `right`."
			required: true
			type: ["any"]
		},
		{
			name: "right"
			description: "The value to check for equality against `left`."
			required: true
			type: ["any"]
		},
		{
			name: "message"
			description: """
				An optional custom error message. If the equality assertion fails, `message` is
				appended to the default message prefix. See the examples below for a sample fully
				formed log message.
				"""
			required: false
			type: ["string"]
		}
	]

	internal_failure_reasons: []

	return: types: ["boolean"]

	examples: [
		{
			title: "Successful assertion"
			source: "assert_eq!(1, 1)"
			return: true
		},
		{
			title: "Unsuccessful assertion"
			source: "assert_eq!(127, [1, 2, 3])"
			return: false
		},
		{
			title: "Unsuccessful assertion with custom log message"
			source: #"""
				assert_eq!(1, 0, message: "Unequal integers")
				"""#
			return: #"function call error for "assert_eq" at (0:78): Unequal integers"#
		}
	]
}
