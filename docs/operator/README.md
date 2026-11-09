# Aurora operator guide

This guide covers installing Aurora, configuring LLM endpoints, adding a runner,
approving plugins and connectors, backing up state and upgrading, and (phases 1 and 2) the
sign-in, gateway, retention, limits, approval, catalogue and freeze settings.

The design source of truth is [`docs/15-deployment-operations.md`](../15-deployment-operations.md).
Every configuration key, route and command in these pages was checked against the
code in this repository:

- configuration keys: `crates/aurora-config/src/raw.rs` and `validate.rs`
- CLI flags: the clap definitions in `services/aurora-*/src/main.rs` and `services/aurora-cli/src/`
- HTTP routes: `services/aurora-api/src/domains/*/routes.rs`
- artifacts: `deploy/helm/aurora/`, `deploy/kind/`, `deploy/dev/`

Where the spec describes something the code does not do yet, the text says
**(not yet implemented)**. Paths such as `POST /v1/admin/runners` are relative to the
public origin, so the full URL is `https://<your-host>/v1/admin/runners`.

## Pages

| Page | Covers |
|------|--------|
| [install-helm.md](install-helm.md) | Kubernetes install with the Helm chart (`deploy/helm/aurora/`) |
| [install-kind-dev.md](install-kind-dev.md) | Local development and test stack on kind with Helm (`deploy/dev/kind-dev.sh`, phase 1, OPS-017; replaces podman compose) |
| [llm-endpoints.md](llm-endpoints.md) | `[[llm.endpoints]]`, key origin, governance ceilings |
| [runners.md](runners.md) | Adding a runner, podman and kubernetes backends, `runner.k8s.*` |
| [plugins-and-connectors.md](plugins-and-connectors.md) | Plugin upload and approval, trusted keys, host claims, connector provisioning and approval |
| [backups.md](backups.md) | Stateful components, what to back up, restore order, key pointers |
| [key-rotation.md](key-rotation.md) | Rotating the signing key and the internal secret (rings, `aurora-api keys`, status endpoints) |
| [upgrades.md](upgrades.md) | Upgrade order, migrations, running sandboxes during an upgrade |
| [sign-in.md](sign-in.md) | OIDC-only sign-in (`auth.oidc_providers`), first administrator, the development IdP (`auth.dev_idp`, loopback only) |
| [gateway-posture.md](gateway-posture.md) | TLS proxy listener (`gateway.proxy_tls.*`), open-lane posture, BYO posture and source allow-lists (`byo.*`, `gateway.trusted_proxies`, PROXY protocol), the database lane |
| [retention-and-quota.md](retention-and-quota.md) | `retention.conversation_days_max`, `attachments.*`, org settings and the daily sweeps |
| [limits-and-approvals.md](limits-and-approvals.md) | API rate limits (`ratelimit.api.*`), approval timeouts (`approvals.*`), approval e-mail (`email.*`) and streaming fan-out (`events.*`) |
| [apps-and-catalogue.md](apps-and-catalogue.md) | The built-in catalogue (signing, icons) and app instances for self-hosted servers |
| [freeze.md](freeze.md) | The emergency freeze of an agent, workspace or organization |

## Phase 1 and 2 topics

