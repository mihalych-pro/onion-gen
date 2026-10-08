{{- define "onion-gen.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "onion-gen.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{- define "onion-gen.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
app.kubernetes.io/name: {{ include "onion-gen.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{- define "onion-gen.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "onion-gen.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{- define "onion-gen.image" -}}
{{ .Values.image.repository }}:{{ .Values.image.tag | default .Chart.AppVersion }}
{{- end }}

{{/*
The filters, as command-line arguments.

Refused below four symbols rather than passed on: at that length a fleet
reports finds faster than the master can take them, and the failure shows up as
workers queueing rather than as an error. Better to say so at install time.
*/}}
{{- define "onion-gen.filterArgs" -}}
{{- if .Values.filtersConfigMap.name }}
- --filter-file
- /etc/onion-gen/{{ .Values.filtersConfigMap.key }}
{{- else }}
{{- if not .Values.filters }}
{{- fail "set .Values.filters, or point .Values.filtersConfigMap at a dictionary" }}
{{- end }}
{{- range .Values.filters }}
{{- $body := . | toString }}
{{- $stripped := regexReplaceAll "^(prefix|contains|suffix|regex):" $body "" }}
{{- if lt (len $stripped) 4 }}
{{- fail (printf "filter %q is %d symbol(s): a fleet needs at least 4, or the workers report finds faster than the master can store them" $body (len $stripped)) }}
{{- end }}
- --filter
- {{ $body | quote }}
{{- end }}
{{- end }}
{{- end }}
