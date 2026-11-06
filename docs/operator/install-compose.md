# Install with podman compose

This is the development and single-host install described in
[docs/15 §Development](../15-deployment-operations.md#development-podman-compose-deploycompose).
It runs every phase-0 service on one host, with the edge on port 4000.
For Kubernetes, see [install-helm.md](install-helm.md).

Compose is for development and evaluation. The shipped `aurora.toml` uses the `fake`
runner backend and `http://localhost:4000` as the public origin.

## Prerequisites

- podman with `podman compose`. The `justfile` calls `podman compose -f deploy/compose/compose.yaml`.
- `openssl` (used by `setup.sh` to generate secrets).
- Free host ports: **4000** (edge), **4102** (gateway proxy). Postgres and the API are not published.
- Network access to pull `docker.io/library/postgres:16` and `docker.io/library/caddy:2`,
  and to build the Rust and web images.

## Files

| File | Purpose |
|------|---------|
| `deploy/compose/compose.yaml` | Services, networks, volumes, profiles |
| `deploy/compose/aurora.toml` | Shared config, mounted at `/etc/aurora/aurora.toml` |
| `deploy/compose/.env.example` | Template for `.env` (`AURORA_VERSION`, secrets, `COMPOSE_PROFILES`) |
| `deploy/compose/setup.sh` | Idempotent setup: `.env`, secrets, `aurora-sandbox` network |
| `deploy/compose/Caddyfile` | Edge routing: `/v1/*` to the API (4101), everything else to the web app (4100) |

## 1. Generate secrets and the sandbox network

```bash
cd deploy/compose
./setup.sh
```

`setup.sh` is idempotent. It does the following:

- copies `.env.example` to `.env` (mode 600) if `.env` does not exist;
- fills every empty secret: `POSTGRES_PASSWORD`, `AURORA_ENCRYPTION_KEYS` (`k1:<base64 32 bytes>`),
  `AURORA_ENCRYPTION_ACTIVE_KEY` (`k1`), `AURORA_SIGNING_KEY`, `AURORA_INTERNAL_SECRET`,
  `AURORA_RUNNER_SESSION_KEY`, `AURORA_RUNNER_TOKEN` (`aur_…`) and `AURORA_CONNECTOR_TOKEN` (`aun_…`);
- creates the `aurora-sandbox` network as `--internal --disable-dns --subnet 10.89.250.0/24`
  if it does not exist.

Do not run `podman compose down -v` unless you intend to delete the Postgres and CA volumes.

## 2. Create the gateway CA

`setup.sh` does not mint the gateway CA. Its own output says so
(`TODO(M2)`). The gateway loads `ca.pem` and `ca.key` from `gateway.ca_dir`
(`/var/lib/aurora/ca`, the `aurora-ca` volume) at startup and fails without them.

The `setup.sh` message suggests `podman run --rm -v aurora-ca:/ca aurora-gateway generate-ca --out /ca`.
Compose names project volumes with the project prefix (`name: aurora`), so that
command may write to a different volume. The following uses the service's own mount.
**Not run in this environment (no container runtime was exercised); check with `podman volume ls` after running it:**

```bash
podman compose -f compose.yaml run --rm --no-deps gateway generate-ca --out /var/lib/aurora/ca
```

`generate-ca` writes `ca.pem` (0644) and `ca.key` (0600) and refuses to overwrite
existing files (`services/aurora-gateway/src/ca.rs`).

## 3. Start the stack

```bash
podman compose -f deploy/compose/compose.yaml up -d
```

`just compose-up` runs `setup.sh` and then `up -d`.

Start order (from `depends_on`): `postgres` (healthy) → `migrate` (one-shot
`aurora-api migrate`, must complete) → `api`, `gateway`. `hookhost` and `web`
depend on the API or start alongside it, and the `edge` starts after `api` and `web`.

Optional profiles, set in `.env` under `COMPOSE_PROFILES` or with `--profile`:

| Profile | Service | Notes |
|---------|---------|-------|
| `hosted` | `runner` | Needed for hosted agents. Mounts `${XDG_RUNTIME_DIR}/podman/podman.sock`. |
| `connector-fake` | `connector-fake` | Test connector, control API on 4190. |

## 4. Verify

```bash
curl -i localhost:4000/v1/instance
curl -i localhost:4102
```

The edge is the only HTTP entry point. `/healthz`, `/readyz` and `/metrics` are on
each service's internal or ops listener and are not reachable through the edge
(docs/15 §Observability). The distroless images have no shell or curl, so compose
waits only for `service_started` (Postgres has a health check).

## First administrator

```bash
podman compose -f deploy/compose/compose.yaml run --rm api bootstrap-admin --email you@example.org
```

`bootstrap-admin` creates or promotes a verified instance administrator and prints a
one-time password-set link built from `server.external_url`
(`services/aurora-api/src/main.rs`, `BootstrapAdmin`). The `setup.sh` message says this
command is "available once BE-016 lands". The subcommand exists in the code. Confirm it
works on your first install.

## Configuration in `deploy/compose/aurora.toml`

The shipped file sets:

| Key | Value | Notes |
|-----|-------|-------|
| `server.external_url` | `http://localhost:4000` | Public origin. Change it for any other host. |
| `server.bind` | `0.0.0.0` | Sandbox-facing listeners (gateway 4102, runner channel 4105). |
| `server.trusted_proxies` | `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16` | Trusted `X-Forwarded-For` source (the edge). |
| `runner.backend` | `"fake"` | Change to `"podman"` to run real sandboxes (see [runners.md](runners.md#podman-backend)). |
| `runner.api_url` | `http://aurora-api:4101` | |
| `hosted.enabled` | `true` | |
| `telemetry.log_level` | `info` | |

Compose sets the listener binds that must not be sandbox-reachable through environment
overrides (SEC-09):

- `AURORA__GATEWAY__INTERNAL_BIND: 10.89.249.2` (gateway internal listener 4103)
- `AURORA__RUNNER__OPS_BIND: 10.89.249.3` (runner ops listener 4106)

Static addresses: gateway `10.89.250.2` and runner `10.89.250.3` on the sandbox
network; `10.89.249.0/24` for the core network.

Switching to real sandboxes also requires `runner.sandbox_hosts`
(`{ "aurora-gateway" = "10.89.250.2", "aurora-runner" = "10.89.250.3" }`). The shipped
file does not set it. Without it, sandboxes get no `/etc/hosts` entries for the gateway and runner.

## Bootstrap tokens in compose

`.env` values flow into the API through compose:

- `AURORA_BOOTSTRAP_RUNNER_TOKEN` is set to `AURORA_RUNNER_TOKEN`. On startup the API
  upserts the runner `default` with that token's hash (`bootstrap_default`).
- `AURORA_BOOTSTRAP_CONNECTOR_TOKENS` is set to `fake:<AURORA_CONNECTOR_TOKEN>`. The
  API does not apply it (**not yet implemented**, see [plugins-and-connectors.md](plugins-and-connectors.md#connectors)).

## Stop and logs

```bash
podman compose -f deploy/compose/compose.yaml logs -f api
podman compose -f deploy/compose/compose.yaml down
```

`down` keeps the named volumes (`aurora-pg`, `aurora-ca`). Back them up before removing them
(see [backups.md](backups.md)).

## Developer loop without containers

`just dev` (mentioned in docs/15) does not exist in the `justfile`. What exists:

- `just dev-db`: starts Postgres 16 in podman on `127.0.0.1:55432`.
- `just migrate`: runs `cargo run -p aurora-api -- migrate` with `deploy/dev/env.sh` sourced.
- `deploy/dev/env.sh` sets throwaway secrets and `AURORA_CONFIG=deploy/dev/aurora.dev.toml`
  (dev only; never reuse these values).