Where to find each phase 1 and phase 2 setting. The key table with every default stays
[docs/15 §Configuration reference](../15-deployment-operations.md#configuration-reference-keys-and-defaults).

| Topic | Section |
|-------|---------|
| TLS proxy listener | [gateway-posture.md §TLS proxy listener](gateway-posture.md#tls-proxy-listener) |
| OIDC-only sign-in and the dev IdP | [sign-in.md](sign-in.md) |
| Open-lane posture and the browser capability | [gateway-posture.md §Open lane](gateway-posture.md#open-lane) |
| BYO posture, source allow-lists, `gateway.trusted_proxies`, PROXY protocol | [gateway-posture.md §BYO agents](gateway-posture.md#byo-agents-posture-and-source-allow-lists) |
| Retention and attachment quota | [retention-and-quota.md](retention-and-quota.md) |
| API rate limits | [limits-and-approvals.md §API rate limits](limits-and-approvals.md#api-rate-limits) |
| Signing-key and internal-secret rotation | [key-rotation.md](key-rotation.md) |
| Approval timeouts and e-mail | [limits-and-approvals.md §Approval timeouts](limits-and-approvals.md#approval-timeouts) |
| App instances (D-74) | [apps-and-catalogue.md §App instances](apps-and-catalogue.md#app-instances-self-hosted-servers) |
| Built-in catalogue | [apps-and-catalogue.md §The built-in catalogue](apps-and-catalogue.md#the-built-in-catalogue) |
| Database lane | [gateway-posture.md §Database lane](gateway-posture.md#database-lane) |
| Freeze | [freeze.md](freeze.md) |
| Sandbox resources and private sandbox registries | [runners.md §Sandbox resources](runners.md#sandbox-resources), [Private sandbox registry](runners.md#private-sandbox-registry) |
| Getting the images (publish, pull, build and load) | [install-helm.md §Images](install-helm.md#images-publish-pull-or-build-and-load) |

## Things to know first

- **Images.** Service images are built from `images/services/Containerfile`
  (targets `api`, `gateway`, `hookhost`, `runner`, `connector-fake`) and
  `images/services/web.Containerfile`. The sandbox image is `images/sandbox`.
  See [docs/15 §Images](../15-deployment-operations.md#images).
- **Config.** Each service reads one TOML file (`AURORA_CONFIG`, default
  `/etc/aurora/aurora.toml`). Environment overrides use `AURORA__<SECTION>__<KEY>`
  (double underscores, upper case), for example `AURORA__GATEWAY__INTERNAL_BIND`.
  Services refuse to start on any validation error and name the key.
- **Secrets.** Secrets come from environment variables or the install tooling, never
  from the TOML file. Setup tooling generates missing secrets and never overwrites an
  existing one. See [docs/15 §Required secrets](../15-deployment-operations.md#required-secrets).
- **Admin actions.** `/v1/admin/*` routes require an instance-administrator session.
  The first administrator is pre-provisioned with `aurora-api bootstrap-admin` and binds their
  identity at the first OIDC sign-in (see [install-helm.md](install-helm.md#post-install-checks)).
- **Edge.** Only `server.external_url` is public. The edge routes `/v1/*` to the API and
  everything else to the web app. `/internal/*` and `/metrics` are never routed. See
  [docs/15 §Networking: one URL](../15-deployment-operations.md#networking-one-url).

## Known gaps (checked against the code)

These are places where the spec, the chart or a comment says something the code does
not do. Treat each as **(not yet implemented)** until the code changes. Gap numbers are
stable (other tasks refer to them). Gaps 3, 4, 5, 6, 9, 10 and 13 were fixed or documented in
OPS-013, and gaps 1, 2, 7 and 8 were fixed in FIX-003; all are listed as resolved below.

- **11. Compose runner backend.** Removed in phase 1 (OPS-017): podman compose no longer ships. The Helm chart and the kind dev stack
  use the `kubernetes` backend ([install-kind-dev.md](install-kind-dev.md)).
- **12. Sandbox env.** Per `docs/PROGRESS.md` (FUP-M7), the API's `SandboxSpec.env` is
  empty, so `AURORA_AGENT_HANDLE` and snapshot app env do not reach sandboxes on any
  backend. See [runners.md](runners.md#known-limitations).

### Resolved in FIX-003

- **1. Connector bootstrap tokens.** The API applies `AURORA_BOOTSTRAP_CONNECTOR_TOKENS` at
  startup: each `id:aun_…` entry upserts the connector with the hashed token (status
  `awaiting_registration` when new; an existing connector keeps its registration state).
  Ids must match `[a-z][a-z0-9-]{1,31}`. See [plugins-and-connectors.md](plugins-and-connectors.md#connectors).
- **2. Connector manifests and `plugins.allow_unsigned`.** With the flag set, a registered
  connector manifest is auto-approved (status `active`, also when it widens the approved
  one) and a warning is logged. Signature checks still apply. Development only.
- **7. OpenTelemetry export.** When `telemetry.otlp_endpoint` is set, every service exports
  traces over OTLP/HTTP protobuf (rustls; a base URL gets `/v1/traces` appended) and propagates
  W3C `traceparent` on its calls to the hook host. Without the key nothing is installed.
- **8. Parsed but unused keys.** `gateway.condition_body_bytes` sets the body prefix buffered
  for policy conditions (never below the 16 KiB approval peek), `gateway.resolve_cache_ttl_secs`
  the host-resolution cache TTL, and `runner.held_awake_ceiling` overrides the derived
  eviction ceiling `max(1, max_sandboxes − 1)`. The runner reports the ceiling at
  registration and the API's eviction uses it for that runner.

### Resolved in OPS-013

- **3. Gateway database URL.** The Helm values comment no longer mentions
  `AURORA_GATEWAY_DATABASE_URL`. The gateway uses `AURORA_DATABASE_URL`, and
  `database.gatewayRole` is the least-privilege switch.
- **4. Sandbox node selector and tolerations.** `runner.k8s.nodeSelector` and
  `runner.k8s.tolerations` render into `[runner.k8s] node_selector` and `tolerations`.
- **5. Plugin packages in Helm.** `plugins.source` mounts packages from a ConfigMap (copied by
  an init container) or an existing PVC at `plugins.dir`. See [install-helm.md](install-helm.md#plugin-packages).
- **6. Connector port key.** The chart reads `callbackPort` (default 4190). The values comment
  and examples use it; `port` is gone.
- **9. `just dev`.** Postgres in podman, migrate, then the API, gateway, hook host and web
  app on the host. See [install-kind-dev.md](install-kind-dev.md#inner-loop-on-the-host-just-dev).
- **10. Gateway CA.** Compose is gone (phase 1, OPS-017). The chart's `gateway-ca` hook mints the CA when it is absent and
  never overwrites it ([install-helm.md](install-helm.md)).
- **13. Helm evaluation Postgres.** Its hook resources cannot be removed by a hook delete
  policy, so the chart documents it instead: `values.yaml` and `NOTES.txt` say it is
  evaluation-only and leaks on uninstall, with the cleanup commands. See [install-helm.md](install-helm.md#database).
