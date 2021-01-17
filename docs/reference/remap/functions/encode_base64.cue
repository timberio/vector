package metadata

remap: functions: encode_base64: {
	arguments: [
		{
			name:        "value"
			description: "The string to encode."
			required:    true
			type: ["string"]
		},
		{
			name:        "padding"
			description: "Whether the Base64 output is [padded](\(urls.base64_padding))."
			required:    false
			type: ["boolean"]
			default: true
		},
		{
			name:        "charset"
			description: ""
			required:    false
			type: ["string"]
			default: "standard"
			enum: {
				standard: "[Standard](\(urls.base64_standard)) Base64 format."
				url_safe: "Modified Base64 for [URL variants](\(urls.base64_url_safe)."
			}
		},
	]
	internal_failure_reasons: []
	return: ["string"]
	category: "Codec"
	description: #"""
		Encodes the provided `value` to [Base64](\(urls.base64)) either padded or non-padded and
		using the specified character set.
		"""#
	examples: [
		{
			title: "Encode string"
			input: log: message: "please encode me"
			source: ".encoded = encode_base64(.message)"
			output: input & {log: {
				encoded: "cGxlYXNlIGVuY29kZSBtZQ=="
			}}
		},
		{
			title: "Encode string without padding"
			input: log: message: "please encode me, no padding though"
			source: ".encoded = encode_base64(.message, padding: false)"
			output: input & {log: {
				encoded: "cGxlYXNlIGVuY29kZSBtZSwgbm8gcGFkZGluZyB0aG91Z2g"
			}}
		},
		{
			title: "Encode URL string"
			input: log: message: "please encode me, but safe for URLs"
			source: #".encoded = encode_base64(.message, charset: "url_safe")"#
			output: input & {log: {
				encoded: "cGxlYXNlIGVuY29kZSBtZSwgYnV0IHNhZmUgZm9yIFVSTHM="
			}}
		},
	]
}
