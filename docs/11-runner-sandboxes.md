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
  "harness": { "id": "claude-code", "capabilities_expected": [ "resume", "steer" ] },  "timezone": "Europe/Zurich"
}
```

(phase 4, P4-HAR, D-122) `harness.id` is the agent's `agents.harness` (`claude-code`, `codex`, `fake`), chosen by the API only; an unknown id or a runtime missing from the image fails the boot with `reason_code = harness_unsupported` (`12 §Harness selection and adapter contract`). The runner registers `harnesses` from the image label `org.aurora.harnesses` (`16 §W5`); an image without the label is treated as `claude-code,fake`, so images built before phase 4 keep working.

(FIX-010, D-87) `gateway.proxy` and `server_name` come from `gateway.sandbox_proxy_url`, not from the
advertised proxy URL BYO agents get (`06`): on the Kubernetes backend the default is `aurora-gateway:4102`, the
host alias the runner writes into every sandbox Pod (sandboxes have no DNS) and a name on the proxy
certificate; on podman it is the advertised URL. The supervisor still verifies the certificate against
`ca_pem` and `server_name`.

Non-secret environment set on the container by the runner: `TZ`, `HOME=/workspace`,
`LANG=C.UTF-8`, `AURORA_SANDBOX_ID`, `AURORA_AGENT_HANDLE`,
`HTTPS_PROXY=HTTP_PROXY=http://127.0.0.1:3128` (+ lowercase),
`NO_PROXY=localhost,127.0.0.1`, CA variables pointing at
`/run/aurora/ca.pem` (`SSL_CERT_FILE`, `NODE_EXTRA_CA_CERTS`,
`REQUESTS_CA_BUNDLE`, `CURL_CA_BUNDLE`, `GIT_SSL_CAINFO`), and app `env` entries
from the snapshot. The proxy URL carries **no credentials**: the supervisor's
loopback forwarder adds them (`12 §Loopback proxies`).

(FIX-014) An app contributes env and stub files only through an **`active`** connection the agent's
allow rules select: a `disabled` (or `reauth_required`) connection contributes nothing, and disabling or
re-enabling one flags `restart_to_update` like an app becoming unavailable. It stays in
`GET /agents/{agent}/effective` with its `status` and no tool decisions.

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

(phase 2, P2-DBL-2, D-89) While the home sync carries database connections the supervisor also
listens on `127.0.0.1:3130`, a PostgreSQL relay that logs in to the gateway's database lane with the
boot credential (or the turn context credential) as the password. The container environment gains
only `PGSERVICEFILE` (set by the supervisor for the processes it starts); the boot credential stays
out of every file and environment the agent can read (`12 §Database relay and files`).

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
 org override &gt; runner default, clamped to runner bounds) → `sandbox.start`. The runner
 default is `RunnerCapabilities.defaults` (`runner.sandbox_cpus × 1000`,
 `sandbox_memory_mib`, `sandbox_pids`, `home_gib`, sent at registration); a runner that
 sends none gets the `docs/15` values (1000 m / 2048 MiB / 512 pids / 10 GiB). There is no
 org override in the code yet.
2. Runner: `provision_home` (idempotent) → `start` (with hold callback) → reports
 `sandbox.status running|failed` with `container_ref`.
3. API on `running`: opens a `runtime_intervals` row, stamps `last_active_at`.
4. Supervisor dials, claims bootstrap → `channel_replica_id` set → supervisor
 sends `ready` (harness capabilities) → API emits the full home sync → turns
 dispatch once `home_applied_generation >= desired`.

### Start readiness deadline (FIX-025, D-158, C-580)

