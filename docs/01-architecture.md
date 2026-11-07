# 01 — System Architecture

## Components

| Component | Binary / image | Role | Port(s) | State | Inbound from |
|-----------|----------------|------|---------|-------|--------------|
| **Edge proxy** | Caddy (compose) / Ingress or Gateway API (k8s) | Single external origin; routes `/v1/*` to API, everything else to web | 4000 (compose) | none | browsers, CLI, webhooks |
| **Web dashboard** | `aurora-web` (Next.js) | Human UI; a pure client of the API | 4100 | none | edge proxy |
| **API server (control plane)** | `aurora-api` | Owns the database; management API; identity; conversation plane; due-work engine; plugin registry compiler; channel core; platform-tool execution | 4101 public (`/v1`); 4111 internal (`/internal`, hook host only) | **Postgres** | edge proxy, runners, connectors; hook hosts on 4111 |
| **Gateway** | `aurora-gateway` | Agent-facing forward proxy: auth, policy, approval holds, credential injection; database lane (phase 2) | **4102** proxy (TLS-only from phase 1); **4103** internal control | reads Postgres; writes approvals, request log, refreshed credentials | agents (4102); API only (4103) |
| **Hook host** | `aurora-hookhost` | Runs plugin WebAssembly hooks in wasmtime | 4104 (internal) | none (module cache) | API, gateway |
| **Runner** | `aurora-runner` | Starts/parks/reaps sandboxes via a backend; relays supervisor traffic | 4105 (sandbox-facing WebSocket) | none | supervisors only |
| **Supervisor** | `aurora-supervisor` (in sandbox image) | Harness driver, home projection, platform-tool bridge, process manager, loopback proxies | loopback only | home volume | nothing (dials runner) |
| **Connector** (per platform) | `aurora-connector-<id>` | Chat platform transport, translation, provisioning | platform callbacks only (e.g. 4110) | none | its chat platform only |
| **SSH terminator** (phase 4) | `aurora-terminator` | Certificate-authenticated SSH into sandboxes | 2222 | none | users |
| **PostgreSQL** | — | System of record, queue, event bus | 5432 | yes | API, gateway |
| **Migrations job** | `aurora-api migrate` | Forward-only schema migrations before API starts | — | — | — |
| **CLI** | `aurora` | Login, BYO `run`, management, plugin tooling | — | `~/.config/aurora` | — |

There is **no Redis** and no message broker: Postgres provides the work queue
(claims with `SKIP LOCKED`) and the event bus (`LISTEN/NOTIFY`).

## The planes

```
                     people                                         agents
       (browser · chat platform · CLI · SDK)                 (in sandbox / on a laptop)
                        │                                              │
                        ▼                                              ▼ :4102 (proxy only)
┌──────────────────────────────────────────────┐       ┌──────────────────────────────────┐
│ MANAGEMENT / CONVERSATION PLANE              │       │ DATA PLANE                       │
│  edge ─► web                                 │       │  Gateway (CONNECT + MITM, open)  │
│  edge ─► API ◄──────► PostgreSQL ◄───────────┼─reads─┤  auth → resolve → policy →       │
│           ▲  ▲  ▲  ▲      (queue, NOTIFY)    │ writes│  hold → select → inject → fwd    │
│           │  │  │  └─ hook host ◄────────────┼───────┤  (slow paths: mint, summarize)   │
│           │  │  │       (wasm hooks)         │       └──────────────────────────────────┘
│           │  │  └──── connectors ◄─► chat platforms (outbound sockets, own callbacks)
│           │  └─────── runner ◄── supervisor (WebSocket, per sandbox)
│           └────────── SSH terminator (phase 4)
└──────────────────────────────────────────────┘
                              COMPUTE PLANE: sandbox (container/pod) + durable home
                                 supervisor ─► harness ─► loopback proxy ─► gateway only
```

**Management / conversation plane** — the API server and Postgres. Everything
durable lives here. The API computes *what work is due* (turns to dispatch,
sandboxes to start or stop, schedules to fire) when a runner polls; there is no
separate scheduler process. API replicas are interchangeable: every
cross-replica signal goes through Postgres `LISTEN/NOTIFY` (see `05`).

**Data plane** — the gateway. Agents reach it as an HTTP(S) forward proxy. It
reads configuration directly from Postgres, caches per (credential, host) for
≤60 s and is told to drop caches via `NOTIFY`, so the API is never on the
request path. Gateway replicas are interchangeable (approvals live in Postgres).

**Compute plane** — runners and sandboxes. A runner never touches the database;
it speaks HTTP to the API (long-poll for work, post reports) and hosts the
WebSocket server that supervisors dial.

