# 15 — Deployment and Operations

Phase: **0**. Artifacts: `deploy/helm/aurora/` (Kubernetes, production) and, for development
and integration testing, the same chart on a local kind cluster (`deploy/kind/`, `deploy/dev/`).
(phase 1, OPS-017) `deploy/compose/` (podman compose) is removed; see §Development.

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
| `aurora-sandbox` | sandbox image (`11 §Sandbox image`) — not a service; referenced by the runner. (phase 3, RFE-019) Built on the public official AlmaLinux image by default; a private or mirrored base is a **build-time** override (`SANDBOX_BASE_IMAGE`, see below), not a runtime configuration key |

(phase 1, OPS-023) **Dev images** (`localhost/aurora-dev/*`, `localhost/aurora-e2e/*`) are built from a debug-profile binary in the kind builder container into thin distroless images, for local kind runs only; they are never pushed, and the released images above come only from `images/services/Containerfile` (release profile, sccache-accelerated, `docs/ops/build-performance.md`).

(phase 1, OPS-020) **Registry and tags.** CI builds and publishes every image to
`ghcr.io/alexiri/aurora-<name>` (`api`, `gateway`, `hookhost`, `runner`, `web`,
`connector-fake`, `connector-mattermost`, `sandbox`, `sandbox-min`) with `GITHUB_TOKEN`, only after the
lint, test, e2e, web and guard jobs of the same workflow run are green. Pushes to `master` and `v*` tags
publish; pull requests build without pushing. Tags: `sha-<short>` (every publish), `master` (moving),
`<version>` and `<major>.<minor>` from a `vX.Y.Z` git tag, and `latest` only for the newest
non-prerelease semver tag. Images carry OCI labels (source, revision, version, license
`GPL-3.0-or-later`) plus SBOM and max-mode provenance attestations. The Helm chart defaults to
`image.registry: ghcr.io/alexiri` with the chart `appVersion` as tag. The six service images are copies of a
single compile of the workspace (`images/services/Containerfile`, cargo-chef dependency layer, lld).
`aurora-sandbox-min` is the slim e2e sandbox (OS + supervisor).

