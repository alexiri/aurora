{{- define "aurora.fullname" -}}
{{- if contains "aurora" .Release.Name -}}{{ .Release.Name | trunc 50 | trimSuffix "-" }}{{- else -}}{{ printf "%s-aurora" .Release.Name | trunc 50 | trimSuffix "-" }}{{- end -}}
{{- end -}}

{{- define "aurora.labels" -}}
app.kubernetes.io/name: aurora
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version }}
{{- end -}}

{{- define "aurora.selector" -}}
app.kubernetes.io/name: aurora
app.kubernetes.io/instance: {{ .root.Release.Name }}
app.kubernetes.io/component: {{ .component }}
{{- end -}}

{{- define "aurora.image" -}}
{{- $reg := .root.Values.image.registry -}}
{{- $tag := default .root.Chart.AppVersion .root.Values.image.tag -}}
{{- if .override -}}{{ .override }}{{- else if $reg -}}{{ printf "%s/%s:%s" (trimSuffix "/" $reg) .name $tag }}{{- else -}}{{ printf "%s:%s" .name $tag }}{{- end -}}
{{- end -}}

{{- define "aurora.secretName" -}}
{{- if .Values.secrets.existingSecret -}}{{ .Values.secrets.existingSecret }}{{- else -}}{{ default (printf "%s-secrets" (include "aurora.fullname" .)) .Values.secrets.generatedName }}{{- end -}}
{{- end -}}

{{- define "aurora.dbSecretName" -}}
{{- if .Values.postgres.enabled -}}{{ include "aurora.secretName" . }}{{- else -}}{{ default (printf "%s-database" (include "aurora.fullname" .)) .Values.database.existingSecret }}{{- end -}}
{{- end -}}

{{- define "aurora.caSecretName" -}}
{{- default (printf "%s-gateway-ca" (include "aurora.fullname" .)) .Values.gateway.ca.existingSecret -}}
{{- end -}}

{{- define "aurora.host" -}}
{{- regexReplaceAll "^https?://([^/:]+).*$" .Values.externalUrl "${1}" -}}
{{- end -}}

{{- define "aurora.podSecurity" -}}
securityContext:
  runAsNonRoot: true
  runAsUser: 65532
  runAsGroup: 65532
  fsGroup: 65532
  seccompProfile: {type: RuntimeDefault}
{{- end -}}

{{- define "aurora.containerSecurity" -}}
securityContext:
  allowPrivilegeEscalation: false
  readOnlyRootFilesystem: true
  capabilities: {drop: [ALL]}
{{- end -}}

{{/* Common env for aurora services. Args: root, db (bool: needs database), runner (bool) */}}
{{- define "aurora.env" -}}
- {name: AURORA_CONFIG, value: /etc/aurora/aurora.toml}
{{- if .db }}
- name: AURORA_DATABASE_URL
  valueFrom: {secretKeyRef: {name: {{ include "aurora.dbSecretName" .root }}, key: AURORA_DATABASE_URL}}
{{- end }}
{{- range $k := .keys }}
- name: {{ $k }}
  valueFrom: {secretKeyRef: {name: {{ include "aurora.secretName" $.root }}, key: {{ $k }}}}
{{- end }}
{{- /* RFE-007: ring active ids; absent for a bare (phase 0) value, so the reference is optional. */ -}}
{{- range $k := .optionalKeys }}
- name: {{ $k }}
  valueFrom: {secretKeyRef: {name: {{ include "aurora.secretName" $.root }}, key: {{ $k }}, optional: true}}
{{- end }}
{{- end -}}

{{/* One workload (Deployment + Service + PDB). Args: root, name, port(s), spec, image, keys, db, args */}}
{{- define "aurora.workload" -}}
{{- $r := .root -}}
{{- $full := include "aurora.fullname" $r -}}
apiVersion: apps/v1
kind: Deployment
metadata:
  name: {{ $full }}-{{ .name }}
  labels: {{- include "aurora.labels" $r | nindent 4 }}
    app.kubernetes.io/component: {{ .name }}
