# 11 — Runner and Sandboxes

Phase: **0**. Binary: `aurora-runner`. Backends: `podman`, `kubernetes`, `fake`.

## Purpose and shape

Each hosted agent gets "a computer": an isolated, resource-limited environment
with a durable home whose **only** route out is the gateway. The runner creates,
parks, wakes and destroys these environments for the control plane. It is
**outbound-only** towards the API and **never touches the database**.

```
API ◄──(HTTPS: register · long-poll work · post reports · tool calls)── runner
                                                                         │ WebSocket server :4105 (sandbox network only)
                                                                         ▼
                                                          sandbox: supervisor ──► harness
                                                                         │ loopback proxies
                                                                         ▼ :4102 (only exit)
                                                                      gateway
```

Runner internals: **backend** (substrate seam), **control-channel server**
(supervisor WebSockets), **API client**, **runner loop** (poll → execute →
report; plus reconcile), **executor** (bounded concurrency per sandbox: one
lifecycle step at a time).

## Registration, replicas, ownership

- An instance admin provisions a runner (`POST /v1/admin/runners`) and gets its
anchor token `aur_…` (once). `installation_fingerprint` =
first 16 hex of `sha256(token)`; it labels every infrastructure object.
- On start the runner calls `register` with the token, declared backend and
**capabilities** (never probed later): backend id, home durability
(`resident` | `snapshot`), supported resource axes and bounds,
`max_sandboxes`, supported harness ids in its image, version. It receives
its replica id and tunables.
- Many replicas may share one runner registration. Every call carries
`X-Aurora-Replica`. A replica is **live** while `last_seen_at` is within
`REPLICA_ONLINE_THRESHOLD`; clean shutdown retires it (`retired_at`), and it
gets no new work. The oldest live replica performs reconciliation.
- **Lifecycle ownership**: every start/stop claim stamps `owner_replica_id`;
while that replica is live only it may claim the sandbox's next lifecycle
step, and lifecycle reports are **fenced** on `(sandbox_id, boot_id, owner_replica_id)`. When the owner dies, any replica takes over.
- **Channel holder**: `channel_replica_id` is the replica holding the
supervisor socket; socket-bound work (deliver, steer, abort, send-now, home
sync) is handed only to it.
- **Placement**: a sandbox is bound to a runner (`sandboxes.runner_id`) at its
first start (the polling runner with free capacity) and keeps it while its
home is `resident` on that runner's substrate. Moving homes between runners is
out of scope.

## The control channel (supervisor ↔ runner)

WebSocket at `ws://<runner host>:4105/v1/supervisor`, admitted in one of two ways:

1. **Bootstrap** (first connect of a boot): the `aup_` token from the boot
 document. The runner asks the API to **claim** it (`POST  /v1/runner/channel/claim`); the API compares hashes, marks it used
 (single use across all replicas) and answers `{ sandbox_id, boot_id }`.
2. **Session** (reconnect): `HMAC-SHA256(AURORA_RUNNER_SESSION_KEY,  sandbox_id ‖ boot_id ‖ issued_at)` issued on every successful connect and
 verifiable by any replica; valid while `boot_id` is the sandbox's current
 boot. During an API outage a locally valid session is accepted and the
 channel claim retried, so running sandboxes stay connected.

Frames use a **sequenced envelope** (`16 §W6`): each side numbers frames; the
receiver applies each seq at most once and acks; after a reconnect the sender
replays unacked frames. Frames &gt; `WS_FRAME_MAX` are rejected, so all senders
bound their frames. The runner stamps `sandbox_id`/`boot_id` on everything it
forwards; it never trusts identity claimed by the supervisor.

## The backend seam

```rust
#[async_trait]
pub trait SandboxBackend: Send + Sync {
    fn id(&self) -> &'static str;
    fn capabilities(&self) -> BackendCapabilities;      // home durability, resource axes/bounds, max_sandboxes
    fn home_ref_for(&self, sandbox_id: Uuid) -> HomeRef; // pure, no I/O
    async fn provision_home(&self, home: &HomeRef, size: ByteSize) -> Result<(), BackendError>; // idempotent
    async fn destroy_home(&self, home: &HomeRef) -> Result<(), BackendError>;
    async fn start(&self, spec: &SandboxSpec, hold: &dyn HoldCallback) -> Result<ContainerRef, BackendError>;
    async fn stop(&self, container: &ContainerRef, grace: Duration) -> Result<(), BackendError>; // removes the container, keeps the home
    async fn inspect(&self, sandbox_id: Uuid) -> Result<Option<Observation>, BackendError>;     // running?, boot_id, phase, observed resources
    async fn list_managed(&self) -> Result<Vec<ManagedObject>, BackendError>;                  // every object with aurora labels, any runner
    async fn destroy(&self, obj: &ManagedObject) -> Result<(), BackendError>;
}
```

