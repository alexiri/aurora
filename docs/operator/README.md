# Aurora operator guide (phase 0)

This guide covers installing Aurora, configuring LLM endpoints, adding a runner,
approving plugins and connectors, backing up state and upgrading.

The design source of truth is [`docs/15-deployment-operations.md`](../15-deployment-operations.md).
Every configuration key, route and command in these pages was checked against the
code in this repository:

- configuration keys: `crates/aurora-config/src/raw.rs` and `validate.rs`
- CLI flags: the clap definitions in `services/aurora-*/src/main.rs` and `services/aurora-cli/src/`
- HTTP routes: `services/aurora-api/src/domains/*/routes.rs`
- artifacts: `deploy/compose/`, `deploy/helm/aurora/`, `deploy/dev/`

Where the spec describes something the code does not do yet, the text says
**(not yet implemented)**. Paths such as `POST /v1/admin/runners` are relative to the
public origin, so the full URL is `https://<your-host>/v1/admin/runners`.

## Pages

| Page | Covers |
|------|--------|
| [install-compose.md](install-compose.md) | Single-host install with podman compose (`deploy/compose/`) |
| [install-helm.md](install-helm.md) | Kubernetes install with the Helm chart (`deploy/helm/aurora/`) |
| [llm-endpoints.md](llm-endpoints.md) | `[[llm.endpoints]]`, key origin, governance ceilings |
| [runners.md](runners.md) | Adding a runner, podman and kubernetes backends, `runner.k8s.*` |
| [plugins-and-connectors.md](plugins-and-connectors.md) | Plugin upload and approval, trusted keys, host claims, connector provisioning and approval |
| [backups.md](backups.md) | Stateful components, what to back up, restore order, key pointers |
| [upgrades.md](upgrades.md) | Upgrade order, migrations, running sandboxes during an upgrade |

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
  The first administrator is created with `aurora-api bootstrap-admin` (see
  [install-compose.md](install-compose.md#first-administrator)).
- **Edge.** Only `server.external_url` is public. The edge routes `/v1/*` to the API and
  everything else to the web app. `/internal/*` and `/metrics` are never routed. See
  [docs/15 §Networking: one URL](../15-deployment-operations.md#networking-one-url).

## Known gaps (checked against the code)

These are places where the spec, the chart or a comment says something the code does
not do. Treat each as **(not yet implemented)** until the code changes.

1. **Connector bootstrap tokens.** `AURORA_BOOTSTRAP_CONNECTOR_TOKENS` is read and
   validated, but nothing applies it. Compose sets it for the fake connector, which
   therefore needs manual provisioning. See [plugins-and-connectors.md](plugins-and-connectors.md#connectors).
2. **Connector manifests and `plugins.allow_unsigned`.** The flag applies to plugin
   packages only. docs/15 says it also auto-approves connectors; the connector
   approval code does not read it.
3. **Gateway database URL.** The Helm values comment mentions
   `AURORA_GATEWAY_DATABASE_URL`. No code reads it; the gateway uses
   `AURORA_DATABASE_URL`. A least-privilege gateway role is set with `database.gatewayRole`.
4. **Sandbox node selector and tolerations.** `runner.k8s.nodeSelector` and
   `runner.k8s.tolerations` in `values.yaml` are not rendered into `aurora.toml`
   (confirmed with `helm template`), so sandbox pods do not receive them.
5. **Plugin packages in Helm.** docs/15 describes plugin packages mounted from a
   ConfigMap or PVC. The chart has no value or template for this.
6. **Connector port key.** The `connectors[]` comment in `values.yaml` lists `port`.
   The template reads `callbackPort` (default 4190) and ignores `port`.
7. **OpenTelemetry export.** `telemetry.otlp_endpoint` is validated, but no exporter
   reads it.
8. **Parsed but unused keys.** `gateway.condition_body_bytes`,
   `gateway.resolve_cache_ttl_secs` and `runner.held_awake_ceiling` are parsed and
   validated, but no runtime code reads them.
9. **`just dev`.** docs/15 describes a `just dev` loop. The `justfile` has `dev-db`,
   `migrate`, `compose-up` and `compose-down` only.
10. **Compose gateway CA.** `setup.sh` does not mint the gateway CA (its own TODO(M2)).
    The gateway fails to start without it. See [install-compose.md](install-compose.md#2-create-the-gateway-ca).
11. **Compose runner backend.** `deploy/compose/aurora.toml` sets `runner.backend = "fake"`.
    Hosted agents run as fake sandboxes until the operator switches the backend.
12. **Sandbox env.** Per `docs/PROGRESS.md` (FUP-M7), the API's `SandboxSpec.env` is
    empty, so `AURORA_AGENT_HANDLE` and snapshot app env do not reach sandboxes on any
    backend. See [runners.md](runners.md#known-limitations).
13. **Helm evaluation Postgres.** Per FUP-M7, its hook resources are not removed on
    uninstall. Use an external database in production.
