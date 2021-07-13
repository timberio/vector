---
date: "2021-07-13"
title: "Introduction of `customConfig` Helm option and deprecation notice for TOML based config keys"
short: "Introduction of `customConfig` in Helm charts"
description: "Configure Vector directly through your Helm `values.yaml` without having to converting to TOML"
authors: ["spencergilbert"]
pr_numbers: [8079]
release: "0.15.0"
hide_on_release_notes: false
badges:
  type: "deprecation"
  platforms: ["helm"]
  domains: ["config"]
---

With the release of Vector 0.15.0, we have introduced a new method of configuring Vector through Helm.
This new method uses YAML configuration files, and in a coming release will become the default configuration
method.

The new `customConfig` key will take precedence over the existing configuration options, if `customConfig` is
set then no default configurations will be rendered.

**You do not need to upgrade immediately. The deprecated keys will not be removed before Vector hits 1.0**

## Upgrade Guide

### Breaking: updated container args

The argument passed to Vector has been changed to `--config-dir` which will load any TOML, YAML, or JSON
files added to the "/etc/vector" directory.

If you have been including files that should not be loaded by Vector as configuration you should move them
into a separate directory or into a sub-directory of "etc/vector".

### globalOptions and logSchema

Any [global options](/docs/reference/configuration/global-options/) can be moved directly
into the `customConfig` key and converted into snake-case.

If you had the default `dataDir` and `logSchema` values, include:

```yaml title="values.yaml"
customConfig:
  data_dir: "/vector-data-dir"
  log_schema:
    host_key: host
    message_key: message
    source_type_key: source_type
    timestamp_key: timestamp
  ...
```

### vectorApi

[Vector API](/docs/reference/api/) options can be moved as is under a `customConfig.api` key.
The `extraContainerPorts` or `service` key should be used to expose the port configured in `customConfig`.

If you had `vectorApi` enabled, include:

```yaml title="values.yaml"
customConfig:
  ...
  api:
    enabled: true
    address: 0.0.0.0:8686
    playground: true
  ...
```

### kubernetesLogsSource

The vector-agent chart will continue to mount the hostPaths required to access Pod logs.

If you had `kubernetesLogsSource` enabled, include:

```yaml title="values.yaml"
customConfig:
  ...
  sources:
  ...
    kubernetes_logs:
      type: kubernetes_logs
  ...
```

### vectorSource

The `service` key should be used to expose the port configured in `customConfig`.

If you had `vectorSource` enabled, include:

```yaml title="values.yaml"
customConfig:
  ...
  sources:
  ...
    vector:
      type: vector
      address: 0.0.0.0:9000
  ...
service:
  ports:
    - name: http
      port: 9000
      protocol: TCP
      targetPort: 9000
```

### vectorSink

If you had `vectorSink` enabled, include:

```yaml title="values.yaml"
customConfig:
  ...
  sinks:
  ...
    vector_sink:
      type: vector
      inputs: ["kubernetes_logs"]
      address: vector:9000
  ...
```

### internalMetricsSource

If you had `internalMetricsSource` enabled, include:

```yaml title="values.yaml"
customConfig:
  ...
  sources:
    ...
    internal_metrics:
      type: internal_metrics
  ...
```

### hostMetricsSource

The `vector-agent` chart will continue to set the `PROCFS_ROOT` and `SYSFS_ROOT` environment variables,
as well as mount the required hostPaths.

If you had `hostMetricsSource` enabled, include:

```yaml title="values.yaml"
customConfig:
  ...
  sources:
    ...
    host_metrics:
      type: host_metrics
      filesystem:
        devices:
          excludes: ["binfmt_misc"]
        filesystems:
          excludes: ["binfmt_misc"]
        mountPoints:
          excludes: ["*/proc/sys/fs/binfmt_misc"]
  ...
```

### prometheusSink

The `extraContainerPorts` key should be used to expose the port configured in `customConfig`.

The `prometheusSink.podMonitor` key has been moved to a top level key and can be accessed directly at
`podMonitor`. The `addPodAnnotations` option has been removed in favor setting the required annotations
with the `podAnnotations` key.

If you had `prometheusSink` enabled, include:

```yaml title="values.yaml"
customConfig:
  ...
  sinks:
    ...
    prometheus_sink:
      type: prometheus_exporter
      inputs: ["host_metrics", "internal_metrics"]
      address: 0.0.0.0:9090
  ...
extraContainerPorts:
  - name: http
    port: 9090
    protocol: TCP
    targetPort: 9090
```

## Using `customConfig`

We've configured the `ConfigMap` template to fail if the deprecated keys are enabled at the same time
as the new YAML based configurations. The following values can be used to disable the old deprecated
keys and use the new YAML based configuration with default values.

With the deprecated keys disabled, a custom Vector configuration can be provided in raw YAML and is passed
through a `tpl` function to allow for the evaluation of Helm templates contained within. Below is an example
of values using `customConfig` and templating.

```yaml title="cusomConfig.yaml"
customConfig:
  data_dir: "/custom-data-dir"
  healthchecks:
    enabled: true
    require_healthy: true
  api:
    enabled: true
    address: "0.0.0.0:{{ with index .Values.service.ports 0 }}{{ .port }}{{ end }}"
    playground: false
  sources:
    internal_logs:
      type: internal_logs
    internal_metrics:
      type: internal_metrics
    kubernetes_logs:
      type: kubernetes_logs
      glob_minimum_cooldown_ms: 1000
      ingestion_timestamp_field: ingestion_timestamp
    statsd_metrics:
      type: statsd
      address: "0.0.0.0:{{ with index .Values.service.ports 1 }}{{ .port }}{{ end }}"
      mode: tcp
  transforms:
    sample:
      type: sample
      inputs: ["*_logs"]
      rate: 20
  sinks:
    datadog_logs:
      type: datadog_logs
      inputs: ["sample"]
      compression: gzip
      default_api_key: "${DATADOG_API_KEY}"
      encoding:
        codec: json
    datadog_metrics:
      type: datadog_metrics
      inputs: ["*_metrics"]
      api_key: "${DATADOG_API_KEY}"
service:
  enabled: true
  ports:
  - name: api
    port: 8686
    protocol: TCP
    targetPort: 8686
  - name: statsd
    port: 8000
    protocol: TCP
    targetPort: 8000
# Disable deprecated keys
globalOptions:
  enabled: false
logSchema:
  enabled: false
vectorApi:
  enabled: false
kubernetesLogsSource:
  enabled: false
vectorSource:
  enabled: false
vectorSink:
  enabled: false
internalMetricsSource:
  enabled: false
hostMetricsSource:
  enabled: false
prometheusSink:
  enabled: false
```