- `SandboxSpec`: `sandbox_id`, `boot_id`, `agent_id`, `workspace_id`, image,
env (non-secret), **boot secret** (the boot document, delivered as a secret
env var / Kubernetes Secret), home ref, resource profile, labels.
- `start` is idempotent per `boot_id`: if an object labelled with this boot
exists it is reused; objects of other boots of the same sandbox are removed
first.
- `HoldCallback` lets a backend report a held wake (`waiting_for_capacity`) so
slow capacity is not turned into a failure; after
`runner.capacity_wait_secs` (default 300) it fails with `at_capacity`.
- Errors with meaning: `ImageUnavailable` → `image_unavailable`;
`Capacity`/`WakeCapacityTimeout` → `at_capacity`; anything else →
`agent_start_failed` with a sanitized message.
- Observed resources are read from the substrate, never echoed from the request.
- The runner never names a container runtime outside `backends/<id>/`. A new
substrate is a module + one registry line.

Labels on every managed object: `aurora.managed=true`, `aurora.installation`,
`aurora.runner`, `aurora.sandbox`, `aurora.agent`, `aurora.boot`,
`aurora.created`.

### Backend `podman` (development and single-host)

- Talks to the Podman API socket (Docker-compatible endpoints via `bollard`;
rootless socket `$XDG_RUNTIME_DIR/podman/podman.sock` or rootful
`/run/podman/podman.sock`), configured by `runner.podman_socket`.
- **Sandbox network**: `runner.sandbox_network` (default `aurora-sandbox`) MUST
be created `--internal --disable-dns` (the compose file creates it). The
gateway and runner are attached to it with static addresses
(`runner.sandbox_hosts = { "aurora-gateway" = "10.89.250.2", "aurora-runner" = "10.89.250.3" }`), injected into sandboxes as `/etc/hosts`
entries (`ExtraHosts`). Sandboxes therefore cannot resolve or reach anything
else. The runner verifies the network's `internal` flag at startup and
refuses to run otherwise (escape hatch `runner.dev_allow_external_network`,
logged as a warning on every start; never set in deployments).
- Homes: named volumes `aurora-home-<sandbox_id>`; durability `resident`.
Size limits are best-effort on podman (documented).
- Container: `--read-only` root, tmpfs `/tmp` (512 MiB) and `/run/aurora`
(16 MiB), `--user 1000:1000`, `--cap-drop=ALL`,
`--security-opt no-new-privileges`, default seccomp profile (never
weakened), `--pids-limit`, `--memory`, `--cpus`, no host mounts other than the
home volume at `/workspace`, no device access, `--init` disabled (the
supervisor is PID 1 and reaps).
- Default limits `SANDBOX_DEFAULTS`; `runner.max_sandboxes` (default 4).

### Backend `kubernetes` (production)

- Namespace `runner.k8s.namespace` (default `aurora-sandboxes`), fenced by the
Helm chart: default-deny NetworkPolicy plus an egress allow to gateway pods on
4102 and runner pods on 4105 only; **no DNS egress**; ResourceQuota and
LimitRange; Pod Security Admission `restricted`.
- Per sandbox: **PVC** `home-<sandbox_id>` (storage class `runner.k8s.storage_class`,
`ReadWriteOnce`, size from the profile; durability `resident`) and per boot a
**Pod** `sbx-<handle>-<boot short id>` with `restartPolicy: Never` (the
platform, not the kubelet, decides restarts) and a **Secret**
`boot-<boot_id>` holding the boot document (deleted when the boot ends).
- Pod spec: `automountServiceAccountToken: false`, `enableServiceLinks: false`,
`dnsPolicy: None` with `dnsConfig.nameservers: ["127.0.0.1"]`, `hostAliases`
for the gateway and runner Service ClusterIPs (looked up by the runner at
spawn), securityContext `runAsUser/runAsGroup/fsGroup 1000`,
`runAsNonRoot`, `allowPrivilegeEscalation: false`, `capabilities.drop: [ALL]`,
`seccompProfile: RuntimeDefault`, `readOnlyRootFilesystem: true` with
`emptyDir` for `/tmp` and `/run/aurora` (memory medium), resource requests =
limits from the profile, optional `runtimeClassName` (`runner.k8s.runtime_class`,
gVisor or Kata recommended), node selector and tolerations from config.
- Unschedulable Pods (`PodScheduled=False`, reason `Unschedulable`) → hold
`waiting_for_capacity`.
- Runner RBAC (from the chart): create/get/list/watch/delete Pods, PVCs and
Secrets in the sandbox namespace; get Services in the core namespace.

