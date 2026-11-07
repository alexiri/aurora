# 15 — Deployment and Operations

Phase: **0**. Artifacts: `deploy/compose/` (podman compose, development and
single host) and `deploy/helm/aurora/` (Kubernetes, production).

## Images

One OCI image per component, all tagged with the Aurora version
(`AURORA_VERSION` pins all at once):

| Image | Contents |
|-------|----------|
| `aurora-api` | `aurora-api` (also the migrations job: `aurora-api migrate`) |
| `aurora-gateway` | `aurora-gateway` |
| `aurora-hookhost` | `aurora-hookhost` |
| `aurora-runner` | `aurora-runner` |
| `aurora-web` | Next.js standalone build |
| `aurora-connector-fake` | test/dev only |
| `aurora-connector-mattermost` | phase 1 |
| `aurora-sandbox` | sandbox image (`11 §Sandbox image`) — not a service; referenced by the runner |

Service images are distroless (`gcr.io/distroless/cc` or equivalent), run as
non-root, contain only their binary, and are multi-arch (amd64, arm64).

## Configuration

Every service reads one TOML file (`AURORA_CONFIG`, default
`/etc/aurora/aurora.toml`) and environment overrides
`AURORA__<SECTION>__<KEY>` (double underscores, uppercase). Secrets SHOULD come
from environment variables (or files referenced as `*_file`), never be baked
into images. Each service validates the sections it uses at startup and
refuses to start on any error, naming the key.

### Required secrets

| Variable | Used by | Format |
|----------|---------|--------|
| `AURORA_DATABASE_URL` | api, gateway | Postgres URL (separate roles recommended: API owner role; gateway least-privilege role) |
| `AURORA_ENCRYPTION_KEYS`, `AURORA_ENCRYPTION_ACTIVE_KEY` | api, gateway | `08 §3` |
| `AURORA_SIGNING_KEY`, `AURORA_SIGNING_ACTIVE_KEY` | api | 32 bytes base64 (phase 0). From phase 2 (RFE-007): a ring `id:base64,id2:base64` + the active id (a bare base64 value remains valid as ring id `v0`); rotation procedure `08 §4` |
| `AURORA_INTERNAL_SECRET`, `AURORA_INTERNAL_ACTIVE_KEY` | api, gateway, hookhost | ≥32 random bytes base64 (phase 0). From phase 2 (RFE-007): a ring `id:base64,…` + the active id; every service holding it must have all entries before the active id changes (`08 §4`) |
| `AURORA_RUNNER_SESSION_KEY` | api, runner | 32 bytes base64 |
| `AURORA_RUNNER_TOKEN` | runner | `aur_…` |
| `AURORA_CONNECTOR_TOKEN` | each connector | `aun_…` |
| `AURORA_BOOTSTRAP_RUNNER_TOKEN` (optional) | api | on startup, upsert runner `default` with this token's hash (compose/Helm bootstrap) |
| `AURORA_BOOTSTRAP_CONNECTOR_TOKENS` (optional) | api | `id:aun_…,id2:aun_…` — upsert connector ids (still require manifest approval unless `plugins.allow_unsigned` dev mode auto-approves) |
| OIDC client secrets, SMTP password | api | referenced from config (`client_secret_env = "…"`) |

Setup tooling generates every secret and **never overwrites** an existing one.
(phase 2, RFE-007) For the two rings it only **appends** new entries on request
(`aurora-api keys generate`); Helm's `secrets` hook supports `existingSecret` rings
and a values flag `secrets.rotate.signing` / `secrets.rotate.internal` that appends
one new entry without activating it.

### Configuration reference (keys and defaults)

