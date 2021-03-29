package metadata

remap: errors: "110": {
	title:       "Invalid argument type"
	description: """
		An argument passed to a [function call expression](\(urls.vrl_expressions)#\(remap.literals.regular_expression.anchor))
		isn't a supported type.
		"""
	rationale:   remap._fail_safe_blurb
	resolution: #"""
		You must guarantee the type of the variable by using the appropriate [type](\(urls.vrl_functions)#type) or
		[coercion](\(urls.vrl_functions)#coerce) function.
		"""#

	examples: [...{
		source: #"""
			downcase(.message)
			"""#
	}]

	examples: [
		{
			"title": "\(title) (guard with defaults)"
			diff: #"""
				+.message = string(.message) ?? ""
				 downcase(.message)
				"""#
		},
		{
			"title": "\(title) (guard with errors)"
			diff: #"""
				downcase(string!(.message))
				"""#
		},
	]
}
