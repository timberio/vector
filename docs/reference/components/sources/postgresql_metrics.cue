package metadata

components: sources: postgresql_metrics: {
	title:       "PostgreSQL Metrics"
	description: "[PostgreSQL][urls.postgresql] PostgreSQL is a powerful, open source object-relational database system with over 30 years of active development that has earned it a strong reputation for reliability, feature robustness, and performance."

	classes: {
		commonly_used: false
		delivery:      "best_effort"
		deployment_roles: ["daemon", "sidecar"]
		development:   "beta"
		egress_method: "batch"
		stateful:      false
	}

	features: {
		collect: {
			checkpoint: enabled: false
			from: {
				service: {
					name:     "PostgreSQL Server"
					thing:    "an \(name)"
					url:      urls.postgresql
					versions: "9.6-13"
				}

				interface: {
					socket: {
						direction: "outgoing"
						protocols: ["tcp", "unix"]
						ssl: "optional"
					}
				}
			}
		}
		multiline: enabled: false
	}

	support: {
		targets: {
			"aarch64-unknown-linux-gnu":      true
			"aarch64-unknown-linux-musl":     true
			"armv7-unknown-linux-gnueabihf":  true
			"armv7-unknown-linux-musleabihf": true
			"x86_64-apple-darwin":            true
			"x86_64-pc-windows-msv":          true
			"x86_64-unknown-linux-gnu":       true
			"x86_64-unknown-linux-musl":      true
		}
		requirements: []

		warnings: []
		notices: []
	}

	installation: {
		platform_name: null
	}

	configuration: {
		endpoints: {
			description: "PostgreSQL server endpoint in libpq-style connection strings."
			required:    true
			type: array: {
				items: type: string: {
					examples: ["postgresql://postgres:vector@localhost:5432/postgres"]
					syntax: "literal"
				}
			}
		}
		scrape_interval_secs: {
			description: "The interval between scrapes."
			common:      true
			required:    false
			type: uint: {
				default: 15
				unit:    "seconds"
			}
		}
		namespace: {
			description: "The namespace of metrics. Disabled if empty."
			common:      false
			required:    false
			type: string: {
				default: "postgresql"
				syntax:  "literal"
			}
		}
		include_databases: {
			description: """
				A list of databases to match (by using [POSIX Regular Expressions][urls.postgresql_matching]) against
				the `datname` column for which you want to collect metrics from.
				If not set, metrics will be collected from all databases.
				Specifying `""` will include metrics where `datname` is `NULL`.
				This can be used in conjunction with [`exclude_databases`](#exclude_databases).
				"""
			common:   false
			required: false
			type: array: {
				default: null
				items: type: string: {
					examples: ["^postgres$", "^vector$", "^foo"]
					syntax: "literal"
				}
			}
		}
		exclude_databases: {
			description: """
				A list of databases to match (by using [POSIX Regular Expressions][urls.postgresql_matching]) against
				the `datname` column for which you don't want to collect metrics from.
				Specifying `""` will include metrics where `datname` is `NULL`.
				This can be used in conjunction with [`include_databases`](#include_databases).
				"""
			common:   false
			required: false
			type: array: {
				default: null
				items: type: string: {
					examples: ["^postgres$", "^template.*", ""]
					syntax: "literal"
				}
			}
		}
		tls: {
			common:      false
			description: "TLS options to connect to the PostgreSQL Server."
			required:    false
			type: object: {
				examples: []
				options: {
					ca_file: {
						description: "Path to CA certificate file."
						required:    true
						warnings: []
						type: string: {
							examples: ["certs/ca.pem"]
							syntax: "literal"
						}
					}
				}
			}
		}
	}

	how_it_works: {
		privileges: {
			title: "Required Privileges"
			body: """
				PostgreSQL Metrics component collects metrics by making queries to the configured PostgreSQL server.
				Ensure the configured user is allowed to make the select queries against the following views:

				- `pg_stat_database`
				- `pg_stat_database_conflicts`
				- `pg_stat_bgwriter`
				"""
		}
	}

	telemetry: metrics: {
		events_in_total:          components.sources.internal_metrics.output.metrics.events_in_total
		collect_completed_total:  components.sources.internal_metrics.output.metrics.collect_completed_total
		collect_duration_seconds: components.sources.internal_metrics.output.metrics.collect_duration_seconds
		request_errors_total:     components.sources.internal_metrics.output.metrics.request_errors_total
	}

	output: metrics: {
		// Default PostgreSQL tags
		_postgresql_metrics_tags: {
			endpoint: {
				description: "PostgreSQL endpoint."
				required:    true
				examples: ["postgresql:///postgres?host=localhost&port=5432"]
			}
			host: {
				description: "The hostname of the PostgreSQL server."
				required:    true
				examples: [_values.local_host]
			}
		}
		_postgresql_metrics_tags_with_db: _postgresql_metrics_tags & {
			type: {
				description: "Database name."
				required:    true
				examples: ["postgres"]
			}
		}

		up: {
			description:       "Whether the PostgreSQL server is up or not."
			type:              "gauge"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags
		}
		pg_stat_database_datid: {
			description:       "OID of this database, or 0 for objects belonging to a shared relation."
			type:              "gauge"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_numbackends: {
			description:       "Number of backends currently connected to this database, or 0 for shared objects. This is the only column in this view that returns a value reflecting current state; all other columns return the accumulated values since the last reset."
			type:              "gauge"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_xact_commit_total: {
			description:       "Number of transactions in this database that have been committed."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_xact_rollback_total: {
			description:       "Number of transactions in this database that have been rolled back."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_blks_read_total: {
			description:       "Number of disk blocks read in this database."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_blks_hit_total: {
			description:       "Number of times disk blocks were found already in the buffer cache, so that a read was not necessary (this only includes hits in the PostgreSQL buffer cache, not the operating system's file system cache)."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_tup_returned_total: {
			description:       "Number of rows returned by queries in this database."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_tup_fetched_total: {
			description:       "Number of rows fetched by queries in this database."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_tup_inserted_total: {
			description:       "Number of rows inserted by queries in this database."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_tup_updated_total: {
			description:       "Number of rows updated by queries in this database."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_tup_deleted_total: {
			description:       "Number of rows deleted by queries in this database."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_conflicts_total: {
			description:       "Number of queries canceled due to conflicts with recovery in this database. (Conflicts occur only on standby servers; see `pg_stat_database_conflicts` for details.)"
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_temp_files_total: {
			description:       "Number of temporary files created by queries in this database. All temporary files are counted, regardless of why the temporary file was created (e.g., sorting or hashing), and regardless of the `log_temp_files` setting."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_temp_bytes_total: {
			description:       "Total amount of data written to temporary files by queries in this database. All temporary files are counted, regardless of why the temporary file was created, and regardless of the `log_temp_files` setting."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_deadlocks_total: {
			description:       "Number of deadlocks detected in this database."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_checksum_failures_total: {
			description:       "Number of data page checksum failures detected in this database (or on a shared object), or 0 if data checksums are not enabled."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_checksum_last_failure: {
			description:       "Time at which the last data page checksum failure was detected in this database (or on a shared object), or 0 if data checksums are not enabled."
			type:              "gauge"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_blk_read_time_seconds_total: {
			description:       "Time spent reading data file blocks by backends in this database, in milliseconds (if `track_io_timing` is enabled, otherwise zero)."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_blk_write_time_seconds_total: {
			description:       "Time spent writing data file blocks by backends in this database, in milliseconds (if `track_io_timing` is enabled, otherwise zero)."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_stats_reset: {
			description:       "Time at which these statistics were last reset."
			type:              "gauge"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_conflicts_confl_tablespace_total: {
			description:       "Number of queries in this database that have been canceled due to dropped tablespaces."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_conflicts_confl_lock_total: {
			description:       "Number of queries in this database that have been canceled due to lock timeouts."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_conflicts_confl_snapshot_total: {
			description:       "Number of queries in this database that have been canceled due to old snapshots."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_conflicts_confl_bufferpin_total: {
			description:       "Number of queries in this database that have been canceled due to pinned buffers."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_database_conflicts_confl_deadlock_total: {
			description:       "Number of queries in this database that have been canceled due to deadlocks."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags_with_db
		}
		pg_stat_bgwriter_checkpoints_timed_total: {
			description:       "Number of scheduled checkpoints that have been performed."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags
		}
		pg_stat_bgwriter_checkpoints_req_total: {
			description:       "Number of requested checkpoints that have been performed."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags
		}
		pg_stat_bgwriter_checkpoint_write_time_seconds_total: {
			description:       "Total amount of time that has been spent in the portion of checkpoint processing where files are written to disk."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags
		}
		pg_stat_bgwriter_checkpoint_sync_time_seconds_total: {
			description:       "Total amount of time that has been spent in the portion of checkpoint processing where files are synchronized to disk."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags
		}
		pg_stat_bgwriter_buffers_checkpoint_total: {
			description:       "Number of buffers written during checkpoints."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags
		}
		pg_stat_bgwriter_buffers_clean_total: {
			description:       "Number of buffers written by the background writer."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags
		}
		pg_stat_bgwriter_maxwritten_clean_total: {
			description:       "Number of times the background writer stopped a cleaning scan because it had written too many buffers."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags
		}
		pg_stat_bgwriter_buffers_backend_total: {
			description:       "Number of buffers written directly by a backend."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags
		}
		pg_stat_bgwriter_buffers_backend_fsync_total: {
			description:       "Number of times a backend had to execute its own fsync call (normally the background writer handles those even when the backend does its own write)."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags
		}
		pg_stat_bgwriter_buffers_alloc_total: {
			description:       "Number of buffers allocated."
			type:              "counter"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags
		}
		pg_stat_bgwriter_stats_reset: {
			description:       "Time at which these statistics were last reset."
			type:              "gauge"
			default_namespace: "postgresql"
			tags:              _postgresql_metrics_tags
		}
	}
}
