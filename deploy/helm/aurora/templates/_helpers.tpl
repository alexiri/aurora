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

{{- /* (FIX-106, D-178 Q-R5-11) The chart-bundled evaluation Postgres creates the gateway's
       least-privilege role itself: renders "true" when postgres.enabled and the operator set
       neither the explicit role pair nor database.allowSharedLogin. */ -}}
{{- define "aurora.gatewayRoleAuto" -}}
{{- if and .Values.postgres.enabled (not .Values.database.gatewayRole) (not .Values.database.gatewayExistingSecret) (not .Values.database.allowSharedLogin) -}}true{{- end -}}
{{- end -}}

{{/* The role the chart creates in the bundled Postgres (the name is fixed; its password is generated). */}}
{{- define "aurora.gatewayAutoRole" -}}aurora_gateway{{- end -}}

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
- {name: AURORA_CONFIG, value: /etc/aurora/{{ default "aurora.toml" .configFile }}}
{{- if .db }}
- name: AURORA_DATABASE_URL
  {{- /* FIX-016: with database.gatewayExistingSecret set, the gateway (gatewayDb) logs in as its least-privilege role.
         FIX-106: with the bundled Postgres and no explicit choice, it logs in as the role the chart creates (key AURORA_GATEWAY_DATABASE_URL of the generated Secret). */}}
  {{- if and .gatewayDb (include "aurora.gatewayRoleAuto" .root) }}
  valueFrom: {secretKeyRef: {name: {{ include "aurora.secretName" .root }}, key: AURORA_GATEWAY_DATABASE_URL}}
  {{- else }}
  valueFrom: {secretKeyRef: {name: {{ if and .gatewayDb .root.Values.database.gatewayExistingSecret }}{{ .root.Values.database.gatewayExistingSecret }}{{ else }}{{ include "aurora.dbSecretName" .root }}{{ end }}, key: AURORA_DATABASE_URL}}
  {{- end }}
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
      {{- else if and .vaultToken (include "aurora.vaultK8s" $r) }}
      {{- /* Vault kubernetes auth: the identity is a projected token (below), not the API token. */}}
      serviceAccountName: {{ $full }}-{{ .name }}
      automountServiceAccountToken: false
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
            {{- /* FIX-139: connector workloads read the PEM bundle named here (the extraCaBundle mount) for their own outbound TLS */}}
            {{- if and .connectorCa (or $r.Values.extraCaBundle.existingConfigMap $r.Values.extraCaBundle.existingSecret) }}
            - {name: AURORA_TLS_EXTRA_CA_BUNDLE, value: {{ printf "/etc/aurora-tls/%s" $r.Values.extraCaBundle.key | quote }}}
            {{- end }}
            {{- with .spec.extraEnv }}{{ toYaml . | nindent 12 }}{{ end }}
          ports:
            {{- range .ports }}
            - {name: {{ .name }}, containerPort: {{ .port }}}
            {{- end }}
          {{- /* (FIX-102, docs/15 §Health) readiness is plain `/readyz`, liveness `/healthz`, on the port
                 that serves the ops router (`probePort`). A workload without an ops router (web, the
                 external connector images) keeps the TCP check on its first port. */}}
          {{- if .probePort }}
          readinessProbe:
            httpGet: {path: /readyz, port: {{ .probePort }}}
            periodSeconds: 10
          livenessProbe:
            httpGet: {path: /healthz, port: {{ .probePort }}}
            initialDelaySeconds: 15
            periodSeconds: 20
          {{- else }}
          readinessProbe:
            tcpSocket: {port: {{ (first .ports).port }}}
            periodSeconds: 10
          livenessProbe:
            tcpSocket: {port: {{ (first .ports).port }}}
            initialDelaySeconds: 15
            periodSeconds: 20
          {{- end }}
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
            {{- if or $r.Values.extraCaBundle.existingConfigMap $r.Values.extraCaBundle.existingSecret }}
            - {name: extra-ca, mountPath: /etc/aurora-tls, readOnly: true}
            {{- end }}
            {{- if and .vaultToken (include "aurora.vaultK8s" $r) }}
            - {name: vault-token, mountPath: /var/run/secrets/aurora-vault, readOnly: true}
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
        {{- if $r.Values.extraCaBundle.existingConfigMap }}
        - name: extra-ca
          configMap: {name: {{ $r.Values.extraCaBundle.existingConfigMap | quote }}, defaultMode: 0444}
        {{- else if $r.Values.extraCaBundle.existingSecret }}
        - name: extra-ca
          secret: {secretName: {{ $r.Values.extraCaBundle.existingSecret | quote }}, defaultMode: 0444}
        {{- end }}
        {{- if and .vaultToken (include "aurora.vaultK8s" $r) }}
        - name: vault-token
          projected:
            defaultMode: 0440
            sources:
              - serviceAccountToken:
                  path: token
                  audience: {{ $r.Values.vault.auth.audience | quote }}
                  expirationSeconds: {{ int $r.Values.vault.auth.tokenExpirationSeconds }}
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
{{- /* (FIX-102) minAvailable: 1 on a single replica would block every node drain: a budget only from two replicas */}}
{{- if and $r.Values.podDisruptionBudget.enabled (ge (int .spec.replicas) 2) }}
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

