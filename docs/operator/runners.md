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
| `runner.image` | `aurora-sandbox:<version>` | Sandbox image. The single source of truth: the runner starts it, advertises it at registration and the API reports it as the sandbox `image`. |
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

`runner.held_awake_ceiling` overrides the eviction ceiling `max(1, max_sandboxes − 1)`. The runner
reports it at registration and the API's due-work eviction uses that value for the runner; the
key is set only in the runner's config.

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
| `runner.dev_supervisor_binary` | unset | Development only (OPS-018, `docs/ops/live-reload.md`): absolute path of a host-built `aurora-supervisor`, bind-mounted read-only over the image's copy in new podman sandboxes. Config validation refuses it unless the backend is `podman` and `server.bind`, `server.external_url` and `runner.api_url` are loopback. It logs a warning on every start. Never set it in a deployment. |

What the runner does with them (docs/11): sandboxes get `--read-only` root, a tmpfs `/tmp`
and `/run/aurora`, `--user 1000:1000`, `--cap-drop=ALL`, `no-new-privileges`, the default
seccomp profile, pid, memory and CPU limits, and no host mounts except the home volume
`aurora-home-<sandbox_id>` at `/workspace`.

Prerequisites on the host (standard podman, not checked by this repository): a running
podman API socket (rootless: `systemctl --user enable --now podman.socket`) and the
`aurora-sandbox` network (`podman network create --internal --disable-dns --subnet 10.89.250.0/24 --gateway 10.89.250.1 aurora-sandbox`).
The podman backend is what the host-process e2e suites (`scripts/e2e/hosted.sh`, `llm.sh`) use; the shipped installs use the Kubernetes backend.

Phase 1 (OPS-017): the podman compose stack that ran this backend in a container is gone (its runner could not open the
rootless socket as a non-root user without `keep-id`). Run `aurora-runner serve` on the host with this backend, or use the
Kubernetes backend on kind ([install-kind-dev.md](install-kind-dev.md)).

## Kubernetes backend

For production (docs/11 §Backend kubernetes). The Helm chart sets `runner.backend = "kubernetes"`.

| Key | Default | Notes |
|-----|---------|-------|
| `runner.k8s.namespace` | `aurora-sandboxes` | Namespace where sandbox Pods, PVCs and Secrets go. |
| `runner.k8s.core_namespace` | the runner pod's own namespace, else `default` | Where the gateway and runner Services are looked up. Helm renders this as the release namespace. |
| `runner.k8s.gateway_service` | `aurora-gateway` | Service for the gateway proxy. Helm renders `<fullname>-gateway-proxy`. |
| `runner.k8s.runner_service` | `aurora-runner` | Service for the runner. Helm renders `<fullname>-runner`. |
| `runner.k8s.storage_class` | unset | StorageClass of each home PVC (`ReadWriteOnce`). A cluster may have **no default StorageClass** (e.g. CERN Magnum with the manila CephFS classes — none is marked default there), so the class must be pinned here (Helm `runner.k8s.storageClass`): a claim created while the value is unset stays unprovisionable. A claim that pins an explicit class overrides the hint in `home_volume_unbound` messages (FIX-049). |
| `runner.k8s.runtime_class` | unset | RuntimeClass of each sandbox Pod. gVisor or Kata recommended. |
| `runner.k8s.node_selector` | `{}` | Labels that sandbox Pods must match. Helm renders `runner.k8s.nodeSelector` here. |
| `runner.k8s.tolerations` | `[]` | Tolerations of each sandbox Pod, as TOML tables. Helm renders `runner.k8s.tolerations` here. |
| `runner.k8s.image_pull_secrets` | `[]` | Names of image-pull Secrets set as `imagePullSecrets` on each sandbox Pod, for a sandbox image in a private registry. Each must be a DNS-1123 subdomain name (startup fails otherwise). Helm renders `runner.sandboxImagePullSecrets` here. |

#### Private sandbox registry

The runner never reads or creates these Secrets, and its RBAC is unchanged. Create them in the
**sandbox namespace** (`runner.k8s.namespace`, default `aurora-sandboxes`) before the first
sandbox starts. The chart's own `imagePullSecrets` (for the service images) live in the release
namespace and Secrets are namespaced, so they do not apply to sandbox Pods; the chart does not
copy them. A sandbox whose Secret is missing or wrong fails with `image_unavailable`.

