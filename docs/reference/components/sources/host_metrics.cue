package metadata

components: sources: host_metrics: {
	title: "Host Metrics"

	description: """
		Examines system data sources on the local system and generates metrics
		describing utilization of various system resources, such as CPU, memory,
		disk, and network utilization.
		"""

	classes: {
		commonly_used: false
		delivery:      "best_effort"
		deployment_roles: ["daemon"]
		development:   "beta"
		egress_method: "batch"
		stateful:      false
	}

	features: {
		collect: {
			checkpoint: enabled: false
			from: service:       services.host
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
		notices: []
		requirements: []
		warnings: [
			"""
				When vector is run under kubernetes, you may experience an error when loading the partition usage data:
				```Failed to load partition usage data. mount_point="/host/proc/sys/fs/binfmt_misc" error=FFI function "statvfs" call failed: Too many levels of symbolic links (os error 40)```
				To work around this configuration issue, add one of the following lines to the
				`host_metrics` configuration section:
				```filesystem.devices.excludes = ["binfmt_misc"]```,
				```filesystem.filesystems.excludes = ["binfmt_misc"]```, or
				```filesystem.mountpoints.excludes = ["*/proc/sys/fs/binfmt_misc"]```.
				This workaround is included by default in the Helm chart distributed with Vector.
				""",
		]
	}

	installation: {
		platform_name: null
	}

	env_vars: {
		PROCFS_ROOT: {
			description: "Sets an arbitrary path to the system's Procfs root. Can be used to expose host metrics from within a container. Unset and uses system `/proc` by default."
			type: string: {
				default: null
				examples: ["/mnt/host/proc"]
				syntax: "literal"
			}
		}

		SYSFS_ROOT: {
			description: "Sets an arbitrary path to the system's Sysfs root. Can be used to expose host metrics from within a container. Unset and uses system `/sys` by default."
			type: string: {
				default: null
				examples: ["/mnt/host/sys"]
				syntax: "literal"
			}
		}
	}

	configuration: {
		collectors: {
			description: "The list of host metric collector services to use. Defaults to all collectors."
			common:      true
			required:    false
			type: array: {
				default: ["cpu", "disk", "filesystem", "load", "host", "memory", "network"]
				items: type: string: {
					enum: {
						cpu:        "Metrics related to CPU utilization."
						disk:       "Metrics related to disk I/O utilization."
						filesystem: "Metrics related to filesystem space utilization."
						load:       "Load average metrics (UNIX only)."
						host:       "Metrics related to host"
						memory:     "Metrics related to memory utilization."
						network:    "Metrics related to network utilization."
					}
					syntax: "literal"
				}
			}
		}
		namespace: {
			description: "The namespace of metrics. Disabled if empty."
			common:      false
			required:    false
			type: string: {
				default: "host"
				syntax:  "literal"
			}
		}
		scrape_interval_secs: {
			description: "The interval between metric gathering, in seconds."
			common:      true
			required:    false
			type: uint: {
				default: 15
				unit:    "seconds"
			}
		}
		disk: {
			common:      false
			description: #"Options for the "disk" metrics collector."#
			required:    false
			type: object: options: {
				devices: {
					common:      false
					required:    false
					description: "Lists of device name patterns to include or exclude."
					type: object: options: {
						includes: {
							required: false
							common:   false
							description: """
								The list of device name patterns for which to gather I/O utilization metrics.
								Defaults to including all devices.
								The patterns are matched using [globbing](#globbing).
								"""
							type: array: {
								default: ["*"]
								items: type: string: {
									examples: ["sda", "dm-*"]
									syntax: "literal"
								}
							}
						}
						excludes: {
							required: false
							common:   false
							description: """
								The list of device name patterns for which to gather I/O utilization metrics.
								Defaults to excluding no devices.
								The patterns are matched using [globbing](#globbing).
								"""
							type: array: {
								default: []
								items: type: string: {
									examples: ["sda", "dm-*"]
									syntax: "literal"
								}
							}
						}
					}
				}
			}
		}
		filesystem: {
			common:      false
			description: #"Options for the "filesystem" metrics collector."#
			required:    false
			type: object: options: {
				devices: {
					common:      false
					required:    false
					description: "Lists of device name patterns to include or exclude."
					type: object: options: {
						includes: {
							required: false
							common:   false
							description: """
								The list of device name patterns for which to gather usage metrics.
								Defaults to including all devices.
								The patterns are matched using [globbing](#globbing).
								"""
							type: array: {
								default: ["*"]
								items: type: string: {
									examples: ["sda", "dm-*"]
									syntax: "literal"
								}
							}
						}
						excludes: {
							required: false
							common:   false
							description: """
								The list of device name patterns for which to gather usage metrics.
								Defaults to excluding no devices.
								The patterns are matched using [globbing](#globbing).
								"""
							type: array: {
								default: []
								items: type: string: {
									examples: ["sda", "dm-*"]
									syntax: "literal"
								}
							}
						}
					}
				}
				filesystems: {
					common:      false
					required:    false
					description: "Lists of filesystem name patterns to include or exclude."
					type: object: options: {
						includes: {
							required: false
							common:   false
							description: """
								The list of filesystem name patterns for which to gather usage metrics.
								Defaults to including all filesystems.
								The patterns are matched using [globbing](#globbing).
								"""
							type: array: {
								default: ["*"]
								items: type: string: {
									examples: ["ntfs", "ext*"]
									syntax: "literal"
								}
							}
						}
						excludes: {
							required: false
							common:   false
							description: """
								The list of filesystem name patterns for which to gather usage metrics.
								Defaults to excluding no filesystems.
								The patterns are matched using [globbing](#globbing).
								"""
							type: array: {
								default: []
								items: type: string: {
									examples: ["ntfs", "ext*"]
									syntax: "literal"
								}
							}
						}
					}
				}
				mountpoints: {
					common:      false
					required:    false
					description: "Lists of mount point path patterns to include or exclude."
					type: object: options: {
						includes: {
							required: false
							common:   false
							description: """
								The list of mount point path patterns for which to gather usage metrics.
								Defaults to including all mount points.
								The patterns are matched using [globbing](#globbing).
								"""
							type: array: {
								default: ["*"]
								items: type: string: {
									examples: ["/home", "/raid*"]
									syntax: "literal"
								}
							}
						}
						excludes: {
							required: false
							common:   false
							description: """
								The list of mount point path patterns for which to gather usage metrics.
								Defaults to excluding no mount points.
								The patterns are matched using [globbing](#globbing).
								"""
							type: array: {
								default: []
								items: type: string: {
									examples: ["/home", "/raid*"]
									syntax: "literal"
								}
							}
						}
					}
				}
			}
		}
		network: {
			common:      false
			description: #"Options for the "network" metrics collector."#
			required:    false
			type: object: options: {
				devices: {
					common:      false
					required:    false
					description: "Lists of device name patterns to include or exclude."
					type: object: options: {
						includes: {
							required: false
							common:   false
							description: """
								The list of device name patterns for which to gather network utilization metrics.
								Defaults to including all devices.
								The patterns are matched using [globbing](#globbing).
								"""
							type: array: {
								default: ["*"]
								items: type: string: {
									examples: ["sda", "dm-*"]
									syntax: "literal"
								}
							}
						}
						excludes: {
							required: false
							common:   false
							description: """
								The list of device name patterns for which to gather network utilization metrics.
								Defaults to excluding no devices.
								The patterns are matched using [globbing](#globbing).
								"""
							type: array: {
								default: []
								items: type: string: {
									examples: ["sda", "dm-*"]
									syntax: "literal"
								}
							}
						}
					}
				}
			}
		}
	}

	output: metrics: {
		_host_metrics_tags: {
			collector: {
				description: "Which collector this metric comes from."
				required:    true
			}
			host: {
				description: "The hostname of the originating system."
				required:    true
				examples: [_values.local_host]
			}
		}

		// Host CPU
		host_cpu_seconds_total: _host & {
			description: "The number of CPU seconds accumulated in different operating modes."
			type:        "counter"
			tags:        _host_metrics_tags & {
				collector: examples: ["cpu"]
				cpu: {
					description: "The index of the CPU core or socket."
					required:    true
					examples: ["1"]
				}
				mode: {
					description: "Which mode the CPU was running in during the given time."
					required:    true
					examples: ["idle", "system", "user", "nice"]
				}
			}
		}

		// Host disk
		disk_read_bytes_total:       _host & _disk_counter & {description: "The accumulated number of bytes read in."}
		disk_reads_completed_total:  _host & _disk_counter & {description: "The accumulated number of read operations completed."}
		disk_written_bytes_total:    _host & _disk_counter & {description: "The accumulated number of bytes written out."}
		disk_writes_completed_total: _host & _disk_counter & {description: "The accumulated number of write operations completed."}

		// Host filesystem
		filesystem_free_bytes:  _host & _filesystem_bytes & {description: "The number of bytes free on the named filesystem."}
		filesystem_total_bytes: _host & _filesystem_bytes & {description: "The total number of bytes in the named filesystem."}
		filesystem_used_bytes:  _host & _filesystem_bytes & {description: "The number of bytes used on the named filesystem."}

		// Host load
		load1:  _host & _loadavg & {description: "System load averaged over the last 1 second."}
		load5:  _host & _loadavg & {description: "System load averaged over the last 5 seconds."}
		load15: _host & _loadavg & {description: "System load averaged over the last 15 seconds."}

		// Host time
		uptime:    _host & _host_metric & {description: "The number of seconds since the last boot."}
		boot_time: _host & _host_metric & {description: "The UNIX timestamp of the last boot."}

		// Host memory
		memory_active_bytes:           _host & _memory_gauge & _memory_nowin & {description: "The number of bytes of active main memory."}
		memory_available_bytes:        _host & _memory_gauge & {description:                 "The number of bytes of main memory available."}
		memory_buffers_bytes:          _host & _memory_linux & {description:                 "The number of bytes of main memory used by buffers."}
		memory_cached_bytes:           _host & _memory_linux & {description:                 "The number of bytes of main memory used by cached blocks."}
		memory_free_bytes:             _host & _memory_gauge & {description:                 "The number of bytes of main memory not used."}
		memory_inactive_bytes:         _host & _memory_macos & {description:                 "The number of bytes of main memory that is not active."}
		memory_shared_bytes:           _host & _memory_linux & {description:                 "The number of bytes of main memory shared between processes."}
		memory_swap_free_bytes:        _host & _memory_gauge & {description:                 "The number of free bytes of swap space."}
		memory_swapped_in_bytes_total: _host & _memory_counter & _memory_nowin & {
			description: "The number of bytes that have been swapped in to main memory."
		}
		memory_swapped_out_bytes_total: _host & _memory_counter & _memory_nowin & {
			description: "The number of bytes that have been swapped out from main memory."
		}
		memory_swap_total_bytes: _host & _memory_gauge & {description: "The total number of bytes of swap space."}
		memory_swap_used_bytes:  _host & _memory_gauge & {description: "The number of used bytes of swap space."}
		memory_total_bytes:      _host & _memory_gauge & {description: "The total number of bytes of main memory."}
		memory_used_bytes:       _host & _memory_linux & {description: "The number of bytes of main memory used by programs or caches."}
		memory_wired_bytes:      _host & _memory_macos & {description: "The number of wired bytes of main memory."}

		// Host network
		network_receive_bytes_total:         _host & _network_gauge & {description: "The number of bytes received on this interface."}
		network_receive_errs_total:          _host & _network_gauge & {description: "The number of errors encountered during receives on this interface."}
		network_receive_packets_total:       _host & _network_gauge & {description: "The number of packets received on this interface."}
		network_transmit_bytes_total:        _host & _network_gauge & {description: "The number of bytes transmitted on this interface."}
		network_transmit_errs_total:         _host & _network_gauge & {description: "The number of errors encountered during transmits on this interface."}
		network_transmit_packets_drop_total: _host & _network_nomac & {description: "The number of packets dropped during transmits on this interface."}
		network_transmit_packets_total:      _host & _network_nomac & {description: "The number of packets transmitted on this interface."}

		// Helpers
		_host: {
			default_namespace: "host"
		}

		_disk_device: {
			description: "The disk device name."
			required:    true
			examples: ["sda", "sda1", "dm-1"]
		}
		_disk_counter: {
			type: "counter"
			tags: _host_metrics_tags & {
				collector: examples: ["disk"]
				device: _disk_device
			}
		}
		_filesystem_bytes: {
			type: "gauge"
			tags: _host_metrics_tags & {
				collector: examples: ["filesystem"]
				device: _disk_device
				filesystem: {
					description: "The name of the filesystem type."
					required:    true
					examples: ["ext4", "ntfs"]
				}
			}
		}
		_loadavg: {
			type: "gauge"
			tags: _host_metrics_tags & {
				collector: examples: ["loadavg"]
			}
			relevant_when: "OS is not Windows"
		}
		_host_metric: {
			type: "gauge"
			tags: _host_metrics_tags & {
				collector: examples: ["host"]
			}
		}
		_memory_counter: {
			type: "counter"
			tags: _host_metrics_tags & {
				collector: examples: ["memory"]
			}
		}
		_memory_gauge: {
			type: "gauge"
			tags: _host_metrics_tags & {
				collector: examples: ["memory"]
			}
		}
		_memory_linux: _memory_gauge & {relevant_when: "OS is Linux"}
		_memory_macos: _memory_gauge & {relevant_when: "OS is macOS X"}
		_memory_nowin: {relevant_when: "OS is not Windows"}
		_network_gauge: {
			type: "gauge"
			tags: _host_metrics_tags & {
				collector: examples: ["network"]
				device: {
					description: "The network interface device name."
					required:    true
					examples: ["eth0", "enp5s3"]
				}
			}
		}
		_network_nomac: _network_gauge & {relevant_when: "OS is not macOS"}
	}

	telemetry: metrics: {
		processed_events_total: components.sources.internal_metrics.output.metrics.processed_events_total
	}
}
