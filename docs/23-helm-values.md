# 23 — Helm values reference

Every value the chart `deploy/helm/aurora/` accepts, with type and allowed
values, the default, what it does, and where the details live. This page is the
**index**; `15 §Configuration reference` stays the source of truth for the
`aurora.toml` keys the chart renders, and `docs/operator/install-helm.md` walks
through an install step by step. The chart is checked with `helm lint` and
`helm template` on every change; keys are consumed by `templates/`, so a key
that is not in this table does not exist.

## Where does a key go? (read this first)

**The rule:**
- **Top level** holds everything **Kubernetes** has to know about: images, replicas, Secrets, volumes, ingress, network policies. It also holds every feature that adds a workload, a listener, an edge route or a network flow.
- **`config:`** holds the **application's own settings**. They are copied verbatim into `aurora.toml` and change nothing in the Kubernetes objects. The key names are the TOML names from [15 §Configuration reference](15-deployment-operations.md#configuration-reference-keys-and-defaults), in snake_case.

**Ask yourself whether turning it on needs something new in the cluster.** If yes, it is a top-level switch. If it only changes how Aurora behaves, it goes under `config:`.

**Features with a top-level switch.** Set only the top-level key, and the chart writes the matching `config` key for you. Don't set the `config` side yourself. For the three on/off switches (`terminal`, `relay`/previews, `scim`), turning it on only in `config`, or (previews) giving the two sides different values, fails the render with a message. For the others, your `config` entry is overridden by the chart's (§The config block). The terminal and SCIM chart values say the deployment **provides** the feature (wiring and route): they render `config.terminal.available` / `config.scim.available`, and whether the feature is *on* is an instance setting (Admin › Settings › Terminal, Directory sync) that defaults to it.

| Set this (top level) | Not this | Because it also renders |
|---|---|---|
| `terminal.enabled` | `config.terminal.enabled` (an instance setting: set it only to lock it) | the runner's stream listener :4108 and the NetworkPolicy flows to it; `config.terminal.available = true` |
| `relay.enabled` | `config.previews.enabled` | the preview relay workload, its Service and flows |
| `ssh.enabled` | `config.ssh.enabled` | the SSH terminator workload, its LoadBalancer and ops Services, the SSH NetworkPolicies, the generated SSH keys and the runner's :4108 |
| `scim.enabled` | `config.scim.enabled` (an instance setting: set it only to lock it) | the `/scim/v2` edge route; `config.scim.available = true` |
| `vault.*` | `config.secrets.stores` (the Vault entry) | service-account tokens and Vault wiring for api and gateway |
| `counters.*` | `config.ratelimit.counters` | the Redis connection and NetworkPolicy peers |
| `externalUrl` | `config.server.external_url` | ingress hosts, gateway TLS names, every service's public origin |
| `plugins.*` | `config.plugins.dir` | the package volume |

The limits and tuning of those features stay under `config:`, for example `config.terminal.awake_cap_secs` and `config.previews.*` caps. The full list of keys the chart injects is §The config block below.

**Everything else is under `config:`**, including e-mail, sign-in (`auth.*`, OIDC providers), LLM endpoints, governance, approvals, retention, `gateway.allowed_private` and hosted defaults.

**Secrets are never values.** A password goes in a Kubernetes Secret, referenced through an environment variable (`api.extraEnv`) whose name the key's row gives. Examples: `AURORA_SMTP_PASSWORD`, or `client_secret_env` for OIDC. From phase 5 (SET-*), it can also be entered in the dashboard.

**A typical install:**

```yaml
externalUrl: https://aurora.example.org

terminal:
  enabled: true            # top level: it adds a listener and network flows

config:
  email:                   # application setting: aurora.toml [email]
    smtp_url: "smtp://mail.example.org:587?tls=required"   # STARTTLS required; see the e-mail row
    from: aurora-noreply@example.org
    # smtp_username: aurora   # only with AURORA_SMTP_PASSWORD in api.extraEnv (from a Secret)
  terminal:
    awake_cap_secs: 3600   # a terminal limit: under config, while the switch stays top level

api:
  extraEnv: []             # e.g. AURORA_SMTP_PASSWORD from a Secret, when the server needs a login
```

From phase 5 (SET-1), setting a `config` key also **locks** it in the dashboard (§Instance settings and Helm locks). Leave a key out to let admins change it in Admin › Settings.

**Reading the tables.** "Value" is the YAML type (and the allowed values where
one is constrained — the chart fails the render on a wrong enum, and the
service refuses to start naming the key on a wrong value that reaches
`aurora.toml`). "Set it when" is the one-line reason to deviate from the
default. Links go to the section that explains the feature in depth.

**Three groups of values behave differently:**

1. **Chart keys** (`externalUrl`, `image.*`, `api.replicas`, …) shape the
   rendered Kubernetes manifests.
