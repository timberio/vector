package metadata

components: _amqp: {
	features: {
		send: to: {
			service: services.amqp
			interface: {
				socket: {
					api: {
						title: "Amqp protocol"
						url:   urls.amqp_protocol
					}
					direction: "outgoing"
					protocols: ["tcp"]
					ssl: "optional"
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

	configuration: {
		connection_string: {
			description: "Connection string to use when connecting to an amqp server in the format of amqp://user:password@host:port/vhost?timeout=seconds"
			required:    true
			warnings: []
			type: string: {
				examples: ["amqp://user:password@127.0.0.1:5672/%2f?timeout=10"]
				syntax: "literal"
			}
		}
		tls: {
			descripton: "Tls options to use when connection to an amqp server"
			required:   false
			warnings: []
			type: object: {
				examples: [
					{
						"ca_cert":                                   "/path/to/ca/cert"
						"identity.client_cert_and_key_der.path":     "/path/to/client/cert"
						"identity.client_cert_and_key_der.password": "password"
					},
				]
				options: {}
			}
		}
	}

	how_it_works: {
		lapin: {
			title: "lapin"
			body:  """
				The `amqp` source and sink uses [`lapin`](\(urls.lapin)) under the hood. This
				is a reliable pure rust library that facilitates communication with Ampq servers
				such as RabbitMQ.
            """
		}
	}
}
