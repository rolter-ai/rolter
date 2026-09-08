{{- define "rolter.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "rolter.fullname" -}}
{{- if .Values.fullnameOverride }}{{ .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}{{ printf "%s-%s" .Release.Name (include "rolter.name" .) | trunc 63 | trimSuffix "-" }}{{- end }}
{{- end }}

{{- define "rolter.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" }}
app.kubernetes.io/name: {{ include "rolter.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{- define "rolter.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}{{ default (include "rolter.fullname" .) .Values.serviceAccount.name }}{{ else }}{{ default "default" .Values.serviceAccount.name }}{{ end }}
{{- end }}

{{- define "rolter.configMapName" -}}
{{- default (printf "%s-config" (include "rolter.fullname" .)) .Values.config.existingConfigMap }}
{{- end }}

{{- define "rolter.image" -}}
{{ printf "%s:%s" .Values.image.repository (default .Chart.AppVersion .Values.image.tag) }}
{{- end }}

{{/*
Pre-boot validation initContainer (#854).

The same `rolter check` an operator runs by hand, run by the cluster with the
exact environment the workload will get. Sharing one template between the
gateway and control deployments is the point: three deployment paths that each
re-implement "is this configured safely" drift, and the one that drifts is the
one nobody notices until a credential was stored unencrypted.

`--strict` is deliberate here. A warning means the deployment works but is not
what a production operator meant — bound to every interface, no pepper, no
redis — and a chart that ships that silently is the failure mode this exists to
close. Set `preflight.strict=false` to accept warnings, or `preflight.enabled=false`
to skip it entirely.
*/}}
{{- define "rolter.preflight" -}}
{{- if .root.Values.preflight.enabled }}
initContainers:
- name: preflight
  image: {{ include "rolter.image" .root | quote }}
  imagePullPolicy: {{ .root.Values.image.pullPolicy }}
  command: ["/usr/local/bin/rolter"]
  args:
    - "check"
    {{- if .root.Values.preflight.strict }}
    - "--strict"
    {{- end }}
    {{- if .root.Values.preflight.connect }}
    - "--connect"
    {{- end }}
  securityContext: {{ toYaml .root.Values.securityContext | nindent 4 }}
  env: {{- .env | nindent 4 }}
  resources: {{ toYaml .root.Values.preflight.resources | nindent 4 }}
{{- end }}
{{- end }}

{{/*
The control plane's environment, defined once so the preflight initContainer
validates exactly what the container will run with. A second copy would be a
copy that drifts.
*/}}
{{- define "rolter.controlEnv" -}}
- name: RUST_LOG
  value: {{ .Values.env.rustLog | quote }}
# a pod has to bind every interface for its Service to reach it. the binary
# defaults to loopback so an unauthenticated control plane cannot reach a
# network by omission (#970), so the chart sets it back explicitly — and with
# no ROLTER_ADMIN_TOKEN in secretEnv the control plane then refuses to start,
# which is the intended outcome for a cluster deployment
- name: ROLTER_CONTROL_HOST
  value: {{ .Values.control.host | quote }}
{{- if .Values.env.databaseUrl }}
- name: ROLTER_DATABASE_URL
  value: {{ .Values.env.databaseUrl | quote }}
{{- end }}
{{- with .Values.control.pool }}
- name: ROLTER_DB_MAX_CONNECTIONS
  value: {{ .maxConnections | quote }}
- name: ROLTER_DB_MIN_CONNECTIONS
  value: {{ .minConnections | quote }}
- name: ROLTER_DB_ACQUIRE_TIMEOUT_SECS
  value: {{ .acquireTimeoutSeconds | quote }}
- name: ROLTER_DB_IDLE_TIMEOUT_SECS
  value: {{ .idleTimeoutSeconds | quote }}
- name: ROLTER_DB_MAX_LIFETIME_SECS
  value: {{ .maxLifetimeSeconds | quote }}
{{- end }}
{{- with .Values.control.login }}
- name: ROLTER_LOGIN_MAX_FAILURES
  value: {{ .maxFailures | quote }}
- name: ROLTER_LOGIN_IP_MAX_FAILURES
  value: {{ .ipMaxFailures | quote }}
- name: ROLTER_LOGIN_FAILURE_WINDOW_SECS
  value: {{ .failureWindowSeconds | quote }}
- name: ROLTER_LOGIN_LOCK_SECS
  value: {{ .lockSeconds | quote }}
- name: ROLTER_LOGIN_MAX_LOCK_SECS
  value: {{ .maxLockSeconds | quote }}
- name: ROLTER_LOGIN_TRUST_FORWARDED_FOR
  value: {{ .trustForwardedFor | quote }}
{{- end }}
- name: ROLTER_UPDATE_CHECK
  value: {{ .Values.control.updateCheck | quote }}
{{- if .Values.env.redisUrl }}
- name: ROLTER_REDIS_URL
  value: {{ .Values.env.redisUrl | quote }}
{{- end }}
{{- if .Values.env.clickhouseUrl }}
- name: CLICKHOUSE_URL
  value: {{ .Values.env.clickhouseUrl | quote }}
{{- end }}
{{- with .Values.secretEnv }}
{{ toYaml . }}
{{- end }}
{{- with .Values.control.extraEnv }}
{{ toYaml . }}
{{- end }}
{{- end }}

{{/*
The gateway's environment, for the same reason.
*/}}
{{- define "rolter.gatewayEnv" -}}
- name: RUST_LOG
  value: {{ .Values.env.rustLog | quote }}
{{- if .Values.env.redisUrl }}
- name: REDIS_URL
  value: {{ .Values.env.redisUrl | quote }}
{{- end }}
{{- /* the gateway writes the usage rows the dashboard reads, so both
       deployments need this one value; without it the analytics screens are
       empty however much traffic is served (#929) */}}
{{- if .Values.env.clickhouseUrl }}
- name: CLICKHOUSE_URL
  value: {{ .Values.env.clickhouseUrl | quote }}
{{- end }}
{{- with .Values.secretEnv }}
{{ toYaml . }}
{{- end }}
{{- with .Values.gateway.extraEnv }}
{{ toYaml . }}
{{- end }}
{{- end }}