{{- /* (FIX-102, FIX-069) The Kubernetes API server addresses a NetworkPolicy matches (the post-DNAT
       endpoint addresses, not the ClusterIP), as a JSON list of CIDRs. In order: networkPolicy.kubeApiCidrs;
       the `kubernetes` EndpointSlice in `default` (discovery.k8s.io/v1, GA since 1.21 — the chart floor is
       1.30; the core v1 Endpoints it replaces is deprecated from 1.33), read by Helm `lookup` at install
       and upgrade time: `.endpoints[].addresses[]` are plain strings, endpoints with `conditions.ready:
       false` are skipped, /32 or /128 by `addressType`. When the cluster names the slice differently the
       namespace's slices are listed and filtered on the label `kubernetes.io/service-name: kubernetes`.
       Empty when nothing is known (`helm template`, --dry-run, no `get` on endpointslices in `default`): the
       callers then open nothing, unless networkPolicy.kubeApiOpen is set. */ -}}
{{- define "aurora.kubeApiCidrs" -}}
{{- $out := list -}}
{{- if .Values.networkPolicy.kubeApiCidrs -}}
{{- $out = .Values.networkPolicy.kubeApiCidrs -}}
{{- else -}}
{{- $slices := list -}}
{{- $one := lookup "discovery.k8s.io/v1" "EndpointSlice" "default" "kubernetes" -}}
{{- if $one -}}
{{- $slices = list $one -}}
{{- else -}}
{{- $all := lookup "discovery.k8s.io/v1" "EndpointSlice" "default" "" -}}
{{- range (dig "items" (list) $all) -}}
{{- if eq (dig "metadata" "labels" "kubernetes.io/service-name" "" .) "kubernetes" }}{{ $slices = append $slices . }}{{ end -}}
{{- end -}}
{{- end -}}
{{- range $s := $slices -}}
{{- $mask := ternary "128" "32" (eq (dig "addressType" "IPv4" $s) "IPv6") -}}
{{- range (dig "endpoints" (list) $s) -}}
{{- if ne (toString (dig "conditions" "ready" true .)) "false" -}}
{{- range (default (list) .addresses) -}}
{{- $out = append $out (printf "%s/%s" . $mask) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- toJson (uniq $out) -}}
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
{{/* True ("true") when the Vault store uses kubernetes auth (projected token for api/gateway). */}}
{{- define "aurora.vaultK8s" -}}
{{- if and .Values.vault.enabled (eq .Values.vault.auth.method "kubernetes") }}true{{ end -}}
{{- end -}}

