package metadata

components: sources: aws_ecs_metrics: {
	title: "AWS ECS Metrics"

	description: """
		Collects the docker container stats for tasks running in AWS ECS or AWS
		Fargate.
		"""

	classes: {
		commonly_used: false
		delivery:      "best_effort"
		deployment_roles: ["sidecar"]
		development:   "beta"
		egress_method: "batch"
		stateful:      false
	}

	features: {
		collect: {
			checkpoint: enabled: false
			from: {
				service: services.aws_ecs

				interface: {
					socket: {
						api: {
							title: "Amazon ECS task metadata endpoint"
							url:   urls.aws_ecs_task_metadata
						}
						direction: "outgoing"
						protocols: ["http"]
						ssl: "disabled"
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
		endpoint: {
			description: """
				Base URI of the task metadata endpoint.
				If empty, the URI will be automatically discovered based on the latest version detected.
				The version 2 endpoint base URI is `169.254.170.2/v2/`.
				The version 3 endpoint base URI is stored in the environment variable `ECS_CONTAINER_METADATA_URI`.
				The version 4 endpoint base URI is stored in the environment variable `ECS_CONTAINER_METADATA_URI_V4`.
				"""
			common:   false
			required: false
			type: string: {
				default: "${ECS_CONTAINER_METADATA_URI_V4}"
				syntax:  "literal"
			}
		}
		namespace: {
			description: "The namespace of the metric. Disabled if empty."
			common:      true
			required:    false
			type: string: {
				default: "awsecs"
				syntax:  "literal"
			}
		}
		scrape_interval_secs: {
			description: "The interval between scrapes, in seconds."
			common:      true
			required:    false
			type: uint: {
				default: 15
				unit:    "seconds"
			}
		}
		version: {
			description: """
					The version of the metadata endpoint.
					If empty, the version will be automatically discovered based on envirionment variables.
				"""
			common:   false
			required: false
			type: string: {
				default: "v4"
				enum: {
					v4: "When the environment variable `ECS_CONTAINER_METADATA_URI_V4` is defined."
					v3: "When fails the v4 check, but the environment variable `ECS_CONTAINER_METADATA_URI` is defined."
					v2: "When fails the v4 and v3 checks."
				}
				syntax: "literal"
			}
		}
	}

	output: metrics: {
		_awsecs: {
			default_namespace: "awsecs"
		}

		_tags: {
			container_id: {
				description: "The identifier of the ECS container."
				required:    true
				examples: ["0cf54b87-f0f0-4044-b9d6-20dc54d5c414-4057181352"]
			}
			container_name: {
				description: "The name of the ECS container."
				required:    true
				examples: ["myapp"]
			}
		}

		_gauge: {
			type: "gauge"
			tags: _tags
		}

		_counter: {
			type: "counter"
			tags: _tags
		}

		_blkio_counter: {
			type: "counter"
			tags: _tags & {
				device: {
					description: "Device identified by its major and minor numbers."
					required:    true
					examples: ["202:26368"]
				}
				op: {
					description: "The operation type."
					required:    true
					examples: ["read", "write", "sync", "async", "total"]
				}
			}
		}

		blkio_recursive_io_merged_total:               _awsecs & _blkio_counter & {description: "Total number of bios/requests merged into requests."}
		blkio_recursive_io_queued_total:               _awsecs & _blkio_counter & {description: "Total number of requests queued up at any given instant."}
		blkio_recursive_io_service_bytes_total:        _awsecs & _blkio_counter & {description: "Number of bytes transferred to/from the disk."}
		blkio_recursive_io_service_time_seconds_total: _awsecs & _blkio_counter & {description: "Total amount of time in seconds between request dispatch and request completion for the IOs done."}
		blkio_recursive_io_serviced_total:             _awsecs & _blkio_counter & {description: "Number of IOs completed to/from the disk."}
		blkio_recursive_io_time_seconds_total:         _awsecs & _blkio_counter & {description: "Disk time allocated per device in seconds."}
		blkio_recursive_io_wait_time_seconds_total:    _awsecs & _blkio_counter & {description: "Total amount of time in seconds the IOs spent waiting in the scheduler queues for service."}
		blkio_recursive_sectors_total:                 _awsecs & _blkio_counter & {description: "Number of sectors transferred to/from disk."}

		cpu_online_cpus:                    _awsecs & _gauge & {description:   "Number of CPU cores."}
		cpu_usage_system_jiffies_total:     _awsecs & _counter & {description: "Jiffies of CPU time used by the system."}
		cpu_usage_usermode_jiffies_total:   _awsecs & _counter & {description: "Jiffies of CPU time spent in user mode by the container."}
		cpu_usage_kernelmode_jiffies_total: _awsecs & _counter & {description: "Jiffies of CPU time spent in kernel mode by the container."}
		cpu_usage_total_jiffies_total:      _awsecs & _counter & {description: "Jiffies of CPU time used by the container."}
		cpu_throttling_periods_total:       _awsecs & _counter & {description: "Number of periods."}
		cpu_throttled_periods_total:        _awsecs & _counter & {description: "Number of periods throttled."}
		cpu_throttled_time_seconds_total:   _awsecs & _counter & {description: "Throttling time in seconds."}

		cpu_usage_percpu_jiffies_total: _awsecs & {
			description: "Jiffies of CPU time used by the container, per CPU core."
			type:        "counter"
			tags:        _tags & {
				cpu: {
					description: "CPU core identifier."
					required:    true
					examples: ["0", "1"]
				}
			}
		}

		memory_used_bytes:                      _awsecs & _gauge & {description:           "Memory used by the container, in bytes."}
		memory_max_used_bytes:                  _awsecs & _gauge & {description:           "Maximum measured memory usage of the container, in bytes."}
		memory_limit_bytes:                     _awsecs & _gauge & {description:           "Memory usage limit of the container, in bytes."}
		memory_active_anonymous_bytes:          _awsecs & _gauge & {description:           "Amount of memory that has been identified as active by the kernel. Anonymous memory is memory that is not linked to disk pages."}
		memory_active_file_bytes:               _awsecs & _gauge & {description:           "Amount of active file cache memory. Cache memory = active_file + inactive_file + tmpfs."}
		memory_cache_bytes:                     _awsecs & _awsecs & _gauge & {description: "The amount of memory used by the processes of this cgroup that can be associated with a block on a block device. Also accounts for memory used by tmpfs."}
		memory_dirty_bytes:                     _awsecs & _gauge & {description:           "The amount of memory waiting to get written to disk."}
		memory_inactive_anonymous_bytes:        _awsecs & _gauge & {description:           "Amount of memory that has been identified as inactive by the kernel."}
		memory_inactive_file_bytes:             _awsecs & _gauge & {description:           "Amount of inactive file cache memory."}
		memory_mapped_file_bytes:               _awsecs & _gauge & {description:           "Indicates the amount of memory mapped by the processes in the cgroup. It doesn’t give you information about how much memory is used; it rather tells you how it is used."}
		memory_page_faults_total:               _awsecs & _counter & {description:         "Number of times that a process of the cgroup triggered a page fault."}
		memory_major_faults_total:              _awsecs & _counter & {description:         "Number of times that a process of the cgroup triggered a major page fault."}
		memory_page_charged_total:              _awsecs & _counter & {description:         "Number of charging events to the memory cgroup. Charging events happen each time a page is accounted as either mapped anon page(RSS) or cache page to the cgroup."}
		memory_page_uncharged_total:            _awsecs & _counter & {description:         "Number of uncharging events to the memory cgroup. Uncharging events happen each time a page is unaccounted from the cgroup."}
		memory_rss_bytes:                       _awsecs & _gauge & {description:           "The amount of memory that doesn’t correspond to anything on disk: stacks, heaps, and anonymous memory maps."}
		memory_rss_hugepages_bytes:             _awsecs & _gauge & {description:           "Amount of memory due to anonymous transparent hugepages."}
		memory_unevictable_bytes:               _awsecs & _gauge & {description:           "The amount of memory that cannot be reclaimed."}
		memory_writeback_bytes:                 _awsecs & _gauge & {description:           "The amount of memory from file/anon cache that are queued for syncing to the disk."}
		memory_total_active_anonymous_bytes:    _awsecs & _gauge & {description:           "Total amount of memory that has been identified as active by the kernel."}
		memory_total_active_file_bytes:         _awsecs & _gauge & {description:           "Total amount of active file cache memory."}
		memory_total_cache_bytes:               _awsecs & _gauge & {description:           "Total amount of memory used by the processes of this cgroup that can be associated with a block on a block device."}
		memory_total_dirty_bytes:               _awsecs & _gauge & {description:           "Total amount of memory waiting to get written to disk."}
		memory_total_inactive_anonymous_bytes:  _awsecs & _gauge & {description:           "Total amount of memory that has been identified as inactive by the kernel."}
		memory_total_inactive_file_bytes:       _awsecs & _gauge & {description:           "Total amount of inactive file cache memory."}
		memory_total_mapped_file_bytes:         _awsecs & _gauge & {description:           "Total amount of memory mapped by the processes in the cgroup."}
		memory_total_page_faults_total:         _awsecs & _counter & {description:         "Total number of page faults."}
		memory_total_major_faults_total:        _awsecs & _counter & {description:         "Total number of major page faults."}
		memory_total_page_charged_total:        _awsecs & _counter & {description:         "Total number of charging events."}
		memory_total_page_uncharged_total:      _awsecs & _counter & {description:         "Total number of uncharging events."}
		memory_total_rss_bytes:                 _awsecs & _gauge & {description:           "Total amount of memory that doesn’t correspond to anything on disk: stacks, heaps, and anonymous memory maps."}
		memory_total_rss_hugepages_bytes:       _awsecs & _gauge & {description:           "Total amount of memory due to anonymous transparent hugepages."}
		memory_total_unevictable_bytes:         _awsecs & _gauge & {description:           "Total amount of memory that can not be reclaimed."}
		memory_total_writeback_bytes:           _awsecs & _gauge & {description:           "Total amount of memory from file/anon cache that are queued for syncing to the disk."}
		memory_hierarchical_memory_limit_bytes: _awsecs & _gauge & {description:           "The memory limit in place by the hierarchy cgroup."}
		memory_hierarchical_memsw_limit_bytes:  _awsecs & _gauge & {description:           "The memory + swap limit in place by the hierarchy cgroup."}

		_network_counter: _awsecs & {
			type: "counter"
			tags: _tags & {
				device: {
					description: "The network interface."
					required:    true
					examples: ["eth1"]
				}
			}
		}

		network_receive_bytes_total:         _awsecs & _network_counter & {description: "Bytes received by the container via the network interface."}
		network_receive_packets_total:       _awsecs & _network_counter & {description: "Number of packets received by the container via the network interface."}
		network_receive_packets_drop_total:  _awsecs & _network_counter & {description: "Number of inbound packets dropped by the container."}
		network_receive_errs_total:          _awsecs & _network_counter & {description: "Errors receiving packets."}
		network_transmit_bytes_total:        _awsecs & _network_counter & {description: "Bytes sent by the container via the network interface."}
		network_transmit_packets_total:      _awsecs & _network_counter & {description: "Number of packets sent by the container via the network interface."}
		network_transmit_packets_drop_total: _awsecs & _network_counter & {description: "Number of outbound packets dropped by the container."}
		network_transmit_errs_total:         _awsecs & _network_counter & {description: "Errors sending packets."}
	}

	telemetry: metrics: {
		events_in_total:           components.sources.internal_metrics.output.metrics.events_in_total
		http_error_response_total: components.sources.internal_metrics.output.metrics.http_error_response_total
		http_request_errors_total: components.sources.internal_metrics.output.metrics.http_request_errors_total
		parse_errors_total:        components.sources.internal_metrics.output.metrics.parse_errors_total
		processed_bytes_total:     components.sources.internal_metrics.output.metrics.processed_bytes_total
		processed_events_total:    components.sources.internal_metrics.output.metrics.processed_events_total
		requests_completed_total:  components.sources.internal_metrics.output.metrics.requests_completed_total
		request_duration_seconds:  components.sources.internal_metrics.output.metrics.request_duration_seconds
	}
}
