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
| `aurora-relay` | `aurora-relay` (phase 4, P4-PRV-2, deployed only when previews are enabled) |
| `aurora-web` | Next.js standalone build |
| `aurora-connector-fake` | test/dev only |
| `aurora-connector-mattermost` | phase 1 |
| `aurora-sandbox` | sandbox image (`11 §Sandbox image`) — not a service; referenced by the runner. (phase 3, RFE-019) Built on the public official AlmaLinux image by default; a private or mirrored base is a **build-time** override (`SANDBOX_BASE_IMAGE`, see below), not a runtime configuration key |

(phase 1, OPS-023) **Dev images** (`localhost/aurora-dev/*`, `localhost/aurora-e2e/*`) are built from a debug-profile binary in the kind builder container into thin distroless images, for local kind runs only; they are never pushed, and the released images above come only from `images/services/Containerfile` (release profile, sccache-accelerated, `docs/ops/build-performance.md`).

(phase 1, OPS-020) **Registry and tags.** CI builds and publishes every image to
`ghcr.io/alexiri/aurora-<name>` (`api`, `gateway`, `hookhost`, `runner`, `relay`, `web`,
`connector-fake`, `connector-mattermost`, `sandbox`, `sandbox-min`) with `GITHUB_TOKEN`, only after the
lint, test, e2e, web and guard jobs of the same workflow run are green. Pushes to `master` and `v*` tags
publish; pull requests build without pushing. Tags: `sha-<short>` (every publish), `master` (moving),
`<version>` and `<major>.<minor>` from a `vX.Y.Z` git tag, and `latest` only for the newest
non-prerelease semver tag. Images carry OCI labels (source, revision, version, license
`GPL-3.0-or-later`) plus SBOM and max-mode provenance attestations. The Helm chart defaults to
`image.registry: ghcr.io/alexiri` with the chart `appVersion` as tag. The seven service images are copies of a
single compile of the workspace (`images/services/Containerfile`, cargo-chef dependency layer, lld).
`aurora-sandbox-min` is the slim e2e sandbox (OS + supervisor). (phase 4, P4-HAR) The
sandbox image installs the pinned Claude Code and Codex runtimes by default; build with
`SANDBOX_HARNESSES="claude-code"` for a slimmer image. The label `org.aurora.harnesses`
tells the runner what is inside. Helm: `config.hosted.enabled_harnesses` (default
`[claude-code]`); on Kubernetes, which cannot read the label, `config.runner.image_harnesses`
names the runtimes of the sandbox image (D-153).

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
| `AURORA_CONNECTOR_HOST_TOKEN` | connector-host | (phase 4, P4-MCON) `auh_…` |
| `AURORA_BOOTSTRAP_CONNECTOR_HOST_TOKEN` (optional) | api | (phase 4, P4-MCON) on startup, upsert this `auh_…` token's hash into `connector_host_tokens` (Helm bootstrap; no admin route mints it) |
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
| `connector_host.api_url` | `http://127.0.0.1:4101` | connector-host | (phase 4, P4-MCON) the API the host registers with; the token comes from `AURORA_CONNECTOR_HOST_TOKEN` (`auh_`) |
| `connector_host.listen` / `connector_host.ops_port` | `0.0.0.0:4110` / `4192` | connector-host | (phase 4, P4-MCON) platform callbacks (`/connectors/<id>/…`) / health and metrics |
| `connector_host.test_types` | `false` | connector-host | (phase 4, P4-MCON) also announce the `fake` type (development and e2e only; startup warns) |
| `skills.library.enabled` | `true` | api | (phase 4, P4-GSL) kill-switch of the git skill library; invisible until an org admin connects a repository; no new infrastructure (`§Git skill library`) |
| `api.gateway_internal_url` | `http://aurora-gateway:4103` | api | (phase 4, P4-GSL, D-154) the gateway's internal listener as the API reaches it for `POST /internal/v1/library-fetch` (`06 §Library fetch`); the internal secret ring authenticates; must be an `http(s)` URL (startup refusal naming the key otherwise) |
| `skills.library.refresh_interval_secs`, `skills.library.bytes_per_agent` | 900 (≥ 300), 1 MiB (lower only) | api | (phase 4, P4-GSL) `LIBRARY_REFRESH_INTERVAL`, `LIBRARY_BYTES_PER_AGENT` (`02 §Limits`) |
| `previews.enabled` | `false` | api, relay | (phase 4, P4-PRV-1) master switch for port previews; **off unless configured**; when `true` the API refuses to start without a valid `previews.domain` (`§Port previews`). Helm: the chart value `relay.enabled` is the single switch — it mirrors `config.previews.enabled` (FIX-056), so `helm upgrade --set relay.enabled=true --set relay.domain=…` wires everything; set the config key only to agree explicitly, never alone |
| `previews.domain` | — | api, relay | (phase 4, P4-PRV-1) the DNS suffix of preview hostnames, e.g. `preview.example.org`; needs a wildcard DNS record and a wildcard TLS certificate; must not be, contain or sit under the dashboard host (startup validation, `§Port previews`) |
| `previews.session_ttl_secs`, `previews.session_idle_secs`, `previews.bytes_per_share_day` | 28800, 3600, 5 GiB | api, relay | (phase 4, P4-PRV-2) lower only for the first two (`02 §Limits`) |
| `relay.api_url` | `http://127.0.0.1:4101` | relay | (phase 4, P4-PRV-2) the API it registers with; the anchor token comes from `AURORA_RELAY_TOKEN` (`aul_`) |
| `relay.listen` / `relay.ops_port` | `0.0.0.0:4112` / `4193` | relay | (phase 4, P4-PRV-2) edge-facing HTTP / health and metrics |
| `relay.max_streams`, `relay.max_streams_per_user`, `relay.request_body_max_bytes` | 2048, 64, 100 MiB (chart: 2048, 64, **32 MiB**) | relay | (phase 4, P4-PRV-2) per replica; lower only for the last two. 100 MiB is the ceiling and the binary's default; the Helm chart deliberately ships the lower 32 MiB (`docs/23`), raise it to at most 100 MiB if previews must accept larger uploads (FIX-074) |
| `runner.stream_bind` / `runner.stream_tls.*` | `server.bind`:4108 / off | runner | (phase 4, P4-PRV-2) the relay-facing stream listener (only opened when the API's `previews.enabled` is true at registration) and optional TLS (`cert_file`, `key_file`). The runner registers the `stream_tls` capability; the API derives the endpoint scheme it hands the relay from it (`ws://` off, `wss://` on, `16 §W18`) — the relay verifies the presented certificate against its own trust configuration |
| `terminal.enabled` | `false` | api | (phase 4, P4-TERM, D-161) the dashboard's web-terminal surface (`GET /v1/agents/{agent}/terminal`): off means every route answers 404 `terminal_disabled` and the runner registers no `terminal` capability; the runner opens :4108 when previews **or** the terminal is enabled. Helm: the chart value `terminal.enabled` is the single switch — it mirrors `config.terminal.enabled` (FIX-053), so `helm upgrade --set terminal.enabled=true` wires everything; set the config key only to agree explicitly, never alone |
| `terminal.awake_cap_secs` | 14400 | api | (phase 4, P4-TERM) `TERMINAL_AWAKE_CAP`, at most `hosted.background_cap_secs` |
| `terminal.wake_window_secs` / `terminal.wake_wait_secs` | 60 / 45 | api | (phase 4, P4-TERM) the wake-demand window on open / how long the WebSocket upgrade may wait for a waking sandbox |
| `previews.shares_per_agent`, `previews.shares_per_org` | 10, 200 | api | (phase 4, P4-PRV-1) lower only (`02 §Limits`) |
| `previews.awake_cap_secs` | 14400 | api | (phase 4, P4-PRV-1) `PREVIEW_AWAKE_CAP`, at most `hosted.background_cap_secs` |
| `database.max_connections` | 20 (api), 10 (gateway) | api, gateway | pool size |
| `database.gateway_role` | — | api (migrations) | login role the **operator creates** (`CREATE ROLE <r> LOGIN PASSWORD '…'`); `migrate` only grants it the gateway privileges (`03`) and fails if it does not exist. The gateway must then connect as it (its own `AURORA_DATABASE_URL`; Helm `database.gatewayExistingSecret`) |
| `auth.signup_mode` | `invite_only` | api | `open`, `invite_only`, `sso_jit` |
| `auth.org_creation` | `instance_admins` | api | or `anyone` |
| `auth.instance_admins` | `[]` | api | emails |
| `auth.trusted_origins` | `[]` | api | extra origins for CSRF checks |
| `auth.session_idle_secs` / `auth.session_absolute_secs` | 1209600 / 2592000 | api | |
| `auth.oidc_providers[]` | — | api | `04 §OIDC`; (phase 1, RFE-004) **at least one required** (or `auth.dev_idp.enabled`), `require_email_verified` removed; (D-118) `trusted_email_domains` (default `[]`): exact domains whose addresses count as verified without the `email_verified` claim; (phase 4, P4-IDP-1, D-139) `role_claim` (dot path), `role_claim_source` (`id_token` \| `userinfo`), `require_role` (default `false`), `role_mappings[]` (`value` → `instance_admin`, `org {slug, role}`, `workspaces [{slug, role}]`), `04 §OIDC role claims`; (phase 4, P4-GRP-1, D-156) `group_claim` (dot path, e.g. `cern_roles` or `groups`) and `group_claim_source` (`id_token` \| `userinfo`): the claim whose values feed IdP-linked groups (`04 §IdP-linked groups`); unset = no IdP-linked groups for this provider |
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
| `llm.enabled_providers`, `llm.provider_preference`, `llm.endpoints[]` | `["litellm","anthropic"]` | api, gateway | `08 §6`; each endpoint may set `allowed_models` and `key_origin` (`method` = `endpoint` \| `litellm_team` \| `provider_account`, with `allowed_organization_ids`, `allowed_workspace_ids`, `allowed_team_ids`); (phase 4, FIX-019, D-136) an endpoint whose models are listed live (`litellm`) and that has no `default_model` starts with a **warning**: hosted agents then need a model chosen per agent, otherwise turns fail `no_default_model`. Set `default_model` (a model id served by the endpoint, e.g. a Claude model for the `claude-code` harness). An unknown `key_origin.method` refuses startup with the nearest valid value suggested |
| `llm.endpoints[]` provider keys (phase 5 (D-138, moved from phase 4), P4-LLM, D-121) | — | api, gateway | per provider, in addition to the common `id`, `provider`, `display_name`, `dialects`, `default_model`, `models`, `allowed_models`, `key_origin`: **`openai`**: `organization`, `project` (optional, not secrets); **`azure_openai`**: `base_url` (required, `https://<resource>.<azure-domain>`, host only), `models` and `default_model` required (deployment names); **`bedrock`**: `region` (required, e.g. `eu-central-1`), `base_url` (optional VPC endpoint override), `dialects` (default `["anthropic_bedrock"]`); **`vertex`**: `project_id` and `location` (required), `models` and `default_model` required; **`openai_compatible`**: `base_url` (required), `auth_header` (`authorization` default \| `api-key` \| `x-api-key`), `models_path`, `allowed_paths` (default derived from `dialects`; `["/**"]` opts out). Every provider also accepts `allowed_paths` to **narrow** its inference set. Credentials never appear here: an entry with a key such as `api_key`, `access_key*`, `secret*`, `private_key`, `password` or `token`, an unknown key, a non-https `base_url` (loopback excepted), or an invalid region/location/project id refuses startup naming the key (`08 §Phase 4 providers`) |
| `llm.enabled_providers` additions | default unchanged | api, gateway | (phase 5 (D-138, moved from phase 4), P4-LLM) accepts `openai`, `azure_openai`, `bedrock`, `vertex`, `openai_compatible` besides `litellm`, `anthropic`; the default stays `["litellm","anthropic"]`, so an existing configuration behaves exactly as before |
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
| `hosted.default_harness` | `claude-code` | api | must be in `hosted.enabled_harnesses` or startup fails |
| `hosted.enabled_harnesses` | `["claude-code"]` | api | (phase 4, P4-HAR, D-122) harness ids agents may use (`claude-code`, `codex`; `fake` only with `hosted.allow_fake_harness`); an unknown id refuses startup. Enabling `codex` also needs a runner image that contains it (image label `org.aurora.harnesses`, `11 §Sandbox image`) and an LLM endpoint serving `openai_responses` (the pinned Codex 0.167.0 does not speak `openai_chat`, D-153; `08 §Phase 4 providers`) |
| `hosted.allow_fake_harness` | `false` | api | tests/dev |
| `hosted.default_timezone` | `UTC` | api | for new workspaces |
| `hosted.idle_stop_secs`, `hosted.background_cap_secs`, `hosted.turn_ceiling_secs`, `hosted.turn_stall_secs` | `02 §Limits` | api | |
| `hosted.sandbox_ready_timeout_secs` | 300 | api | (FIX-025, D-158) end-to-end start deadline, 60–1800: from "a turn needs a sandbox" (start claimed; the runner's capacity wait counts inside it) to "the supervisor is ready". At the deadline the readiness sweep fails the attempt and the queued turns with the code naming where it stopped (`10 §Failure semantics`, `11 §Start readiness deadline`). Raise it for images that pull slowly (the pull counts inside the deadline) |
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
| `runner.image_harnesses` | `[]` | runner | (phase 4, P4-HAR, D-153) harness runtimes the sandbox image contains, for the Kubernetes backend, which cannot read the image label `org.aurora.harnesses`; empty = `claude-code`, `fake`; podman reads the label and ignores it |
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
| `ratelimit.counters.backend` | `memory` | api, gateway | (phase 4, P4-CNT, D-119) `memory` (per replica, today's behaviour, no infrastructure), `postgres` (shared, tables of `03`, no new infrastructure) or `redis`; any other value refuses startup. See *Shared rate-limit counters* |
| `ratelimit.counters.api`, `ratelimit.counters.gateway` | `shared` | api, gateway | (phase 4, P4-CNT) `shared` (use the backend above) or `memory` (keep that limiter per replica): lets an operator share only the gateway's rule quotas and keep the API limiter local. With `backend = memory` both are `memory` whatever they say |
| `ratelimit.counters.on_unavailable.security`, `.management`, `.gateway` | `local`, `local`, `local` | api (`security`, `management`), gateway (`gateway`) | (phase 4, P4-CNT) behaviour when the shared backend cannot be reached: `local` (per-replica in-memory counters until it recovers) or `closed` (refuse with 503 `counter_unavailable`). **There is no `open`**: any other value, `open` included, refuses startup (SEC-62). Classes in `05 §Conventions` *Shared counters* |
| `ratelimit.counters.timeout_ms` | 50 | api, gateway | (phase 4, P4-CNT) `COUNTER_TIMEOUT` per store call, 10–2000 |
| `ratelimit.counters.postgres.pool_size` | 4 | api, gateway | (phase 4, P4-CNT) size of the dedicated pool of the `postgres` backend, 1–32, separate from `database.max_connections` so a slow counter query cannot starve the data path; uses the service's own database role |
| `ratelimit.counters.redis.address` | — (required for `redis`) | api, gateway | (phase 4, P4-CNT) `rediss://host[:port]` (TLS) with no userinfo, path or query; `redis://` is accepted only for a loopback host or with `allow_plaintext = true` (startup warning); a URL carrying a password refuses startup |
| `ratelimit.counters.redis.allow_plaintext` | `false` | api, gateway | (phase 4, P4-CNT) explicit opt-in for `redis://` to a non-loopback host (in-cluster Redis without TLS) |
| `ratelimit.counters.redis.username_env`, `.password_env` | unset | api, gateway | (phase 4, P4-CNT) names of environment variables holding the Redis ACL user and password (both optional; an unset variable that is named refuses startup); never the values (`08 §4`) |
| `ratelimit.counters.redis.ca_file` | unset (system roots, then `tls.extra_ca_bundle`) | api, gateway | (phase 4, P4-CNT) PEM bundle verifying the Redis server |
| `ratelimit.counters.redis.db`, `.key_prefix` | `0`, `aurora:rl:` | api, gateway | (phase 4, P4-CNT) database index and key prefix (lets several Aurora instances share one Redis); `key_prefix` ≤ 64 chars of `[A-Za-z0-9:_-]` |
| `scim.enabled` | `false` | api | (phase 4, P4-GRP-2, D-132) serve `/scim/v2` and show the Directory sync page; while `false` every `/scim/v2` path is 404 and no org can enable SCIM. Helm `scim.enabled` also adds the edge route (`15 §Networking`) |
| `scim.allowed_email_domains` | `[]` | api | (phase 4, P4-GRP-2) optional instance allow-list of the e-mail domains an org may provision (exact, case-insensitive); empty = any domain the org owner lists. Entries must be domains (no `@`, wildcard or single label), else startup refuses naming the key |
| `scim.rate_per_minute` | 600 | api | (phase 4, P4-GRP-2) `SCIM_RATE` per token (burst is a fifth of it), 30–6000 |
| `scim.token_max_days` | 730 | api | (phase 4, P4-GRP-2) longest lifetime of an `aud_` token, 1–1825; the default offered in the dashboard is 365 (never above this) |
| `approvals.park.enabled` | `false` | gateway, api | (phase 5 (D-138, moved from phase 4), RFE-011b, D-133) park approvals with long deadlines instead of holding the connection (`07 §Park and resume`). Off = exactly phase 2 behaviour. The gateway and the API must agree (both read it from the shared configuration) |
| `approvals.park.threshold_secs` | 900 | gateway | (phase 5 (D-138, moved from phase 4), RFE-011b) effective timeouts above this park; 60–`approvals.park.max_timeout_secs` |
| `approvals.park.max_timeout_secs` | 86400 | gateway, api | (phase 5 (D-138, moved from phase 4), RFE-011b) longest deadline of a parked approval, 3600–604800 (also the upper bound of a rule's `approval_timeout_secs` when parking is enabled); requests that cannot park stay clamped to `approvals.max_timeout_secs` |
| `approvals.park.resume_window_secs` | 1800 | api | (phase 5 (D-138, moved from phase 4), RFE-011b) how long an approved parked request can be repeated once, 60–86400 |
| `approvals.park.deny_cooldown_secs` | 900 | gateway | (phase 5 (D-138, moved from phase 4), RFE-011b) a denied parked request repeated within it is answered `manual_approval_denied`, 0–86400 |
| `approvals.park.max_body_bytes` | 262144 | gateway | (phase 5 (D-138, moved from phase 4), RFE-011b) largest request body that can be parked (4096–1048576); larger requests are held |
| `approvals.park.max_pending_per_agent` | 20 | gateway | (phase 5 (D-138, moved from phase 4), RFE-011b) parked pending approvals per agent, 1–200 |
| `push.enabled` | `false` | api | (phase 5 (D-138, moved from phase 4), RFE-011b, D-134) Web Push; needs outbound HTTPS from the API to the push services. See *Approval park-and-resume and Web Push* below |
| `push.vapid.subject` | the external URL | api | (phase 5 (D-138, moved from phase 4), RFE-011b) VAPID `sub` claim: `mailto:` or `https:` contact for the push services |
| `push.vapid.private_key_env` | unset | api | (phase 5 (D-138, moved from phase 4), RFE-011b) name of an environment variable holding an operator-managed ES256 (P-256) VAPID private key (PKCS#8 PEM or raw base64url); unset = Aurora generates, seals and rotates its own key. Never the value |
| `push.allowed_endpoint_hosts` | `fcm.googleapis.com`, `updates.push.services.mozilla.com`, `*.push.services.mozilla.com`, `web.push.apple.com`, `*.push.apple.com`, `*.notify.windows.com` | api | (phase 5 (D-138, moved from phase 4), RFE-011b) host patterns (`07 §Matching primitives`; a leading label wildcard only) a subscription endpoint may have; checked at registration and before every send; an empty list refuses startup; an entry that is not a host pattern refuses startup |
| `push.concurrency`, `push.timeout_secs` | 16, 10 | api | (phase 5 (D-138, moved from phase 4), RFE-011b) pushes in flight per replica (1–64) and per-request timeout (2–30) |
| `blobs.backend` | `postgres` | api | (phase 5 (D-138, moved from phase 4), P4-S3, D-120) where **new** attachment bytes go: `postgres` (the `blobs` table) or `s3`; existing rows keep their own backend (`attachments.blob_backend`); any other value refuses startup. See *S3-compatible blob store* |
| `blobs.s3.bucket`, `blobs.s3.region` | — (required for `s3`) | api | (phase 5 (D-138, moved from phase 4), P4-S3) bucket name (3–63 chars, DNS-compatible) and signing region (any non-empty string for S3-compatible servers that ignore it, e.g. `us-east-1`) |
| `blobs.s3.endpoint` | unset (AWS S3 for the region) | api | (phase 5 (D-138, moved from phase 4), P4-S3) `https://host[:port]` of an S3-compatible server (MinIO, Ceph, …), no userinfo, path or query; `http://` accepted only for a loopback host or with `allow_insecure_http = true` |
| `blobs.s3.allow_insecure_http` | `false` | api | (phase 5 (D-138, moved from phase 4), P4-S3) opt-in for `http://` to a non-loopback endpoint (in-cluster MinIO without TLS); startup warning |
| `blobs.s3.path_style` | `false` | api | (phase 5 (D-138, moved from phase 4), P4-S3) path-style addressing (`https://host/bucket/key`); needed by most self-hosted servers |
| `blobs.s3.prefix` | `aurora/` | api | (phase 5 (D-138, moved from phase 4), P4-S3) key prefix, ≤ 64 chars of `[A-Za-z0-9._/-]`, no leading `/`, no `..`, empty or ending in `/` |
| `blobs.s3.credentials.source` | `provider_chain` | api | (phase 5 (D-138, moved from phase 4), P4-S3) `provider_chain` (the standard AWS chain: web identity / IRSA / pod identity / instance profile / environment; **recommended**, no static key in Aurora's configuration) or `env` (static keys from the variables named below); never inline values |
| `blobs.s3.credentials.access_key_id_env`, `.secret_access_key_env`, `.session_token_env` | — | api | (phase 5 (D-138, moved from phase 4), P4-S3) `source = env`: names of the environment variables holding the keys (`session_token_env` optional); an unset or empty named variable refuses startup; values are filled from a Secret (Helm `blobs.s3.existingSecret`), never from `aurora.toml` or the database |
| `blobs.s3.sse` | `AES256` | api | (phase 5 (D-138, moved from phase 4), P4-S3) server-side encryption requested on every `PutObject`: `AES256` (SSE-S3), `aws:kms` (with `sse_kms_key_id`) or `none` (startup warning; only for servers without encryption support). The startup probe verifies that the server **confirms** the requested encryption (`x-amz-server-side-encryption` in the response) and refuses to start otherwise |
| `blobs.s3.sse_kms_key_id` | unset | api | (phase 5 (D-138, moved from phase 4), P4-S3) KMS key id/ARN for `aws:kms` (not a secret) |
| `blobs.s3.ca_file` | unset (system roots, then `tls.extra_ca_bundle`) | api | (phase 5 (D-138, moved from phase 4), P4-S3) PEM bundle verifying the endpoint |
| `blobs.s3.allow_versioned_bucket` | `false` | api | (phase 5 (D-138, moved from phase 4), P4-S3) accept a bucket with versioning enabled; purges then delete every version (`10 §Attachments`). Object-Lock buckets are never accepted |
| `blobs.s3.upload_concurrency` | 16 | api | (phase 5 (D-138, moved from phase 4), P4-S3) `S3_UPLOAD_CONCURRENCY`, 1–64 uploads in flight per replica |
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
removed from the list automatically. (phase 5 (D-138, moved from phase 4), P4-LLM) The list is unchanged: the
hosts of OpenAI, Azure OpenAI, Bedrock and Vertex are already known LLM hosts, so
until an operator configures an endpoint they stay refused under `sanctioned_only`;
configuring one reserves exactly that endpoint's host and removes it from the list.

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
  `/metrics` are never routed. (phase 4, P4-GRP-2) With `scim.enabled = true` the edge
  also routes **`/scim/v2/*`** → API :4101 (Helm: the Ingress and HTTPRoute templates add the
  path only when `scim.enabled` is true, so a default deployment exposes nothing new, and the
  edge-routes guard checks both renderings); SCIM requests are small and need no body or
  buffering exception. (phase 4, P4-TERM, D-161) The terminal needs **no** edge change: its
  WebSocket is `/v1/agents/{agent}/terminal` on the ordinary origin (the proxy must allow
  WebSocket upgrades for `/v1/*`, which it already does for SSE keepalives; no timeout change
  beyond `TERMINAL_IDLE_TIMEOUT`'s client-side pings).
- (phase 4, P4-PRV-2) With `relay.enabled` (the chart's single previews switch,
  FIX-056) the edge also serves the **wildcard host** `*.<previews.domain>` →
  relay :4112 (a second host, never a path of `server.external_url`;
  `§Port previews`). Without it nothing about the edge changes.
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
  API; the `secrets` hook pod → Kubernetes API (a hook-rendered policy, so it
  exists before the job on `helm upgrade`, when default deny is already in
  place); connectors → api, their platform. (phase 4, P4-CNT) With the Redis counter
  backend api and gateway also reach the configured Redis peers
  (`counters.redis.networkPolicy`) on its port; nothing else changes. (phase 5 (D-138, moved from phase 4), P4-S3)
  With the `s3` blob backend the **api** (only) also reaches the configured bucket
  endpoint (`blobs.s3.networkPolicy`).
- (phase 4, P4-MCON) **Connector host** (`connectorHost.enabled`, default `true`; replicas
  default 1, `connectorHost.testTypes` default `false`): Deployment `aurora-connector-host`,
  Service on 4110 (callbacks) routed by the edge rule `/connectors/` (Ingress and HTTPRoute;
  4192 health/metrics unrouted); its `auh_` token is generated by the `secrets` hook into the
  instance Secret (`AURORA_CONNECTOR_HOST_TOKEN`) and given to the API as
  `AURORA_BOOTSTRAP_CONNECTOR_HOST_TOKEN`; NetworkPolicy: ingress from the edge (and the Cilium
  edge entities) on 4110, egress to api:4101, DNS and external egress (`externalEgress`, plus
  `networkPolicy.connectorHostExtraEgress` for platforms on private ranges). The `connectors`
  list (self-hosted connectors run by the chart) stays for types Aurora does not ship.
- **Sandboxes namespace**: ResourceQuota, LimitRange, PSA `restricted`, runner
  ServiceAccount + Role (`11 §kubernetes`).
- **Postgres**: external database expected in production (values: URL secret,
  and the least-privilege gateway login: `database.gatewayRole` plus
  `database.gatewayExistingSecret`, set together; the DBA creates the role first and
  `aurora-api migrate` grants it, `03 §Gateway database role grants`); a bundled single-instance Postgres subchart is available for
  evaluation only.
- **Values highlights**: image registry/tag, replicas, resources, external
  URL, auth config, OIDC providers, LLM endpoints, secret stores, runner
  backend settings (storage class, runtime class, node selectors), connectors
  (list of `{id, image, token secret, callback ingress}`), plugin packages
  (ConfigMap or PVC mounted at `plugins.dir`).
- (phase 4) **Every Helm value in one place**: `docs/23-helm-values.md` is the
  reference of all chart values — type and allowed values, default, what it
  does, which keys the chart injects into `aurora.toml` (do not override those
  in `config`), and links to the section of this or the operator docs that
  explains the feature.
- The chart renders the `aurora.toml` structure into a ConfigMap (the kind dev profiles use the same chart).

## Upgrades

- Migrations are forward-only and run before any new API replica serves;
  every migration MUST be compatible with the previous API and gateway version
  still running (expand → migrate → contract across two releases).
- Wire contracts: API supports runner/connector/supervisor protocol versions N
  and N−1; plugin manifest contracts N and N−1; registry snapshots
  "newest contract wins" (`09 §9`).
- Order: migrate → api → hookhost → gateway → runner → relay (when `relay.enabled`; after the API, whose relay plane
  it registers with, and after the runners, whose :4108 stream listener it dials) → web → connectors.
  Gateways and runners of the previous version keep working during the roll.
- (phase 4, P4-TERM, FIX-053) Turning the web terminal on in an existing
  deployment is a single-switch upgrade: `helm upgrade … --set
  terminal.enabled=true` (the chart value mirrors `config.terminal.enabled`
  and renders the runner :4108 listener and NetworkPolicy flows with it;
  `docs/23 §NetworkPolicy`).
- (phase 4, P4-PRV-2, FIX-056) Turning port previews on in an existing
  deployment is likewise a single-switch upgrade: `helm upgrade … --set
  relay.enabled=true --set relay.domain=<previews.domain>` (the chart value
  mirrors `config.previews.enabled` and renders the relay, the wildcard edge
  rules and the NetworkPolicy flows with it; `docs/23 §NetworkPolicy`,
  `§Port previews` below).
- (FIX-069) Narrower egress on upgrade: the `secrets` hook's NetworkPolicy (`<release>-aurora-hooks`) now allows
  443/6443 only to `networkPolicy.kubeApiCidrs` when you set it, else to the API server addresses that Helm
  reads from the `kubernetes` Endpoints in `default` at `helm install/upgrade` time (it was always `0.0.0.0/0`).
  `helm template` and `--dry-run` cannot read them and still render `0.0.0.0/0`, as does a Helm user without
  read access to that Endpoints object. No value changes are required; set `kubeApiCidrs` to pin it
  explicitly (managed control planes whose endpoint is not in `default/kubernetes`). The relay's policy lost its
  external egress (it only ever needs the api and the runners). An instance whose Secret still carries the
  unused `AURORA_BOOTSTRAP_RELAY_TOKEN` key can delete it.
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
- (phase 4, P4-CNT) **Counter backend.** The `aurora_counters_*` series and alerts of
  *Shared rate-limit counters* below: a `fallback` state means limits are per replica
  again (warning), `policy="closed"` refusals are an outage of the affected limiter.
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

Stateful: Postgres (system of record), (phase 5 (D-138, moved from phase 4), P4-S3) the attachment bucket when
`blobs.backend = "s3"` (restore it together with Postgres), per-agent home volumes (PVC snapshots /
volume backups are the operator's job), the gateway CA Secret, the encryption
keyring and signing key (phase 2: the signing-key and internal-secret **rings**,
including retired entries still inside their rotation window). **Secrets are
unrecoverable without the keyring.**

## Sizing

Runner hosts: `max_sandboxes × sandbox memory + 1 GiB`. Held-awake sandboxes
occupy a slot permanently: size for steady state, not peak. Postgres: modest;
`request_log` dominates (≈300 bytes/request; partitioned and pruned).

### Attachment storage (phase 2, RFE-015)

Attachments live in Postgres (`blobs`, `bytea`) by default (phase 5 (D-138, moved from phase 4), P4-S3: or in an
S3-compatible bucket with `blobs.backend = "s3"`, *S3-compatible blob store* below), so in
the default they share the database's disk and backups. Budget for
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

## Shared rate-limit counters (phase 4, P4-CNT, D-119, SEC-62)

Phase 0–3 keep every rate-limit counter in memory per replica (D-05, D-57): with N
replicas a limit can be exceeded up to N×. From phase 4 the counters sit behind the
**`CounterStore`** trait so replicas can share them. Nothing changes unless
`ratelimit.counters.backend` is set: an existing `aurora.toml` and Helm values keep
the `memory` backend, which is today's behaviour exactly.

**Trait** (crate `aurora-counters`: the trait, the `memory` backend and the `redis`
backend behind a cargo feature; the `postgres` backend lives in `aurora-db`, which
implements the trait, so `aurora-counters` never depends on `aurora-db`):

| Method | Semantics |
|--------|-----------|
| `fixed_window_peek(probes)` | read the current count of each `(key, period)`; `period` ∈ minute / hour / day, aligned to UTC by the **store's** clock; no side effect |
| `fixed_window_consume(units)` | for each `(key, period, limit, cost = 1)`: add `cost` to the current window **only if** the result stays ≤ `limit`; **all-or-nothing** across the units of one call; returns `Allowed` or `Refused { key, retry_after }`; atomic (never exceeds `limit`, however many replicas call concurrently); a window's state expires with it (TTL) |
| `bucket_take(takes)` | for each `(key, emission_interval, burst)`: GCRA token take, all-or-nothing across the takes of one call; returns `Allowed` or `Refused { key, retry_after }`; used by the API limiter |
| `health()` | cheap liveness probe for the circuit breaker |

Errors are only `Unavailable` (connection error, timeout, breaker open) and
`Invalid` (a bug). **A backend error never means "allowed"**: the caller applies the
class policy (`local` or `closed`) of `ratelimit.counters.on_unavailable.*`
(`06 §Rate limiting`, `05 §Conventions`). `COUNTER_TIMEOUT` (50 ms), the breaker
(`COUNTER_BREAKER_FAILURES` 3, `COUNTER_BREAKER_COOLDOWN` 5 s) and the `local`
fallback are implemented once, in a wrapper over the trait, so every backend and
every limiter behaves the same.

**Limiters and failure policy** (the reason column is the design record):

| Limiter | Class | Default when the backend is down | Why |
|---------|-------|----------------------------------|-----|
| gateway `rate_limit` rule quotas | `gateway` | `local` | a quota on an allowed access; the per-replica bound is today's behaviour; `closed` for hard caps |
| sign-in/OIDC `start`/`callback`, rejected-bearer per-IP, unauthenticated per-IP GETs | `security` | `local` | brute-force and token-guessing protection must keep working at today's strength; **never open**; `closed` available (locks sign-in out during a Redis outage) |
| API `general`, class and `org` buckets, per-webhook limit | `management` | `local` | availability of the management plane; per-replica limits are today's behaviour; `closed` turns a counter outage into an API outage |
| stream concurrency cap, CLI polling, BYO admission (CIDR check), approval e-mail digest | — | not on the trait | gauge, row-based or not counters (`05`, `06`, `07`) |

**Backends.**
- `memory`: today's per-replica counters (default; exact with one replica).
- `postgres`: the `rate_limit_windows` / `rate_limit_buckets` tables of `03`,
  `UNLOGGED`, through a small dedicated pool; adds **no infrastructure**, one
  round-trip per consume; suited to the gateway's rule quotas, auth limits and small
  API deployments. The window and bucket arithmetic is done in SQL with the
  database's clock; rows are pruned by the API's `counters_prune` arm. During a
  Postgres outage this backend is unavailable too and falls back as above.
- `redis`: a standalone Redis or a primary with replicas/Sentinel failover (**not
  Redis Cluster**: a request's keys span slots); atomic scripts (`EVALSHA`, loaded
  at start and on `NOSCRIPT`) implement the operations with `PEXPIRE` TTLs and the
  Redis `TIME`. TLS by default (`rediss://`), ACL user and password from
  environment variables, keys under `ratelimit.counters.redis.key_prefix`. The
  Redis ACL needs only `EVALSHA`/`SCRIPT LOAD`/`GET`/`PING` on that prefix. Run it
  with `maxmemory-policy noeviction` or `volatile-ttl` (an LRU policy evicts live
  counters, which resets limits); persistence may be off. Redis is **optional and
  never required**; every Aurora feature works without it.

**Clock.** Replicas should be NTP-synchronized anyway, but counters do not depend
on it: window starts and bucket times come from the store.

**Metrics** (all services using the trait): `aurora_counters_calls_total{backend,op,result}`
(`result` = `allowed|refused|error|timeout|breaker_open`), `aurora_counters_call_seconds{backend,op}`,
`aurora_counters_state{backend,class,state="shared|fallback"}` (1 for the current
state), `aurora_counters_fallback_total{class,policy}` (requests decided by the local
fallback or refused by `closed`), `aurora_counters_pruned_total` (api). Alert on
`aurora_counters_state{state="fallback"} == 1` for 5 min (warning: limits are per
replica again) and on `aurora_counters_fallback_total{policy="closed"}` rate > 0
(critical for `closed` classes: requests are being refused). `readyz` is **not**
affected by a counter fallback.

**Helm** (`deploy/helm/aurora/values.yaml`; all keys optional, defaults render the
`memory` backend and the same manifests as before):

```yaml
counters:
  backend: memory            # memory | postgres | redis  -> ratelimit.counters.backend
  api: shared                # shared | memory
  gateway: shared            # shared | memory
  onUnavailable: {security: local, management: local, gateway: local}   # local | closed
  timeoutMs: 50
  postgres: {poolSize: 4}
  redis:
    address: ""              # rediss://redis.example.org:6379 (no password in it)
    allowPlaintext: false
    existingSecret: ""       # Secret with keys REDIS_USERNAME (optional) and REDIS_PASSWORD (optional);
                             # exposed to api and gateway as the variables named by username_env/password_env
    caFile: ""
    db: 0
    keyPrefix: "aurora:rl:"
    networkPolicy: {egressTo: []}   # NetworkPolicy peers (podSelector/namespaceSelector/ipBlock) api and gateway may reach on the Redis port
```

With `backend = redis` the chart adds the Redis peers to the api and gateway
NetworkPolicy egress (default deny stays: no peer, no egress) and fails `helm
template` if `address` is empty or `existingSecret` and `address` carry a
password inline. The chart **does not ship Redis**; an operator who wants one
uses their own or any chart. With `postgres` nothing is added (the pods already
reach Postgres).

**Migrating and rolling back.** Switching `backend` is a rolling restart; windows
and buckets start empty on the new backend (a limit's usage resets once). Going
back to `memory` leaves the tables in place (they are pruned, not dropped). A mixed
fleet (some replicas shared, some `memory`) is safe: each counts what it sees, so
the effective limit is between the configured value and N× until the roll finishes.

## S3-compatible blob store (phase 5 (D-138, moved from phase 4), P4-S3, D-120, SEC-63)

A second attachment backend beside Postgres, behind the same **BlobStore** trait
(`put`, `get`, `delete`, `list(prefix)`, `health`; crate `aurora-blobstore`, with the
`postgres` backend moved behind it unchanged and the `s3` backend built on
`aws-sdk-s3` with rustls). It is **optional**: with no `blobs.*` configuration the
deployment behaves exactly as in phase 3. Behaviour, key layout, streaming, purge,
orphan sweep, unavailability and migration are specified in `10 §Attachments`; this
section is the operator side.

- **Who talks to the bucket.** Only the **API** (uploads, downloads, purge, sweep).
  The gateway, runner, supervisor, connectors and sandboxes never reach it and
  never see its credentials (the runner downloads inbox files through the API). Only
  the API NetworkPolicy gains an egress rule (`blobs.s3.networkPolicy`).
- **Credentials.** Prefer `provider_chain` (IRSA / pod identity / instance profile):
  no static key exists in Aurora's configuration. For `env`, the Helm chart reads
  a Secret (`blobs.s3.existingSecret` with keys `AWS_ACCESS_KEY_ID`,
  `AWS_SECRET_ACCESS_KEY`, optional `AWS_SESSION_TOKEN`) and the configuration names
  the variables (`*_env`); the values are not in `aurora.toml`, the database, a
  response, a log line, a metric label or a sandbox (SEC-63).
- **Least-privilege policy.** On `arn:aws:s3:::<bucket>/<prefix>attachments/*`:
  `s3:PutObject`, `s3:GetObject`, `s3:DeleteObject`; on the bucket, with
  `s3:prefix = <prefix>attachments/*`: `s3:ListBucket`; plus `s3:GetBucketVersioning`
  and `s3:GetEncryptionConfiguration` for the startup probe (and `s3:ListBucketVersions`,
  `s3:DeleteObjectVersion` only with `allow_versioned_bucket`). Nothing else; no
  `s3:*`, no `PutBucketPolicy`, no ACL actions.
- **Bucket requirements.** Private (S3 *Block Public Access* on), default encryption
  enabled, **versioning off** (or `allow_versioned_bucket`), **Object Lock off**.
  Aurora requests the configured SSE on every put and the startup probe writes,
  reads and deletes a probe object under `<prefix>_probe/` to prove it; a server that
  does not confirm the requested encryption, a missing bucket, a refused credential
  or a versioned or locked bucket fails startup with a message naming the key.
- **Timeouts and retries.** `S3_CONNECT_TIMEOUT` 5 s, `S3_REQUEST_TIMEOUT` 60 s per
  call, idle body timeout 30 s; `S3_RETRIES` (2) with jittered backoff for
  connection errors, 5xx and throttling on `get`, `delete`, `list`, `head` and on
  `put` (a key is written once, so a retry is safe; a `412` on a retried conditional
  put counts as success when the stored checksum equals the upload's); other 4xx are final.
- **Metrics:** `aurora_blob_store_requests_total{backend,op,result}`,
  `aurora_blob_store_seconds{backend,op}`, `aurora_blob_store_up{backend}`,
  `aurora_blob_bytes{backend}` (from `attachments`), `aurora_blob_orphans_removed_total{backend}`.
  Alert on `aurora_blob_store_up == 0` for 2 min: uploads and downloads of attachments
  fail with 503 while it holds; the rest of the API keeps working and `readyz` does not
  change.
- **Backups and DR.** The bucket's durability, replication and backup are the operator's
  job and are part of "stateful" (`§Backups and state`): restoring Postgres without the
  bucket (or the reverse) leaves attachments readable only for the rows whose bytes exist
  (a missing object reads as 503/502 and is logged, never silently empty).
- **Helm:**

  ```yaml
  blobs:
    backend: postgres            # postgres | s3  -> blobs.backend
    s3:
      bucket: ""
      region: ""
      endpoint: ""               # empty = AWS; https://minio.example.org for S3-compatible servers
      allowInsecureHttp: false
      pathStyle: false
      prefix: aurora/
      credentials: {source: provider_chain}   # provider_chain | env
      existingSecret: ""         # source=env: Secret with AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY [/ AWS_SESSION_TOKEN]
      serviceAccountAnnotations: {}   # e.g. eks.amazonaws.com/role-arn for IRSA, on the api ServiceAccount only
      sse: AES256                # AES256 | aws:kms | none
      sseKmsKeyId: ""
      caFile: ""
      allowVersionedBucket: false
      uploadConcurrency: 16
      networkPolicy: {egressTo: []}   # peers/ports the api may reach (default deny stays)
  ```

  `helm template` fails when `backend: s3` has an empty `bucket` or `region`, when
  `credentials.source: env` has no `existingSecret`, or when `existingSecret` and an
  inline key are both set. The chart does not ship MinIO or any bucket.
- **Switching and migrating.** Setting `blobs.backend: s3` changes only where **new**
  attachments go (rolling restart). To move existing attachments run
  `aurora-api blobs migrate --to s3` (resumable; `--dry-run` first; as a Kubernetes
  Job in the chart's `jobs` values). Keep `blobs.s3` configured for as long as any row
  names `s3`: removing it makes those attachments unreadable (503
  `blob_backend_not_configured`), not deleted.

## SCIM directory sync (phase 4, P4-GRP-2, D-132, SEC-72, SEC-73)

Optional; nothing changes for a deployment that does not set `scim.enabled`. SCIM needs **no new
infrastructure**: the API serves it, state is in Postgres, rate limits use the configured
`CounterStore`.

- **Turn it on.** Set `scim.enabled: true` (Helm; the chart then also routes `/scim/v2` at the
  edge, *Networking*) and, if the instance should restrict which domains an org may provision,
  `scim.allowed_email_domains`. An org **owner** then enables SCIM for the org in the dashboard
  (Directory sync), lists the org's e-mail domains, creates a token and pastes the base URL
  (`<external_url>/scim/v2`) and the token into the IdP's provisioning settings.
- **IdP setup** (details in the dashboard guide): Entra ID (*Enterprise application → Provisioning →
  Automatic*, tenant URL = base URL, secret token = the `aud_` token, mappings: `userName`,
  `emails[type eq "work"].value`, `displayName`, `active`, `externalId`; **remove** any `roles`
  mapping), Okta (SCIM 2.0 app with *Push New Users*, *Push Profile Updates*, *Push Groups*,
  unique identifier `userName`), Keycloak (a SCIM extension; same attributes). Aurora ignores the
  enterprise extension.
- **Token rotation.** Tokens expire (default 365 days, at most `scim.token_max_days`). Create
  the new token (up to 5 may be active), update the IdP, revoke the old one. The org owners get a
  notice and an audit row 14 days and 1 day before expiry; an expired token makes the IdP's next
  sync fail with 401, which stops provisioning but never changes existing state.
- **What it never does**: sign anyone in, change a role, create or touch an org owner, change the
  global account of a person who has signed in, link by `externalId`, or reactivate a person an org
  admin suspended by hand (`04 §SCIM`).
- **Observability.** `aurora_scim_requests_total{outcome}` (`ok`, `unauthorized`, `rate_limited`,
  `client_error`, `server_error`) and `aurora_scim_auth_failures_total` (no labels: never an org, token
  name or e-mail); alert on a rising `outcome="unauthorized"` / auth-failure rate for a token that was
  recently valid (expired token or a rotated one still in the IdP) and on `outcome="server_error"`. What
  was provisioned is in the audit log and the Directory sync page, not in a metric (there is no
  `aurora_scim_provisioned_total`).
- **Offboarding runbook.** SCIM deprovisioning does not rotate BYO agent tokens the person saw and
  cannot reassign their automations (`04 §SCIM`); follow the offboarding view of the member
  (`/o/<org>/members`) after the IdP removes them.
- **Helm values**: `scim.enabled` (false) and `scim.ratePerMinute` (600); the other keys are config keys
  (`config.scim.allowed_email_domains` (`[]`), `config.scim.token_max_days` (730)); no Secret is generated (tokens are created in the dashboard).

## Approval park-and-resume and Web Push (phase 5 (D-138, moved from phase 4), RFE-011b, D-133, D-134, SEC-74..SEC-76)

Both are optional and off by default; an existing `aurora.toml` and Helm values change nothing.

**Park and resume** (`approvals.park.*`). Neither infrastructure nor a migration beyond `03` is
needed. Turn it on only when approvers realistically take longer than `approvals.max_timeout_secs`;
set a rule's *Approval timeout* above `approvals.park.threshold_secs` (default 15 min) to use it. The
gateway and the API read the same `approvals.park.*` values (`resume_window_secs` is the API's,
`threshold_secs`/`max_body_bytes`/`max_pending_per_agent`/`deny_cooldown_secs` the gateway's); a
rolling upgrade is safe because an older gateway simply holds (no `parked` row is ever created by it)
and ignores grants. Operators should know the trade-off: the agent is *told* "not sent, wait for a
message", so agents that cannot repeat a request byte-identically (timestamps or nonces in the body)
will need a second approval. Metrics: `aurora_gateway_approvals_parked_total`,
`aurora_gateway_approval_grants_consumed_total{result="consumed"|"no_match"|"expired"}`,
`aurora_approvals_parked_pending`, `aurora_approvals_resume_expired_total`. Alert on a growing
`aurora_approvals_parked_pending` (nobody is deciding).

**Web Push** (`push.*`). Requirements: the external URL is **https** (service workers; `localhost`
works for development), the API can make outbound HTTPS to the push services (Chrome/Edge/Android:
FCM and WNS; Firefox: Mozilla autopush; Safari and iOS Home Screen web apps: Apple) — Helm
`push.networkPolicy.egressTo` adds the peers the api may reach, and nothing else changes in the
default-deny policy; behind an egress proxy configure it at the node or mesh (the destination guard
resolves and pins the real addresses itself). No new Secret is needed: Aurora generates the VAPID key
pair on first start and seals the private key with the usual store; to manage it yourself set
`push.vapid.private_key_env` (Helm `push.vapid.existingSecret`/`key`).
`push.vapid.subject` should be a monitored contact. **Rotate** with `aurora-api push rotate-vapid`:
subscribers re-subscribe silently on their next visit and the old key is destroyed after 30 days
(or earlier when unreferenced); with an operator-managed key a rotation is a redeploy and every
user must re-enable notifications. `aurora-api push status` shows key ids, subscription counts by
state and host. Metrics: `aurora_push_sent_total{result}`, `aurora_push_duration_seconds`,
`aurora_push_subscriptions{state}`; alert on sustained `result=forbidden` (VAPID mismatch after a
rotation or a wrong operator key) and on `result=rejected_host` (a push service moved to a host not
in `push.allowed_endpoint_hosts`). Helm values: `approvals.park.*`, `push.enabled`, `push.vapid.*`,
`push.allowedEndpointHosts`, `push.networkPolicy.egressTo`. A subscription (and so the notification)
belongs to the browser profile and the Aurora session that created it: shared-computer users should
sign out, which ends delivery.

## Port previews (phase 4, P4-PRV-1, P4-PRV-2, D-144..D-147, SEC-80..SEC-82, SEC-85)

**Optional and off by default.** Port previews (`11 §Port previews`) need infrastructure no other
feature needs, so a deployment that does not set `relay.enabled` has no relay, no wildcard DNS or
certificate, no extra Service, no extra Ingress rule and no extra NetworkPolicy; every other
behaviour is unchanged and an upgrade changes nothing. The feature is also off per organization
until an org owner or admin switches **Port previews** on.

**Requirements** (all three, before `relay.enabled: true` is accepted):

1. **A preview domain** `previews.domain`, a DNS suffix **you control**, e.g. `preview.example.org`.
   Browsers treat every host under it as a separate origin, so it is the isolation boundary between
   previews and the dashboard (`04 §Preview access`). Startup validation (API and relay) refuses
   the configuration (exit with the reason) when the domain: is not a valid lower-case DNS name of at
   most 120 characters; **is, contains or sits under** the host of `server.external_url`, of any
   `auth.trusted_origins` entry or of the gateway's advertised proxy host (so a preview hostname can
   never be, or be a parent of, a host that carries a session or a credential); is a single label;
   or is a bare public suffix. Prefer a **separate registrable domain** (`aurora-previews.example.org`
   next to `aurora.example.org` is acceptable, `example.org` is not); the Aurora session cookie is
   host-only and previews never see it either way, but a separate registrable domain also protects
   *other* applications on your domain from cookies a preview app could set.
2. **Wildcard DNS**: `*.<previews.domain>` → the edge (an `A`/`AAAA`/`CNAME` record). Nothing else
   resolves under it; Aurora never creates DNS records.
3. **A wildcard TLS certificate** for `*.<previews.domain>` served by the edge (HTTP-01 cannot issue
   wildcards; use DNS-01 through cert-manager). The chart has no `previews.tls.*` values and renders
   no TLS block for the wildcard host: configure the certificate on your ingress controller or
   Gateway listener ([23 §Port previews (relay)](23-helm-values.md#port-previews-relay)). The relay serves plain HTTP behind the edge (it
   never holds the certificate key) and trusts the edge's overwritten `X-Forwarded-Proto`/`-For`
   exactly like the API (`server.trusted_proxies`). Browsers must reach the preview host on 443;
   previews are **https only** (the edge redirects `http` and the relay refuses to mint a grant for
   a non-https redirect target).

**Turn it on** (one switch): set `relay.enabled: true` and `relay.domain` (the chart value
mirrors `config.previews.enabled`, FIX-056 — set the config key only to agree explicitly,
never alone), wait for `up{job="aurora-relay"}` and the Admin → Overview "Previews" line (relay
replicas seen by the API), then an org owner or admin enables **Port previews** in the org
settings. **Turn it off**: set `relay.enabled: false` (or unset it; shares are kept but
inert; the supervisor allow-list is emptied at the next sync) or switch the org setting
off.

**Wildcard edge rule**: Ingress and HTTPRoute templates add one host rule `*.<previews.domain>`
→ `aurora-relay:4112` only when `relay.enabled` is true (no change to the rules of
`server.external_url`, `/internal` and `/metrics` stay unrouted). The edge must not buffer
responses, must allow WebSocket upgrades, read timeouts ≥ 600 s and request bodies up to
`PREVIEW_REQUEST_BODY_MAX`. The edge-routes guard is extended to render the chart with previews
enabled and check that the wildcard host routes **only** to the relay and that the dashboard
host is untouched (an implementation task of P4-PRV-2).

**nginx WebSocket recipe (edge front door)**: "allow WebSocket upgrades" concretely means the
Upgrade/Connection headers must reach the backend — an nginx edge that proxies with
`proxy_set_header Connection "";` (the keepalive idiom) silently strips the browser's
handshake; the dashboard renders but every WebSocket feature (terminal, previews live view)
dies at the edge with nothing in the api logs. Found in production 2026-11-12
(aurora-ui.cern.ch). The minimal working posture:

```nginx
# http context: map the hop-by-hop Connection header to the upgrade
map $http_upgrade $connection_upgrade {
    default upgrade;
    ''      close;
}
# server/location context, on every location proxying /v1 (or `/`):
proxy_http_version 1.1;
proxy_set_header Upgrade $http_upgrade;
proxy_set_header Connection $connection_upgrade;
```

Keep the chart's defaults for the rest (`proxy_buffering off`, `proxy_read_timeout`/
`proxy_send_timeout` ≥ 600 s — an idle terminal longer than the read timeout is cut by the
edge; the tab auto-reconnects, but raise the timeout to avoid the visible reconnect). Note on
`http2 on`: browsers do not send HTTP/1.1-style Upgrade over h2 (nginx has no RFC 8441
CONNECT-UDP/extended connect), so the WebSocket arrives as a separate HTTP/1.1 connection —
h2 on the vhost is fine as long as the headers above pass through. The same class was fixed
inside the product in FIX-052 (the gateway must not send `Host` beside `:authority` on h2
upstream hops); front-door oracle tests are TEST-021.

**Runners.** The relay reaches each runner replica on its **stream listener** `runner.stream_bind`
(default `server.bind`, port **4108**, `16 §W17`), at the replica's `advertised_host`. A runner
inside the cluster needs only the NetworkPolicy of *Port previews relay* below; a runner **outside**
the cluster must make 4108 reachable from the relay (overlay network or private link). Without
that, previews for sandboxes of that runner answer 502 `preview_unreachable` and the dashboard
says so; nothing else is affected. The sandboxes namespace never reaches 4108 (SEC-09 style
guard, `18`).

**The relay** (`aurora-relay`, image `aurora-relay`, P4-PRV-2). A stateless Rust service (hyper,
`tokio-tungstenite`; no database, no key, no Vault). Helm (`relay.*`; rendered **only** when
`relay.enabled`, FIX-056): Deployment `aurora-relay` (`relay.replicas`, default 1; `relay.resources`, PodDisruptionBudget,
read-only root filesystem, dropped capabilities, non-root), Service on 4112 (edge-routed) and a headless
ops Service on 4193. Its anchor token `aul_` is generated by the `secrets` hook into **one** key of the
instance Secret, `AURORA_RELAY_TOKEN` (never shown): the relay reads it under that name and the chart hands
the same key to the API as its `AURORA_BOOTSTRAP_RELAY_TOKEN` env var (FIX-069: the hook used to also generate a
separate `AURORA_BOOTSTRAP_RELAY_TOKEN` secret key that nothing read; an instance that has it keeps it,
harmlessly, and it can be deleted). With `secrets.existingSecret`, create the key **`AURORA_RELAY_TOKEN`**
(`aul_` + 48 hex characters) when `relay.enabled`. The relay's NetworkPolicy allows ingress from the edge on
4112 and egress to the API (:4101), the runners' stream listener (:4108) and DNS only — no external egress
(FIX-069; it is an edge-exposed pod and talks to nothing else). The relay sets no cookie other than `__Host-aurora_preview` and
`__Host-aurora_pstate` and keeps no state beyond in-memory caches (resolve results ≤ 15 s), so a
restart loses only open streams.

**NetworkPolicy** (default deny stays; each rule is rendered only with `relay.enabled`, FIX-056):
relay ingress from the edge on 4112; **no policy of the chart opens any ops/metrics port** (4193 included, like
the other components' ops ports; the chart has no monitoring-namespace selector, FIX-074 decision), so a
Prometheus in another namespace needs an additional NetworkPolicy of the operator's own allowing its pods to the
component's ops port; relay egress to **api:4101** and **runner:4108** (runner pods of this release;
a runner outside the cluster must be reachable from the relay by the overlay/private link described
above, there is no chart value for extra egress) and DNS **only**: no Postgres, no
gateway, no hook host, no sandboxes, no Internet; api ingress from the relay on 4101; runner ingress
on 4108 from the relay pods **only** (the sandboxes namespace gets nothing new: sandboxes → gateway:4102
and runner:4105 only, unchanged). The `aurora-relay` pod runs in the control-plane namespace, never in
the sandboxes namespace.

**Metrics** (4193 `/metrics`, plus one API gauge and counter; FIX-074: this list is what the code emits):
`aurora_relay_requests_total{outcome}` (`ok`, `login`, `redeemed`, `not_found`, `bad_request`, `cross_site`,
`quota`, `too_large`, `unreachable`, `upstream_error`, `invalid`), `aurora_relay_streams_open` (gauge),
`aurora_relay_stream_bytes_total{direction}` (`to_app`, `from_app`), `aurora_relay_stream_closed_total{reason}`
(`closed`, `eof`, `detached`, `revoked`, `idle`, `lifetime`), `aurora_relay_stream_dial_seconds` (histogram) and
`aurora_relay_stream_dial_errors_total{kind}` (`connect`, `timeout`), `aurora_relay_grants_total{result}`
(`redeemed`, `invalid`, `no_state`), `aurora_relay_resolve_cache{hit}` (`true`/`false`), `aurora_relay_refused_total{reason}` (FIX-072, `docs/16 §W18`, C-636:
`share_streams`, `user_streams`, `quota`, `grant_rate_address`, `grant_rate_share`; no ids in labels; the matching responses are 429 `preview_streams_limit` / WebSocket 4409,
429 `preview_quota` with `Retry-After`, and 429 `grant_rate_limited`); API (every replica):
`aurora_preview_shares{org}` (active shares per org slug, set by the share job) and
`aurora_preview_stream_bytes_total{direction}`. There is no `aurora_relay_up`: relay liveness is the
Prometheus-generated `up{job="aurora-relay"}` of the 4193 scrape (`== 0` means the relay is down or unscrapable)
and the Deployment's readiness. Alert on `up{job="aurora-relay"} == 0`, on a rising
`grants_total{result="no_state"}` or `result="invalid"` (attempted login CSRF, a replayed or tampered link) and
on a rising `aurora_relay_stream_dial_errors_total` (a runner's 4108 unreachable). Not emitted (earlier drafts
named them): `aurora_relay_request_duration_seconds`, `aurora_relay_wake_wait_seconds`,
`aurora_relay_resolve_cache_hit_ratio` (compute it from `aurora_relay_resolve_cache`), `aurora_preview_sessions`,
`aurora_preview_wakes_total`, `aurora_runner_preview_*`. Logs carry the label, share id, outcome and byte counts;
never paths with query strings, headers, cookies or bodies.

**Upgrade and rollback.** The relay and the preview columns are additive: an older API ignores them, an
older runner reports no `previews` capability. Rolling back removes the relay Deployment; shares stay in
the database inert. Preview traffic is not part of the request log.

## Web terminal operations (phase 4, P4-TERM, D-161, SEC-87; FIX-061)

The browser leg of every terminal is a WebSocket **terminated on one API replica**, so the live-session caps
`TERMINALS_PER_USER` (2) and `TERMINALS_PER_AGENT` (4) are counted **per API replica** in memory (a guard
released on every exit path: close, error, dial failure, panic; nothing is stored, a restart starts at zero).
With N API replicas the effective ceilings are N× those numbers (a user can hold 2 terminals on each replica);
`TERMINALS_PER_SANDBOX` is still enforced at the runner across replicas. Over the cap the upgrade completes and
the socket closes `4409`. An open terminal re-checks its holder every 10 s (dashboard session live, org
membership, `can_use`, agent not frozen) and closes `4403` when access is lost.

## Git skill library (phase 4, P4-GSL, D-148, D-149, SEC-83, SEC-84)

Optional by use: nothing to deploy, no new Service, Secret or NetworkPolicy. Requirements: the gateway
can reach the git host's API **as it already does for agents** (the host is already one of the app's
claimed hosts; the gateway's egress policy covers it), the API can reach the gateway's internal
port 4103 (already true), and the **git-hosting package** (the built-in GitHub or GitLab package, at
the release that adds the `library` block, re-approved by an instance admin like any package update,
`09 §10`) is installed and approved. To use it an **org owner/admin** (1) connects the git host as an
**org-scoped** app connection with a **read-only** credential (a fine-grained token limited to the
skills repository is ideal; GitHub: *Contents: read*; GitLab: *read_repository*), then (2) Skills →
Libraries → *Connect a repository*. Disable with `skills.library.enabled: false` (stored skills keep
working; the dashboard hides the feature and the refresh job stops).

- **What leaves the cluster**: `GET`s of repository metadata and the files of skills people added or
  browsed, from the git host's API, through the gateway. **What is stored**: only skills added to
  agents, workspaces or the org (content, commit sha, blob ids) and open proposed updates (bounded,
  `02 §Limits`). Nothing is cloned.
- **Rate limits of the host**: a library with added skills costs one `resolve_ref` call per
  `LIBRARY_REFRESH_INTERVAL` (15 min) while unchanged, plus a tree and changed files after a push;
  browsing costs a tree and up to 200 file reads (cached 5 minutes). Self-hosted instances apply
  their own limits; raise the interval with `skills.library.refresh_interval_secs`.
- **Operations**: the Libraries page shows each library's status and last error (revoked token, repository
  moved, host unreachable). A library in `error` leaves agents untouched. Metrics:
  `aurora_skill_library_refresh_total{result}` (`unchanged`, `proposed`, `error`),
  `aurora_skill_library_pending_updates`, `aurora_skill_library_refresh_duration_seconds`,
  `aurora_gateway_library_fetch_total{step,status}`. Alert on sustained `result="error"` and on pending
  updates older than a week (nobody is reviewing: agents keep running the pinned skills, which is safe
  but stale).
- **Upgrade and rollback**: additive tables and columns; rolling the API back leaves the tables and the
  pinned skills (which are ordinary skill rows) in place and the library features disappear.
