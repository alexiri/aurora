# Runners

A runner hosts agent sandboxes. The runner registers with the API, takes work from
it, and starts sandboxes on its backend. The design is in
[docs/11 §Registration](../11-runner-sandboxes.md#registration-replicas-ownership) and
[docs/15 §Sizing](../15-deployment-operations.md#sizing).

Hosted agents need at least one runner. Compose and Helm set one up for you (see the
bootstrap notes below), but a runner that uses the `fake` backend does not run real
sandboxes.

## Add a runner

1. **Sign in as an instance administrator.** Use the dashboard, or `aurora login --url https://aurora.example.org`
   (device flow, `services/aurora-cli`).
2. **Create the runner.** Send `POST /v1/admin/runners` with an instance-administrator
   session and the body `{"name": "runner-a"}` (the name is 1 to 64 characters). The
   response is `201` with `{ "runner": …, "token": "aur_…" }`. **The token is shown once.**
   Store it as `AURORA_RUNNER_TOKEN` in the runner's secret store.
3. **Configure the runner** (`aurora.toml` plus environment, see below) and start it with
   `aurora-runner serve` (the image's default command).
4. **Check it appears.** `GET /v1/admin/runners` lists runners with replicas and capacity.

Other admin actions:

| Action | Route | Result |
|--------|-------|--------|
| List | `GET /v1/admin/runners` | Runners, replicas, capacity |
| Rotate token | `POST /v1/admin/runners/{id}/token` | `200` with a new `aur_` token, shown once |
| Delete | `DELETE /v1/admin/runners/{id}` | `204`. `409` while sandboxes run on it. |

Every change is audited (`admin.runner_create`, `admin.runner_delete`, `admin.runner_rotate`).

### Bootstrap alternative

`AURORA_BOOTSTRAP_RUNNER_TOKEN` makes the API upsert the runner `default` with that
token's hash at startup (`services/aurora-api/src/domains/runners_admin/service.rs`,
`bootstrap_default`). Compose and Helm set it to the same value as `AURORA_RUNNER_TOKEN`
(Helm: `api.bootstrap.runnerToken`, default true). It is a convenience for single-runner
installs. It does not replace the admin route for a second runner.

## Runner configuration

| Key or variable | Default | Notes |
|-----------------|---------|-------|
| `AURORA_RUNNER_TOKEN` | — (required) | `aur_…`, from the admin route. |
| `AURORA_RUNNER_SESSION_KEY` | — (required) | 32 random bytes, base64. Must match the API. |
| `runner.api_url` | — | Where the runner reaches the API, for example `http://aurora-api:4101`. |
| `runner.backend` | — (required) | `podman`, `kubernetes` or `fake`. Set in config, never detected. |
| `runner.ops_bind` | `server.bind` | Address of the ops listener (4106, health and metrics). Must not be sandbox-reachable (SEC-09). |
| `runner.advertised_host` | — | Host sandboxes use to reach the runner's WebSocket. |
| `runner.image` | `aurora-sandbox:<version>` | Sandbox image. |
| `runner.max_sandboxes` | `4` | Concurrent sandboxes on this runner. |
| `runner.sandbox_memory_mib` | `2048` | Per-sandbox memory. |
| `runner.sandbox_cpus` | `1` | Per-sandbox CPUs. |
| `runner.sandbox_pids` | `512` | Per-sandbox pid limit. |
| `runner.home_gib` | `10` | Per-agent home size. |
| `runner.reconcile_interval_secs` | `60` | How often the oldest live replica reconciles. |
| `runner.orphan_grace_secs` | `3600` | Age before an orphan object is destroyed. |
| `runner.orphan_reap` | `reap` | `reap` or `log_only`. |
| `runner.capacity_wait_secs` | `300` | How long a start waits for capacity. |
| `server.bind` | `0.0.0.0` | Also the default bind of the sandbox-facing channel listener, port 4105. |

`runner.held_awake_ceiling` is parsed and validated, but no runtime code reads it
(**not yet implemented**).

Start-up checks: the runner validates the backend and the sandbox network, and warns
when the ops bind overlaps the sandbox-facing bind.

Ports (docs/15 §Helm): the runner's sandbox-facing channel is 4105, and the ops listener
is 4106. The Helm chart exposes 4105 only to the sandboxes namespace.

## Podman backend

For development and single-host installs (docs/11 §Backend podman).

| Key | Default | Notes |
|-----|---------|-------|
| `runner.podman_socket` | unset | Podman API socket. When unset, the runner uses `$XDG_RUNTIME_DIR/podman/podman.sock`, or `/run/podman/podman.sock` when `XDG_RUNTIME_DIR` is not set (`services/aurora-runner/src/backends/podman.rs`). |
| `runner.sandbox_network` | `aurora-sandbox` | Must be created `--internal --disable-dns`. The runner verifies the flag at startup and refuses to run otherwise. |
| `runner.sandbox_hosts` | `{}` | Map of host names to IPs injected into sandboxes as `/etc/hosts` entries, for example `{ "aurora-gateway" = "10.89.250.2", "aurora-runner" = "10.89.250.3" }`. |
| `runner.dev_allow_external_network` | `false` | Escape hatch for development. It logs a warning on every start. Never set it in a deployment. |

What the runner does with them (docs/11): sandboxes get `--read-only` root, a tmpfs `/tmp`
and `/run/aurora`, `--user 1000:1000`, `--cap-drop=ALL`, `no-new-privileges`, the default
seccomp profile, pid, memory and CPU limits, and no host mounts except the home volume
`aurora-home-<sandbox_id>` at `/workspace`.

Prerequisites on the host (standard podman, not checked by this repository): a running
podman API socket (rootless: `systemctl --user enable --now podman.socket`) and the
`aurora-sandbox` network. `deploy/compose/setup.sh` creates the network.

To switch compose from the fake backend, set in `deploy/compose/aurora.toml`:

```toml
[runner]
backend = "podman"
sandbox_hosts = { "aurora-gateway" = "10.89.250.2", "aurora-runner" = "10.89.250.3" }
```

The compose file already mounts the podman socket and gives the runner its static address
(`10.89.250.3`). Test on a host before relying on it. `compose/aurora.toml` does not set
`sandbox_hosts` or `podman_socket` today (see [install-compose.md](install-compose.md#configuration-in-deploycomposeauroratoml)).

## Kubernetes backend

For production (docs/11 §Backend kubernetes). The Helm chart sets `runner.backend = "kubernetes"`.

| Key | Default | Notes |
|-----|---------|-------|
| `runner.k8s.namespace` | `aurora-sandboxes` | Namespace where sandbox Pods, PVCs and Secrets go. |
| `runner.k8s.core_namespace` | the runner pod's own namespace, else `default` | Where the gateway and runner Services are looked up. Helm renders this as the release namespace. |
| `runner.k8s.gateway_service` | `aurora-gateway` | Service for the gateway proxy. Helm renders `<fullname>-gateway-proxy`. |
| `runner.k8s.runner_service` | `aurora-runner` | Service for the runner. Helm renders `<fullname>-runner`. |
| `runner.k8s.storage_class` | unset | StorageClass of each home PVC (`ReadWriteOnce`). |
| `runner.k8s.runtime_class` | unset | RuntimeClass of each sandbox Pod. gVisor or Kata recommended. |
| `runner.k8s.node_selector` | `{}` | Parsed by the runner. **The chart does not render it** (known issue). |
| `runner.k8s.tolerations` | `[]` | Parsed by the runner. **The chart does not render it** (known issue). |

Per sandbox the runner creates a PVC `home-<sandbox_id>`, a Pod `sbx-<handle>-<boot>` with
`restartPolicy: Never`, and a Secret `boot-<boot_id>`, which is deleted when the boot ends.
Pods run with `automountServiceAccountToken: false`, `dnsPolicy: None` with the nameserver
`127.0.0.1` (no DNS), the restricted security context and read-only root. These follow the
spec in docs/11. The sandbox namespace enforces Pod Security `restricted` (the chart labels it).

The runner's RBAC from the chart allows Pods, PVCs and Secrets in the sandbox namespace,
and Services in the core namespace.

## Fake backend

`runner.backend = "fake"` is in memory. It simulates homes, boots, crashes and capacity
holds. The shipped compose config uses it. It does not run real sandboxes. Do not use it in production.

## Reconcile and orphans

The oldest live replica reconciles every `reconcile_interval_secs`. It destroys runner-managed
objects whose sandbox the API no longer knows, and sweeps objects with this installation's
fingerprint that the API does not know, once they are older than `orphan_grace_secs`. An API
error aborts the sweep with no deletions. Set `runner.orphan_reap = "log_only"` to log instead
of deleting (docs/11 §Reconcile and orphan reaping).

## Sizing

From docs/15: runner hosts need `max_sandboxes × sandbox memory + 1 GiB`. A held-awake
sandbox occupies a slot permanently, so size for steady state, not peak.

With the defaults (`max_sandboxes = 4`, `sandbox_memory_mib = 2048`), a runner host needs
4 × 2048 MiB + 1 GiB = 9 GiB.

## Known limitations

From `docs/PROGRESS.md` (FUP-M7, RUN-004). These are open at this commit:

- The API's `SandboxSpec.env` is empty. `AURORA_AGENT_HANDLE` and snapshot app env do not
  reach sandboxes on any backend.
- Sandboxes cannot use an `imagePullSecrets` setting. The config has no key for it.
- The per-pod pid limit cannot be enforced on Kubernetes.
- The `/run/aurora` emptyDir cannot be mounted `noexec`.
- `runner.k8s.node_selector` and `tolerations` are not rendered by the chart.