```sh
kubectl -n aurora-sandboxes create secret docker-registry sandbox-registry \
  --docker-server=registry.example.org \
  --docker-username=<user> --docker-password=<token>
```

Then set `runner.k8s.image_pull_secrets = ["sandbox-registry"]` (Helm:
`--set 'runner.sandboxImagePullSecrets={sandbox-registry}'`) and restart the runner.

Per sandbox the runner creates a PVC `home-<sandbox_id>`, a Pod `sbx-<handle>-<boot>` with
`restartPolicy: Never`, and a Secret `boot-<boot_id>`, which is deleted when the boot ends.
Pods run with `automountServiceAccountToken: false`, `dnsPolicy: None` with the nameserver
`127.0.0.1` (no DNS), the restricted security context and read-only root. These follow the
spec in docs/11. The sandbox namespace enforces Pod Security `restricted` (the chart labels it).

The runner's RBAC from the chart allows Pods, PVCs and Secrets in the sandbox namespace,
and Services in the core namespace.

## Fake backend

`runner.backend = "fake"` is in memory. It simulates homes, boots, crashes and capacity
holds. It does not run real sandboxes. Do not use it in production.

## Reconcile and orphans

The oldest live replica reconciles every `reconcile_interval_secs`. It destroys runner-managed
objects whose sandbox the API no longer knows, and sweeps objects with this installation's
fingerprint that the API does not know, once they are older than `orphan_grace_secs`. An API
error aborts the sweep with no deletions. Set `runner.orphan_reap = "log_only"` to log instead
of deleting (docs/11 §Reconcile and orphan reaping).

## Troubleshooting: a turn is stuck on "Sandbox starting" (FIX-025, D-158)

A turn that needs a sandbox is bounded end to end by `hosted.sandbox_ready_timeout_secs`
(default 300 s): if the sandbox is not ready in time, the turn fails with a code naming
where the start stopped, the agent header pill says **Sandbox failed** with the reason, and
the Computer tab shows **Last start attempt** (phase, detail, elapsed, the pod's Warning
events). Admin › Runners lists each runner's recent start failures. You should not need
kubectl for the first diagnosis; the commands below are the fallback.

**What the failure codes mean** (the chat error block, the pill tooltip and the admin page
all show the same reason):

| Code | Meaning | First thing to check |
| --- | --- | --- |
| `runner_unavailable` | No runner claimed the start | Admin › Runners: is any runner live? Is the runner's token valid, can it reach the API? |
| `pod_refused` | The cluster refused pod creation | `kubectl -n <sandboxes ns> describe pod …` / the runner's log: quota, admission, image pull secret? |
| `pod_failed` | The pod ended while it was starting | Pod Warning events (Computer tab / admin page): crash loop, OOM? |
| `home_volume_unbound` | The home PVC never bound | `kubectl get pvc -n <ns>`: storage class, capacity? Run `kubectl describe pvc <claim> -n <ns>` and read the events **bottom-up** before deleting anything. On Magnum + manila CephFS CSI clusters a `ProvisioningFailed` warning from `persistentvolume-controller` ("Error saving claim: … the object has been modified; please apply your changes to the latest version") is a transient annotation race: when it is followed by Normal `Provisioning` / `ProvisioningSucceeded` events from the CSI provisioner, the volume bound anyway and it is not a failure. The runner treats such a warning as slow provisioning, not a verdict: it keeps waiting while a newer provisioning event exists or the warning is younger than `PVC_PROVISION_GRACE` (90 s), bounded by `hosted.sandbox_ready_timeout_secs`; only a `FailedBinding`, or a `ProvisioningFailed` that is the newest event and older than the grace, fails the start (FIX-049, D-164). If a start failed anyway, **Retry now** works from `failed` even during the backoff (it overrides the failure-backoff lease), and **Restart** performs the same recovery: the computer then starts on the now-bound volume. Delete the claim only when it is still `Pending` with no newer provisioning event. |
| `image_unavailable` | The image pull failed | The image exists and the pull secret is attached to the runner? |
| `supervisor_unreachable` | The pod is Running but its supervisor never connected to the runner on :4105 within 60 s (`RUNNER_SUPERVISOR_HELLO`) | Network egress from the sandboxes namespace to the runner (see below) |
| `supervisor_boot_failed` | The supervisor connected but the boot never finished | The runner's log; harness supported by the image (`12 §Harness selection`)? |
| `harness_unsupported` | The supervisor refused the boot document's harness | The sandbox image contains the harness (`15 §Hosted`)? |