The whole path from step 1 to the `ready` of step 4 is bounded by
`hosted.sandbox_ready_timeout_secs` (60–1800, default 300, `15`): the runner's
capacity wait counts inside it. The API's readiness sweep (10 s interval,
`services/aurora-api/src/domains/hosted/readiness.rs`) fails the attempt at the
deadline with the code that names where it stopped (`10 §Failure semantics`):
`runner_unavailable`, `pod_refused`, `pod_failed`, `home_volume_unbound`,
`image_unavailable`, `supervisor_unreachable`, `supervisor_boot_failed`. Each
failure ends the queued turns with a human, sanitised reason, shows on the
Computer tab (`C-584`) and is recorded for Admin › Runners (C-586). Every
attempt stamps `start_attempt_at` / `start_phase` / `start_detail` /
`start_progress_at` as the runner reports progress; a new boot resets them.
Each attempt also appends to the **start log** (`start_log`, C-612): a bounded
ring of the boot's phase reports, its failure verdict and any retry note —
the live trail of a starting sandbox and the frozen record of a failed one,
rendered on the Computer tab.
Automatic retries stop after 3 failures (`start_failures`); a message sent
after the last failure retries once the backoff has passed.

**Retry after a failure** (FIX-029, D-160, C-610): `POST
/v1/agents/{agent}/sandbox/retry` (same authority as restart, `can_use`), only
from `failed` (409 `sandbox_not_failed` otherwise or while a lifecycle lease is
live; 423 `frozen`). It clears the failure fields, **resets `start_failures`
to 0** — the bound stops failure storms, not recovery after an operator fix —
claims the ordinary lifecycle lease, notes the retry in the start log (audited
`agent.sandbox_retry`) and notifies due work: the next start is the ordinary
due-work start arm with a fresh `boot_id`, bounded by the same readiness
deadline; a failed retry is a start failure like any other (new failure row,
C-587). The dashboard offers **Retry now** in the chat error block and on the
Computer tab (`C-615`); after a retry in chat the failed message's text is
re-posted as a new message. An agent's past start failures are visible to its
users (`GET /v1/agents/{agent}/start-failures`, C-613) and to admins with the
runner named (`GET /v1/admin/agents/{agent}/start-failures`, C-614) — both read
`sandbox_start_failures`, no new table.

### Keep-awake (single definition)

A running sandbox is **kept awake** if any of:

- an active turn or `joining` follow-up exists;
- a **busy** process (recent output or CPU) on the current boot, or an **armed
watch** — both only while `now − last_engaged_at < BACKGROUND_CAP`;
- (phase 4) a lease-current SSH session;
- (phase 4, P4-PRV-1, D-145) a **preview in use**: `preview_active_until > now()` and fewer than
  `PREVIEW_AWAKE_CAP` (4 h) since the first preview heartbeat of this boot (`§Port previews`);
- (phase 4, P4-TERM, D-161) a **terminal session in use**: `terminal_active_until > now()` and
  fewer than `TERMINAL_AWAKE_CAP` (4 h) since the first session byte of this boot
  (`§Web terminal`).

(phase 4, RFE-011b) A **parked approval is not a keep-awake reason** (`07 §Park and resume`):
the agent was refused, its turn ends, and the pending decision lives in Postgres. The
sandbox idles out and parks like any other; the resume follow-up (`10 §Turns`) is ordinary
due work (start arm → new boot, harness session restored from `harness_session_ref`, home volume
intact, processes of the old boot `parked`/`lost`). A *held* approval still keeps the sandbox awake
through its active turn, as before.

`last_engaged_at` is stamped by person/turn engagement (a turn created by a
person, schedule, webhook or watch); it is distinct from `last_active_at`
(stamped also by every boot). The same predicate feeds the stop arm, eviction
and the dashboard's "awake because…" badge.

### Stop (park)

- **Idle**: no keep-awake reason and `now − last_engaged_at ≥ IDLE_STOP` →
`stop_reason = idle`.
- **Frozen** (phase 2, RFE-006): the agent is effectively frozen (`04 §Freeze`) →
`stop_reason = frozen`, regardless of keep-awake reasons; the home volume is kept
and a frozen agent is never started or reconciled away.
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

## Port previews (phase 4, P4-PRV-1, D-144, D-145, SEC-80)

A **preview share** lets people of the workspace open a web app that the agent runs on a TCP port
inside its sandbox (a dev server, a report, a notebook) from their browser, at a stable
hostname, without any inbound path into the sandbox. This section owns the share model and
its lifecycle; the browser-facing service, authentication and byte streams are P4-PRV-2
(`16 §W17`, `04 §Preview access`, `15 §Port previews`).

