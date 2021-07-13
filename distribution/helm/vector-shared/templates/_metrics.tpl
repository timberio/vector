{{/* vim: set filetype=mustache: */}}

{{/*
Common Vector configuration partial containing built-in metrics pipeline.
Internal metrics are common, so we share and reuse the definition.
*/}}
{{- define "libvector.metricsConfigPartial" -}}

{{- if .Values.prometheusSink -}}
{{- $values := .Values -}}

{{- $prometheusInputs := .prometheusInputs -}}
{{- with $values.internalMetricsSource }}
{{- if .enabled }}
# Emit internal Vector metrics.
{{- $value := merge (dict) .config -}}
{{- $_ := set $value "type" "internal_metrics" -}}
{{- $_ := set $value "rawConfig" .rawConfig -}}
{{- tuple .sourceId $value | include "libvector.vectorSourceConfig" | nindent 0 -}}
{{- end }}
{{- end }}

{{- with $values.prometheusSink }}
{{- if .enabled }}

{{- $inputs := .inputs }}
{{- if $prometheusInputs -}}
{{-   $inputs = concat $inputs $prometheusInputs }}
{{- end }}
{{- if and $values.internalMetricsSource.enabled (not .excludeInternalMetrics) -}}
{{-   $inputs = prepend $inputs $values.internalMetricsSource.sourceId }}
{{- end }}
# Expose metrics for scraping in the Prometheus format.
{{- $value := merge (dict) .config -}}
{{- $_ := set $value "type" "prometheus" -}}
{{- $_ := set $value "inputs" $inputs -}}
{{- $_ := set $value "address" (printf "%v:%v" .listenAddress .listenPort) -}}
{{- $_ := set $value "rawConfig" .rawConfig -}}
{{- tuple .sinkId $value | include "libvector.vectorSinkConfig" | nindent 0 -}}
{{- end }}
{{- end }}

{{- end }}
{{- end }}

{{/*
Common Vector container ports used by the built-in metrics pipeline.
Internal metrics are common, so we share and reuse the definition.
*/}}
{{- define "libvector.metricsContainerPorts" -}}
{{- with .Values.prometheusSink }}
{{- if and .enabled (not $.Values.customConfig) -}}
- name: metrics
  containerPort: {{ .listenPort }}
  protocol: TCP
{{- end }}
{{- end }}
{{- end }}

{{/*
Common Vector Pod annotations to expose the built-in metrics pipeline for
Prometheus scraping.
Internal metrics are common, so we share and reuse the definition.
*/}}
{{- define "libvector.metricsPrometheusPodAnnotations" -}}
{{- if .Values.prometheusSink -}}
{{- with .Values.prometheusSink }}
{{- if and .enabled .addPodAnnotations -}}
prometheus.io/scrape: "true"
prometheus.io/port: "{{ .listenPort }}"
{{- end }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Common Vector `PodMonitor` to expose the built-in metrics pipeline for
`prometheus-operator`-powered scraping.
Internal metrics are common, so we share and reuse the definition.
*/}}
{{- define "libvector.metricsPodMonitor" -}}
{{- if .Values.podMonitor.enabled -}}
apiVersion: monitoring.coreos.com/v1
kind: PodMonitor
metadata:
  name: {{ include "libvector.fullname" . }}
  labels:
    {{- include "libvector.labels" . | nindent 4 }}
    {{- with .Values.podMonitor.additionalLabels }}
    {{- toYaml . | nindent 4 }}
    {{- end }}
spec:
  jobLabel: app.kubernetes.io/name

  selector:
    matchLabels:
      {{- include "libvector.selectorLabels" . | nindent 6 }}

  namespaceSelector:
    matchNames:
      - "{{ .Release.Namespace }}"

  podMetricsEndpoints:
    - port: metrics
      path: /metrics
      relabelings:
        - action: labeldrop
          regex: __meta_kubernetes_pod_label_skaffold_dev.*
        - action: labeldrop
          regex: __meta_kubernetes_pod_label_pod_template_hash.*
        - action: labelmap
          regex: __meta_kubernetes_pod_label_(.+)
        {{- with .Values.podMonitor.extraRelabelings }}
        {{- toYaml . | nindent 8 }}
        {{- end }}
      metricRelabelings:
        {{- with .Values.podMonitor.metricRelabelings }}
        {{- toYaml . | nindent 8 }}
        {{- end }}
{{- end }}
{{/* DEPRECATED */}}
{{- if .Values.prometheusSink -}}
{{- if and .Values.prometheusSink.enabled .Values.prometheusSink.podMonitor.enabled -}}
apiVersion: monitoring.coreos.com/v1
kind: PodMonitor
metadata:
  name: {{ include "libvector.fullname" . }}
  labels:
    {{- include "libvector.labels" . | nindent 4 }}
    {{- with .Values.prometheusSink.podMonitor.additionalLabels }}
    {{- toYaml . | nindent 4 }}
    {{- end }}
spec:
  jobLabel: app.kubernetes.io/name

  selector:
    matchLabels:
      {{- include "libvector.selectorLabels" . | nindent 6 }}

  namespaceSelector:
    matchNames:
      - "{{ .Release.Namespace }}"

  podMetricsEndpoints:
    - port: metrics
      path: /metrics
      relabelings:
        - action: labeldrop
          regex: __meta_kubernetes_pod_label_skaffold_dev.*
        - action: labeldrop
          regex: __meta_kubernetes_pod_label_pod_template_hash.*
        - action: labelmap
          regex: __meta_kubernetes_pod_label_(.+)
        {{- with .Values.prometheusSink.podMonitor.extraRelabelings }}
        {{- toYaml . | nindent 8 }}
        {{- end }}
      metricRelabelings:
        {{- with .Values.prometheusSink.podMonitor.metricRelabelings }}
        {{- toYaml . | nindent 8 }}
        {{- end }}
{{- end }}
{{- end }}
{{- end }}