| Section.key | Default | Services | Meaning |
|-------------|---------|----------|---------|
| `server.external_url` | — (required) | api, web, gateway | public origin, e.g. `https://aurora.example.org` |
| `server.bind` | `0.0.0.0` | all | listen address |
| `gateway.internal_bind` | `server.bind` | gateway | address for the internal listener :4103; must not be sandbox-reachable (SEC-09); startup warns if it overlaps `server.bind` |
| `runner.ops_bind` | `server.bind` | runner | address for the ops listener :4106; same rule |
| `server.trusted_proxies` | `[]` | api | CIDRs whose `X-Forwarded-For` is trusted |
| `database.max_connections` | 20 (api), 10 (gateway) | api, gateway | pool size |
| `database.gateway_role` | — | api (migrations) | role to grant gateway privileges (`03`) |
| `auth.signup_mode` | `invite_only` | api | `open`, `invite_only`, `sso_jit` |
| `auth.org_creation` | `instance_admins` | api | or `anyone` |
| `auth.instance_admins` | `[]` | api | emails |
| `auth.trusted_origins` | `[]` | api | extra origins for CSRF checks |
| `auth.session_idle_secs` / `auth.session_absolute_secs` | 1209600 / 2592000 | api | |
| `auth.oidc_providers[]` | — | api | `04 §OIDC`; (phase 1, RFE-004) **at least one required** (or `auth.dev_idp.enabled`), `require_email_verified` removed |
| `auth.dev_idp.enabled`, `auth.dev_idp.issuer` | `false`, — | api | (phase 1, RFE-004) registers the development provider `dev` served by `aurora-dev-idp`; refused on a non-loopback https `server.external_url` (`04 §Development and end-to-end sign-in`) |
| `email.smtp_url`, `email.from` | — | api | unset ⇒ log-only mode; used for invitations and (phase 2, RFE-011a) approval notifications |
| `secrets.default_store`, `secrets.stores[]` | `inline` | api, gateway | `08 §2` |
| `llm.enabled_providers`, `llm.provider_preference`, `llm.endpoints[]` | `["litellm","anthropic"]` | api, gateway | `08 §6`; each endpoint may set `allowed_models` and `key_origin` (`method` = `endpoint` \| `litellm_team` \| `provider_account`, with `allowed_organization_ids`, `allowed_workspace_ids`, `allowed_team_ids`) |
| `governance.llm.allowed_key_scopes` | `["organization","workspace"]` | api, gateway | key scopes any org may permit (`08 §11`) |
| `governance.llm.minimum_posture` | `open` | api, gateway | `open` or `sanctioned_only` — floor for every org |
| `governance.llm.require_key_origin` | `false` | api, gateway | when `true`, startup fails unless every endpoint declares a `key_origin` |
| `governance.llm.known_llm_hosts` | built-in list (below) | api, gateway | model-API hosts without a configured endpoint; `known_llm_hosts_extra` appends |
| `governance.egress.minimum_posture` | `open` | api, gateway | `open` or `credentialed_only` — floor for every org |
| `governance.egress.baseline_allowlist` | `[]` | api, gateway | `[{ host, path? }]` merged into every org's egress allow-list |
| `governance.egress.open_lane_minimum` | `enabled` | api, gateway | (phase 1, RFE-003) `enabled` \| `allowlist_only` \| `disabled` — floor of the open-lane posture for every org and workspace; `allowlist_only` uses the baseline allow-list (set one) |
| `agents.default_name` | `Daneel` | api | default agent and shared-bot name |
| `agents.default_brief` | (text in `10`) | api | |
| `hosted.enabled` | `true` | api | hosted agents available |
| `hosted.default_harness` | `claude-code` | api | |
| `hosted.allow_fake_harness` | `false` | api | tests/dev |
| `hosted.default_timezone` | `UTC` | api | for new workspaces |
| `hosted.idle_stop_secs`, `hosted.background_cap_secs`, `hosted.turn_ceiling_secs`, `hosted.turn_stall_secs` | `02 §Limits` | api | |
| `gateway.advertised_proxy_url` | derived from external host + `:4102` | api, gateway | what BYO agents are told; (phase 1, RFE-002) always `https://host:port` — startup refuses an `http://` value |
| `gateway.proxy_tls.names` | `[]` | gateway | (phase 1, RFE-002) extra DNS names/IPs for the CA-issued proxy certificate (LoadBalancer hostname, extra service names) |
| `gateway.proxy_tls.cert_file`, `gateway.proxy_tls.key_file` | — | gateway | (phase 1, RFE-002) optional operator-supplied proxy certificate chain + key (both or neither); replaces the CA-issued leaf for the outer leg |
| `gateway.allowed_private` | `[]` | gateway | private destinations allowed |
| `gateway.egress_proxy` | — | gateway | upstream forward proxy |
| `gateway.condition_body_bytes` | 262144 | gateway | 4 KiB–8 MiB |
| `gateway.upstream_header_timeout_secs` | 300 | gateway | |
| `gateway.ca_dir` | `/var/lib/aurora/ca` | gateway | `ca.pem`, `ca.key` |
| `gateway.hookhost_url`, `api.hookhost_url` | `http://aurora-hookhost:4104` | gateway, api | |
| `approvals.timeout_secs` | 300 | gateway | default approval deadline (rules without their own timeout) |
| `approvals.max_timeout_secs` | 3600 | api, gateway | (phase 2, RFE-011a) upper bound of per-rule/per-grant timeouts, 300–14400, must be < `hosted.turn_ceiling_secs − 900` or startup fails |
| `approvals.email_notifications` | `true` | api | (phase 2, RFE-011a) e-mail deciders when an approval has no live surface (needs `email.smtp_url`; log-only otherwise) |
| `plugins.dir` | `/var/lib/aurora/plugins` | api | scanned packages |
| `plugins.allow_unsigned` | `false` | api | dev only (warning banner) |
| `hookhost.api_internal_url` | `http://aurora-api:4111` | hookhost | snapshot/blob source |
| `runner.api_url` | — | runner | e.g. `http://aurora-api:4101` |
| `runner.backend` | — (required) | runner | `podman`, `kubernetes`, `fake` (config, never detection) |
| `runner.image` | `aurora-sandbox:<version>` | runner | |
| `runner.advertised_host` | — | runner | sandbox-facing host for the WS URL |
| `runner.max_sandboxes`, `runner.held_awake_ceiling` | 4, derived | runner | |
| `runner.sandbox_memory_mib`, `runner.sandbox_cpus`, `runner.sandbox_pids`, `runner.home_gib` | 2048, 1, 512, 10 | runner | defaults |
| `runner.reconcile_interval_secs`, `runner.orphan_grace_secs`, `runner.orphan_reap` | 60, 3600, `reap` | runner | `reap` or `log_only` |
| `runner.capacity_wait_secs` | 300 | runner | |
| `runner.podman_socket`, `runner.sandbox_network`, `runner.sandbox_hosts` | — | runner (podman) | `11` |
| `runner.k8s.namespace`, `runner.k8s.core_namespace` (namespace holding the gateway/runner Services; default: the runner pod's own namespace, else `default`), `runner.k8s.storage_class`, `runner.k8s.runtime_class`, `runner.k8s.node_selector`, `runner.k8s.tolerations`, `runner.k8s.gateway_service`, `runner.k8s.runner_service` | — | runner (kubernetes) | `11` |
| `tls.extra_ca_bundle` | — | api, gateway, hookhost, runner, connectors | PEM file of additional trusted CAs for all outbound TLS (e.g. an organization CA for internal LLM endpoints, OIDC, Vault) |
| `events.delta_bus` | `notify` | api | (phase 1, RFE-016) `DeltaBus` implementation id; unknown id fails startup |
| `events.delta_coalesce_ms` | 100 | api | (phase 1, RFE-016) delta coalescing window, 100–500 |
| `retention.request_log_days` | 30 | api | |
| `attachments.default_org_quota_mib`, `attachments.org_quota_mib_max` | 10240, 51200 | api | (phase 2, RFE-015) default and ceiling of an org's attachment storage quota (MiB; `0` = unlimited) |
| `attachments.days_max` | — (no ceiling) | api | (phase 2, RFE-015) instance maximum for an org's attachment retention window |
| `retention.conversation_days_max` | — (no ceiling) | api | (phase 2, RFE-010) instance maximum for an org's conversation retention window (tighten-only, 7..3650) |
| `telemetry.otlp_endpoint` | — | all | optional tracing export |
| `telemetry.log_level` | `info` | all | |

### Governance example (corporate LLM accounts only, strict egress)

```toml
[[llm.endpoints]]
id = "central"
provider = "litellm"
base_url = "https://llm.example.org"
key_origin = { method = "endpoint" }

[[llm.endpoints]]
id = "anthropic"
provider = "anthropic"
key_origin = { method = "provider_account", allowed_organization_ids = ["<corporate org uuid>"] }

[governance.llm]
allowed_key_scopes = ["organization", "workspace"]   # users may hold their own keys…
minimum_posture = "sanctioned_only"                  # …but only verified corporate ones carry prompts
require_key_origin = true

[governance.egress]
minimum_posture = "credentialed_only"
baseline_allowlist = [ { host = "pypi.org" }, { host = "files.pythonhosted.org" },
                       { host = "registry.npmjs.org" }, { host = "github.com" },
                       { host = "codeload.github.com" }, { host = "*.githubusercontent.com" } ]
```

Built-in `known_llm_hosts` (maintained with releases): `api.openai.com`,
`chatgpt.com`, `generativelanguage.googleapis.com`, `*-aiplatform.googleapis.com`,
`*.openai.azure.com`, `*.services.ai.azure.com`, `bedrock-runtime.*.amazonaws.com`,
`api.mistral.ai`, `api.cohere.com`, `api.cohere.ai`, `api.groq.com`,
`api.together.xyz`, `api.fireworks.ai`, `api.deepseek.com`, `api.x.ai`,
`api.perplexity.ai`, `openrouter.ai`, `api.anthropic.com` (when no
`anthropic` endpoint is configured). Hosts that are also endpoint hosts are
removed from the list automatically.

## Networking: one URL

- `server.external_url` is the single public origin. The **edge** routes
  `/v1/*` → API :4101 and everything else → web :4100. `/internal/*` and
  `/metrics` are never routed.
- Edge requirements: request body ≥ 32 MiB (attachments), no buffering for SSE
  and long-polls (`/v1/conversations/*/stream`, `/v1/approvals/*`,
  `/v1/runner/work`, `/v1/connector/ops/claim`), read timeouts ≥ 60 s, HTTP/2,
  and it MUST **set** (overwrite) `X-Forwarded-For` / `X-Forwarded-Proto`.
- The gateway proxy port 4102 is raw TCP and bypasses the edge. In-cluster and
  sandbox agents use it directly; external BYO agents need a TCP (layer-4,
  **TLS passthrough**) load balancer or NodePort. (phase 1, RFE-002) The
  listener is **TLS-only** for everyone; there is no plaintext variant and the
  "VPN recommended" caveat of phase 0 is dropped. Do not terminate TLS in front
  of it unless the front end re-encrypts to the gateway; the load balancer's
  hostname MUST be listed in the certificate (`gateway.advertised_proxy_url`
  host or `gateway.proxy_tls.names`).
- Runners and connectors may run outside the cluster: they only need outbound
  HTTPS to `external_url` (`/v1/runner/*`, `/v1/connector/*` are routed by the
  edge). A connector's own platform-callback port is exposed only to its
  platform.
- A CI test checks that the documented edge configs (Caddyfile, Helm Ingress
  and HTTPRoute templates) route every API path prefix and dashboard page
  correctly.

## Development: podman compose (`deploy/compose/`)

Files: `compose.yaml`, `Caddyfile`, `aurora.toml`, `.env.example`, `setup.sh`.

`setup.sh` (idempotent): generates `.env` secrets (never overwriting), creates
the `aurora-sandbox` network (`podman network create --internal --disable-dns
--subnet 10.89.250.0/24 --gateway 10.89.250.1 aurora-sandbox`, see S-03) if missing, mints the gateway CA into
the fixed-name `aurora-ca` volume with `aurora-gateway generate-ca --out /ca` if the volume has none (the
volume root is first chowned to the gateway's nonroot uid, because a new volume is root-owned; an existing
CA is never overwritten), and prints the first-admin bootstrap command (phase 1, RFE-004:
`aurora-api bootstrap-admin --email <e>` pre-provisions the admin; the person then
signs in through the configured OIDC provider or, in development, the `dev-idp`
profile).

Services:

| Service | Image | Networks | Notes |
|---------|-------|----------|-------|
| `postgres` | `postgres:16` | core | volume `aurora-pg` |
| `migrate` | aurora-api `migrate` | core | one-shot; `api` depends on its successful completion |
| `api` | aurora-api | core | |
| `gateway` | aurora-gateway | core, sandbox (`10.89.250.2`) | volume `aurora-ca`; publishes 4102 (TLS-only from phase 1; the CA-issued leaf covers `aurora-gateway`, `10.89.250.2` and `localhost`/`127.0.0.1` for host-side tools) |
| `hookhost` | aurora-hookhost | core | |
| `web` | aurora-web | core | |
| `edge` | `caddy:2` | core | publishes 4000 → routes as above |
| `runner` (profile `hosted`) | aurora-runner | core, sandbox (`10.89.250.3`) | mounts the podman socket (`${XDG_RUNTIME_DIR}/podman/podman.sock` for rootless) with `security_opt: [label=disable]` |
| `connector-fake` (profile `connector-fake`) | aurora-connector-fake | core | test control API published on 4190 |
| `dev-idp` (profile `dev-idp`) | aurora-dev-idp | core | (phase 1, RFE-004) development OIDC provider (`04`), users from `deploy/compose/dev-idp/users.json`, published on `127.0.0.1:4410`; development only |
| `vidaimock` (profile `llm-mock`) | `ghcr.io/vidaiuk/vidaimock` pinned by digest | core | scripted Anthropic-API stand-in, config `deploy/compose/vidaimock/` mounted read-only; **not a model** |
| `litellm` (profile `llm-mock`) | `ghcr.io/berriai/litellm` pinned by digest | core | real LiteLLM proxy, `deploy/compose/litellm.yaml`, dev master key `sk-aurora-mock-dev`; published on `127.0.0.1:4400`; endpoint `litellm-mock` in `aurora.toml` |

Networks: `core` (bridge) and `sandbox` (external, created by `setup.sh`,
internal, DNS disabled). `COMPOSE_PROFILES=hosted` enables hosted agents.
`AURORA_BOOTSTRAP_RUNNER_TOKEN` / `AURORA_BOOTSTRAP_CONNECTOR_TOKENS` wire the
runner and fake connector without manual provisioning.

`COMPOSE_PROFILES=hosted,llm-mock` adds a mock model behind a real LiteLLM (decision I-30) so hosted
`claude-code` agents work without a model account; see `docs/operator/llm-endpoints.md` (§Running with the
mock) and `scripts/e2e/llm.sh`. The dev master key is a dummy; the mock only replays scenario files.

Developer loop without containers: `just dev` runs Postgres in podman and the
API, gateway, hook host and web as host processes; the runner (podman backend)
is optional. The runner deliberately has no auto-restart file watcher:
restarting it invalidates in-flight turns (bootstrap tokens are single use).

## Production: Helm (`deploy/helm/aurora/`)

- Kubernetes ≥ 1.30, a NetworkPolicy-enforcing CNI, a default StorageClass
  supporting `ReadWriteOnce` (and `allowVolumeExpansion` recommended).
- **Namespaces**: release namespace (core services) and `sandboxes`
  (configurable).
- **Deployments**: api (≥2 replicas), gateway (≥2), hookhost (≥2), web (≥2),
  runner (≥1; more replicas share one runner registration), optional
  connectors. PodDisruptionBudgets for each. All pods run as non-root with
  read-only root filesystems and dropped capabilities.
- **Jobs / hooks**:
  - `secrets` (pre-install): generates missing Secrets idempotently, never
    overwrites; supports `existingSecret` values instead.
  - `gateway-ca` (pre-install): runs `aurora-gateway generate-ca` and stores
    the CA as a Secret via the Kubernetes API (or uses `gateway.ca.existingSecret`).
  - `migrate` (pre-install/pre-upgrade hook).
- **Services**: api (4101, and 4111 internal), web, gateway proxy (4102,
  optionally `LoadBalancer` for external BYO agents), gateway internal (4103),
  hookhost (4104), runner (4105, only reachable from the sandboxes namespace).
  (phase 1, RFE-002) The gateway proxy Service port 4102 is TLS: the chart
  renders `gateway.proxy_tls.names` from the Service names plus the configured
  LoadBalancer/NodePort hostname, and an `existingSecret` option for an
  operator certificate; the optional `LoadBalancer` MUST be layer 4 (no TLS
  termination).
- **Routing**: Ingress or Gateway API `HTTPRoute` (values toggle) implementing
  the edge rules.
- **NetworkPolicies**: default deny in both namespaces, then:
  sandboxes → gateway:4102 and runner:4105 only (no DNS); api → postgres,
  hookhost, gateway:4103 (status), external egress (OIDC, SMTP, OAuth
  providers, LLM endpoints for model listing, secret stores); gateway →
  postgres, hookhost, external egress; hookhost → api:4111 and external egress
  (hooks' fetch is additionally filtered in-process); runner → api, Kubernetes
  API; connectors → api, their platform.
- **Sandboxes namespace**: ResourceQuota, LimitRange, PSA `restricted`, runner
  ServiceAccount + Role (`11 §kubernetes`).
- **Postgres**: external database expected in production (values: URL secret,
  gateway role); a bundled single-instance Postgres subchart is available for
  evaluation only.
- **Values highlights**: image registry/tag, replicas, resources, external
  URL, auth config, OIDC providers, LLM endpoints, secret stores, runner
  backend settings (storage class, runtime class, node selectors), connectors
  (list of `{id, image, token secret, callback ingress}`), plugin packages
  (ConfigMap or PVC mounted at `plugins.dir`).
- Helm and compose consume the **same** `aurora.toml` structure (Helm renders
  it into a ConfigMap).

## Upgrades

- Migrations are forward-only and run before any new API replica serves;
  every migration MUST be compatible with the previous API and gateway version
  still running (expand → migrate → contract across two releases).
- Wire contracts: API supports runner/connector/supervisor protocol versions N
  and N−1; plugin manifest contracts N and N−1; registry snapshots
  "newest contract wins" (`09 §9`).
- Order: migrate → api → hookhost → gateway → runner → web → connectors.
  Gateways and runners of the previous version keep working during the roll.
- Running sandboxes keep their image until their next boot; the dashboard shows
  "restart to update" when an agent's boot image differs from the runner's.

## Observability

- Health: `/healthz` (liveness: process up) and `/readyz` (ready: DB reachable,
  snapshot loaded, listener connected) on every service.
- Metrics: Prometheus `/metrics` per service (internal listener). Required
  series: HTTP requests by route/status/latency (api), gateway decisions and
  latencies, holds pending, refresh/mint outcomes, due-work arm durations and
  items, sandboxes by state, turns by status, inbox/ops depth and age,
  connector replicas live, hook calls/errors/duration, snapshot hash info.
- Logs: JSON to stdout, correlation ids per `02 §Coding rules`.
- Tracing: optional OTLP export; `traceparent` propagated API ↔ gateway-internal
  ↔ hookhost ↔ runner ↔ connector calls.

## Backups and state

Stateful: Postgres (system of record), per-agent home volumes (PVC snapshots /
volume backups are the operator's job), the gateway CA Secret, the encryption
keyring and signing key (phase 2: the signing-key and internal-secret **rings**,
including retired entries still inside their rotation window). **Secrets are
unrecoverable without the keyring.**

## Sizing

Runner hosts: `max_sandboxes × sandbox memory + 1 GiB`. Held-awake sandboxes
occupy a slot permanently: size for steady state, not peak. Postgres: modest;
`request_log` dominates (≈300 bytes/request; partitioned and pruned).

### Attachment storage (phase 2, RFE-015)

Attachments live in Postgres (`blobs`, `bytea`) until the S3-compatible BlobStore
exists, so they share the database's disk and backups. Budget for
`Σ org quotas` (default 10 GiB each) in the worst case, expect real use to be far
lower, and prefer short retention windows. `GET /v1/admin/storage` shows where the
bytes are; the Postgres size alert should fire well before the disk is full
(attachment growth is bounded per org by the quota, not globally — raise or
lower `attachments.org_quota_mib_max` to bound the sum).

### Streaming fan-out ceiling (phase 1, RFE-016)

Streaming text takes the `DeltaBus` (`05 §The delta bus seam`); with the default
`notify` implementation every delta is a Postgres `NOTIFY`, serialized in the
commit path and delivered to **every** API replica. Back-of-envelope, for
planning (to be re-measured by `scripts/perf/delta_bus.sh`, an acceptance item of
the phase-1 task, on the target Postgres):

| Quantity | Value |
|----------|-------|
| Load per **concurrently streaming turn** | ≤ 10 notifications/s (100 ms coalescing), ≤ 2 000 chars each → ≤ ≈20 KB/s into Postgres, and the same **per API replica** (`R`× fan-out) |
| Comfortable region | ≤ ~1 000 notifications/s total ≈ **100 concurrently streaming turns** (p99 publish < 50 ms on a modest managed Postgres) |
| Ceiling before the NOTIFY queue/lock dominates | ~2 000–3 000 notifications/s ≈ **200–300 concurrently streaming turns**; beyond this commit latency of *all* writers rises |
| Replica effect | each replica's listener must absorb the full stream: at 100 streaming turns ≈ 2 MB/s of payload per replica regardless of `R` |

A turn streams only while the model emits text, so 100 *concurrent streams*
typically corresponds to several hundred *active* agents. Watch
`aurora_delta_publish_seconds` p99, `aurora_delta_dropped_total` and
`aurora_pg_notify_queue_usage` (alert at 0.5; at 1.0 NOTIFY fails). Operators
approaching the ceiling have three levers without code changes: raise coalescing
(`events.delta_coalesce_ms`, default 100, up to 500), run fewer API replicas per
streaming-heavy deployment, or switch `events.delta_bus` to a later
implementation when available.