**Plugin execution** — the hook host runs untrusted plugin code with no
database, no keys, no ambient network. It is called only on slow paths.

## Trust boundaries

1. **Agent ↔ platform.** The agent (harness, everything in the sandbox, every
   BYO client) is untrusted. Its only egress is the gateway proxy port. Real
   credentials never enter a sandbox. The *network* enforces this — an
   internal-only sandbox network (podman) or NetworkPolicy (Kubernetes), with
   DNS egress denied — not application code.
2. **Sandbox ↔ control plane.** The sandbox never talks to the API. The
   supervisor talks only to the runner over a WebSocket admitted by a
   single-use bootstrap token, then by an HMAC session. The runner stamps
   sandbox identity on everything it forwards; the API fences every report
   against the runner's ownership.
3. **Runner / connector / terminator ↔ API.** Each authenticates with an
   operator-provisioned anchor token and calls the API outbound. Replicas are
   tracked; lifecycle ownership is fenced per sandbox / per presence.
4. **Gateway ↔ database.** The gateway uses a least-privilege database role:
   read on configuration tables; write only on approvals, request log,
   refreshed connection credentials (compare-and-set), reauth flags and its own
   instance rows.
5. **Gateway control port (4103)** is internal: reachable from API replicas
   only (network policy), authenticated with the internal secret. It is never
   on the sandbox network.
6. **Hook host.** Plugin code is untrusted. It receives per call exactly the
   data one hook invocation may see; egress is limited to the package's
   approved hosts; `approval.summarize` has no egress at all.
7. **Connectors.** A connector is untrusted towards the core: it cannot act as
   a user, cannot authorize anything, and sees only the credentials of its own
   platform's integrations and presences. The core authorizes every click and
   every speaker. (phase 1, RFE-001) A connector also cannot **mint user
   links** by assertion: the core cannot tell which binary holds a connector
   token, so a link is created only when the signed-in Aurora person confirms
   it (sign-in link). The one exception is the per-integration, org-admin
   opt-in `verified_email_autolink` (default **off**), which explicitly extends
   trust to that connector's `email`/`email_verified` claims (`13 §5.9`).
8. **Secrets.** Only the API, the gateway and (phase 3) the configured
   external stores ever see plaintext. The hook host sees one connection's
   plaintext per call. Connectors see their own platform credentials.
9. **Chat traffic** to platforms is platform traffic, not agent traffic: it
   does not pass through the gateway.

## End-to-end flows

### A. An agent makes an HTTPS call (data plane)

1. The agent's HTTP client uses the gateway as `HTTPS_PROXY` and trusts the
   gateway CA. Inside a sandbox the proxy is a loopback forwarder run by the
   supervisor, which adds the right gateway credential and reaches the
   gateway over TLS (phase 1, RFE-002: the gateway proxy listener is TLS-only,
   in-cluster and external alike).
2. `CONNECT host:443` with `Proxy-Authorization`. The gateway resolves the
   credential → agent, workspace, org (+ conversation, turn).
3. Connect-time resolution (cached ≤60 s): policy views (org + workspace),
   injection candidates (app connections via the registry snapshot's host
   index; secrets via host/path patterns; LLM keys), guidance context.
4. TLS is terminated with a leaf certificate minted from the gateway CA.
5. Per request: destination guard → method-override refusal → policy decision →
   optional approval hold → credential selection → token freshness (refresh or
   `token.mint`) → injection → optional signing → forward → stream response;
   upstream auth failures on uncredentialed requests become guidance.
6. Request log rows are batched and written off the hot path.

Detail: `06`, `07`, `08`, `09`.

### B. A person chats with a hosted agent

1. A message arrives (dashboard, connector, schedule, webhook, watch, peer). The
   API creates a **turn** (`queued`) in the right conversation. A message sent
   while a turn is active becomes a **follow-up** to steer into the live run.
2. On the next runner poll the API computes due work. If the sandbox is not
   running it emits `sandbox.start` (image, env, files, resources, boot token,
   bootstrap token); the runner provisions the home and starts the container.
3. The supervisor dials the runner, reports `ready`; the API sends the home
   sync (skills, memory, instruction render inputs, tool catalogue) and then
   `turn.deliver` with a fresh **context credential**.
4. The supervisor runs the turn on the harness, streams canonical events up and
   relays platform-tool calls.
5. The API persists durable events, fans out to SSE subscribers via `NOTIFY`,
   and, if the turn came in through a chat thread, enqueues the answer as a
   connector operation.
