package metadata

remap: functions: assert: {
	category: "Debug"
	description: """
		Asserts the `condition`, which must be a Boolean expression. The program is aborted with the `message` if the
		condition evaluates to `false`.
		"""
	notices: [
		"""
		The `assert` function should be used in a standalone fashion and only when you want to abort the program. You
		should avoid it in logical expressions and other situations in which you want the program to continue if the
		condition evaluates to `false`.
		""",
	]

	arguments: [
		{
			name:        "condition"
			description: "The condition to check."
			required:    true
			type: ["boolean"]
		},
		{
			name:        "message"
			description: "The failure message that's reported if `condition` evaluates to `false`."
			required:    true
			type: ["string"]
		},
	]
	internal_failure_reasons: [
		"`condition` evaluates to `false`",
	]
	return: types: ["null"]

	examples: [
		{
			title: "Assertion (true)"
			source: #"""
				ok, err = assert("foo" == "foo", message: "\"foo\" must be \"foo\"!")
				"""#
			return: true
		},
		{
			title: "Assertion (false)"
			source: #"""
				ok, err = assert("foo" == "bar", message: "\"foo\" must be \"foo\"!")
				"""#
			return: #"function call error for "assert" at (10:69): "foo" must be "foo"!"#
		},
	]
}
