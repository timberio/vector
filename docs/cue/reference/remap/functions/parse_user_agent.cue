package metadata

remap: functions: parse_user_agent: {
	category:    "Parse"
	description: """
		Parses the `value` as a user agent string. Which has [a loosly defined format](\(urls.user_agent))
		so this parser only provides best effort guarantee.
		"""
	notices: [
		"All values are returned as strings or as null. We recommend manually coercing values to desired types as you see fit.",
		"Different modes return different schema.",
		"Field which were not parsed out are set as `null`."
	]

	arguments: [
		{
			name:        "value"
			description: "The string to parse."
			required:    true
			type: ["string"]
		},
		{
			name:        "mode"
			description: "Determines performance and reliability characteristics."
			required:    false
			enum: {
				fast: "Fastest mode but most unreliable. Uses parser from project [Woothee](\(urls.woothee))."
				reliable: """
					Provides greater reliability than `fast` and retains it's speed in common cases.
					Parses with [Woothee](\(urls.woothee)) parser and with parser from [uap project](\(urls.uap)) if
					there are some missing fields that the first parser wasn't able to parse out
					but the second one maybe can.
					"""
				enriched: """
					Parses with both parser from [Woothee](\(urls.woothee)) and parser from [uap project](\(urls.uap))
					and combines results. Result has the full schema.
					"""
			}
			default: "fast"
			type: ["string"]
		},
	]
	internal_failure_reasons: []
	return: types: ["object"]

	examples: [
		{
			title: "Fast mode"
			source: #"""
				parse_user_agent!(
					"Mozilla Firefox 1.0.1 Mozilla/5.0 (X11; U; Linux i686; de-DE; rv:1.7.6) Gecko/20050223 Firefox/1.0.1"
				)
				"""#
			return: {
				browser: {
					family:  "Firefox"
					version: "1.0.1"
				}
				device: {
					category: "pc"
				}
				os: {
					family:  "Linux"
					version: null
				}
			}
		},
		{
			title: "Reliable mode"
			source: #"""
				parse_user_agent!(
					"Mozilla/4.0 (compatible; MSIE 7.66; Windows NT 5.1; SV1; .NET CLR 1.1.4322)"
					mode: "reliable"
				)
				"""#
			return: {
				browser: {
					family:  "Internet Explorer"
					version: "7.66"
				}
				device: {
					category: "pc"
				}
				os: {
					family:  "Windows XP"
					version: "NT 5.1"
				}
			}
		},
		{
			title: "Enriched mode"
			source: #"""
				parse_user_agent!(
					"Opera/9.80 (J2ME/MIDP; Opera Mini/4.3.24214; iPhone; CPU iPhone OS 4_2_1 like Mac OS X; AppleWebKit/24.783; U; en) Presto/2.5.25 Version/10.54"
					mode: "enriched"
				)
				"""#
			return: {
				browser: {
					family:  "Opera Mini"
					major:   "4"
					minor:   "3"
					patch:   "24214"
					version: "10.54"
				}
				device: {
					brand:    "Apple"
					category: "smartphone"
					family:   "iPhone"
					model:    "iPhone"
				}
				os: {
					family:      "iOS"
					major:       "4"
					minor:       "2"
					patch:       "1"
					patch_minor: null
					version:     "4.2.1"
				}
			}
		},
	]
}