**Off unless configured, at three levels.** A share exists and works only when (1) the
instance sets `previews.enabled` **and** a valid `previews.domain` (a wildcard DNS name and
TLS certificate are required, `15 §Port previews`); (2) the organization's owner or admin turned
**Port previews** on for the org (`organizations.settings.previews_enabled`, default `false`,
audited `org.previews.update`, security-critical); (3) a person created the share. With any of
the three missing the product behaves exactly as before: no tab, no routes beyond
`404 previews_disabled`, no allow-list in the sandbox (the supervisor refuses every stream, a
second line of defence), no relay needed. Existing configurations are unchanged.

**Share model** (`03 §Planned changes`, `preview_shares`).

- One share per `(agent, port)`. Only **hosted** agents (a BYO agent has no sandbox: 422
  `preview_not_supported`). Port `1024..65535` and not one of the supervisor's own
  loopback listeners (`3128`, `3129`, `3130`, the per-session proxy ports are ephemeral and refused
  because the supervisor knows them, `12`): 422 `preview_port_forbidden`. The port does not
  need to be listening when the share is created (a start command may bring it up).
- **Hostname**: `<label>.<previews.domain>`, `label = "p" + 24 base32 characters` (lowercase
  `a-z2-7`, 120 random bits from the OS CSPRNG, unique index). The label contains **no**
  agent, org, workspace or port information, is **stable for the life of the share**
  (survives park, wake, restart, pause, freeze) and is **never reused**: deleting a share and
  creating it again for the same port yields a new hostname (a bookmark, a cached redirect
  or a stale grant can never reach a later share). The hostname is an address, not a secret:
  access is always authenticated (below).