**`supervisor_unreachable` is the case that used to hang forever** (the owner's production
incident): the pod looks healthy, but the supervisor inside cannot dial the runner's
sandbox-channel port `4105`. The failure message names the runner endpoint, e.g.
"The agent's computer started, but its supervisor could not reach the runner at
`aurora-runner.aurora` (10.0.0.9):4105 within 60 s."

With Cilium as the CNI, the chart's `networkPolicy.cilium.enabled=true` renders
**CiliumNetworkPolicies**; the sandboxes-namespace rules match by pod identity (namespace
and pod labels), which Cilium handles natively, so the egress path sandbox → runner:4105
needs no extra policy. Prove the path from a pod in the sandboxes namespace:

```sh
kubectl -n <sandboxes ns> exec <sandbox pod> -- sh -c 'nc -zv -w3 aurora-runner.<release-ns> 4105'
# blocked → the turn fails supervisor_unreachable within the deadline (default 300 s)
```

Start attempts are recorded per runner (`sandbox_start_failures`, 30 days) and shown on
Admin › Runners:

```sh
curl -H "Authorization: Bearer <session>" https://aurora.example.org/v1/admin/runners/<runner-id>/start-failures
```

kubectl fallback (the API already shows these in the Computer tab and the admin page):

```sh
kubectl -n <sandboxes ns> get pods -l org.aurora/sandbox=<agent-id>
kubectl -n <sandboxes ns> describe pod <pod> | sed -n '/Events:/,$p'
kubectl -n <release ns> logs deploy/aurora-runner --tail=200
```

Automatic retries stop after 3 consecutive failures; a new message retries once the
backoff has passed. `hosted.sandbox_ready_timeout_secs` also bounds a slow image pull —
raise it (max 1800) if pulls legitimately take minutes.

## Troubleshooting: chat says `ERR_PROXY_TUNNEL` (the proxy refused the tunnel) (FIX-050, D-165)

The harness error means the gateway answered the sandbox's **CONNECT hop with a
non-2xx**. The gateway's answers at this stage carry an Aurora error body; from
FIX-050 every refusal is also logged (`connect_auth_refused` with a reason
class) and counted (`aurora_gateway_connect_refused_total{reason}`) — check
`kubectl -n <release ns> logs deploy/aurora-gateway --tail=100` first.

| Reason | Meaning | Fix |
| --- | --- | --- |
| `no_active_owner` | The credential's **organization has no active owner** — the credential lookup requires one, so *every* CONNECT of *every* credential of the org is refused 407 while message delivery keeps working (the only symptom is the chat error). | Repair with one UPDATE (below); the durable fix is a `role_mappings` entry granting `owner` (`docs/04 §OIDC role claims`). |
| `unknown` / `revoked` / `expired` | The token did not resolve (stale boot after an upgrade, revoked or expired credential). | Re-send the message (a fresh boot mints fresh credentials); check `gateway_credentials.revoked_at` / `expires_at` if it persists. |
| `frozen` | The agent (or workspace/org) is paused. | Unfreeze in the dashboard. |
| `store_unavailable` | The gateway could not reach its database. | Check the gateway's DB (Postgres) and its logs for `state_change`. |

Find and repair an ownerless org (the 2026-11-12 production incident: an org
claim-created by a member-only IdP mapping — the product floor of FIX-050
prevents new ones):

```sql
SELECT o.slug, count(m.user_id) AS owners, o.created_at
FROM organizations o LEFT JOIN org_members m
  ON m.org_id = o.id AND m.role = 'owner' AND m.status = 'active'
GROUP BY o.id, o.slug, o.created_at HAVING count(m.user_id) = 0;

UPDATE org_members SET role='owner', status='active', updated_at=now()
WHERE org_id = (SELECT id FROM organizations WHERE slug='<slug>')
  AND user_id = (SELECT id FROM users WHERE email='<admin email>');
```

The repair takes effect within seconds (the gateway's credential cache is
positive-60 s / negative-5 s).

Automatic retries stop after 3 consecutive failures; a new message retries once the
backoff has passed. `hosted.sandbox_ready_timeout_secs` also bounds a slow image pull —
raise it (max 1800) if pulls legitimately take minutes.

