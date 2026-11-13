# 01 — System Architecture

## Components

| Component | Binary / image | Role | Port(s) | State | Inbound from |
|-----------|----------------|------|---------|-------|--------------|
| **Edge proxy** | Ingress or Gateway API (k8s; ingress-nginx on the kind dev cluster; Caddy in phase 0 compose, removed in phase 1, OPS-017) | Single external origin; routes `/v1/*` to API, everything else to web | 4000 (kind dev) | none | browsers, CLI, webhooks |
| **Web dashboard** | `aurora-web` (Next.js) | Human UI; a pure client of the API | 4100 | none | edge proxy |
| **API server (control plane)** | `aurora-api` | Owns the database; management API; identity; conversation plane; due-work engine; plugin registry compiler; channel core; platform-tool execution | 4101 public (`/v1`); 4111 internal (`/internal`, hook host only) | **Postgres** | edge proxy, runners, connectors; hook hosts on 4111 |
| **Gateway** | `aurora-gateway` | Agent-facing forward proxy: auth, policy, approval holds, credential injection; database lane (phase 2) | **4102** proxy (TLS-only from phase 1); **4103** internal control | reads Postgres; writes approvals, request log, refreshed credentials | agents (4102); API only (4103) |
| **Hook host** | `aurora-hookhost` | Runs plugin WebAssembly hooks in wasmtime | 4104 (internal) | none (module cache) | API, gateway |
| **Runner** | `aurora-runner` | Starts/parks/reaps sandboxes via a backend; relays supervisor traffic | 4105 (sandbox-facing WebSocket) | none | supervisors only |
| **Supervisor** | `aurora-supervisor` (in sandbox image) | Harness driver, home projection, platform-tool bridge, process manager, loopback proxies | loopback only | home volume | nothing (dials runner) |
| **Connector** (per platform) | `aurora-connector-<id>` | Chat platform transport, translation, provisioning | platform callbacks only (e.g. 4110) | none | its chat platform only |
| **Connector host** (phase 4, P4-MCON) | `aurora-connector-host` | Runs the built-in connector types as **managed connectors** added from the dashboard (`13 §3.1`); speaks the connector contract on their behalf | 4110 platform callbacks (`/connectors/<id>/…` via the edge); 4192 health/metrics | none | edge (platform callbacks) |
| **Preview relay** (phase 4, P4-PRV-2; **only when `previews.enabled`**) | `aurora-relay` | Browser-facing HTTP/WebSocket reverse proxy for preview hostnames: authenticates the browser (`04 §Preview access`), resolves hostnames through the API, opens byte streams to a sandbox port through the runner (`16 §W17`, `§W18`) | 4112 (edge-routed wildcard host); 4193 health/metrics | none (in-memory caches only) | edge (wildcard preview host) |
| **SSH terminator** (phase 4) | `aurora-terminator` | Certificate-authenticated SSH into sandboxes | 2222 | none | users |
| **PostgreSQL** | — | System of record, queue, event bus | 5432 | yes | API, gateway |
| **Migrations job** | `aurora-api migrate` | Forward-only schema migrations before API starts | — | — | — |
| **CLI** | `aurora` | Login, BYO `run`, management, plugin tooling | — | `~/.config/aurora` | — |

There is **no Redis** and no message broker: Postgres provides the work queue
(claims with `SKIP LOCKED`) and the event bus (`LISTEN/NOTIFY`). (phase 4, P4-CNT)
This stays true by default: **Redis is an optional backend** of the rate-limit
`CounterStore` only (`15 §Shared rate-limit counters`); nothing is stored there that
is a system of record, no feature requires it, and a deployment without it behaves as
before. The rate-limit counters themselves may also live in Postgres (`postgres`
backend). Trust boundary 4 (gateway ↔ database) gains one grant: the counter table
(`03`).

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
(phase 3, RFE-014) If Postgres becomes unreachable the gateway **degrades** rather
than stops: it serves what its caches already hold, for at most the cache TTL,
and refuses everything that needs new state (fail closed); after a lost LISTEN
connection it reconnects first and flushes afterwards (`06 §Operation during a
Postgres outage`).

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
   DNS egress denied — not application code. (phase 2, RFE-025) A BYO client is
   outside that network: the creation, source address and attribution of BYO
   credentials are governed by the BYO posture, source CIDR list and optional
   context-credential requirement (`08 §11.9`), enforced at the gateway.
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
   instance rows (nothing on `api_keys`). The role is **required** (FIX-106,
   D-178 Q-R5-11): the Helm chart's bundled evaluation Postgres creates it
   automatically; with an external database the chart refuses to render without
   `database.gatewayRole` + `database.gatewayExistingSecret` unless
   `database.allowSharedLogin` explicitly lets the gateway share the main login
   (`15`, `23`). A non-Helm install configures `database.gateway_role` and gives the
   gateway its own `AURORA_DATABASE_URL`; sharing the login there is the
   operator's explicit exception.