- **Visibility**: exactly one value exists in phase 4, **`private`**: a person who has a live
  dashboard session and for whom `can_use(workspace)` holds at the time of the request
  (the access law, `04`: members, org owners and admins). There is **no** `public`,
  `org`, link-with-token or per-person visibility, and no API key access (a preview is a browser
  surface; machine callers use the agent's own tools). The column is `CHECK (visibility IN
  ('private'))`; a later visibility is an additive migration and a new decision.
- **Who may create, change and delete a share**: `can_use` of the agent's workspace
  (`04 §Permission matrix`): it is the same authority as operating the agent (chat, schedules,
  restart) and exposes the app only to people who can already read the agent's transcripts.
  **Agent tools can never create, change or delete a share** (SEC-80): an agent cannot publish
  itself; it can only tell its people the port in chat. Limits: `PREVIEW_SHARES_PER_AGENT` (10),
  `PREVIEW_SHARES_PER_ORG` (200); 409 `preview_limit` beyond them; 409 `preview_exists` for a
  second share on the same port.
- **State**: `active` or `paused` (an owner or operator pauses a share without deleting it: the
  hostname is kept, access is refused with 403 `preview_paused`, no wake, no start command).
  The *displayed* status adds `waking`, `listening`, `not_listening`, `frozen`,
  derived from the sandbox and `ports.state` (not stored).
- **Start command** (optional, ≤ `PREVIEW_START_COMMAND_MAX` 1 024 chars, one line, plus an
  optional `cwd` relative to `/workspace`): the command the supervisor runs **on demand** to bring
  the app up when a preview request finds the port not listening (below). It runs through the
  process manager as the agent user with the container's ordinary environment (machine proxy, no
  platform credential, no `AURORA_*` variable): exactly the privilege the agent already has,
  never more. The command text is stored in the share row, shown to people who can use the
  workspace, **never** written to the audit log (only `start_command_set` and the first 12 hex
  characters of its SHA-256), and never given to the relay.

**Port discovery.** The supervisor reports `ports.state { ports: [int] }` (`16 §W5`) on its
10 s harvest tick (first scan at boot), scanning `/proc/net/tcp{,6}` for TCP LISTEN ports on
loopback-reachable addresses (loopback or wildcard bind), sent only on change, stored as
`sandboxes.listening_ports` (a churn column, reset at
every boot). The dashboard's
"Add share" dialog offers the currently listening ports (from `GET /v1/agents/{a}/sandbox`),
and a share's `listening` status is `port ∈ listening_ports` of the current boot. Discovery is
informational only: it never creates a share and the allow-list never contains a port merely
because it listens.

**Allow-list in the sandbox.** The supervisor learns which ports are shared only from the API,
through a new home-sync part `previews` (`16 §W5`, `12 §Previews`): `{ shares: [{ share_id,
port, start_command?, cwd? }] }`, containing the **active** shares of the agent (none while the
instance, the org or the agent is switched off or the agent is frozen). It is held in the
supervisor's memory (never read back from the home, which the agent can write). A share
change bumps the agent's `home_sync_generation` (like skills) so a running sandbox receives it
at once; a bump never wakes a parked sandbox. The runner independently enforces the same
binding from the API's answer when a stream opens (`16 §W17`): both ends must agree on
the port, so neither a compromised relay nor a compromised agent can reach another port.

**Wake and start on a preview request** (D-145). A request to a share of a **parked** sandbox
is a **wake demand**: the relay's resolve call (`05 §Relay plane`) sets
`sandboxes.preview_wake_at = now()` (rate limit `PREVIEW_WAKES_PER_SHARE_HOUR` 12 per share;
beyond it 429 `preview_wake_limited`, shown by the relay as a "try later" page), and the **start arm**
of the due-work engine (`05`) treats a demand younger than `PREVIEW_WAKE_WINDOW` (60 s) as due work
for a sandbox that is not effectively frozen, subject to the ordinary start backoff
(`§Failure`) and the runner's capacity. The boot is the normal boot (`§Start`); then:

1. when `home_applied_generation >= desired` (the `previews` part is applied) and the share has
   a start command and its port is not in `listening_ports`, the API enqueues the work item
   `preview.start { share_id }` (`16 §W5`), the runner forwards the frame `preview.start { port }`
   and the supervisor starts the share's command as a process named `preview:<port>`
   (kind `preview` in `process.state`, visible in the Computer tab, ends with the boot). It looks
   the command up in its own allow-list by port: the frame carries no command text;
2. the relay keeps the browser's request waiting until the port is listening, at most
   `PREVIEW_WAKE_WAIT` (45 s); otherwise it answers a `503` page that refreshes itself;
3. a start command that exits is relaunched by a later request, at most `PREVIEW_START_ATTEMPTS`
   (3) times per share per `PREVIEW_START_WINDOW` (10 min); the dashboard shows the last exit
   code and the tail of the process output for people who can use the workspace.

A share **without** a start command never starts anything: a request to a running sandbox
whose port is not listening waits up to `PREVIEW_LISTEN_WAIT` (5 s) and then answers 502
"nothing is listening on this port"; a request to a parked sandbox still wakes it (the agent
may bring the app up by itself, e.g. from its shell history or a schedule) and waits the same
`PREVIEW_WAKE_WAIT`. A preview **never** creates a turn, a conversation event or a
`last_engaged_at` stamp.

**Keep-awake** (`§Keep-awake`). While people are really using a preview the sandbox must not
park under them: the relay's heartbeat (`05 §Relay plane`) refreshes
`sandboxes.preview_active_until = now() + PREVIEW_AWAKE_GRACE` (120 s) whenever at least one
stream is open or one completed in the last grace period, and the keep-awake predicate gains
the reason **`preview`** while `preview_active_until > now()` **and** `now() − preview_awake_since
< PREVIEW_AWAKE_CAP` (4 h; `preview_awake_since` is set when the first heartbeat of a boot
arrives and reset at the next boot), so an open WebSocket cannot hold a sandbox awake forever.
Past the cap the sandbox parks like any idle one (`stop_reason = cap`) and the next request wakes
it again. The dashboard's "awake because…" badge says "Preview in use".

**Lifecycle of a share** (the table is normative; "stays" means the row and hostname are kept):

| Event | Effect on shares | Effect on access and streams |
|-------|------------------|------------------------------|
| Sandbox idle-parks, cap, evict | stay | request wakes the sandbox (above); open streams were closed by the stop (`channel_lost`) |
| Agent restart, runner restart, W6 reconnect that lost state | stay | streams closed; the browser retries |
| Agent **frozen** (agent, workspace or org, RFE-006) | stay, inert | **no wake, no start, no stream**: 423 `frozen` page "paused by an administrator"; open streams closed within `PREVIEW_AUTH_CACHE` (15 s); allow-list emptied; unfreezing restores access, nothing is replayed |
| Share paused | stay | 403 `preview_paused`; streams closed within 15 s; allow-list loses the port |
| Share deleted | row deleted; hostname never reused | sessions of that share deleted; 404; streams closed within 15 s; allow-list loses the port at the next sync |
| Agent deleted (or workspace, org deleted) | cascade-deleted with the agent | as deleted; the sandbox is destroyed by reconcile |
| Org **Port previews** switched off, or `previews.enabled` removed | stay, inert | 404 `previews_disabled`; streams closed; allow-list emptied; switching on restores them |
| Member loses access, is suspended, removed, or signs out | stay | that person's preview sessions end (below, `04 §Preview access`) within 15 s |
| Harness changed, agent moved to another runner | stay | streams closed; the next request resolves the new placement |

**Runner stream listener** (P4-PRV-2, D-147). When `previews.enabled`, each runner replica opens a
third listener, `runner.stream_bind` (default `server.bind`) port **4108**, that serves only
`GET /v1/preview-stream` (`16 §W18`); startup warns when its address overlaps the one
of the sandbox-facing 4105 listener (the same rule as `runner.ops_bind`). It admits a connection only with a valid stream
grant, which the runner redeems at the API; it holds no secret and never accepts a
port or sandbox from the caller. It is reachable from the relay and nothing else: the sandboxes
namespace has no route to it (NetworkPolicy and the `listener_binds`-style guard, SEC-09/SEC-82).
The runner multiplexes the streams of a sandbox over that sandbox's supervisor channel
(`16 §W18`), keeps per-stream buffers of at most `PREVIEW_STREAM_WINDOW`, enforces
`PREVIEW_STREAMS_PER_SANDBOX`, closes every stream of a sandbox when its channel ends, the boot
changes, or the sandbox stops, and tells the API when a stream closes (accounting only). A runner
without `previews` support (older version) does not open the listener and registers without the
capability `previews`; the API places no preview on it (`unavailable`).

**Audit** (`04 §Audit`; security-critical, same transaction): `preview.share.create`,
`preview.share.update` (pause/resume, start command changed), `preview.share.delete`,
`org.previews.update`. Not audited per request (volume): access is visible as metrics and
`preview_shares.last_accessed_at`; a person's first access per preview session writes the
best-effort row `preview.session.create` (`04 §Preview access`).

## Web terminal (phase 4, P4-TERM, D-161, SEC-87)

An interactive shell into the sandbox from the dashboard (**Terminal** tab, `14`), rendered
with wterm and its Ghostty core and transported over the preview-stream machinery with a
**PTY target** instead of a TCP port (`16 §W19` on `16 §W18`'s listener, grant pattern and
credit flow control). Off unless the operator sets `terminal.enabled` (like previews, an
interactive path into sandboxes is a deliberate deployment choice); every surface answers
404 `terminal_disabled` while off. No relay and no previews domain: the browser upgrades a
same-origin WebSocket on the API (`GET /v1/agents/{agent}/terminal`, session cookie only,
`can_use` of the agent's workspace — the same authority as operating the agent; BYO agents
422 `terminal_not_supported`), and the API mints and redeems the single-use `auy_` grant at
the runner's existing :4108 listener (`kind: "terminal"`, C-617); the sandbox itself gains
**no** new network path.

- **Wake and keep-awake.** Opening a terminal on a parked sandbox is a wake demand (the
  preview start-arm path, `TERMINAL_WAKE_WINDOW` 60 s, `TERMINAL_WAKES_PER_AGENT_HOUR` 12,
  never for a frozen agent). While a session carries bytes (grace `TERMINAL_AWAKE_GRACE`
  120 s after the last byte) the sandbox is kept awake with the reason **`terminal`**, capped
  at `TERMINAL_AWAKE_CAP` (4 h per boot, refreshed from the first session byte); past the cap
  it parks like any idle sandbox and the next open wakes it. A terminal is never a turn, a
  conversation event or a `last_engaged_at` stamp.
- **Supervisor side** (`12 §Terminal`): the runner's `pty.open` (only for an API-redeemed
  grant of the current boot) makes the supervisor spawn the image shell in a PTY as the agent
  user, cwd `/workspace`, container environment minus every `AURORA_*` variable and the boot
  document — the shell has exactly the privilege a `process_start` command already has, in a
  terminal. `pty.resize` applies `TIOCSWINSZ`; a W6 reconnect or a stop closes every PTY.
- **Lifecycle**: frozen agent → sessions closed within `TERMINAL_AUTH_CACHE` 15 s and no new
  open (423); restart/park → sessions closed (4503/`channel_lost`), the tab shows "Waking…"
  and reopens on ready; harness change or runner move → sessions closed; agent deleted →
  destroyed with the sandbox. Bytes are never logged, stored or request-log-rowed (byte
  totals in metrics only); the best-effort audit row `agent.terminal.open` records the open
  (agent, workspace), never content (`04 §Audit`).
- **Listener and capabilities**: the runner opens :4108 when previews **or** the terminal is
  enabled and registers the matching capabilities (`previews`, `terminal`, plus `stream_tls`
  when `runner.stream_tls` is on — the API derives the endpoint scheme it hands the relay
  from it, `16 §W18`); the NetworkPolicy
  admits :4108 from the relay pods **and** the api pods (only those), `15 §Port previews`.
  An older runner without the `kind` extension refuses terminal grants (the API answers 4502
  — the tab shows "the runner does not support terminals").

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

- **Base layer** (rebuilt rarely, tagged by content hash): AlmaLinux 10 (phase 0–2 named the CERN registry image `gitlab-registry.cern.ch/linuxsupport/alma10-base`; **from phase 3 (RFE-019, D-63)** the specified base is the **public official AlmaLinux image**, `docker.io/library/almalinux:10`, pinned by digest in `images/sandbox/Containerfile` as `ARG SANDBOX_BASE_IMAGE=docker.io/library/almalinux:10@sha256:<digest recorded at release>`; a deployment that must use a private or mirrored base overrides the build argument `SANDBOX_BASE_IMAGE` — the package set and the image labels do not depend on which base is used; this deployment's override is the CERN `alma10-base` image, `15 §Deployment-specific settings example`), `git`, `curl`, `jq`, `ripgrep`, `fd-find`, `less`,`openssh-client`, `python3` + `pip` + `venv` + `uv`, Node.js LTS + npm, `build-essential`, `chromium` (headless), `unzip`, `zip`, `procps`, `tini`-free. (phase 1, RFE-003) The image carries the label `org.aurora.capabilities=browser` (it ships the Chromium wrapper); a custom image without Chromium omits it and its sandboxes get no open-lane listener.

- **App layer**: `aurora-supervisor` binary, pinned harness runtimes (Claude Code
CLI at a pinned version installed under `/opt/aurora/harness/claude-code`;
(phase 4, P4-HAR, D-122) the pinned Codex CLI under `/opt/aurora/harness/codex`, installed
by default and omitted with the build argument `SANDBOX_HARNESSES="claude-code"`, which
shrinks the image by the size of the Codex binary; each runtime comes from a pinned,
checksum-verified release artifact, and the image label `org.aurora.harnesses` lists the
runtimes present, e.g. `claude-code,codex,fake`),
entrypoint `/usr/local/bin/aurora-supervisor run`, user `agent` (uid 1000),
`WORKDIR /workspace`.
- Durable user-space installs live in the home: `npm config prefix ~/.local`, `pip --user`, `uv tool`; `~/.local/bin` is on `PATH`.
- No root, no sudo, no package manager usable by the agent.

Upgrades: running sandboxes keep their image until their next boot; the runner
config `runner.image` pins the version (follows the Aurora version by default).