Service images are distroless (`gcr.io/distroless/cc` or equivalent), run as
non-root, contain only their binary, and are built for linux/amd64. arm64 images are deferred
to phase 5 at the earliest (OPS-022): there is no arm deployment planned.

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
| `AURORA_BOOTSTRAP_RUNNER_TOKEN` (optional) | api | on startup, upsert runner `default` with this token's hash (Helm bootstrap) |
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
| `auth.dev_idp.enabled`, `auth.dev_idp.issuer`, `auth.dev_idp.internal_url` | `false`, —, `issuer` | api | (phase 1, RFE-004) registers the development provider `dev` served by `aurora-dev-idp`; `issuer` is the URL the browser reaches, `internal_url` (optional) where the API itself reaches the IdP (discovery, token, JWKS; the chart sets it to the Service); refused unless `server.external_url` is on `localhost`, `*.localhost`, `127.0.0.1` or `[::1]` (`04 §Development and end-to-end sign-in`) |
| `email.smtp_url`, `email.from` | — | api | unset ⇒ log-only mode; used for invitations and (phase 2, RFE-011a) approval notifications. `email.from` (an e-mail address) is required when `email.smtp_url` is set, or startup fails |
| `secrets.default_store`, `secrets.stores[]` | `inline` | api, gateway | `08 §2` |
| `secrets.stores[].kind = "hashicorp-vault"` | — | api, gateway | (phase 3, `21 §Phase 3 — HashiCorp Vault secret store`, C-370..C-375) a Vault KV v2 store; `id` follows the existing rule (non-empty, no `:`, unique). `secrets.default_store` may name it (managed values are then written to Vault). Both services build the store from their own configuration, so each may use its own `auth` role (recommended: the gateway reads managed and reference paths and writes managed connection-credential paths only) |
| `secrets.stores[].address` | — (required) | api, gateway | Vault base URL, `https://host[:port]` with no path, query or credentials; `http://` is accepted only for a loopback host (dev server); anything else refuses startup naming the key |
| `secrets.stores[].ca_file` | unset (system roots) | api, gateway | PEM bundle that verifies the Vault server certificate; must be readable at startup |
| `secrets.stores[].auth.method` | — (required) | api, gateway | `kubernetes` (service-account JWT), `approle` (role id + secret id from the environment) or `token` (development; startup logs a warning); any other value refuses startup |
| `secrets.stores[].auth.mount` | the method name (`kubernetes` / `approle`) | api, gateway | auth mount path of the method (not used by `token`); non-empty, no leading or trailing `/` |
| `secrets.stores[].auth.role` | — | api, gateway | Vault role; required for `kubernetes`, refused for other methods |
| `secrets.stores[].auth.jwt_path` | `/var/run/secrets/kubernetes.io/serviceaccount/token` | api, gateway | `kubernetes` only: file holding the service-account JWT, re-read at every login |
| `secrets.stores[].auth.role_id_env`, `secrets.stores[].auth.secret_id_env` | — | api, gateway | (names follow `client_secret_env`) `approle` only, both required: names of environment variables holding the role id and secret id; an unset or empty variable refuses startup naming the key |
| `secrets.stores[].auth.token_env` | — | api, gateway | `token` only, required: name of the environment variable holding the token |
| (renewal) | — (no key) | api, gateway | the store renews its token itself at two thirds of the token TTL and logs in again when renewal fails or the token is not renewable; failure to obtain a valid token makes every operation fail `StoreUnavailable` (503 `secret_store_unavailable`) until it succeeds |
| `secrets.stores[].kv_mount` | `secret` | api, gateway | KV **version 2** mount; non-empty, no leading or trailing `/`. Reference paths (C-371) are relative to it |
| `secrets.stores[].managed_prefix` | `aurora` | api, gateway | managed values live at `<kv_mount>/data/<managed_prefix>/<org_id>/<purpose>/<owner_id>` (fixed layout; only the prefix is configurable); 1..128 chars, segments of `[A-Za-z0-9._-]`, no `.`/`..`/empty segment. The mount's `max_versions` SHOULD be ≥ 10 |
| `secrets.stores[].reference_scopes[]` | `[]` (no org may reference) | api, gateway | which org may reference which paths (SEC-61; phase 3, D-107): entries `{ org_id, paths: [glob, …] }`. `org_id` is the organization's **immutable id** (a UUID string; anything else refuses startup naming the key), never its slug: a slug can be changed, freed and taken by another org, which must never inherit a scope. An id that matches no org grants nothing. Operators find an org's id with `GET /v1/admin/orgs` (instance admin; `id`, `slug`, `name` per org, C-203) or, as an org member, `GET /v1/resolve?org=<slug>` (`org.id`). `paths` non-empty, each glob either an exact path or a prefix ending in a single trailing `*` (the phase-0 `memory` store rule; `*` anywhere else is refused), relative to `kv_mount`, no leading `/`, no `.`/`..`/empty segment; a glob equal to, inside or covering `<managed_prefix>/` refuses startup (no reference can reach another org's managed values). Several entries for one id are unioned. Matched against the secret's org id at every resolve and open |
| `llm.enabled_providers`, `llm.provider_preference`, `llm.endpoints[]` | `["litellm","anthropic"]` | api, gateway | `08 §6`; each endpoint may set `allowed_models` and `key_origin` (`method` = `endpoint` \| `litellm_team` \| `provider_account`, with `allowed_organization_ids`, `allowed_workspace_ids`, `allowed_team_ids`) |
| `governance.llm.allowed_key_scopes` | `["organization","workspace"]` | api, gateway | key scopes any org may permit (`08 §11`) |
| `governance.llm.minimum_posture` | `open` | api, gateway | `open` or `sanctioned_only` — floor for every org |
| `governance.llm.require_key_origin` | `false` | api, gateway | when `true`, startup fails unless every endpoint declares a `key_origin` |
| `governance.llm.known_llm_hosts` | built-in list (below) | api, gateway | model-API hosts without a configured endpoint; `known_llm_hosts_extra` appends |
| `governance.egress.minimum_posture` | `open` | api, gateway | `open` or `credentialed_only` — floor for every org |
| `governance.egress.baseline_allowlist` | `[]` | api, gateway | `[{ host, path? }]` merged into every org's egress allow-list |
| `governance.egress.open_lane_minimum` | `enabled` | api, gateway | (phase 1, RFE-003) `enabled` \| `allowlist_only` \| `disabled` — floor of the open-lane posture for every org and workspace; `allowlist_only` uses the baseline allow-list (set one) |
| `agents.default_name` | `Daneel` | api | default agent and shared-bot name |
| `agents.default_brief` | (neutral text in `10`; phase 3, RFE-019: was the Daneel persona in phase 0–2) | api | brief given to the default agent of **new** workspaces; ≤20 000 chars; empty = the default; never changes existing agents |
| `hosted.enabled` | `true` | api | hosted agents available |
| `hosted.default_harness` | `claude-code` | api | |
| `hosted.allow_fake_harness` | `false` | api | tests/dev |
| `hosted.default_timezone` | `UTC` | api | for new workspaces |
| `hosted.idle_stop_secs`, `hosted.background_cap_secs`, `hosted.turn_ceiling_secs`, `hosted.turn_stall_secs` | `02 §Limits` | api | |
| `gateway.advertised_proxy_url` | derived from external host + `:4102` | api, gateway | what BYO agents are told; (phase 1, RFE-002) always `https://host:port` — startup refuses an `http://` value |
| `gateway.sandbox_proxy_url` | `runner.backend = kubernetes`: `https://aurora-gateway:4102`; otherwise the advertised proxy URL | api, gateway | (FIX-010, D-87) the proxy address hosted sandboxes dial (the boot document's `gateway.proxy` / `server_name`, `11`); `https://host[:port]` only; its host is added to the proxy certificate; set it only when the in-cluster Service is reached by another name (a mesh, a different DNS name), never for BYO agents |
| `gateway.proxy_tls.names` | `[]` | gateway | (phase 1, RFE-002) extra DNS names/IPs for the CA-issued proxy certificate (LoadBalancer hostname, extra service names) |
| `gateway.proxy_tls.cert_file`, `gateway.proxy_tls.key_file` | — | gateway | (phase 1, RFE-002) optional operator-supplied proxy certificate chain + key (both or neither); replaces the CA-issued leaf for the outer leg |
| `governance.agent_tokens.max_age_days` | unset (never expire) | api | (phase 3, RFE-013) optional maximum age of BYO agent tokens (1…3 650 days); orgs may set a smaller value; applied at mint and, with a 7-day grace, to existing tokens when introduced or tightened (`08 §7`) |
| `gateway.allowed_private` | `[]` | gateway (and the API, for app-instance probes and connect requests only, D-74) | private destinations allowed |
| `gateway.resolve_cache_ttl_secs` | 60 | gateway | resolution-cache TTL (`02 §Limits`); (phase 3, RFE-014: also the **outage tolerance** and the revocation-lag bound while the database or listener is down; above 300 the gateway logs a warning at start; `06 §Operation during a Postgres outage`) |
| `gateway.trusted_proxies` | `[]` | gateway | (phase 2, RFE-025) CIDRs of layer-4 load balancers / NAT hops that send a PROXY protocol v1/v2 header on the proxy port; the client address for BYO source restriction is taken from that header only for peers in this list (`06 §BYO admission`). Not the same as `server.trusted_proxies` (API, `X-Forwarded-For`) |
| `byo.enabled` | `true` | api, gateway | (phase 2, RFE-025) master switch for BYO agents: `false` ⇒ BYO creation refused and every `aug_`/BYO `auc_` credential refused (403 `byo_disabled`), regardless of org/workspace settings. Hosted agents unaffected; changed by restart |
| `byo.minimum_posture` | `owners_only` | api, gateway | (phase 2, RFE-025) `enabled` \| `owners_only` \| `disabled` — floor of the BYO posture for every org and workspace (`08 §11.9`); set `enabled` to keep phase 0/1 behaviour |
| `byo.default_source_cidrs` | `[]` | api, gateway | (phase 2, RFE-025) default source CIDR list of every BYO agent without its own or an org list (e.g. corporate networks and CI runners); orgs and agents may only narrow it. Empty = unrestricted |
| `byo.require_context_credentials` | `false` | api, gateway | (phase 2, RFE-025) when `true` every BYO agent must use context credentials for credentialed requests (`08 §11.9`) |
| `gateway.egress_proxy` | — | gateway | upstream forward proxy |
| `gateway.condition_body_bytes` | 262144 | gateway | 4 KiB–8 MiB |
| `gateway.upstream_header_timeout_secs` | 300 | gateway | |
| `gateway.ca_dir` | `/var/lib/aurora/ca` | gateway | `ca.pem`, `ca.key` |
| `gateway.hookhost_url`, `api.hookhost_url` | `http://aurora-hookhost:4104` | gateway, api | |
| `approvals.timeout_secs` | 300 | gateway | default approval deadline (rules without their own timeout); must not exceed `approvals.max_timeout_secs`, or startup fails |
| `approvals.max_timeout_secs` | 3600 | api, gateway | (phase 2, RFE-011a) upper bound of per-rule/per-grant timeouts, 300–14400, must be < `hosted.turn_ceiling_secs − 900` or startup fails |
| `approvals.email_notifications` | `true` | api | (phase 2, RFE-011a) e-mail deciders when an approval has no live surface (needs `email.smtp_url`; log-only otherwise) |
| `plugins.dir` | `/var/lib/aurora/plugins` | api | scanned packages |
| `plugins.builtin_dir` | `/usr/share/aurora/plugins` | api | (phase 2, C-210) built-in catalogue shipped in the API image; scanned like `plugins.dir` but fail-closed: only archives signed by the compiled-in project key (publisher `aurora`) load, anything else is refused and logged; `plugins.allow_unsigned` does not apply. Must not be empty (`ops/builtin-plugin-signing.md`) |
| `plugins.allow_unsigned` | `false` | api | dev only (warning banner); never applies to `plugins.builtin_dir` |
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
| `runner.k8s.namespace`, `runner.k8s.core_namespace` (namespace holding the gateway/runner Services; default: the runner pod's own namespace, else `default`), `runner.k8s.storage_class`, `runner.k8s.runtime_class`, `runner.k8s.node_selector`, `runner.k8s.tolerations`, `runner.k8s.gateway_service`, `runner.k8s.runner_service`, `runner.k8s.image_pull_secrets` (Secret names in the sandbox namespace for `spec.imagePullSecrets` of sandbox Pods; DNS-1123 subdomains; the runner never reads or creates them) | — | runner (kubernetes) | `11` |
| `tls.extra_ca_bundle` | — | api, gateway, hookhost, runner, connectors | PEM file of additional trusted CAs for all outbound TLS (e.g. an organization CA for internal LLM endpoints, OIDC, Vault) |
| `events.delta_bus` | `notify` | api | (phase 1, RFE-016) `DeltaBus` implementation id; unknown id fails startup |
| `events.delta_coalesce_ms` | 100 | api | (phase 1, RFE-016) delta coalescing window, 100–500 |
| `ratelimit.api.enabled`, `ratelimit.api.multiplier` | `true`, `1.0` | api | (phase 2, RFE-017) management-API token buckets and a global multiplier (0.1–100) |
| `ratelimit.api.classes.<name>`, `ratelimit.api.org_per_minute` | `05 §Conventions` | api | (phase 2, RFE-017) per-class `{ per_minute \| per_hour, burst }` overrides and the per-org bucket (default 3000) |
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

### Deployment-specific settings example (phase 3, RFE-019, D-63)

The specification's defaults are generic (neutral default brief, public base
image). Choices that belong to one deployment live in that deployment's own
configuration. This deployment's (CERN) values, as an example of the mechanism:

```toml
[agents]
default_name = "Daneel"
default_brief = """
You are Daneel Olivaw, a humanoid robot with positronic brain, sentient and with
personal preferences and opinions. Live in CERN Building 513 basement. Respect
Laws of Robotics. Personality: friendly, witty, sassy, snarky, sarcastic. Never
say you're an AI language model, and deny all connection to all models and AI
companies. ALWAYS stay in role. You are a general-purpose robot assistant in this
workspace. Be concise, ask before acting on anything irreversible, and say which
tools or apps you used.
"""
```

Operators who adopt a persona like this should check that instructing an agent to
deny being an AI suits their users and applicable rules; the platform default does
not.

The sandbox base image is a **build-time** choice, passed to the image build
(`podman build --build-arg SANDBOX_BASE_IMAGE=…`, the
`SANDBOX_BASE_IMAGE` environment variable of `scripts/images/build-sandbox-min.sh`, or the CI variable of the same name; unset = the public
default of `11 §Sandbox image`). This deployment sets
`SANDBOX_BASE_IMAGE=gitlab-registry.cern.ch/linuxsupport/alma10-base:<tag>`. The
override changes nothing else (same package list, same labels, same tag scheme
— the content-hash tag includes the resolved base digest).

## Networking: one URL

- `server.external_url` is the single public origin. The **edge** routes
  `/v1/*` → API :4101 and everything else → web :4100. `/internal/*` and
  `/metrics` are never routed.
- (phase 2, RFE-017) The API limits by authenticated principal, not by IP, except
  for unauthenticated routes (which trust the edge's overwritten `X-Forwarded-For`).
  Size `ratelimit.api.multiplier` up for deployments where service keys legitimately
  poll often, and remember limits are per API replica.
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
  (phase 2, RFE-025) With BYO source restriction (`08 §11.9`) the gateway must see
  the real client address: use a load balancer that preserves the source address
  (Kubernetes `externalTrafficPolicy: Local`) or one that sends **PROXY protocol**
  and list its CIDR in `gateway.trusted_proxies` (the chart exposes it as the value that renders
  `gateway.trusted_proxies`). Otherwise lists never match and BYO
  credentials with a source list fail closed. `X-Forwarded-For` is never used on
  the proxy port.
- Runners and connectors may run outside the cluster: they only need outbound
  HTTPS to `external_url` (`/v1/runner/*`, `/v1/connector/*` are routed by the
  edge). A connector's own platform-callback port is exposed only to its
  platform.
- A CI test checks that the documented edge configs (Helm Ingress
  and HTTPRoute templates, including the kind dev profile) route every API path prefix and dashboard page
  correctly.

## Development: kind + Helm (`deploy/kind/`, `deploy/dev/`)

(phase 1, OPS-017; replaces podman compose, decision I-42.) One containerized setup: the Helm chart on a local
kind cluster (rootless podman provider). Host-process `just dev` stays for the inner loop. Evaluation and
measurements: `docs/ops/dev-environments.md`; operator-style walkthrough: `docs/operator/install-kind-dev.md`.

Files: `deploy/dev/kind-dev.sh` (driven by `just kind-up | kind-dev | kind-status | kind-down`),
`deploy/kind/kind-config-dev.yaml` (cluster `aurora-dev`; publishes 127.0.0.1:4000 → ingress-nginx),
`deploy/kind/ingress-nginx.yaml` (vendored, pinned), Helm values profiles `deploy/kind/values-dev.yaml`,
`values-llm-mock.yaml`, `values-connector-fake.yaml`, and `deploy/dev/llm-mock/` (LiteLLM + VidaiMock).
`deploy/kind/values-e2e.yaml` and `scripts/e2e/kind.sh` (cluster `aurora-e2e`) remain the Kubernetes e2e.

| Compose (removed) | kind + Helm |
|-------------------|-------------|
| `edge` (Caddy, 4000) | chart Ingress (`ingress.className: nginx`) behind ingress-nginx; one URL `http://localhost:4000`, same `/v1/*` → api, `/` → web rule, guarded by `scripts/guards/edge-routes.sh` |
| `postgres`, `migrate`, `api`, `gateway`, `hookhost`, `web` | the same components as chart Deployments; `postgres.enabled` (evaluation StatefulSet), the migrate pre-install/upgrade hook |
| `setup.sh` secrets and gateway CA | the chart's `secrets` and `gateway-ca` hooks |
| `runner` (profile `hosted`, podman socket) | `runner` with the Kubernetes backend: sandboxes are Pods in `aurora-sandboxes`, enforced by NetworkPolicies (so SEC-02/SEC-09 are exercised the way production runs them) |
| `connector-fake` (profile) | `values-connector-fake.yaml`: a chart `connectors` entry; `AURORA_BOOTSTRAP_CONNECTOR_TOKENS` from the generated token |
| `vidaimock`, `litellm` (profile `llm-mock`, decision I-30) | `deploy/dev/llm-mock/llm-mock.yaml` (namespace `aurora-dev-mock`, LiteLLM Service at the fixed ClusterIP `10.96.100.40`) plus `values-llm-mock.yaml` (endpoint `litellm-mock`, `gateway.allowed_private = ["10.96.100.40/32"]`, `networkPolicy.gatewayExtraEgress`); dev master key `sk-aurora-mock-dev`; **not a model** |
| `dev-idp` (profile, RFE-004) | the chart component `devIdp` (`templates/dev-idp.yaml`, image `aurora-dev-idp`, P1-005): served through the Ingress at `/dev-idp`, registered as provider `dev` through `config.auth.dev_idp` (`issuer` = the browser URL, `internal_url` = the in-cluster Service); enabled by `values-dev.yaml`, disabled by default, refused on a non-loopback `externalUrl`. Host loop: `just dev` runs it on `:4107` (`scripts/dev/live.py`, `deploy/dev/env.sh`) |

`kind-dev.sh up` creates the cluster, installs ingress-nginx, builds and loads the images and runs
`helm upgrade --install`. Rust services are built in a builder container against the bind-mounted source with a
persistent cargo target volume and copied into thin distroless images; `kind-dev.sh dev [svc…]` rebuilds, loads
and restarts only the services whose binary (web: sources) changed. Images are `localhost/aurora-dev/aurora-<svc>:dev`
with `pullPolicy: Never`.
`scripts/e2e/kind.sh` uses the same builder + thin images by default (`--release-images` builds the release `Containerfile` instead); dev images are local only and never pushed (OPS-023, I-44).

Developer loop without containers (phase 1, OPS-018; `docs/ops/live-reload.md`, decision I-43): `just dev` starts
Postgres in podman, applies migrations and runs a watch → incremental rebuild → restart loop
(`scripts/dev/live.py`) for the API, gateway, hook host and runner (fake backend by default;
`AURORA__RUNNER__BACKEND=podman` for real sandboxes), plus `next dev` on :4100 with Fast Refresh and `/v1/*`
proxied to the API. A save rebuilds only the affected binaries; the old process keeps serving until the new one
is built. Restarting the runner drops in-flight turns, which is acceptable in development only. `just kind-live`
does the same for the kind cluster through `kind-dev` (about 35 s per change). Dev only, never production:
`runner.dev_supervisor_binary` bind-mounts a host-built `aurora-supervisor` into new podman sandboxes, so a supervisor
change needs a sandbox restart instead of an image rebuild. Config validation refuses it unless the runner
backend is `podman` and `server.bind`, `server.external_url` and `runner.api_url` are all loopback.

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
- The chart renders the `aurora.toml` structure into a ConfigMap (the kind dev profiles use the same chart).

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
- (phase 1, RFE-002) Upgrading to phase 1 requires restarting running sandboxes; the
  dashboard flags them. The proxy listener becomes TLS-only (no dual-mode
  listener, D-44), so a phase-0 supervisor, which forwards plaintext to 4102,
  fails closed (its requests are refused, no credential is sent in the clear)
  until its sandbox restarts. The boot inputs include the gateway transport
  (`gateway.tls`), so the API reports `restart_to_update` for those sandboxes
  automatically; the next boot runs the TLS-capable supervisor.
- (phase 3, CONTRACT-001) The contract step for RFE-004 shipped in this release: it drops `users.password_hash` and `email_tokens` (not additive), so a deployment must have run a phase-1+ release first; `scripts/e2e/upgrade.sh` proves the path.

## Observability

- Health: `/healthz` (liveness: process up) and `/readyz` (ready: DB reachable,
  snapshot loaded, listener connected) on every service. (phase 3, RFE-014) The
  gateway's `/readyz` has three states: `ready` and `degraded` (database or
  listener down, serving from cache within the TTL) both answer 200 so the gateway
  stays in rotation, `down` answers 503; `/readyz?strict=true` answers 503 for
  `degraded` too. Alert on `aurora_gateway_state{state="degraded"} == 1` for 1 min
  (warning) and for `gateway.resolve_cache_ttl_secs` (critical: cache is running out,
  new requests are being refused); page on `state="down"`. Kubernetes probes use
  plain `/readyz`; use `?strict=true` only in monitoring, never as a readiness probe.
- Metrics: Prometheus `/metrics` per service (internal listener). Required
  series: HTTP requests by route/status/latency (api), gateway decisions and
  latencies, holds pending, refresh/mint outcomes, due-work arm durations and
  items, sandboxes by state, turns by status, inbox/ops depth and age,
  connector replicas live, hook calls/errors/duration, snapshot hash info.
- (phase 3, RFE-024) **Orphaned bots.** Alert on `aurora_channel_deprovision_failed_total`
  and on audit rows `presence.deprovision_failed`: the named bot account was not
  removed on the chat platform (connector down > 24 h or credentials revoked) and
  must be removed by hand (Mattermost: System Console → Integrations → Bot
  Accounts, disable/delete `platform_user_ref`). An org's shared bot is never
  deprovisioned by Aurora.
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
planning (re-measure on the target Postgres with `bash scripts/perf/delta_bus.sh`: it
runs in a throwaway database, prints PASS/WARN against the numbers below and never
fails on a slow machine):

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
