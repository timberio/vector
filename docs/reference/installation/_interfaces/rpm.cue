package metadata

installation: _interfaces: rpm: {
	archs: ["x86_64", "ARM64", "ARMv7"]
	roles: {
		_commands: {
			_config_path: "/etc/vector/vector.{config_format}"
			install: #"""
				sudo rpm -i https://packages.timber.io/vector/{version}/vector-{arch}.rpm
				"""#
			configure: #"""
				cat <<-VECTORCFG > \#(_config_path)
				{config}
				VECTORCFG
				"""#
			start: #"""
				sudo systemctl start vector
				"""#
			stop: #"""
				sudo systemctl stop vector
				"""#
			reload: #"""
				systemctl kill -s HUP --kill-who=main vector.service
				"""#
			logs: #"""
				sudo journalctl -fu vector
				"""#
			variables: {
				arch: ["x86_64", "aarch64", "armv7"]
				version: true
			}
		}
		agent: commands: _commands & {
			variables: config: sources: in: type: components.sources.journald.type
		}
		sidecar: commands:    _commands
		aggregator: commands: _commands
	}
	package_manager_name: installation.package_managers.rpm.name
	title:                "RPM"
}