2. **`config.*`** is rendered **verbatim** into `aurora.toml` (the
   `aurora-api`/`aurora-gateway`/… configuration): every key of
   [15 §Configuration reference](15-deployment-operations.md#configuration-reference-keys-and-defaults)
   can be set here, plus the chart-injected keys listed in §The config block.
3. **Feature flags** (`vault.*`, `counters.*`, `previews.*`, `terminal.*`, …) are chart keys
   that additionally render TOML sections or whole workloads. They are listed
   here with their target docs.

Secret material never lives in values: every secret is generated by the
`secrets` hook (idempotent, never overwriting) or referenced from an existing
Secret. See
[15 §Required secrets](15-deployment-operations.md#required-secrets).

## The config block (`config.*`)

`config.*` maps 1:1 to `aurora.toml` sections. The chart injects some keys
itself (from the values above or from the release); **do not override these** —
a conflicting `config` entry is merged away or fails the render:

| Injected key | From | Note |
|---|---|---|
| `server.external_url` | `externalUrl` | the single public origin (`15 §Networking: one URL`) |
| `runner.backend` | — | always `kubernetes` on Helm |
| `runner.api_url` | — | `http://<fullname>-api:4101` |
| `runner.image` | `image.registry`, `image.tag`, `runner.sandboxImage` | derived (D-159); a `config.runner.image` you set wins (legacy) |
| `runner.k8s.*` | `sandboxes.namespace`, `runner.k8s.*` | namespace, core namespace, Service names, storage/runtime class, node selector, tolerations, pull secrets |
| `connector_host.api_url` | — | in-cluster API Service |
| `connector_host.test_types` | `connectorHost.testTypes` | |
| `api.gateway_internal_url` | — | `http://<fullname>-gateway:4103` (P4-GSL, D-154) |
| `database.max_connections` | `database.maxConnections.api` | |
| `database.gateway_role` | `database.gatewayRole` | |
| `gateway.ca_dir` | — | always `/var/lib/aurora/ca` |
| `gateway.proxy_tls.names` / `cert_file` / `key_file` | `gateway.proxy.tls.*` | Service names + `externalHost` + `names`; `existingSecret` sets the cert/key files |
| `gateway.trusted_proxies` | `gateway.proxy.trustedProxies` | |
| `gateway.insecure_loopback_injection` | **none, by design** | test-only (FIX-104, D-180); the chart refuses it even in `config`, and the gateway refuses it in a cluster |
| `plugins.dir` | `plugins.dir` | do not set it in `config` |
| `tls.extra_ca_bundle` | `extraCaBundle.*` | set when either `existingConfigMap` or `existingSecret` is set |
| `auth.dev_idp.*` | `devIdp.*` | only when `devIdp.enabled` |
| `config.terminal.available` | `terminal.enabled` | (phase 5, SET-5, D-237(d)) `true` when the top-level value is on, else not rendered; `config.terminal.enabled` is not mirrored any more — `config.terminal.enabled: true` without the top-level value fails the render, and an operator-set `config.terminal.enabled` (either value) locks the setting |
| `config.scim.available` | `scim.enabled` | (phase 5, SET-5, D-237(d)) `true` when the top-level value is on, else not rendered |
| `config.scim.rate_per_minute` | `scim.ratePerMinute` | rendered only when the value is set (it then locks the setting) |
| `ssh.enabled` / `[terminator]` | `ssh.enabled`, `terminator.max_preauth`, `.max_preauth_per_ip` | (phase 5, P4-SSH-2) `ssh.enabled` is mirrored from the top-level value; `config.ssh.enabled=true` alone, or both set and unequal, fails the render. With it on the chart also renders `[terminator]` (`api_url`, `listen = 0.0.0.0:2222`, `ops_port = 4194`, the two limits) — a `config.terminator.*` entry wins |
| `previews.enabled` | `relay.enabled` | mirrored from the top-level value (FIX-056): rendered only when the config side is unset; both set and unequal — or `config.previews.enabled` alone — fails the render |
| `ratelimit.counters` | `counters.*` | rendered only when `backend ≠ memory` |
| `secrets.stores[]` (Vault) | `vault.*` | api and gateway each get their own store config |

Everything else in `config` is passed through verbatim — the full key list and
their semantics live in
[15 §Configuration reference](15-deployment-operations.md#configuration-reference-keys-and-defaults)
(`auth.*`, `llm.*`, `governance.*`, `secrets.*`, `email.*`, `hosted.*`,
`previews.*`, `terminal.*`, `scim.*`, `push.*`, `blobs.*`, `retention.*`,
`attachments.*`, `byo.*`, `gateway.*`, `approvals.*`, `plugins.*`,
`ratelimit.api.*`, `events.*`, `telemetry.*`, …). Set them exactly as the
reference names them (TOML-style keys, e.g. `hosted:
{enabled_harnesses:
["claude-code"]}`).

## Instance settings and Helm locks (phase 5, SET-1, D-195(c), D-236..D-238)

Not built yet (SET-1 and the section briefs SET-2..SET-5, FIX-124). From then on most of the
`config.*` keys that tune behaviour — governance and BYO floors, LLM endpoints, sign-in and OIDC
providers, e-mail, secret stores, agent and hosted defaults, approvals, retention, skills, SCIM,
preview and terminal caps, API rate-limit tuning and `allowed_private` — are **instance settings**
edited on Admin › Settings. The full list, by section and Admin page, is
[15 §Instance settings](15-deployment-operations.md#instance-settings-phase-5-spec-settings-d-236d-238-sec-89-sec-90).

For each of those keys the values file is **optional**:

- **Unset** (the chart's default): the key is edited in the dashboard; its default is the
  application's (the *Default* column of `15`), never the chart's.
- **Set** under `config.*` (or through the chart value that renders it): the setting is
  **locked** — the dashboard shows "Configured in Helm (`config.<key>`)" and no edit control, and
  the API refuses changes (409 `setting_locked`). Lists lock as a whole, except the keyed lists
  `auth.oidc_providers`, `secrets.stores` and `config.llm.endpoints` (per entry `id`; the `vault.*`
  store is one such entry) and `ratelimit.api.classes` (per class): an entry you define is locked
  ("Managed by Helm"), admins add entries with other ids beside it, and an app-defined entry may not
  reuse a Helm entry's id. `config.llm.enabled_providers` and `config.llm.provider_preference` lock
  as whole values.
- **Removed later** from the values file: the setting moves to the dashboard **with the value
  Helm had** (the API keeps a copy of every locked value), so a `helm upgrade` that drops a key
  never silently reverts it. (phase 5, P4-LLM spec batch, D-262) One exception in effect, not in
  value: an LLM endpoint on a **private address** passes the gateway's destination guard
  implicitly only while it is defined here; before dropping such an endpoint from the values
  file, add its host to `gateway.allowed_private` (Admin › Limits, or `config.gateway.allowed_private`),
  otherwise agents' requests to it are refused `destination_refused` after the upgrade
  (Admin › LLM endpoints marks the entry `needs_allowed_private`).

**Upgrading an existing install changes nothing**: every key your values file sets stays locked;
the release notes and a banner on Admin › Settings list them. To hand a key to the dashboard,
delete it from the values file and upgrade.

Chart rules that keep this meaningful (SET-1, D-237(c)/(d)):

- `values.yaml` renders **no default for a movable key**: its `config` block keeps only
  infrastructure keys (`server.*`, binds, `telemetry.log_level`); movable keys appear only as
  comments with their documented default. The `config.hosted.enabled` example line is removed
  (the application default is the same), and the chart value `scim.ratePerMinute` defaults to
  unset (rendered as `config.scim.rate_per_minute` only when you set it; the application default is
  600). A chart guard renders the defaults and fails if any key of the movable-key list appears in
  `aurora.toml`.
- `terminal.enabled` and `scim.enabled` keep rendering their wiring (runner :4108 listener and
  NetworkPolicy flows; the `/scim` edge route) and render the Helm-only keys
  `config.terminal.available` / `config.scim.available` instead of mirroring the feature switch.
  The dashboard switch defaults to that value — the chart switch on still means the feature is on —
  and an admin can turn it off (or on again) live; it cannot be turned on while the chart switch is
  off.
- Secrets never move into values: dashboard-entered secrets are sealed in the database's inline
  store. A secret supplied through the environment (`client_secret_env` + `api.extraEnv`,
  `AURORA_SMTP_PASSWORD`) locks that field, as before.

What stays a Helm value or configuration-only key in any case: secrets and key rings, the database,
binds, ingress and Services, certificates, the runner and sandbox configuration (sizes and
`runner.max_sandboxes` included; D-235(a)), previews' switch and domain, counters, telemetry,
`push.*`, `blobs.*`, `auth.instance_admins` (the break-glass list) and every development switch.

## Global

| Value | Type / values | Default | What it does | Set it when / details |
|---|---|---|---|---|
| `externalUrl` | URL string | `https://aurora.example.org` | the single public origin, rendered as `server.external_url`; the ingress host and the dev-IdP loopback check derive from it | always (`15 §Networking: one URL`) |
| `image.registry` | string | `ghcr.io/alexiri` | registry of every service image; `""` = bare names (locally built) | a mirror or private registry ([install-helm §Images](operator/install-helm.md#images-publish-pull-or-build-and-load)) |
| `image.tag` | string | `""` (= `appVersion`) | tag of every service image **and** the derived sandbox image (D-159): one bump retargets everything | pinning a version |
| `image.pullPolicy` | `IfNotPresent` \| `Always` \| `Never` | `IfNotPresent` | Kubernetes pull policy for the service pods | `Always` for a moving tag in dev |
| `image.pullSecrets` | list of `{name}` | `[]` | pull Secrets for the **service** pods in the release namespace | private ghcr packages ([install-helm §Images](operator/install-helm.md#images-publish-pull-or-build-and-load)) |
| `hookImage` | image ref, digest-pinned | `alpine/k8s:1.31.2@sha256:d489…` | image of the `secrets`/`gateway-ca` hook Jobs and the `seed-plugins` init container; must provide `sh`, `cp`, `kubectl`, `base64`, `head`, `od`, `tr` | mirroring for air-gapped clusters (keep the `@sha256:`) (D-100) |
| `podDisruptionBudget.enabled` / `.minAvailable` | bool / int ≥ 1 | `true` / `1` | PDB for every workload | — |

## Secrets and keys

| Value | Type / values | Default | What it does | Set it when / details |
|---|---|---|---|---|
| `secrets.existingSecret` | Secret name | `""` | use a pre-existing Secret holding **all** required keys (`AURORA_ENCRYPTION_KEYS`, `AURORA_SIGNING_KEY`, `AURORA_INTERNAL_SECRET`, `AURORA_RUNNER_SESSION_KEY`, `AURORA_RUNNER_TOKEN`, `AURORA_CONNECTOR_TOKEN`, + optional active-key ids, + `AURORA_CONNECTOR_HOST_TOKEN` with the connector host, + `POSTGRES_PASSWORD` when `postgres.enabled`); the generator hook is not rendered | you manage the Secret (e.g. sealed-secrets) ([install-helm §Secrets](operator/install-helm.md#secrets)) |
| `secrets.generatedName` | Secret name | `<release>-aurora-secrets` | name of the generated Secret | naming conventions |
| `secrets.rotate.signing` / `.internal` | bool | `false` | on the **next** `helm upgrade`, append one entry to the `AURORA_SIGNING_KEY` / `AURORA_INTERNAL_SECRET` ring (never activates; a pending entry blocks another append); set the flag, upgrade, then follow the rotation doc | key rotation ([key-rotation.md](operator/key-rotation.md)) |
| `gateway.ca.existingSecret` | Secret name (`ca.pem`, `ca.key`) | `""` | reuse an operator CA for the proxy listener; empty: the `gateway-ca` hook mints one (never overwritten) | importing a CA ([gateway-posture §TLS proxy listener](operator/gateway-posture.md#tls-proxy-listener)) |
| `extraCaBundle.existingConfigMap` / `.existingSecret` / `.key` | names / string | `""` / `""` / `ca-bundle.pem` | mount a PEM bundle of extra trusted CAs read-only into every workload; rendered as `tls.extra_ca_bundle` (org CA in front of Vault, OIDC, an internal LLM endpoint); set ConfigMap **or** Secret, never both. (FIX-139) The connector host and every `connectors[]` workload also get `AURORA_TLS_EXTRA_CA_BUNDLE` pointing at the mounted file (the Mattermost connector reads that variable, not `aurora.toml`); no separate connector value exists | private CA in the chain (`15 §Configuration reference`, `tls.extra_ca_bundle`) |

## Database

| Value | Type / values | Default | What it does | Set it when / details |
|---|---|---|---|---|
| `database.existingSecret` | Secret name (key `AURORA_DATABASE_URL`) | `""` = `<release>-aurora-database` | Secret the api, hookhost and migrate Job read the URL from; the gateway too, unless the least-privilege pair below is set | always in production ([install-helm §Database](operator/install-helm.md#database)) |
| `database.maxConnections.api` / `.gateway` | int | `20` / `10` | `database.max_connections` per service | sized to your Postgres `max_connections` |
| `database.gatewayRole` + `database.gatewayExistingSecret` | role name + Secret name (key `AURORA_DATABASE_URL`) | `""` / `""` | least-privilege gateway login (FIX-016); **both or neither** (`helm` fails otherwise): `migrate` grants the role exactly the gateway privileges of `03`, the gateway logs in with the second Secret's URL. **Required with an external database** unless `database.allowSharedLogin` is set (FIX-106, D-178 Q-R5-11: `helm` refuses to render otherwise, with a message naming both ways out). With the bundled `postgres.enabled` and neither the pair nor `allowSharedLogin`, the chart creates the role `aurora_gateway` itself (a `<release>-gateway-role` pre-install/pre-upgrade hook Job, password generated into the generated Secret as `POSTGRES_GATEWAY_PASSWORD`, URL as `AURORA_GATEWAY_DATABASE_URL`) and the gateway logs in as it; with `secrets.existingSecret` you add those two keys yourself | DBA co-operation available ([install-helm §Database](operator/install-helm.md#database)) |
| `database.allowSharedLogin` | bool | `false` | the explicit, **not recommended** opt-in to the gateway sharing the main database login (every privilege the API has). Mutually exclusive with the pair (`helm` fails); with `postgres.enabled` it switches the automatic role off | never in production |
| `database.cidrs` / `.port` | CIDR list / int | `[]` / `5432` | NetworkPolicy egress to an **external** database (ignored when `postgres.enabled`) | external DB on a private range |
| `postgres.enabled` | bool | `false` | bundled single-node Postgres 16, **evaluation only**; its StatefulSet/Service/PVC are pre-install hook resources, so `helm uninstall` leaves them (NOTES.txt has the cleanup commands) | never in production ([install-helm §Database](operator/install-helm.md#database)) |
| `postgres.image` / `.storage` / `.storageClass` | image / quantity / class | `postgres:16` pinned by digest (FIX-103) / `5Gi` / `""` | the evaluation database's image, PVC size and class | evaluation only |
| `postgres.initScriptsConfigMap` | ConfigMap name | `""` | `*.sql`/`*.sh` run once on an empty data dir (the gateway role is created by the chart; an init script is for other objects, or for the explicit role pair) | evaluation |

## Workloads

Per-service keys (`<svc>.replicas`, `<svc>.resources`,
`<svc>.nodeSelector`, `<svc>.tolerations`, `<svc>.extraEnv`) exist for
`api`, `gateway`, `hookhost`, `web`, `runner`, `connectorHost` and the
per-connector entries of `connectors[]`; `extraEnv` supports `valueFrom`
(OIDC client secrets etc.) and is also given to the migrate Job.
[install-helm §Workloads](operator/install-helm.md#workloads) has the
ports-per-service table.

Request tracing (phase 5, FIX-146, D-232): the **web** workload gets the environment variable
`AURORA_TRACING_SAMPLE_RATIO` **only when `config.telemetry.otlp_endpoint` is set**, with the value of
`config.telemetry.sample_ratio` (default `1.0`, `0.0` is kept as `0.0`). The dashboard's root layout reads
it at request time; absent, the dashboard loads no tracing code and posts nothing. It is not a chart value
of its own: it follows the two `config.telemetry.*` keys (`15 §Configuration reference`). The collector's
egress is `networkPolicy.telemetryTo` ([§NetworkPolicy](#networkpolicy)).

| Value | Type / values | Default | What it does | Set it when / details |
|---|---|---|---|---|
| `<svc>.replicas` | int | api `2`, gateway `2`, hookhost `2`, web `2`, runner `1`, connectorHost `1` | Deployment replicas (`15 §Production` recommends ≥ 2 for the user-facing services; runner replicas share one runner registration) | — |
| `api.bootstrap.runnerToken` / `.connectorHostToken` | bool | `true` / `true` | inject `AURORA_BOOTSTRAP_RUNNER_TOKEN` / `AURORA_BOOTSTRAP_CONNECTOR_HOST_TOKEN` (from the instance Secret) into the api pods so a first runner/connector host registers itself | a runner registers via CLI instead |
| `gateway.proxy.trustedProxies` | CIDR list | `[]` | `gateway.trusted_proxies`: layer-4 LBs / NAT hops whose PROXY-protocol header on :4102 is trusted for BYO source checks (RFE-025) | a layer-4 LB sits in front ([gateway-posture §BYO agents](operator/gateway-posture.md#byo-agents-posture-and-source-allow-lists)) |
| `gateway.proxy.service.type` | `ClusterIP` \| `LoadBalancer` \| `NodePort` | `ClusterIP` | how :4102 is published; `LoadBalancer` exposes the proxy to external BYO agents and **bypasses the edge**; it must be layer 4 (never terminate TLS in front) | external BYO agents dial the cluster |
| `gateway.proxy.service.loadBalancerSourceRanges` | CIDR list | `[]` | restrict the proxy Service's sources | tighten BYO ingress |
| `gateway.proxy.service.annotations` | map | `{}` | annotations on the proxy Service | provider-specific LB config |
| `gateway.proxy.tls.externalHost` / `.names` | strings / list | `""` / `[]` | extra DNS names in the CA-issued proxy certificate (LoadBalancer hostname, extra Service names); rendered into `gateway.proxy_tls.names` | the proxy is reached under another name ([install-helm §Gateway proxy addresses](operator/install-helm.md#gateway-proxy-addresses)) |
| `gateway.proxy.tls.existingSecret` | `kubernetes.io/tls` Secret | `""` | operator certificate replacing the CA-issued leaf; must cover every name clients use | corporate CA for the proxy listener |
| `runner.enabled` | bool | `true` | render the runner Deployment (the only component that creates sandboxes) | an external runner registers instead ([runners.md](operator/runners.md)) |
| `runner.sandboxImagePullSecrets` | list of names | `[]` | `[runner.k8s] image_pull_secrets` — Secrets in the **sandboxes namespace** for the sandbox image (created by you, not the chart) | private sandbox registry ([runners §Private sandbox registry](operator/runners.md#private-sandbox-registry)) |
| `runner.sandboxImage` | image ref | `""` (= derived `<registry>/aurora-sandbox:<tag>`) | override the sandbox image only when it is genuinely different (D-159 derivation covers the normal case) | mirrored/custom sandbox image |
| `runner.k8s.storageClass` / `.runtimeClass` | string | `""` | StorageClass of sandbox home PVCs / RuntimeClass (gVisor, Kata recommended) | cluster has a hardened runtime ([runners §Kubernetes backend](operator/runners.md#kubernetes-backend)) |
| `runner.k8s.nodeSelector` / `.tolerations` | map / list | `{}` / `[]` | node selector and tolerations **of sandbox pods** | dedicated sandbox nodes |
| `connectorHost.enabled` | bool | `true` | the built-in connector host (phase 4, P4-MCON; Mattermost); `false` = no managed connectors (self-hosted `connectors[]` unaffected) | no managed connectors |
| `connectorHost.replicas` | int ≥ 1 | `1` | every replica runs every enabled managed connector | — |
| `connectorHost.perHostToken` | bool | `true` | (FIX-120, D-189) the hooks job generates a token bound to the connector types this Deployment runs (`mattermost`, plus `fake` with `testTypes`) into the instance Secret (`AURORA_CONNECTOR_HOST_TOKEN_BOUND`, and `AURORA_BOOTSTRAP_CONNECTOR_HOST_TOKENS` JSON for the api); the host presents it instead of the shared token, and the api then refuses the shared one (`15 §Upgrades`) | `false` keeps the instance-wide shared token |
| `connectorHost.testTypes` | bool | `false` | also announce the `fake` connector type + test control API on the ops port; the host warns at start; **dev/e2e only** | local testing ([install-helm §Connector host](operator/install-helm.md#connector-host)) |
| `connectors[]` | list of `{id, image, callbackPort (4190), tokenSecret: {name, key}, callbackIngress: {enabled, host, path}, replicas, resources, env}` | `[]` | self-hosted connectors the chart runs (types Aurora does not ship); each gets its own Deployment/Service/Ingress | a self-hosted connector ([install-helm §Connectors](operator/install-helm.md#connectors-optional)) |

## Vault

Renders a `hashicorp-vault` store into the api **and** a gateway-specific
`aurora-gateway.toml`; the store is additive — with `vault.enabled: false`
nothing is rendered and the inline store stays the default. Full setup:
[vault.md](operator/vault.md), keys: `15 §Configuration reference`
(`secrets.stores[]`). (phase 5, FIX-124 on SET-1) Stores are also instance settings: a store
rendered from `vault.*` is a locked entry of the keyed list `secrets.stores` (and `vault.makeDefault`
locks `secrets.default_store`); with `vault.enabled: false` stores are added on Admin › Secret
stores. The Kubernetes-auth token projection (`vault.auth.audience`, `.tokenExpirationSeconds`) and
`vault.auth.approle.existingSecret` stay chart values in either case.

| Value | Type / values | Default | What it does | Set it when / details |
|---|---|---|---|---|
| `vault.enabled` | bool | `false` | render the Vault store for api + gateway | using Vault ([vault.md §Helm](operator/vault.md#helm)) |
| `vault.storeId` | string, no `:` | `vault` | `secrets.stores[].id` (refs: `ext:<storeId>:…`); must not collide with a `config.secrets.stores[]` id (`helm template` fails) | multiple stores |
| `vault.address` | `https://host[:port]` | `""` (**required when enabled**) | Vault base URL, no path/query/credentials; `http://` only for loopback | always when enabled |
| `vault.kvMount` | string | `secret` | KV **v2** mount | non-default mount |
| `vault.managedPrefix` | string | `aurora` | where Aurora's managed values live in the mount | — |
| `vault.caFile` | PEM path in pods | `""` | extra trust anchor for **this store only** (private CA); for every service use `extraCaBundle` | private CA on Vault ([vault.md §TLS](operator/vault.md#tls)) |
| `vault.makeDefault` | bool | `false` | make this store `secrets.default_store` (new managed values go to Vault; follows the migration procedure) | migrating to Vault ([vault.md §Migrating](operator/vault.md#4-migrating-values)) |
| `vault.referenceScopes[]` | list of `{orgId (UUID), paths: [glob]}` | `[]` | which org may reference which paths (SEC-61); org **id**, never slug; `paths` relative to `kv_mount`, prefix globs end in a single `*` | orgs bring their own Vault paths ([vault.md §Reference scopes](operator/vault.md#reference-scopes-and-org-ids)) |
| `vault.auth.method` | `kubernetes` \| `approle` | `kubernetes` | how api/gateway authenticate to Vault (`token` is dev-only and not rendered) | AppRole outside k8s |
| `vault.auth.mount` | string | `""` = method name | Vault auth mount path | non-default mount |
| `vault.auth.apiRole` / `.gatewayRole` | string | `aurora-api` / `aurora-gateway` | Vault roles bound to the chart's api/gateway ServiceAccounts (kubernetes auth) | custom role names |
| `vault.auth.audience` | string | `vault` | audience of the projected token; must match the Vault role's | custom audience |
| `vault.auth.tokenExpirationSeconds` | int ≥ 600 | `3600` | projected-token TTL | tighter tokens |
| `vault.auth.approle.existingSecret` | Secret name (`API_ROLE_ID`, `API_SECRET_ID`, `GATEWAY_ROLE_ID`, `GATEWAY_SECRET_ID`) | `""` (**required for approle**) | where the AppRole credentials come from; exposed as `AURORA_VAULT_ROLE_ID` / `AURORA_VAULT_SECRET_ID` | approle auth |
| `vault.egress.cidrs` / `.port` | CIDR list / int | `[]` / `8200` | NetworkPolicy egress to an **in-cluster/private** Vault (a public address needs nothing extra) | in-cluster Vault |

## Shared rate-limit counters

Chart-side of P4-CNT (D-119): with `backend: memory` (the default) **nothing
extra renders** and per-replica counters keep today's behaviour. Full semantics
and failure policy: `15 §Shared rate-limit counters`, `06 §Rate limiting`.

| Value | Type / values | Default | What it does | Set it when / details |
|---|---|---|---|---|
| `counters.backend` | `memory` \| `postgres` \| `redis` | `memory` | shared `CounterStore` backend across replicas | N > 1 replicas and shared limits matter |
| `counters.api` / `.gateway` | `shared` \| `memory` | `shared` | whether the API limiter / the gateway rule quotas use the shared backend (or stay per-replica) | share only one limiter |
| `counters.onUnavailable.security` / `.management` / `.gateway` | `local` \| `closed` | `local` | per limiter class, behaviour when the backend is unreachable: keep local counting or refuse with 503 `counter_unavailable`; **there is no `open`** (the render fails; SEC-62) | fail closed for security classes |
| `counters.timeoutMs` | int 10–2000 | `50` | `COUNTER_TIMEOUT` per store call | slow shared backend |
| `counters.postgres.poolSize` | int 1–32 | `4` | dedicated pool of the postgres backend (separate from `database.max_connections`) | postgres backend |
| `counters.redis.address` | `rediss://host[:port]` | `""` (**required for redis**) | TLS Redis URL; no `@` credentials allowed (render fails); `redis://` only for loopback or with `allowPlaintext` | redis backend |
| `counters.redis.allowPlaintext` | bool | `false` | explicit opt-in for `redis://` to a non-loopback host | in-cluster Redis without TLS |
| `counters.redis.existingSecret` | Secret name (`REDIS_USERNAME`, `REDIS_PASSWORD`, both optional) | `""` | Redis ACL credentials (rendered as `*_env` names, never values) | authenticated Redis |
| `counters.redis.caFile` | PEM path | `""` | bundle verifying the Redis server (mount the CA via `extraCaBundle`) | private CA on Redis |
| `counters.redis.db` / `.keyPrefix` | int / string ≤ 64 | `0` / `aurora:rl:` | database index and key prefix (share one Redis between instances) | shared Redis |
| `counters.redis.networkPolicy.egressTo` | NetworkPolicy peers | `[]` | extra peers api/gateway may reach on the Redis port (default deny stays) | Redis on a private range |

## Routing

| Value | Type / values | Default | What it does | Set it when / details |
|---|---|---|---|---|
| `ingress.enabled` | bool | `true` | render the Ingress (edge rules: `/v1/*` → api, `/connectors` → connector host, everything else → web; `15 §Networking`) | using Ingress |
| `ingress.className` | string | `""` | IngressClass (e.g. `nginx`) | non-default controller |
| `ingress.host` | string | `""` = host of `externalUrl` | hostname of the dashboard Ingress | split DNS |
| `ingress.tls.secretName` | Secret name | `""` = no TLS block | TLS for the dashboard host | the edge terminates TLS |
| `ingress.annotations` | map | ingress-nginx set (64 m body, no buffering, 600 s timeouts, XFF overwrite off) | annotations tuned for SSE/long-poll; keep "no buffering" and the timeouts when you change them | other controllers |
| `gatewayApi.enabled` | bool | `false` | use Gateway API `HTTPRoute`s instead of Ingress (set with `ingress.enabled: false`) | a Gateway-API cluster |
| `gatewayApi.parentRefs` | list | `[]` | `parentRefs` of the routes | your Gateway name |
| `gatewayApi.hostnames` | list | `[]` = host of `externalUrl` | route hostnames | — |
| `gatewayApi.requestTimeout` | duration | `600s` | HTTPRoute request timeout (SSE/long-poll need long timeouts; the implementation must not buffer) | — |

## Port previews (relay)

The preview relay (P4-PRV-2, FIX-056; [15 §Port previews](15-deployment-operations.md#port-previews-phase-4-p4-prv-1-p4-prv-2-d-144d-147-sec-80sec-82-sec-85),
[11 §Port previews](11-runner-sandboxes.md)). `relay.enabled` is the **single switch**: it renders the relay
Deployment, Service and PodDisruptionBudget, the wildcard `*.<relay.domain>` edge rules (relay only) and the relay
NetworkPolicy, and mirrors `config.previews.enabled`. The relay's `[relay]` section is chart-owned. The relay may
reach only the api (:4101), the runners' stream listener (:4108) and DNS — it has **no** external egress (FIX-069).
The generic per-service keys `<svc>.replicas`, `.resources`, `.nodeSelector`, `.tolerations` and `.extraEnv`
([§Workloads](#workloads)) apply to `relay` too; their relay defaults are in the table.

| Value | Type / values | Default | What it does | Set it when / details |
|---|---|---|---|---|
| `relay.enabled` | bool | `false` | the single previews switch: relay workload + wildcard edge + policies; mirrors `config.previews.enabled` — set only this; both set and unequal, or `config.previews.enabled: true` alone, fails the render | enabling port previews |
| `relay.domain` | DNS suffix | `""` (**required when `relay.enabled`**; the render fails without it) | the preview DNS suffix: sessions are served at `<label>.<relay.domain>`; must equal `config.previews.domain` (the relay refuses a mismatch after registering) | always with `relay.enabled`; needs wildcard DNS and a wildcard certificate for `*.<domain>` |
| `relay.replicas` | int ≥ 1 | `1` | relay Deployment replicas | previews carry load or must survive a node loss |
| `relay.resources` | resources | requests `100m` / `128Mi`, memory limit `512Mi` | relay pod resources | many concurrent streams |
| `relay.nodeSelector` / `relay.tolerations` | map / list | `{}` / `[]` | scheduling of the relay pods | a dedicated edge node pool |
| `relay.extraEnv` | list of env entries | `[]` | extra environment for the relay container (`valueFrom` supported) | corporate proxy / tracing variables |
| `relay.max_streams` | int | `2048` | `[relay] max_streams`: concurrent proxied streams per relay replica | tuning edge capacity |
| `relay.max_streams_per_user` | int | `64` | `[relay] max_streams_per_user`: the per-user share of them | tuning fairness |
| `relay.request_body_max_bytes` | bytes | `33554432` (32 MiB) | `[relay] request_body_max_bytes`: request-body cap on a preview request | previews that upload large bodies |
| `relay.trusted_proxies` | CIDR list | `[]` | `[relay] trusted_proxies` (same semantics and default as `server.trusted_proxies`): the edge proxies / ingress pods in front of the relay. When the socket peer is inside one, the visitor's address is the rightmost untrusted `X-Forwarded-For` entry; without it the peer address is the client address (every visitor behind an ingress would share one grant-redemption bucket). `/0` is refused | an ingress or load balancer sits in front of the relay |

## SSH access (terminator)

The SSH terminator (P4-SSH-2, D-239, C-797; [15 §SSH access](15-deployment-operations.md#ssh-access-phase-5-p4-ssh-1-p4-ssh-2-d-239-sec-92sec-96),
[16 §W20](16-wire-protocols.md)). `ssh.enabled` is the **single switch**: it renders the `aurora-terminator`
Deployment, its LoadBalancer Service (published port → 2222) and a headless ops Service (4194), a
PodDisruptionBudget (from two replicas) and the terminator NetworkPolicy, adds the api ingress from the terminator and
the runner ingress on :4108, opens the runner's :4108 beside previews and the terminal, makes the `secrets` hook
generate `AURORA_TERMINATOR_TOKEN`, `AURORA_SSH_CA_KEY` and `AURORA_SSH_HOST_KEY` when absent (never overwriting;
with `secrets.existingSecret` create them yourself), mounts the CA key into the **api pods only** (and the `migrate` hook Job, which runs the API image and
validates the same configuration) and the host key into the **terminator pods only**, and mirrors `config.ssh.enabled`. **SSH needs the web terminal enabled, because the sandbox boot document carries the exec capability only with it**
(`domains/runner_plane/boot.rs`: `terminal` is added only when `terminal.enabled`; the supervisor refuses every exec
without it) — an open follow-up is to widen that gate so SSH can run without the web terminal. The render **fails**
when `ssh.enabled` is set without `runner.enabled`, without `terminal.enabled` or — while `networkPolicy.enabled` — with an empty `networkPolicy.sshFrom`; and when
`config.ssh.enabled=true` stands alone or disagrees. The terminator has no service-account token, no database URL and no
Vault; the sandboxes namespace gains nothing. The generic per-service keys (`replicas`, `resources`, `nodeSelector`,
`tolerations`, `extraEnv`) apply to `terminator` too.

| Value | Type / values | Default | What it does | Set it when / details |
|---|---|---|---|---|
| `ssh.enabled` | bool | `false` | the single SSH switch (above); mirrors `config.ssh.enabled` — set only this | turning SSH access on (`config.ssh.public_host` is the name people connect to) |
| `terminator.replicas` | int ≥ 1 | `2` | terminator Deployment replicas (each holds its own connections; a node loss cuts only its own) | one replica on a small cluster |
| `terminator.resources` | resources | requests `100m` / `128Mi`, memory limit `512Mi` | terminator pod resources | many concurrent sessions |
| `terminator.nodeSelector` / `terminator.tolerations` | map / list | `{}` / `[]` | scheduling of the terminator pods | an edge node pool |
| `terminator.extraEnv` | list of env entries | `[]` | extra environment for the terminator container (`valueFrom` supported) | tracing variables |
| `terminator.service.type` | `LoadBalancer` \| `NodePort` | `LoadBalancer` | the published SSH Service type | `NodePort` on kind (`deploy/kind`, host port 2222) |
| `terminator.service.port` | int | `22` | the published SSH port (container port 2222) | set `config.ssh.public_port` to the same value when it is not 22 |
| `terminator.service.nodePort` | int / null | `null` | with `type: NodePort`: the fixed node port | kind maps 2222 |
| `terminator.service.annotations` | map | `{}` | annotations of the SSH Service (cloud load-balancer options) | a cloud load balancer |
| `terminator.service.loadBalancerSourceRanges` | CIDR list | `[]` | the balancer's source ranges (no restriction when empty; `networkPolicy.sshFrom` still applies) | restricting at the balancer |
| `terminator.service.externalTrafficPolicy` | `Local` \| `Cluster` | `Local` | keeps the client address for the per-address limits; with a balancer that cannot keep it every client shares one address | `Cluster` when `Local` is not available (then raise `terminator.max_preauth_per_ip`) |
| `terminator.max_preauth` / `terminator.max_preauth_per_ip` | int | `256` / `8` | `[terminator] max_preauth` / `max_preauth_per_ip` (`SSH_PREAUTH_MAX`, `SSH_PREAUTH_PER_IP`): unauthenticated connections in total and per client address | many clients behind one address |

## SCIM

| Value | Type / values | Default | What it does | Set it when / details |
|---|---|---|---|---|
| `scim.enabled` | bool | `false` | the instance switch for the SCIM plane (P4-GRP-2, SEC-72): `true` routes `/scim` to the API in the Ingress and the HTTPRoute; `false` renders no `/scim` route and the API answers 404 for the whole surface | directory sync (IdP → Aurora provisioning; [15 §SCIM directory sync](15-deployment-operations.md)). (phase 5, SET-5, D-237(d)) (phase 5, SET-5, D-237(d)) It renders `config.scim.available = true` (nothing else of `[scim]`); the SCIM switch itself is an instance setting (Admin › Settings › Directory sync) defaulting to it, and a `config.scim.enabled` you set locks it |
| `scim.ratePerMinute` | int | unset | `[scim] rate_per_minute`: the per-SCIM-token limit (burst is a fifth of it; the application default is 600). (phase 5, SET-5) Unset by default: edit it on Admin › Settings › Directory sync; a set value is rendered and locks the setting | an IdP that syncs in large bursts |

## Web Push

(phase 5, P4-011B-3; RFE-011b, D-134, D-266(a), C-837; `15 §Approval park-and-resume and Web Push`, SEC-75, SEC-76.) Off by default. The api sends end-to-end encrypted, fixed-field pushes to the browser vendors' push services, so it needs outbound HTTPS to them; nothing else of push is a chart value.

| Value | Type / values | Default | What it does | Set it when / details |
|---|---|---|---|---|
| `push.enabled` | bool | `false` | the single switch: renders `config.push.enabled` (mirrored like `relay.enabled`: both set and unequal, or `config.push.enabled=true` alone, fails the render) and the api's egress NetworkPolicy rule to `push.networkPolicy.egressTo` (TCP 443) | deciders should be woken in the browser when an approval waits and no Aurora tab is open |
| `push.networkPolicy.egressTo` | list of NetworkPolicy peers | `[]` | the peers the api may reach on TCP 443; **required** while `push.enabled` (the render fails while empty); default deny stays. The push services are public hosts, so the usual value is an explicit `0.0.0.0/0` with the private ranges in `except` (or your egress proxy) | always with `push.enabled` |
| `push.vapid.existingSecret` | string | unset | a Secret holding an operator-managed VAPID private key (PKCS#8 PEM or the base64url raw 32-byte P-256 scalar) under the key `push.vapid.key`; mounted into the api pods **and the migrate Job** (which checks the same configuration) as `AURORA_PUSH_VAPID_KEY`, and `config.push.vapid.private_key_env` names it. Unset = Aurora generates its own key (first api replica) and seals it in the api's own store; `aurora-api push rotate-vapid` then rotates it, and is refused (exit 3) for an operator key | your security policy wants the signing key in your secret manager; rotate by replacing the Secret and restarting |
| `push.vapid.key` | string | `AURORA_PUSH_VAPID_KEY` | the key inside that Secret | the Secret uses another key name |
| `push.allowedEndpointHosts` | list | `[]` (the application's list: FCM, Mozilla, Apple, Windows) | rendered into `config.push.allowed_endpoint_hosts` only when set; re-checked before every send | narrowing or adding a push service |

`config.push.vapid.subject` (a `mailto:` or `https:` contact; the default is `server.external_url` when it is https), `config.push.concurrency` and `config.push.timeout_secs` are config keys (`15`). `config.push.dev_endpoint_overrides` (a loopback test seam) is **never rendered**: the chart refuses it, and so does the config layer inside a cluster. None of this is an instance setting (D-237(b)).

## Plugin packages

| Value | Type / values | Default | What it does | Set it when / details |
|---|---|---|---|---|
| `plugins.source` | `none` \| `configMap` \| `pvc` | `none` | where the api pods find `*.aurora-plugin` packages: no volume, a ConfigMap (its keys are package files, 1 MiB Kubernetes limit; an init container copies them into an emptyDir) or an existing PVC | operator-supplied plugin packages ([15 §Production](15-deployment-operations.md)) |
| `plugins.dir` | path | `/var/lib/aurora/plugins` | rendered into `[plugins] dir`; the package volume is mounted here (do not set it in `config`) | almost never |
| `plugins.configMapName` | ConfigMap name | `""` | the ConfigMap for `source: configMap` (restart the api pods after changing it; no auto-reload) | `source: configMap` |
| `plugins.pvcName` | PVC name | `""` | the PersistentVolumeClaim for `source: pvc` (every api replica scans it: use ReadWriteMany when `api.replicas` > 1) | `source: pvc` |

## Sandboxes namespace

| Value | Type / values | Default | What it does | Set it when / details |
|---|---|---|---|---|
| `sandboxes.namespace` | DNS label | `aurora-sandboxes` | namespace of all sandbox Pods (rendered into `runner.k8s.namespace`) | naming conventions |
| `sandboxes.create` | bool | `true` | chart creates the namespace, labelled with the PSA labels `pod-security.kubernetes.io/{enforce,audit,warn}: restricted` | the namespace exists already |
| `sandboxes.quota.pods` / `.requestsCpu` / `.requestsMemory` / `.limitsCpu` / `.limitsMemory` / `.pvcs` / `.storage` | quantities | `50` / `50` / `100Gi` / `100` / `200Gi` / `100` / `1Ti` | ResourceQuota of the sandboxes namespace (all sandboxes together) | sized to `runner.max_sandboxes` × sandbox size ([runners §Sizing](operator/runners.md#sizing)) |
| `sandboxes.seccomp.install` | bool | `true` | DaemonSet that copies the sandbox seccomp profile (runtime default minus `ptrace`, `process_vm_readv/writev`; D-394, SEC-137) into the kubelet's seccomp directory on the sandbox nodes (`runner.k8s.nodeSelector`/`tolerations`) | you provide `<hostDir>/aurora/sandbox-seccomp.json` on every sandbox node yourself; without it no sandbox pod starts ([runners §Sandbox seccomp profile](operator/runners.md)) |
| `sandboxes.seccomp.namespace` | DNS label | `aurora-seccomp` | namespace of that DaemonSet, created with Pod Security `privileged` (it writes a hostPath) | naming conventions |
| `sandboxes.seccomp.hostDir` | path | `/var/lib/kubelet/seccomp` | the kubelet's seccomp directory on the node | a kubelet with a different `--root-dir` |
| `sandboxes.limitRange.defaultRequest` / `.default` / `.max` | CPU/mem maps | `250m/512Mi` / `1/2Gi` / `8/16Gi` | LimitRange defaults and maxima **per sandbox pod** (a sandbox that fits only under `max` needs `runner.sandbox_*` lowered) | smaller sandboxes |

## NetworkPolicy

Default-deny in both namespaces, then explicit allows. Everything here only
**adds** egress/ingress peers to the policies of
[install-helm §NetworkPolicies](operator/install-helm.md#networkpolicies);
nothing opens the sandboxes to the cluster.

| Value | Type / values | Default | What it does | Set it when / details |
|---|---|---|---|---|
| `networkPolicy.enabled` | bool | `true` | render any policies at all (leave on unless the cluster forbids them) | never off in production |
| `networkPolicy.edgeNamespaceSelector` | label selector | `kubernetes.io/metadata.name: ingress-nginx` | which namespace's pods may reach api:4101, web:4100, connector callbacks | another ingress namespace |
| `networkPolicy.dnsNamespaceSelector` | label selector | `kube-system` | DNS egress | custom DNS |
| `networkPolicy.kubeApiCidrs` / `.kubeApiPorts` | CIDR list / port list | `[]` (= the addresses of the `kubernetes` EndpointSlice in `default`, read at install/upgrade; no rule where unreadable) / `[443, 6443]` | runner and `secrets` hook → Kubernetes API egress | pinned API-server endpoints; any cluster where the installing identity cannot read endpointslices |
| `networkPolicy.kubeApiOpen` | bool | `false` | explicitly allow any:`kubeApiPorts` for the runner and the `secrets` hook (the pre-FIX-102 default); never silent | only when the API server addresses cannot be pinned |
| `networkPolicy.telemetryTo` | list of raw NetworkPolicy `to` entries | `[]` | (phase 5, FIX-146, D-232, C-732 (5)) where the OTLP collector named by `config.telemetry.otlp_endpoint` lives. Rendered as one egress-only policy `<release>-telemetry` for the exporting workloads: api, gateway, hookhost, runner, relay, connector host and the connector Deployments. Never the web workload (its browser spans go through the API) and never the sandboxes namespace (sandboxes and the supervisor export nothing). Empty (default): no policy, so the default-deny policies block the collector | set it whenever `config.telemetry.otlp_endpoint` is set; `NOTES.txt` warns when an endpoint is set and this is empty. The kind dev stack sets it in `deploy/kind/values-tracing.yaml` (`just kind-up --tracing`) |
| `networkPolicy.monitoringFrom` | list of NetworkPolicy peers | `[]` | peers allowed to scrape every ops/metrics port (4101, 4103, 4104, 4106, 4192, 4193, 4194); nothing opens them by default | Prometheus in another namespace |
| `networkPolicy.sshFrom` | list of NetworkPolicy peers | `[]` (**required when `ssh.enabled` and `networkPolicy.enabled`**: the render fails while empty) | (phase 5, P4-SSH-2) who may reach the terminator's port 2222. The terminator's egress is the api (:4101), the runner (:4108) and DNS only — no Postgres, gateway, sandboxes or Internet | admitting everyone is explicit: `[{ipBlock: {cidr: 0.0.0.0/0}}, {ipBlock: {cidr: "::/0"}}]` |
| `monitoring.podMonitor.enabled` / `.interval` / `.labels` | bool / duration / map | `false` / `30s` / `{}` | one Prometheus Operator `PodMonitor` per component (ops listeners have no Service); needs the Prometheus Operator CRDs (API group monitoring.coreos.com) | Prometheus Operator |
| `secrets.allowRegenerate` | bool | `false` | let an **upgrade** create fresh encryption keys when the generated Secret is absent (otherwise the hook refuses: stored credentials would become undecryptable) | deliberately starting over after a restore without the Secret |
| `networkPolicy.cilium.enabled` | bool | `false` | add CiliumNetworkPolicies for runner and `secrets` hook → kube-apiserver (Cilium matches those as entities, not ipBlocks) and admit the `ingress` entity on the edge ports | Cilium cluster (`15 §Production`) |
| `networkPolicy.cilium.edgeEntities` | list of Cilium entities | `[ingress]` | entities admitted on the edge ports (host-network ingresses: `[host, remote-node]`) | host-network ingress |
| `networkPolicy.externalEgress.cidr` / `.except` | CIDR / list | `0.0.0.0/0` / private ranges | external egress (OIDC, SMTP, LLM endpoints, push services) for api/gateway/hookhost/connectors; `except` carves private ranges out (they need explicit rules) | lock egress down |
| `networkPolicy.gatewayExtraEgress` | raw NP `egress` entries | `[]` | appended to the gateway policy (a private-range LLM endpoint; an endpoint defined under `config.llm.endpoints` passes the gateway's destination guard implicitly, one added in the dashboard must also be in `gateway.allowed_private`, D-262) | private LLM endpoints |
| `networkPolicy.apiExtraEgress` | raw NP `egress` entries | `[]` | appended to the api policy. Empty in real deployments (the external-egress rule excludes the private ranges: SSRF protection); the kind dev profile `values-llm-mock.yaml` opens the in-cluster LiteLLM mock so key checks and grader tests can reach it | dev stack only |
| `networkPolicy.connectorHostExtraEgress` | raw NP `egress` entries | `[]` | appended to the connector host's policy (a chat platform on a private range) | private Mattermost |
| `privateHosts.names` | list of host names / `*.suffix` | `[]` | (FIX-200, D-437) internal-only hosts (names resolving to private addresses) Aurora may reach, by name: each joins `config.gateway.allowed_private` (anything set there is kept; the list then shows read-only in Administration › Limits, D-237), and with `networkPolicy.enabled` a CiliumNetworkPolicy `<release>-private-hosts` lets the gateway, API and hook host reach them on `privateHosts.ports` (its DNS rule only routes their lookups through Cilium's DNS proxy, to `dnsNamespaceSelector`). Needs `networkPolicy.cilium.enabled` (render fails otherwise: a NetworkPolicy cannot match names). Lower-case names only, at least two labels after `*.`; addresses and CIDRs are refused (use `config.gateway.allowed_private` and ipBlocks). In Cilium `*` stays within one label; list a CNAME's target too | a self-hosted app or addon host on the intranet (`asm.cern.ch`) |
| `privateHosts.ports` | list of TCP ports | `[443]` | the ports of `privateHosts.names` the policy opens | a service on another port |
| `terminal.enabled` | bool | `false` | the web terminal (P4-TERM, C-622): says the deployment provides the terminal — it renders `config.terminal.available = true` (never `config.terminal.enabled`, an instance setting; `config.terminal.enabled=true` without this value fails the render, because only this value renders the chart wiring), opens the runner's stream listener :4108 beside previews (either switch opens it) and the NetworkPolicy flows api → runner:4108 (the terminal's leg 1, C-617) and relay/api → runner:4108. While off the API answers 404 `terminal_disabled` and nothing renders. The limits (`terminal.awake_cap_secs`, …) are instance settings (Admin › Settings › Terminal), locked only by a `config.terminal.*` you set. (phase 5, SET-5, D-237(d)) The feature switch is an instance setting defaulting to `terminal.available` (the dashboard can turn the terminal off or on again live; it cannot turn it on while this value is off) | the Terminal tab is wanted |

## Development IdP

**Development only.** The chart refuses `devIdp.enabled` unless `externalUrl`
is a loopback origin, and so does the API at startup
([sign-in.md §Development IdP](operator/sign-in.md#development-idp-development-only),
`04 §Development and end-to-end sign-in`).

| Value | Type / values | Default | What it does | Set it when / details |
|---|---|---|---|---|
| `devIdp.enabled` | bool | `false` | deploy `aurora-dev-idp` and register the `dev` OIDC provider (`auth.dev_idp`) | local kind/dev clusters |
| `devIdp.image` | image ref | `""` = `<registry>/aurora-dev-idp:<tag>` | the IdP image | local build |
| `devIdp.pathPrefix` | URL path | `/dev-idp` | ingress path forwarding to the IdP (`""` = at the root) | — |
| `devIdp.issuer` | URL | `""` = `externalUrl` + pathPrefix | issuer URL as the **browser** reaches it (the internal URL is set by the chart) | — |
| `devIdp.allowAnyHint` | bool | `false` | sign in any `login_hint`, even unlisted ones | e2e harnesses |
| `devIdp.users[]` | list of `{sub, email, name, email_verified}` | three dev users | the users the login page offers | your dev users |
| `devIdp.resources` | resources | small | pod resources | — |

## What is not a Helm value

Many `aurora.toml` keys look like chart values but are **not** — they go in
`config.*` (see §The config block) or are set per org in the dashboard:

- Feature toggles with their own config sections: previews
  (limits `previews.session_ttl_secs`, …; the switch is the chart value
  `relay.enabled`, which mirrors `config.previews.enabled`), the web terminal
  (limits `terminal.awake_cap_secs`, … are instance settings; the chart value
  `terminal.enabled` renders `config.terminal.available`), SSH access
  (phase 5: `ssh.public_host`, `ssh.cert_ttl_secs`, `ssh.ca.previous_public_keys`, … in
  `config.ssh.*`; the switch is the chart value `ssh.enabled`,
  [§SSH access (terminator)](#ssh-access-terminator)), SCIM
  (`scim.enabled`, …), Web Push (`push.*`),
  the S3 blob store (`blobs.*`), approval parking (`approvals.park.*`) —
  set these in `config`, the sections' keys and limits are in
  [15 §Configuration reference](15-deployment-operations.md#configuration-reference-keys-and-defaults)
  with their sections below (`§Port previews`, `§SCIM directory sync`, `§Approval park-and-resume and Web Push`, `§S3-compatible blob store`).
- Everything an org sets in the dashboard (postures, allow-lists, previews
  org toggle, SCIM per org): `04 §Permission matrix`, `11 §Port previews`.
- Image build-time overrides (`SANDBOX_BASE_IMAGE`, `SANDBOX_HARNESSES`):
  [deployment-defaults.md](operator/deployment-defaults.md),
  [harnesses.md](operator/harnesses.md).

## Values planned for later phases

These Helm keys are **specified** (with config keys in `15`) but not yet
implemented in the chart — a brief names them so the chart work lands with the
feature:

- previews: `previews.domain` is a **config** key (set via `config.previews.domain`, `15 §Port previews`); the chart values are `relay.*` ([§Port previews (relay)](#port-previews-relay)). There are no `previews.tls.*` chart values: the wildcard certificate is the operator's own edge configuration (the chart renders no TLS block for `*.<relay.domain>`), and the relay's egress is fixed (api, runners, DNS).
- web terminal: `terminal.awake_cap_secs`, `terminal.wake_window_secs`, `terminal.wake_wait_secs` (P4-TERM, D-161; `15`, `11 §Web terminal` — note: instance settings since SET-5, set via `config.terminal.*` only to lock them; the deployment switch is the chart value `terminal.enabled`, above — it renders `config.terminal.available`)
- SCIM: `scim.enabled` (renders `config.scim.available`) and `scim.ratePerMinute` (rendered only when set) are implemented ([§SCIM](#scim)); the remaining SCIM keys (`scim.allowed_email_domains`, `scim.token_max_days`) are instance settings (`config.scim.*` locks them, `15 §SCIM directory sync`)
- Web Push (phase 5, P4-011B-3): implemented, see [§Web Push](#web-push).
- Approval parking (phase 5, P4-011B): **no chart values** — `approvals.park.*` are instance settings edited in Admin › Approvals (D-265(c)); set `config.approvals.park.*` only to lock them.
- S3 blob store (phase 6, D-318; not in the chart yet): `blobs.s3.existingSecret`, `blobs.s3.networkPolicy` (P4-S3; `15 §S3-compatible blob store`)

- instance settings (phase 5, SET-1, SET-5; [§Instance settings and Helm locks](#instance-settings-and-helm-locks-phase-5-set-1-d-195c-d-236d-238)): `scim.ratePerMinute` defaults to unset; `terminal.enabled` and `scim.enabled` render `config.terminal.available` / `config.scim.available` instead of the feature switch (built in SET-5; `scripts/guards/chart-ops.sh` checks the rendering); `values.yaml`'s `config` block loses `hosted.enabled`; a chart guard (`scripts/guards/settings-chart.sh`) checks that the default render contains no movable key. No new chart value.

Until a feature's chart work merges, set its `config.*` keys directly (the
chart passes `config` through verbatim).
