{{/* vim: set filetype=mustache: */}}

{{/*
Internal template to render service ports depending on whether service is a headless service or not. Use
either 'vector-aggregator.servicePorts' or 'vector-aggregator.headlessServicePorts' as entry points.
*/}}
{{- define "vector-aggregator.internalServicePorts" -}}
{{- $headless := index . 0 -}}
{{- $values := index . 1 -}}
{{- if and $values.vectorSource.enabled (not $values.customConfig) }}
{{- $servicePort := dict -}}
{{- $_ := set $servicePort "name" "vector" -}}
{{- $_ := set $servicePort "port" $values.vectorSource.listenPort -}}
{{- $_ := set $servicePort "nodePort" $values.vectorSource.nodePort -}}
{{- $_ := set $servicePort "protocol" "TCP" -}}
{{- $_ := set $servicePort "targetPort" $values.vectorSource.listenPort -}}
{{ tuple $headless $servicePort | include "libvector.servicePort" }}
{{- end }}
{{- range $values.service.ports }}
{{ tuple $headless . | include "libvector.servicePort" }}
{{- end }}
{{- end -}}

{{/*
Generate effective service ports for normal (non-headless) service definition.
*/}}
{{- define "vector-aggregator.servicePorts" -}}
{{- tuple false .Values | include "vector-aggregator.internalServicePorts" -}}
{{- end -}}

{{/*
Generate effective service ports for headless service definition.
*/}}
{{- define "vector-aggregator.headlessServicePorts" -}}
{{- tuple true .Values | include "vector-aggregator.internalServicePorts" -}}
{{- end }}

{{/*
Determines whether there are any ports present.
*/}}
{{- define "vector-aggregator.servicePortsPresent" -}}
{{- or (and .Values.vectorSource.enabled (not .Values.customConfig)) (not (empty .Values.service.ports)) }}
{{- end }}