spec:
  replicas: {{ .spec.replicas }}
  selector:
    matchLabels: {{- include "aurora.selector" (dict "root" $r "component" .name) | nindent 6 }}
  template:
    metadata:
      labels: {{- include "aurora.labels" $r | nindent 8 }}
        app.kubernetes.io/component: {{ .name }}
      annotations:
        checksum/config: {{ include (print $.root.Template.BasePath "/configmap.yaml") $r | sha256sum }}
    spec:
      {{- if .serviceAccount }}
      serviceAccountName: {{ .serviceAccount }}
      automountServiceAccountToken: true
      {{- else }}
      automountServiceAccountToken: false
      {{- end }}
      enableServiceLinks: false
      {{- with $r.Values.image.pullSecrets }}
      imagePullSecrets: {{- toYaml . | nindent 8 }}
      {{- end }}
      {{- include "aurora.podSecurity" $r | nindent 6 }}
      {{- with .spec.nodeSelector }}
      nodeSelector: {{- toYaml . | nindent 8 }}
      {{- end }}
      {{- with .spec.tolerations }}
      tolerations: {{- toYaml . | nindent 8 }}
      {{- end }}
      topologySpreadConstraints:
        - maxSkew: 1
          topologyKey: kubernetes.io/hostname
          whenUnsatisfiable: ScheduleAnyway
          labelSelector:
            matchLabels: {{- include "aurora.selector" (dict "root" $r "component" .name) | nindent 14 }}
      {{- if and .pluginsMount (eq $r.Values.plugins.source "configMap") }}
      # ConfigMap keys are symlinks, which the plugin scan (scan.rs) skips, so copy them with cp -L.
      initContainers:
        - name: seed-plugins
          image: {{ $r.Values.hookImage | quote }}
          {{- include "aurora.containerSecurity" $r | nindent 10 }}
          command: ["/bin/sh", "-ec"]
          args:
            - |
              for f in /plugin-src/*.aurora-plugin; do [ -f "$f" ] || continue; cp -L "$f" /plugins/; done
          volumeMounts:
            - {name: plugin-src, mountPath: /plugin-src, readOnly: true}
            - {name: plugins, mountPath: /plugins}
      {{- end }}
      containers:
        - name: {{ .name }}
          image: {{ .image | quote }}
          imagePullPolicy: {{ $r.Values.image.pullPolicy }}
          {{- with .args }}
          args: {{ toJson . }}
          {{- end }}
          {{- include "aurora.containerSecurity" $r | nindent 10 }}
          env:
            {{- if .env }}{{ .env | nindent 12 }}{{ end }}
            {{- with .spec.extraEnv }}{{ toYaml . | nindent 12 }}{{ end }}
          ports:
            {{- range .ports }}
            - {name: {{ .name }}, containerPort: {{ .port }}}
            {{- end }}
          readinessProbe:
            tcpSocket: {port: {{ (first .ports).port }}}
            periodSeconds: 10
          livenessProbe:
            tcpSocket: {port: {{ (first .ports).port }}}
            initialDelaySeconds: 15
            periodSeconds: 20
          resources: {{- toYaml .spec.resources | nindent 12 }}
          volumeMounts:
            - {name: config, mountPath: /etc/aurora, readOnly: true}
            - {name: tmp, mountPath: /tmp}
            {{- if or .ca .caCert }}
            - {name: ca, mountPath: /var/lib/aurora/ca, readOnly: true}
            {{- end }}
            {{- if .proxyTlsSecret }}
            - {name: proxy-tls, mountPath: /var/lib/aurora/proxy-tls, readOnly: true}
            {{- end }}
            {{- if and .pluginsMount (ne $r.Values.plugins.source "none") }}
            - {name: plugins, mountPath: {{ $r.Values.plugins.dir | quote }}, readOnly: true}
            {{- end }}
      volumes:
        - name: config
          configMap: {name: {{ $full }}-config}
        - name: tmp
          emptyDir: {}
        {{- if .ca }}
        - name: ca
          secret: {secretName: {{ include "aurora.caSecretName" $r }}, defaultMode: 0440}
        {{- else if .caCert }}
        {{- /* api: only the public certificate (embedded in boot documents), never ca.key */}}
        - name: ca
          secret:
            secretName: {{ include "aurora.caSecretName" $r }}
            defaultMode: 0440
            items: [{key: ca.pem, path: ca.pem}]
        {{- end }}
        {{- if .proxyTlsSecret }}
        - name: proxy-tls
          secret: {secretName: {{ .proxyTlsSecret | quote }}, defaultMode: 0440}
        {{- end }}
        {{- if and .pluginsMount (ne $r.Values.plugins.source "none") }}
        {{- if eq $r.Values.plugins.source "pvc" }}
        - name: plugins
          persistentVolumeClaim: {claimName: {{ $r.Values.plugins.pvcName | quote }}, readOnly: true}
        {{- else }}
        - name: plugins
          emptyDir: {}
        - name: plugin-src
          configMap: {name: {{ $r.Values.plugins.configMapName | quote }}, defaultMode: 0444}
        {{- end }}
        {{- end }}
---
apiVersion: v1
kind: Service
metadata:
  name: {{ $full }}-{{ .name }}
  labels: {{- include "aurora.labels" $r | nindent 4 }}
    app.kubernetes.io/component: {{ .name }}
spec:
  selector: {{- include "aurora.selector" (dict "root" $r "component" .name) | nindent 4 }}
  ports:
    {{- range .ports }}
    {{- if not .skipService }}
    - {name: {{ .name }}, port: {{ .port }}, targetPort: {{ .port }}}
    {{- end }}
    {{- end }}
{{- if $r.Values.podDisruptionBudget.enabled }}
---
apiVersion: policy/v1
kind: PodDisruptionBudget
metadata:
  name: {{ $full }}-{{ .name }}
  labels: {{- include "aurora.labels" $r | nindent 4 }}
spec:
  minAvailable: {{ $r.Values.podDisruptionBudget.minAvailable }}
  selector:
    matchLabels: {{- include "aurora.selector" (dict "root" $r "component" .name) | nindent 6 }}
{{- end }}
{{- end -}}

{{- define "aurora.peer" -}}
- podSelector:
    matchLabels: {app.kubernetes.io/name: aurora, app.kubernetes.io/instance: {{ .root.Release.Name }}, app.kubernetes.io/component: {{ .c }}}
{{- end -}}
{{- define "aurora.external" -}}
- to:
    - ipBlock:
        cidr: {{ .Values.networkPolicy.externalEgress.cidr }}
        {{- with .Values.networkPolicy.externalEgress.except }}
        except: {{- toYaml . | nindent 10 }}
        {{- end }}
{{- end -}}
{{- define "aurora.dbEgress" -}}
{{- if .Values.postgres.enabled }}
- to:
    - podSelector:
        matchLabels: {app.kubernetes.io/instance: {{ .Release.Name }}, app.kubernetes.io/component: postgres}
  ports: [{protocol: TCP, port: 5432}]
{{- else if .Values.database.cidrs }}
- to:
    {{- range .Values.database.cidrs }}
    - ipBlock: {cidr: {{ . }}}
    {{- end }}
  ports: [{protocol: TCP, port: {{ .Values.database.port }}}]
{{- end }}
{{- end -}}
