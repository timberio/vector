package metadata

components: sinks: syslog: {
	title: "Syslog"

	description: """
		Sends data to a Syslog collector. Both [RFC3164](https://tools.ietf.org/html/rfc3164)
		and [RFC5424](https://tools.ietf.org/html/rfc5424) are supported. If TCP or a stream Unix
		domain socket is used then the message will be encoded according to the
		[RFC6587](https://tools.ietf.org/html/rfc6587).
		"""

	classes: {
		commonly_used: true
		delivery:      "best_effort"
		development:   "beta"
		egress_method: "stream"
		service_providers: []
		stateful: false
	}

	features: {
		buffer: enabled:      true
		healthcheck: enabled: true
		send: {
			compression: enabled: false
			encoding: enabled:    false
			send_buffer_bytes: {
				enabled:       true
				relevant_when: "mode = `tcp` or mode = `udp`"
			}
			keepalive: enabled: true
			request: enabled:   false
			tls: {
				enabled:                true
				can_enable:             true
				can_verify_certificate: true
				can_verify_hostname:    true
				enabled_default:        false
			}
			to: {
				service: services.syslog

				interface: {
					socket: {
						api: {
							title: "Syslog"
							url:   urls.syslog
						}
						direction: "outgoing"
						protocols: ["tcp", "udp", "unix"]
						ssl: "required"
					}
				}
			}
		}
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

	configuration: sinks.socket.configuration & {
		"type": "type": string: enum: syslog: "The type of this component."
		format: {
			common:      true
			description: "The Syslog format used to send message, RFC3164 and RFC5424 are supported."
			required:    false
			warnings: []
			type: string: {
				default: "rfc5424"
				enum: {
					rfc3164: "Format message according to [RFC3164](https://tools.ietf.org/html/rfc3164)."
					rfc5424: "Format message according to [RFC5424](https://tools.ietf.org/html/rfc5424)."
				}
				syntax: "literal"
			}
		}
		include_extra_fields: {
			common:      false
			description: "If this is set to `true` all fields that have not been mapped to a Syslog field will be included as structured data in the resulting message."
			required:    false
			warnings: []
			type: bool: default: false
		}
		appname_key: {
			common:      false
			description: "The key for the field that will be used as the `APP-NAME`."
			required:    false
			warnings: []
			type: string: {
				default: "appname"
				examples: ["application_name"]
				syntax: "literal"
			}
		}
		facility_key: {
			common: false
			description: """
				The key for the field that will be used as the Syslog facility. The following value will be recognized as
				valid facility: integer from 0 (included) to 23 (included) and the matching string representation: `kern`, `user`,
				`mail`, `daemon`, `auth`, `syslog`, `lpr`, `news`, `uucp`, `cron`, `authpriv`, `ftp`, `ntp`, `audit`, `alert`, `clockd`,
				`local0`, `local1`, `local2`, `local3`, `local4`, `local5`, `local6` and `local7`. Please check the
				[RFC5424](https://tools.ietf.org/html/rfc5424) for additional information.
				"""
			required: false
			warnings: []
			type: string: {
				default: "facility"
				examples: ["facility"]
				syntax: "literal"
			}
		}
		host_key: {
			common:      false
			description: "The key for the field that will be used to as the host in Syslog messages. This overrides the [global `host_key` option][docs.reference.configuration.global-options#host_key]."
			required:    false
			warnings: []
			type: string: {
				default: "hostname"
				examples: ["host"]
				syntax: "literal"
			}
		}
		msgid_key: {
			common:      false
			description: "The key for the field that will be used as the Syslog `MSGID`."
			required:    false
			warnings: []
			type: string: {
				default: "msgid"
				examples: ["msg_is"]
				syntax: "literal"
			}
		}
		procid_key: {
			common:      false
			description: "The key for the field that will be used as the Syslog `PROCID`."
			required:    false
			warnings: []
			type: string: {
				default: "procid"
				examples: ["proc_id"]
				syntax: "literal"
			}
		}
		severity_key: {
			common: false
			description: """
				The key for the field that will be used as the Syslog severity. The following value will be recognized as
				valid severity: integer from 0 (included) to 7 (included) and the matching string representation: `emerg`,
				`alert`, `crit`, `err`, `warning`, `notice`, `info` and `debug`. Please check the
				[RFC5424](https://tools.ietf.org/html/rfc5424) for additional information.
				"""
			required: false
			warnings: []
			type: string: {
				default: "severity"
				examples: ["sev"]
				syntax: "literal"
			}
		}
		default_facility: {
			common: false
			description: """
				The default facility when there is no facility in the message or when it cannot be matched to a known
				facility. Possible values and their descriptions comes from the [RFC5424](https://tools.ietf.org/html/rfc5424.
				"""
			required: false
			warnings: []
			type: string: {
				default: "user"
				enum: {
					kern:     "kernel messages (numerical code 0)"
					user:     "user-level messages (numerical code 1)"
					mail:     "mail system (numerical code 2)"
					daemon:   "system daemons (numerical code 3)"
					auth:     "security/authorization messages (numerical code 4)"
					syslog:   "messages generated internally by syslogd (numerical code 5)"
					lpr:      "line printer subsystem (numerical code 6)"
					news:     "network news subsystem (numerical code 7)"
					uucp:     "UUCP subsystem (numerical code 8)"
					cron:     "clock daemon (numerical code 9)"
					authpriv: "security/authorization messages (numerical code 10)"
					ftp:      "FTP daemon (numerical code 11)"
					ntp:      "NTP subsystem messages (numerical code 12)"
					audit:    "log audit (numerical code 13)"
					alert:    "log alert (numerical code 14)"
					clockd:   "clock daemon (numerical code 15)"
					local0:   "local use 0 (numerical code 16)"
					local1:   "local use 0 (numerical code 17)"
					local2:   "local use 0 (numerical code 18)"
					local3:   "local use 0 (numerical code 19)"
					local4:   "local use 0 (numerical code 20)"
					local5:   "local use 0 (numerical code 21)"
					local6:   "local use 0 (numerical code 22)"
					local7:   "local use 0 (numerical code 23)"
				}
				syntax: "literal"
			}
		}
		default_severity: {
			common: false
			description: """
				The default severity when there is no severity in the message or when it cannot be matched to a known
				severity. Possible values and their descriptions comes from the [RFC5424](https://tools.ietf.org/html/rfc5424).
				"""
			required: false
			warnings: []
			type: string: {
				default: "info"
				enum: {
					emerg:   "Emergency: system is unusable (numerical code 0)"
					alert:   "Alert: action must be taken immediately (numerical code 1)"
					crit:    "Critical: critical conditions (numerical code 2)"
					err:     "Error: error conditions (numerical code 3)"
					warning: "Warning: warning conditions (numerical code 4)"
					notice:  "Notice: normal but significant condition (numerical code 5)"
					info:    "Informational: informational messages (numerical code 6)"
					debug:   "Debug: debug-level messages (numerical code 7)"
				}
				syntax: "literal"
			}
		}
	}

	input: {
		logs:    true
		metrics: null
	}

}
