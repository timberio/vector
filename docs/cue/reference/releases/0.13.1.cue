package metadata

releases: "0.13.1": {
	date: "2021-04-29"

	description: """
		This release includes a high-priority bug fix for a regression in [0.13.0](/releases/0.13.0) that caused very
		high memory usage when using the [`kafka`](\(urls.vector_sources)/kafka) source and backpressure was
		experienced.
		"""

	whats_next: []

	commits: [
		{sha: "38f9b78aa693b941be33d33b7520fe3821d15df6", date: "2021-04-28 20:44:52 UTC", description: "Fix runaway memory usage of kafka source ", pr_number: 7266, scopes: ["kafka source"], type: "fix", breaking_change: false, author: "Jesse Szwedko", files_count: 2, insertions_count: 118, deletions_count: 126},
	]
}