### Backend `fake`

In-memory; simulates homes, boots, crashes, capacity holds and image errors; the
runner loop's tests and the API's due-work tests run against it with a scripted
supervisor.

## The boot document

Delivered as the secret `AURORA_BOOT` (base64url JSON, ≤ 64 KiB) via the
backend's secret mechanism. The supervisor reads it first thing, marks itself
non-dumpable (`PR_SET_DUMPABLE=0`, so same-uid agent processes cannot read its
environment or memory), removes it from its environment and never passes it on.

```json
{
  "v": 1,
  "sandbox_id": "…", "boot_id": "…", "agent": { "id": "…", "handle": "daneel", "name": "Daneel" },
  "runner_url": "ws://aurora-runner:4105/v1/supervisor",
  "bootstrap_token": "aup_…",
  "gateway": { "proxy": "aurora-gateway:4102", "tls": true, "server_name": "aurora-gateway", "boot_token": "aub_…" },
  "capabilities": [ "browser" ],
  "ca_pem": "-----BEGIN CERTIFICATE-----…",
  "files": [ { "path": "/workspace/.config/…", "mode": "0600", "content_b64": "…", "only_if_absent_or_sentinel": true } ],
  "harness": { "id": "claude-code", "capabilities_expected": [ "resume", "steer" ] },
  "timezone": "Europe/Zurich"
}
```

Non-secret environment set on the container by the runner: `TZ`, `HOME=/workspace`,
`LANG=C.UTF-8`, `AURORA_SANDBOX_ID`, `AURORA_AGENT_HANDLE`,
`HTTPS_PROXY=HTTP_PROXY=http://127.0.0.1:3128` (+ lowercase),
`NO_PROXY=localhost,127.0.0.1`, CA variables pointing at
`/run/aurora/ca.pem` (`SSL_CERT_FILE`, `NODE_EXTRA_CA_CERTS`,
`REQUESTS_CA_BUNDLE`, `CURL_CA_BUNDLE`, `GIT_SSL_CAINFO`), and app `env` entries
from the snapshot. The proxy URL carries **no credentials**: the supervisor's
loopback forwarder adds them (`12 §Loopback proxies`).

(phase 1, RFE-003) **`capabilities`** (array, absent = empty) lists what the
sandbox may do beyond ordinary traffic; phase 1 defines `browser`. Its source:
the **sandbox image declares** the capabilities it supports in the OCI label
`org.aurora.capabilities` (comma list; the default `aurora-sandbox` image
declares `browser` because it ships headless Chromium and its wrapper); the
runner reports them at registration as `RunnerCapabilities.image_capabilities`
(`16 §W5`; absent ⇒ none, fail closed); at `sandbox.start` the API sets
`capabilities = image_capabilities ∩ {browser unless the effective open-lane
posture is disabled for the agent's workspace}` and mints the boot credential
with the same set. The supervisor starts the open proxy `127.0.0.1:3129` **only
if `browser` is present**; the gateway independently refuses the open lane for a
credential without it (`06`). An image without the label never gets a tunnel.