{{/* The [secrets] section with the Vault store added. Args: root, cfg (config so far), svc (api|gateway). Returns JSON. */}}
{{- define "aurora.vaultSecrets" -}}
{{- $v := .root.Values.vault -}}
{{- $sec := deepCopy (default (dict) .cfg.secrets) -}}
{{- if not $v.address }}{{ fail "vault.address is required when vault.enabled" }}{{ end -}}
{{- if or (not $v.storeId) (contains ":" $v.storeId) }}{{ fail "vault.storeId must be non-empty and contain no ':'" }}{{ end -}}
{{- if not (has $v.auth.method (list "kubernetes" "approle")) }}{{ fail "vault.auth.method must be kubernetes or approle (token auth is development only)" }}{{ end -}}
{{- if and (eq $v.auth.method "approle") (not $v.auth.approle.existingSecret) }}{{ fail "vault.auth.approle.existingSecret is required for approle auth" }}{{ end -}}
{{- if and (eq $v.auth.method "kubernetes") (lt (int $v.auth.tokenExpirationSeconds) 600) }}{{ fail "vault.auth.tokenExpirationSeconds must be at least 600" }}{{ end -}}
{{- $auth := dict "method" $v.auth.method -}}
{{- if eq $v.auth.method "kubernetes" -}}
{{- $_ := set $auth "mount" (default "kubernetes" $v.auth.mount) -}}
{{- $_ := set $auth "role" (ternary $v.auth.apiRole $v.auth.gatewayRole (eq .svc "api")) -}}
{{- $_ := set $auth "jwt_path" "/var/run/secrets/aurora-vault/token" -}}
{{- else -}}
{{- $_ := set $auth "mount" (default "approle" $v.auth.mount) -}}
{{- $_ := set $auth "role_id_env" "AURORA_VAULT_ROLE_ID" -}}
{{- $_ := set $auth "secret_id_env" "AURORA_VAULT_SECRET_ID" -}}
{{- end -}}
{{- $store := dict "id" $v.storeId "kind" "hashicorp-vault" "address" $v.address "kv_mount" $v.kvMount "managed_prefix" $v.managedPrefix "auth" $auth -}}
{{- with $v.caFile }}{{ $_ := set $store "ca_file" . }}{{ end -}}
{{- if $v.referenceScopes -}}
{{- $scopes := list -}}
{{- range $v.referenceScopes -}}
{{- if or (not .orgId) (not .paths) }}{{ fail "each vault.referenceScopes entry needs orgId (the org's id, a UUID) and paths" }}{{ end -}}
{{- $scopes = append $scopes (dict "org_id" .orgId "paths" .paths) -}}
{{- end -}}
{{- $_ := set $store "reference_scopes" $scopes -}}
{{- end -}}
{{- range (default (list) $sec.stores) }}{{ if eq .id $v.storeId }}{{ fail "config.secrets.stores already defines a store with the id of vault.storeId" }}{{ end }}{{ end -}}
{{- $_ := set $sec "stores" (append (default (list) $sec.stores) $store) -}}
{{- if $v.makeDefault }}{{ $_ := set $sec "default_store" $v.storeId }}{{ end -}}
{{- toJson $sec -}}
{{- end -}}

{{/* AppRole credentials for one service as env entries (empty unless approle auth). Args: root, svc */}}
{{- define "aurora.vaultEnv" -}}
{{- if and .root.Values.vault.enabled (eq .root.Values.vault.auth.method "approle") }}
- name: AURORA_VAULT_ROLE_ID
  valueFrom: {secretKeyRef: {name: {{ .root.Values.vault.auth.approle.existingSecret | quote }}, key: {{ upper .svc }}_ROLE_ID}}
- name: AURORA_VAULT_SECRET_ID
  valueFrom: {secretKeyRef: {name: {{ .root.Values.vault.auth.approle.existingSecret | quote }}, key: {{ upper .svc }}_SECRET_ID}}
{{- end }}
{{- end -}}

{{/* (P4-CNT) Redis credentials of the shared counters: the Secret's keys become the variables
     the configuration names (counters.redis.existingSecret; never values, never a URL). */}}
{{- define "aurora.countersEnv" -}}
{{- if and (eq (default "memory" .Values.counters.backend) "redis") .Values.counters.redis.existingSecret }}
- name: AURORA_COUNTERS_REDIS_USERNAME
  valueFrom: {secretKeyRef: {name: {{ .Values.counters.redis.existingSecret | quote }}, key: REDIS_USERNAME, optional: true}}
- name: AURORA_COUNTERS_REDIS_PASSWORD
  valueFrom: {secretKeyRef: {name: {{ .Values.counters.redis.existingSecret | quote }}, key: REDIS_PASSWORD, optional: true}}
{{- end }}
{{- end -}}

{{/* (P4-CNT) Egress to the Redis of the shared counters (default deny stays: no peer, no egress). */}}
{{- define "aurora.countersEgress" -}}
{{- if and (eq (default "memory" .Values.counters.backend) "redis") .Values.counters.redis.networkPolicy.egressTo }}
- to:
    {{- toYaml .Values.counters.redis.networkPolicy.egressTo | nindent 4 }}
  ports: [{protocol: TCP, port: {{ default 6379 (regexFind ":[0-9]+$" (trimSuffix "/" .Values.counters.redis.address) | trimPrefix ":" | atoi) }}}]
{{- end }}
{{- end -}}

{{/* Egress to a private Vault (the external-egress rule excludes private ranges). */}}
{{- define "aurora.vaultEgress" -}}
{{- if and .Values.vault.enabled .Values.vault.egress.cidrs }}
- to:
    {{- range .Values.vault.egress.cidrs }}
    - ipBlock: {cidr: {{ . }}}
    {{- end }}
  ports: [{protocol: TCP, port: {{ .Values.vault.egress.port }}}]
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