5. **Gateway control port (4103)** is internal: reachable from API replicas
   only (network policy), authenticated with the internal secret. It is never
   on the sandbox network. (phase 4, P4-GSL) It also serves `POST /internal/v1/library-fetch`,
   through which the API's skill-library jobs read repository files with an org connection's
   credential without ever holding it (`06 §Library fetch`).
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
10. **Optional external stores** (phase 4, P4-CNT; the S3 half, P4-S3, is phase 5, D-138). The **API** may talk to an
    S3-compatible bucket (attachment bytes, nothing else) and the API and gateway
    to a Redis (rate-limit counters, nothing else). Both are operator-run
    infrastructure reached outbound over TLS with credentials from the workload
    identity or environment variables; neither is reachable from a sandbox, neither
    is a system of record for authorization, and an outage degrades attachments or
    quota accuracy, never policy (SEC-62, SEC-63). Bucket bytes are served only
    through the API's authorization, never by a pre-signed URL.
11. **Identity provider directory** (phase 4, P4-GRP-2). An org's IdP may call the API's
    `/scim/v2` with an org-scoped `aud_` token to provision members and groups. The IdP is
    trusted for *directory state of its own org* and for nothing else: it cannot sign anyone in
    (identity still comes from the OIDC sign-in and its verified e-mail), cannot set a role,
    create or touch an owner, link a person by `externalId`, reach another org, or read any
    Aurora data. Its writes pass the same authorization, limits and critical audit as the
    dashboard's (SEC-72, SEC-73).
12. **Browser push services** (phase 5 (D-138, moved from phase 4), RFE-011b). When `push.enabled`, the **API** (only)
    makes outbound HTTPS requests to the push service named by a user's subscription (FCM,
    Mozilla, Apple, WNS), through the destination guard and only to allow-listed hosts. The push
    service is untrusted for confidentiality: the payload is encrypted to the browser and
    contains no secret, request detail or approval title (SEC-75); a subscription URL supplied by
    a user is never trusted as a destination (SEC-76). No sandbox, gateway or runner takes part.
13. **Preview relay and preview origins** (phase 4, P4-PRV-2). When `previews.enabled`, the
    relay is the only component that faces browsers on preview hostnames. It is **untrusted
    with respect to authorization**: it holds no database access, no secret key and no
    credential that opens anything by itself; every decision (who, which share, which sandbox,
    which port) is made by the API and re-enforced by the runner and the supervisor, so a
    compromised relay can at most deny service or relay for a user who already holds a valid
    preview session of a share (SEC-82). A preview **origin** is untrusted web content (the
    app inside a sandbox is untrusted, boundary 1): it is a different origin from the dashboard,
    never receives the dashboard session, and cannot make credentialed requests to the dashboard
    or to other previews (SEC-81). The path *browser → relay → runner → supervisor → loopback port*
    is the only inbound path into a sandbox; there is no listening socket in the sandbox other than
    the app's own, and the sandbox network policy is unchanged (it still reaches only the gateway
    and runner channel ports, SEC-02).

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

(phase 5 (D-138, moved from phase 4), RFE-011b, opt-in) **Long deadlines park instead of hold.** In step 1, for an approval
whose deadline is above `approvals.park.threshold_secs`, the gateway does not hold: it records the
approval as *parked* with a salted fingerprint of the request and answers the agent at once
`403 approval_parked` ("not sent; end your turn, you will be messaged"). Steps 2–3 are unchanged
(cards, dashboard, e-mail, Web Push). In step 4 the decision becomes a **single-use grant** and the
API queues a system follow-up in the same conversation; the agent's next turn repeats the identical
request and the gateway consumes the grant atomically. No container or connection is kept while
people decide (`07 §Park and resume`).

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

- **Development (kind + Helm)** (phase 1, OPS-017; phase 0 used podman compose): the production chart on a
  local kind cluster with ingress-nginx as the edge; values profiles `llm-mock` and
  `connector-fake` add a mock model and the fake connector. See `15`.
- **Production (Kubernetes, Helm)**: same images; sandboxes are one Pod + PVC
  each in a fenced namespace with NetworkPolicy and ResourceQuota; API, gateway,
  hookhost, runner and connectors scale horizontally. See `15`.

## Design principles (apply everywhere)

- **Registries keyed by ids with load-time completeness checks** (LLM providers,
  sandbox backends, harness adapters, secret stores, platform tools,
  mechanisms; from phase 1 also the delta bus, `05 §The delta bus seam`). Adding one is a module plus one registration line. An id with no
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