6. After the idle window the API emits `sandbox.stop`; the runner parks the
   sandbox (container removed, home kept).

Detail: `10`, `11`, `12`.

### C. A held request approved from chat

1. The gateway inserts a pending **approval** row (with conversation and turn
   from the context credential) and `NOTIFY`s.
2. The API's channel core sees the new approval for a conversation that has a
   thread link and enqueues a `message.post` card operation carrying core-signed
   **action tokens**.
3. The connector renders the card. A click returns `action.invoked` (token +
   clicker's platform id). The core verifies the token, resolves the clicker
   via the user link, applies the access law, writes the decision
   (`UPDATE … WHERE status='pending'`) and `NOTIFY`s.
4. The holding gateway replica wakes, releases or refuses the request; the card
   is updated to its settled state.

### D. Connect an app (OAuth)

1. Dashboard → `POST /v1/apps/{app}/connect` → the API builds the authorize URL
   from the manifest's `oauth_clients` entry (or `connect.authorize` hook), with
   an encrypted state bound to the browser.
2. Provider redirects to `/v1/oauth/callback`. The API exchanges the code (or
   calls `connect.exchange` in the hook host), runs the optional probe, stores
   credentials through the secret store, and creates the connection.
3. The API `NOTIFY`s gateway cache invalidation for the workspace/org.
4. From then on the gateway keeps the token fresh (`oauth2_refresh` or
   `token.mint`) under a per-connection advisory lock.

### E. Install a plugin package

1. An instance admin uploads a package (or it is present in the plugins
   directory / baked into the image as built-in).
2. The API verifies the signature against trusted keys, validates and lints
   the manifest, and records the package as `pending_approval` (or activates it
   directly when it does not widen permissions of an already-approved version).
3. The instance admin approves; host-claim conflicts are resolved.
4. The API compiles a new **registry snapshot**, stores it by content hash,
   activates it and `NOTIFY`s. Gateways and hook hosts load it atomically.

### F. A message arrives on a chat platform

1. The connector receives the platform event and `POST`s a neutral
   `message.posted` event to `/v1/connector/events`. The API writes it to the
   durable inbox (deduplicated by platform event id) and answers; only then
   does the connector ack the platform.
2. An inbox worker resolves the target agent (dedicated bot, space binding or
   DM default), the speaker (user link), applies the access law / reach, maps
   the thread to a conversation and creates the turn (idempotent on event id).
3. A receipt operation (reaction or status) is enqueued. Flow B continues.

### G. A schedule fires

On a runner poll the due-work engine claims due schedules (`fire_lease_until`),
computes the next real occurrence, re-checks the creator's authority, and
creates a turn in the schedule's run conversation. The sandbox wakes as in B.

## Deployment topologies

- **Development (podman compose)**: postgres, migrate (one-shot), api, web,
  gateway, hookhost, edge (Caddy); profile `hosted` adds runner; profile
  `connector-fake` adds the fake connector. See `15`.
- **Production (Kubernetes, Helm)**: same images; sandboxes are one Pod + PVC
  each in a fenced namespace with NetworkPolicy and ResourceQuota; API, gateway,
  hookhost, runner and connectors scale horizontally. See `15`.

## Design principles (apply everywhere)

- **Registries keyed by ids with load-time completeness checks** (LLM providers,
  sandbox backends, harness adapters, secret stores, platform tools,
  mechanisms). Adding one is a module plus one registration line. An id with no
  implementation fails at startup, loudly.
- **Every wire message is schema-validated at the receiver.** Codes, provider
  ids and reasons are **open bounded strings**: unknown values degrade (treated
  as absent or generic), never crash.
- **Bound at the sender.** Producers enforce sizes so their own messages are
  never rejected by their own validation.
- **Poll-time truth.** Liveness and dueness are computed from timestamps and
  leases at query time. A crashed daemon ages out without anyone noticing.
- **Claims, not locks.** Multi-replica safety via
  `UPDATE … WHERE due AND (lease IS NULL OR lease < now()) … FOR UPDATE SKIP LOCKED RETURNING`
  with a lease in a **single-purpose column** (never reused as `updated_at`).
- **Fail closed on uncertainty.** Truncated body, unparsable GraphQL, unknown
  condition, unknown mechanism, unreachable secret store → the stricter outcome.
- **Idempotency in the database** for every at-least-once source (connector
  events, runner reports, webhook deliveries).
- **Untrusted text is never promoted to platform voice.** Model-, user-,
  plugin- and platform-authored strings are sanitized (`02 §Untrusted text`)
  and rendered as data.
- **No platform or app names in the core.** A grep-based CI check enforces it
  (`18`).