(phase 1, RFE-002) `gateway.proxy` is **always dialled with TLS**: the
forwarder verifies the gateway's proxy certificate against `ca_pem` and the name
`gateway.server_name` (default: the host part of `proxy`, which MUST be in the
certificate's SAN set, `06 §Proxy listener TLS`); `gateway.tls` is `true` and any
other value is refused at start. The only non-TLS proxy hop left is the
**client → loopback forwarder** leg on `127.0.0.1` *inside the sandbox's own
network namespace*, which carries no credentials (the URL has none; the
forwarder adds them) and never leaves the sandbox, so `HTTPS_PROXY` in the
container environment stays `http://127.0.0.1:3128` — that is the documented
exception to "`HTTPS_PROXY=https://…` everywhere", and it is not an
agent-reachable listener on any other interface.

## Lifecycle

Sandbox states: `unprovisioned → starting → running → stopping → stopped`, plus
`failed`. One row per agent, created with the agent (hosted only).

### Start

1. Due-work arm claims the row (`lifecycle_lease_until`, `owner_replica_id`),
 mints `boot_id`, the `aub_` boot token (`gateway_credentials`), the `aup_`
 bootstrap token (hash stored), resets per-boot fields
 (`channel_replica_id`, `listening_ports`, `home_applied_generation = 0`,
 `stop_*`, failure fields), computes the resource profile (agent override &gt;
 org override &gt; runner default, clamped to runner bounds) → `sandbox.start`.
2. Runner: `provision_home` (idempotent) → `start` (with hold callback) → reports
 `sandbox.status running|failed` with `container_ref`.
3. API on `running`: opens a `runtime_intervals` row, stamps `last_active_at`.
4. Supervisor dials, claims bootstrap → `channel_replica_id` set → supervisor
 sends `ready` (harness capabilities) → API emits the full home sync → turns
 dispatch once `home_applied_generation >= desired`.

### Keep-awake (single definition)

A running sandbox is **kept awake** if any of:

- an active turn or `joining` follow-up exists;
- a **busy** process (recent output or CPU) on the current boot, or an **armed
watch** — both only while `now − last_engaged_at < BACKGROUND_CAP`;
- (phase 4) a lease-current SSH session.

`last_engaged_at` is stamped by person/turn engagement (a turn created by a
person, schedule, webhook or watch); it is distinct from `last_active_at`
(stamped also by every boot). The same predicate feeds the stop arm, eviction
and the dashboard's "awake because…" badge.

### Stop (park)

- **Idle**: no keep-awake reason and `now − last_engaged_at ≥ IDLE_STOP` →
`stop_reason = idle`.
- **Cap**: the background cap expired → `cap`.
- **Evict**: a runner over its held-awake ceiling (`max(1, max_sandboxes − 1)`,
override `runner.held_awake_ceiling`) needs a slot for a start; the
longest-idle held sandbox is evicted → `evict`.
- **Restart** (user action): stop immediately, active turns failed
`agent_restarted`, no session refs persisted from the old boot; the next due
work starts a fresh boot.
- **Delete**: agent deleted → reconcile destroys container and home.

Stop: claim → `stopping` → `sandbox.stop` → runner `stop` (supervisor gets
`shutdown` first, up to 10 s to flush) → report `stopped` → API closes the
runtime interval, revokes the boot token, marks processes `parked`, records
`stop_reason`. A parked sandbox is woken by the next due work.

### Failure

A container exiting outside a stop → `failed` with the substrate phase as reason
code; active turns are revived once or failed (`10`); processes `lost`. Restart
backoff for repeated start failures: 30 s, 2 min, 10 min (then only on new user
engagement).

## Reconcile and orphan reaping

Every `RECONCILE_INTERVAL`, by the oldest live replica:

1. `GET /v1/runner/sandboxes` — the control plane's expected set for this runner
 (sandbox ids, current boot ids, desired state).
2. Destroy managed objects of **this runner** whose sandbox the control plane no
 longer knows (agent deleted: container, Secret and home), and containers of
 stale boots.
3. **Installation sweep**: objects bearing this installation's fingerprint but
 another (or no) runner label, whose sandbox id the control plane does not
 know (`POST /v1/runner/sandboxes/check`) and older than `ORPHAN_GRACE`, are
 destroyed. Any API error aborts the sweep with **zero destruction**.
 `runner.orphan_reap = log_only` logs instead.
4. Report containers running without an expected boot (`stray`) so the API can
 fail/record them.

Two installations sharing a daemon or cluster never touch each other's objects
(different fingerprints).

## Sandbox image

Built from `images/sandbox/Containerfile`, published as
`aurora-sandbox:<version>`:

- **Base layer** (rebuilt rarely, tagged by content hash): AlmaLinux 10 ([gitlab-registry.cern.ch/linuxsupport/alma10-base](http://gitlab-registry.cern.ch/linuxsupport/alma10-base)), `git`, `curl`, `jq`, `ripgrep`, `fd-find`, `less`,`openssh-client`, `python3` + `pip` + `venv` + `uv`, Node.js LTS + npm, `build-essential`, `chromium` (headless), `unzip`, `zip`, `procps`, `tini`-free. (phase 1, RFE-003) The image carries the label `org.aurora.capabilities=browser` (it ships the Chromium wrapper); a custom image without Chromium omits it and its sandboxes get no open-lane listener.

- **App layer**: `aurora-supervisor` binary, pinned harness runtimes (Claude Code
CLI at a pinned version installed under `/opt/aurora/harness/claude-code`),
entrypoint `/usr/local/bin/aurora-supervisor run`, user `agent` (uid 1000),
`WORKDIR /workspace`.
- Durable user-space installs live in the home: `npm config prefix ~/.local`, `pip --user`, `uv tool`; `~/.local/bin` is on `PATH`.
- No root, no sudo, no package manager usable by the agent.

Upgrades: running sandboxes keep their image until their next boot; the runner
config `runner.image` pins the version (follows the Aurora version by default).