**Retry without deleting the agent** (FIX-029, D-160, C-610): the dashboard's
**Retry now** (chat error block or Computer tab) calls
`POST /v1/agents/{agent}/sandbox/retry`, which resets the failure bound and
starts through the ordinary start arm — the operator fix (NetworkPolicy patch,
quota raise, image push) followed by one click, no delete-and-recreate. The
Computer tab's **Start log** (C-612) shows the boot's phases, the failure
verdict and any retry note; the agent's start-failure history (C-613, admin:
C-614) lists past failures with the runner named, from the same
`sandbox_start_failures` rows the command below reads.

## Sizing

From docs/15: runner hosts need `max_sandboxes × sandbox memory + 1 GiB`. A held-awake
sandbox occupies a slot permanently, so size for steady state, not peak.

With the defaults (`max_sandboxes = 4`, `sandbox_memory_mib = 2048`), a runner host needs
4 × 2048 MiB + 1 GiB = 9 GiB.

## Sandbox resources

Four keys set what **each** sandbox gets, and the runner reports them to the API at registration:

| Key | Default | Notes |
|-----|---------|-------|
| `runner.sandbox_cpus` | `1` | CPUs (reported as `cpus × 1000` millicores). |
| `runner.sandbox_memory_mib` | `2048` | Memory in MiB. |
| `runner.sandbox_pids` | `512` | Process limit. Enforced by podman only (see [Known limitations](#known-limitations)). |
| `runner.home_gib` | `10` | Size of the agent's home volume in GiB. |

All four must be greater than 0 (startup fails naming the key). The effective profile of a sandbox is the agent's own override if an administrator set one, otherwise the runner default above, clamped to the runner's bounds
([docs/11](../11-runner-sandboxes.md): an organization-level override is specified but not in the code yet). On Kubernetes, requests equal limits, so a sandbox that does not fit its node stays `Unschedulable`, and the
chart's sandbox namespace adds a ResourceQuota and a LimitRange (`sandboxes.quota.*`, `sandboxes.limitRange.*`; the LimitRange maximum is 8 CPUs and 16 GiB by default). Size the quota to `runner.max_sandboxes` times these values.

```toml
[runner]
backend = "kubernetes"
max_sandboxes = 8
sandbox_cpus = 2
sandbox_memory_mib = 4096
home_gib = 20
```

With Helm, put them under `config.runner` and keep `sandboxes.quota` at least `max_sandboxes` times the per-sandbox size.
A sandbox image in a **private registry** is configured with `runner.k8s.image_pull_secrets`
([Private sandbox registry](#private-sandbox-registry), FIX-013); the service images use the chart's `image.pullSecrets` instead.

## Known limitations

Accepted by the owner on 2026-11-08 (FUP-M7, RUN-004). These are limits of Kubernetes itself, not of Aurora:

- **No per-Pod process limit on Kubernetes.** The podman backend sets `--pids-limit` from `runner.sandbox_pids`. Kubernetes has no per-Pod field for it, so the value is only validated there. Kubernetes limits processes per Pod through the kubelet: set `podPidsLimit` in the KubeletConfiguration of the nodes that run sandboxes (for example 1024; kind accepts it through a `kubeadmConfigPatches` KubeletConfiguration patch). Without it, a sandbox can fork until the node's own limit stops it, which affects other Pods on that node. Run sandboxes on dedicated nodes (`runner.k8s.node_selector` / `tolerations`) if that matters to you.
- **`/run/aurora` is not `noexec` on Kubernetes.** It is a small memory-backed `emptyDir` (16 MiB; `/tmp` is 512 MiB), and Kubernetes can't set mount options on an `emptyDir`. The sandbox container still runs as non-root with a read-only root filesystem, all capabilities dropped, no privilege escalation and the `RuntimeDefault` seccomp profile. So a file written there can run as the sandbox user, which can already run code in its own home. `/run/aurora` holds only the public gateway CA (`ca.pem`) and the supervisor's tools socket. Boot material never touches it: it arrives in the supervisor's environment, which the supervisor scrubs, and the supervisor is non-dumpable (SEC-34).

Resolved since the first list: sandbox env and `AURORA_AGENT_HANDLE` reach every backend (FIX-002). Pod names are `sbx-<handle>-<boot>`. Private sandbox image registries use `runner.k8s.image_pull_secrets` (FIX-013; see §Kubernetes backend, Private sandbox registry).
