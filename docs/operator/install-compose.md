# Install with podman compose

This is the development and single-host install described in
[docs/15 §Development](../15-deployment-operations.md#development-podman-compose-deploycompose).
It runs every phase-0 service on one host, with the edge on port 4000.
For Kubernetes, see [install-helm.md](install-helm.md).

Compose is for development and evaluation. The shipped `aurora.toml` uses the `fake`
runner backend (the `hosted` profile switches the runner to podman) and `http://localhost:4000` as the public origin.

## Prerequisites

- podman with `podman compose`. The `justfile` calls `podman compose -f deploy/compose/compose.yaml`.
- `openssl` (used by `setup.sh` to generate secrets).
- For the `hosted` profile: an active rootless podman API socket
  (`systemctl --user enable --now podman.socket`) and a sandbox image in your own podman
  (see [Hosted agents](#hosted-agents-profile-hosted)).
- Free host ports: **4000** (edge), **4102** (gateway proxy). Postgres and the API are not published.
- Network access to pull `docker.io/library/postgres:16` and `docker.io/library/caddy:2`,
  and to build the Rust and web images.

## Files

| File | Purpose |
|------|---------|
| `deploy/compose/compose.yaml` | Services, networks, volumes, profiles |
| `deploy/compose/aurora.toml` | Shared config, mounted at `/etc/aurora/aurora.toml` |
| `deploy/compose/.env.example` | Template for `.env` (`AURORA_VERSION`, secrets, `COMPOSE_PROFILES`) |
| `deploy/compose/setup.sh` | Idempotent setup: `.env`, secrets, `aurora-sandbox` network, gateway CA |
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
- writes `AURORA_UID` and `AURORA_GID` (your uid/gid; the hosted runner runs as this user) and
  warns, with the fix, if `systemctl --user is-active podman.socket` is not active;
- creates the `aurora-sandbox` network as `--internal --disable-dns --subnet 10.89.250.0/24`
  if it does not exist;
- mints the gateway CA into the `aurora-ca` volume if the volume has none (see [section 2](#2-create-the-gateway-ca)).

It also copies the public `ca.pem` out of the volume to `deploy/compose/ca/ca.pem` (git-ignored); compose
mounts it read-only into the `api` service, which hands it to the runner when a sandbox boots. `ca.key` stays
in the volume, readable only by the gateway.

Do not run `podman compose down -v` unless you intend to delete the Postgres and CA volumes.

## 2. Create the gateway CA

`setup.sh` mints the gateway CA into the `aurora-ca` volume (`aurora-gateway generate-ca`)
when the volume holds no CA. The gateway loads `ca.pem` and `ca.key` from `gateway.ca_dir`
(`/var/lib/aurora/ca`, the `aurora-ca` volume) at startup and fails without them. The
volume has a fixed name (`name: aurora-ca` in `compose.yaml`), so the command below and
the gateway mount the same volume.

`setup.sh` runs this as part of its normal run, and running it again is safe: an existing CA is
never touched. `generate-ca` writes `ca.pem` (0644) and `ca.key` (0600), and refuses to overwrite
existing files (`services/aurora-gateway/src/ca.rs`). If only one of the two files exists,
`setup.sh` stops with that error and changes nothing. Check with:

```bash
podman run --rm -v aurora-ca:/ca:ro --entrypoint /bin/sh docker.io/library/postgres:16 -c 'ls -l /ca'
```

Back up `ca.key` with the other keys (see [backups.md](backups.md#keyring-signing-key-and-ca)).

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
| `hosted` | `runner` | Needed for hosted agents. Podman backend, mounts `${XDG_RUNTIME_DIR}/podman/podman.sock` (see [Hosted agents](#hosted-agents-profile-hosted)). |
| `llm-mock` | `vidaimock`, `litellm` | Scripted mock model for `claude-code` agents; dev only ([llm-endpoints.md](llm-endpoints.md#running-with-the-mock-dev-and-e2e-only)). |
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
(`services/aurora-api/src/main.rs`, `BootstrapAdmin`). Verified on a fresh install (AUDIT-001):
open the printed link's token with `POST /v1/auth/password/reset`, then sign in. The shipped
config has `auth.org_creation = "instance_admins"` and `signup_mode = "invite_only"`, so the
administrator creates the first organization (`POST /v1/orgs`), which also creates the default
workspace and the hosted agent Daneel. Cookie-authenticated `curl` calls need an `Origin:
http://localhost:4000` header (SEC-22).

## Hosted agents (profile `hosted`)

Without the profile no runner exists, so chat turns stay queued. With `COMPOSE_PROFILES=hosted` the `runner`
service starts with the **podman backend** (`AURORA__RUNNER__BACKEND=podman`), the sandbox image from
`AURORA_SANDBOX_IMAGE` in `.env` (`runner.image`) and the `aurora-sandbox` network. It talks to the host's rootless podman,
so sandboxes are ordinary containers of your user, named `aurora-sbx-*`.

Why the runner runs as you: the socket is `srw-rw---- you:you`. Under rootless podman's default user namespace the
image's distroless uid 65532 maps to a host sub-uid that cannot open it. The runner service therefore sets
`userns_mode: keep-id` and `user: ${AURORA_UID}:${AURORA_GID}`, so it is the same uid as the socket owner. That is no more
privilege than the socket access itself (whoever can open the socket can start any container as you), and no other
service changes. Treat the `hosted` profile as giving the stack control of your user's podman.

Before starting:

1. `systemctl --user enable --now podman.socket` (`setup.sh` warns if it is not active).
2. Build the sandbox image into your own podman. The default is `localhost/aurora-sandbox:0.1.0`, which runs the `fake`
   harness only (`scripts/images/build-sandbox-min.sh aurora-sandbox:0.1.0`). For `claude-code` through the mock use
   `AURORA_SANDBOX_IMAGE=localhost/aurora-llm-sandbox:0.1.0`, built with
   `podman build -f scripts/e2e/llm.Containerfile -t localhost/aurora-llm-sandbox:0.1.0 .`.

Then, in `deploy/compose/.env`: `COMPOSE_PROFILES=hosted,llm-mock` (and `AURORA_SANDBOX_IMAGE` as above), and `podman compose up -d`.

Transcript of a clean run (project `aurora-o15`; AUDIT-001 follow-up OPS-015, evidence in
[phase0-exit.md](../audit/phase0-exit.md#follow-up-ops-015)). All calls go to `localhost:4000`, with an `Origin: http://localhost:4000` header on
cookie-authenticated calls:

1. `bootstrap-admin --email o15@example.org` prints the reset link; `POST /v1/auth/password/reset` with its token and a password (204), then
   `POST /v1/auth/login` (200).
2. `POST /v1/orgs` `{"name":"O15","slug":"o15"}` (201) creates the org, the default workspace and the hosted agent Daneel.
3. `PATCH /v1/agents/{agent}` `{"harness":"claude-code","model_endpoint_id":"litellm-mock","model_id":"scn-pong"}` (200).
4. `POST /v1/workspaces/{ws}/secrets` `{"name":"litellm-mock-key","kind":"llm","llm":{"endpoint_id":"litellm-mock","auth_mode":"api_key"},"value":"sk-aurora-mock-dev"}` (201),
   then `PUT /v1/agents/{agent}/grants/secrets/{secret}` `{}` (200).
5. `POST /v1/agents/{agent}/conversations/direct` `{}` (200), then `POST /v1/conversations/{id}/messages` `{"text":"Reply with exactly the word: pong"}` (202).
6. Events: `turn.queued`, `turn.started`, `text "pong"`, `turn.finished done`. A container `aurora-sbx-<id>` from the sandbox image ran in the host's podman.

The model name chooses the mock scenario; the mock only replays scenario files and is not a model.

Settings that make this work (all in `deploy/compose`): `aurora.toml` has `runner.sandbox_hosts`, `gateway.advertised_proxy_url = "http://aurora-gateway:4102"`
(what sandboxes dial; the default derived from `external_url` is `localhost`, unreachable from a sandbox, so a BYO agent on the host uses
`http://localhost:4102`) and `gateway.allowed_private = ["10.89.249.4/32"]` (the gateway refuses private destinations; only the static core address of the mock
LiteLLM is opened, inert without the `llm-mock` profile). The `api` service mounts the public `ca.pem`.

Cleanup: `podman compose down` leaves the sandbox containers and their `home-*` volumes (the runner owns them); remove them with
`podman rm -f aurora-sbx-<id>` and `podman volume rm home-<id>`.

## Configuration in `deploy/compose/aurora.toml`

The shipped file sets:

| Key | Value | Notes |
|-----|-------|-------|
| `server.external_url` | `http://localhost:4000` | Public origin. Change it for any other host. |
| `server.bind` | `0.0.0.0` | Sandbox-facing listeners (gateway 4102, runner channel 4105). |
| `server.trusted_proxies` | `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16` | Trusted `X-Forwarded-For` source (the edge). |
| `runner.backend` | `"fake"` | The `hosted` profile overrides it to `podman` on the runner service (environment). |
| `runner.sandbox_hosts` | gateway and runner sandbox addresses | `/etc/hosts` entries for sandboxes; used by the podman backend. |
| `gateway.advertised_proxy_url` | `http://aurora-gateway:4102` | The proxy address sandboxes dial. |
| `gateway.allowed_private` | `10.89.249.4/32` | The mock LiteLLM only. |
| `runner.api_url` | `http://aurora-api:4101` | |
| `hosted.enabled` | `true` | |
| `telemetry.log_level` | `info` | |

Compose sets the listener binds that must not be sandbox-reachable through environment
overrides (SEC-09):

- `AURORA__GATEWAY__INTERNAL_BIND: 10.89.249.2` (gateway internal listener 4103)
- `AURORA__RUNNER__OPS_BIND: 10.89.249.3` (runner ops listener 4106)

Static addresses: gateway `10.89.250.2` and runner `10.89.250.3` on the sandbox
network; `10.89.249.0/24` for the core network (gateway `10.89.249.2`, runner `10.89.249.3`).
The core network's `ip_range` is `10.89.249.128/25`, so dynamically addressed containers
(Postgres first) cannot take the static addresses. Without it `up -d` fails with
`IPAM error: requested ip address 10.89.249.2 is already allocated`.

If an earlier install left resources behind (the `aurora-sandbox` network, the fixed-name
`aurora-ca` volume, `aurora-*:dev` images, a project named `aurora`), use a separate project
name (`COMPOSE_PROJECT_NAME=aurora-audit`) and `AURORA_VERSION` in `.env`, so you do not reuse
old data or overwrite old images.

Static addresses on the core network: gateway `10.89.249.2`, runner `10.89.249.3` and, in the `llm-mock` profile, LiteLLM `10.89.249.4`.

## Bootstrap tokens in compose

`.env` values flow into the API through compose:

- `AURORA_BOOTSTRAP_RUNNER_TOKEN` is set to `AURORA_RUNNER_TOKEN`. On startup the API
  upserts the runner `default` with that token's hash (`bootstrap_default`).
- `AURORA_BOOTSTRAP_CONNECTOR_TOKENS` is set to `fake:<AURORA_CONNECTOR_TOKEN>`. The
  API upserts the connector at startup (see [plugins-and-connectors.md](plugins-and-connectors.md#connectors)).

## Stop and logs

```bash
podman compose -f deploy/compose/compose.yaml logs -f api
podman compose -f deploy/compose/compose.yaml down
```

`down` keeps the named volumes (`aurora-pg`, `aurora-ca`). Back them up before removing them
(see [backups.md](backups.md)).

## Developer loop without containers

`just dev` runs the loop in [docs/15 §Development](../15-deployment-operations.md#development-podman-compose-deploycompose):

```bash
just dev
```

It does the following:

1. `just dev-db` starts Postgres 16 in podman on `127.0.0.1:55432`, and `just migrate` applies migrations.
2. It sources `deploy/dev/env.sh` (throwaway secrets, `AURORA_CONFIG=deploy/dev/aurora.dev.toml`).
   Use these values for development only.
3. On the first run it mints a dev gateway CA into `deploy/dev/ca/` (gitignored).
4. It runs the API (4101 and 4111), the gateway (4102 and 4103), the hook host (4104) and the
   web app (`pnpm --dir web dev`, port 4100) as host processes. Ctrl-C stops them all.

The web app needs `pnpm install` in `web/` once. The runner is not started. The dev config
uses the `fake` backend; to run hosted agents on the host, start `aurora-runner serve` yourself
with the podman backend (see [runners.md](runners.md#podman-backend)).
