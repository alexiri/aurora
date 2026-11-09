# 05 — Control-Plane API Server

Phase: **0**. Binary: `aurora-api`. Port 4101.

The API server is the single writer of platform state (except the narrow
gateway writes in `03 §Gateway database role grants`) and the single place
where "what happens next" is decided. Replicas are stateless and
interchangeable; all coordination is through Postgres.

## Responsibilities

1. Management API (`/v1/…`) for the dashboard, CLI and API keys; OpenAPI
   document at `/v1/openapi.json`.
2. Identity: sign-up/in, sessions, OIDC, invitations, API keys, CLI login (`04`); from phase 1 sign-in is OIDC only (RFE-004).
3. Secrets and LLM keys through the secret-store registry (`08`).
4. Plugin registry: package ingestion, verification, validation, operator
   approval, snapshot compilation (`09`); app connect flows; OAuth clients.
5. Policy authoring, grants compilation, simulation via the shared
   `aurora-policy` crate (`07`).
6. Approvals feed and decisions; action approvals (`07`).
7. Conversation plane: conversations, turns, follow-ups, transcript, SSE,
   attachments (`10`).
8. Hosted-agent state: memory, skills, schedules, webhooks, processes, watches,
   agent links, peer tasks (`10`).
9. The **due-work engine** serving runners (this document, `11`).
10. Platform-tool execution through the **tool registry** (`12`).
11. Channel core: inbox, routing, speaker authorization, outbox operations,
    action tokens, messaging tools (`13`).
12. Audit log (best-effort and, from phase 2, atomic security-critical rows, `04 §Audit`), request-log reads, runtime usage.

## Internal structure

Modules (one per domain) each expose: repository functions (`aurora-db`),
a service layer (business rules, authorization), and axum routers. Cross-domain
calls go through service traits, never through another domain's SQL. The
composition root (`main.rs`) builds the registries (secret stores, LLM
providers, platform tools, inbox handlers, operation kinds) and fails startup
if any declared id lacks an implementation.

## Authentication surfaces

| Caller | Mechanism | Routes |
|--------|-----------|--------|
| Browser | Session cookie + Origin check | `/v1/**` (except planes below) |
| CLI / scripts | `Authorization: Bearer auk_…|auo_…` | `/v1/**` except session-only |
| BYO agent | `Authorization: Bearer aug_…` (agent token; phase 3, RFE-013: 401 `agent_token_expired` once past a configured maximum age) | only `POST /v1/agent/context-credentials` and `GET /v1/agent/container-config` |
| Runner | `Authorization: Bearer aur_…` + `X-Aurora-Replica: <id>` (after register) | `/v1/runner/**` |
| Connector | `Authorization: Bearer aun_…` + `X-Aurora-Replica` | `/v1/connector/**` |
| Public capability | token in path | `POST /v1/hooks/{token}`, `GET /v1/public/avatars/{key}`, `GET /v1/oauth/callback`, OIDC callbacks |
| Internal services (hook host) | `Authorization: Bearer <internal secret>` (phase 2, RFE-007: any key of the ring + `X-Aurora-Key-Id`) | `/internal/v1/**` |
| Metrics / health | none | `/healthz`, `/readyz`, `/metrics` |

`/metrics` and `/internal/**` MUST NOT be routed by the edge proxy (the edge
routes only `/v1/**`); in Kubernetes a NetworkPolicy additionally restricts
`/internal` callers to hook-host pods by serving it on a second listener
(port 4111) that the edge never reaches.

Internal routes (API port 4111):

| Method & path | Purpose |
|---|---|
| GET `/internal/v1/registry/active?contract_max=` | active snapshot JSON + content hash |
| GET `/internal/v1/registry/blobs/{digest}` | hook module bytes |

### Conversation visibility (phase 1, RFE-009)

Every handler that returns, streams or names a conversation applies
`can_see_conversation` (`04 §The access law`) in addition to the permission
matrix:

- A conversation the caller cannot see answers **404** (not 403, as above):
  `GET /v1/conversations/{id}`, `…/events`, `…/stream` (SSE), `…/messages`,
  `…/attachments`, `GET /v1/attachments/{id}/content`, turn `abort`/`send-now`
  (owners may), and any `conversation=` filter.
- `GET /v1/agents/{agent}/conversations` lists only visible conversations (a
  member sees their own direct one; owners see every direct one, marked
  `direct_user: {id, display_name}`).
- `POST …/messages` and attachment upload on a `direct` conversation by anyone but
  its user → 403 `direct_conversation_forbidden` (even for owners).
- **Conversation references elsewhere** (`conversation_id` / `origin_conversation_id`
  on approvals, action approvals, schedules, webhooks, peer tasks, reach items)
  are returned as `null` with `conversation_hidden: true` when the caller cannot
  see the conversation; approvals and action approvals naming such a conversation
  are **omitted** from lists, long-polls, the `approvals/stream` SSE and batch
  decisions for that caller, and a direct decision attempt answers 404.
- `GET /v1/workspaces/{ws}/activity` omits request-log rows whose conversation is
  an invisible direct conversation; `…/usage` (aggregates only) is unaffected.
- Any present or future search over conversations or transcript events applies
  the same predicate inside the query (never as a post-filter).

### Request scoping

- Paths carry ids (`/v1/workspaces/{workspace_id}/…`, `/v1/agents/{agent_id}`).
  `GET /v1/resolve?org=<slug>&workspace=<slug>` maps dashboard slugs to ids.
- Every handler resolves the target's workspace/org from the database and runs
  the permission check of `04 §Permission matrix`; a workspace key used on
  another workspace's resource → 403 `workspace_mismatch`.
- (phase 2, RFE-006) Agent, workspace and org resources carry `frozen` (null or
  `{ at, by_label, reason, authority, level: "agent"|"workspace"|"org" }`, `level` =
  where it is set; `effective_frozen: bool` on agents). Operations that start
  agent work on an effectively frozen agent (messages, attachments, send-now,
  schedule run-now, webhook receive, peer messaging) answer **423 `frozen`**.
- Resources the caller cannot see answer 404 (not 403) to avoid existence
  oracles, except where the caller can see the parent.

### Conventions

- JSON in and out; errors per `02 §Error envelope`.
- List endpoints: `?limit=` (default 50, max 200) and opaque `?cursor=`;
  response `{ "items": [...], "next_cursor": "…" | null }`, ordered by
  `created_at desc, id desc` unless documented.
- Optimistic concurrency: resources with a `revision` accept
  `If-Match: <revision>` or a `revision` body field; stale → 409 `stale_revision`.
- Long-poll endpoints accept `?wait=<secs>` capped at `LONG_POLL_MAX`.
- **Rate limiting (phase 2, RFE-017, D-57).** Phase 0 limited only the auth
  endpoints and webhook deliveries; any member or a leaked workspace key could
  degrade the instance with unlimited posts, uploads, simulations or searches. From
  phase 2 every **authenticated** `/v1/**` request passes the API limiter, after
  the principal is resolved and before authorization (so probing is limited too).
  - **Principal**: an API key (`key:<api_key_id>`), a signed-in user
    (`user:<user_id>` — all of a user's sessions share the budget, so opening more
    sessions buys nothing) or a BYO agent token (`agent:<agent_id>`, its two routes
    only). Unauthenticated routes keep their per-IP limits (`04 §Rate limits`; plus
    `API_RATE_UNAUTH` 120/min/IP for public GETs and 30/min/IP for requests with a
    rejected bearer). **Exempt**: `/healthz`, `/readyz`, `/metrics`, the machine
    planes (`/v1/runner/*`, `/v1/connector/*`, `/internal/*`) which apply their own
    back-pressure, and `POST /v1/hooks/{token}` (its own 60/min per webhook).
  - **Token buckets**, in memory per API replica (the same approximation as the
    gateway, D-05; N replicas can allow up to N×; a shared `CounterStore` backend
    is the later seam, `06 §Rate limiting`). A request must obtain a token from
    **every** bucket that applies: the principal's *general* bucket, the principal's
    *class* bucket, and the *org* bucket of the resource's organization:

    | Bucket | Applies to | Refill / burst (defaults) |
    |--------|------------|---------------------------|
    | `general` | every authenticated request of the principal | 600 / min, burst 120 |
    | `write` | any POST/PUT/PATCH/DELETE not in a stricter class | 120 / min, burst 30 |
    | `message` | `POST /v1/conversations/{id}/messages`, `/v1/turns/{id}/send-now` | 30 / min, burst 10 |
    | `upload` | `POST /v1/conversations/{id}/attachments`, avatar/skill-file/plugin uploads | 20 / min, burst 5 |
    | `simulate` | `POST …/policy/simulate`, `GET /v1/agents/{agent}/effective` | 20 / min, burst 5 |
    | `search` | `GET …/memories/search` and any future search route | 30 / min, burst 10 |
    | `export` | `GET /v1/conversations/{id}/export`, `GET /v1/admin/storage`, `GET /v1/orgs/{org}/storage` | 6 / hour, burst 2 |
    | `org` | every authenticated request touching an org's resources (all principals together) | 3 000 / min, burst 600 |

  - **Concurrency cap** for streams: at most `API_STREAMS_PER_PRINCIPAL` (20)
    concurrent SSE connections and long-polls per principal; the next one gets 429
    `too_many_streams`.
  - **Refusal**: 429 with the standard envelope, code `rate_limited`, `details:
    { scope: "general"|"class"|"org", class, limit, per, retry_after_secs }`, header
    **`retry-after`** (whole seconds until the blocking bucket holds a token) and the
    informational `ratelimit-limit` / `ratelimit-remaining` / `ratelimit-reset` of
    the blocking bucket; `x-should-retry: true`. A refused request consumes no
    tokens from the buckets that would have allowed it. Idempotent safe retries are
    the client's responsibility (phase 3, RFE-018: message creation accepts an
    `Idempotency-Key`, below).
  - **Tuning**: `ratelimit.api.enabled` (default `true`), `ratelimit.api.multiplier`
    (0.1–100, scales every bucket), per-class overrides
    `ratelimit.api.classes.<name> = { per_minute | per_hour, burst }` and
    `ratelimit.api.org_per_minute`; minimum 1 per period. Metrics:
    `aurora_api_rate_limited_total{class,scope}`. Buckets idle for 10 minutes are
    evicted (LRU 100 000 entries); eviction only ever *refills* (fail-open for the
    individual, never fail-open for the instance because the entry cap bounds memory).
- (phase 2, RFE-010) The conversation SSE adds the ephemeral `event.redacted
  { seq }` (no `id:`), telling clients to re-read that event.
- SSE: `Content-Type: text/event-stream`; durable events carry `id: <seq>`;
  ephemeral events carry no id; keepalive comment every `SSE_KEEPALIVE`;
  resume with `Last-Event-ID` or `?after_seq=`.

### Idempotency keys (phase 3, RFE-018, D-62)

Phase 0–2: a client that retried `POST /v1/conversations/{id}/messages` after a
timeout created two turns (the second becoming a follow-up). From phase 3 the
route accepts an optional **`Idempotency-Key`** request header; it reuses the
mechanism channel doors already use (`turns.source_event_id` and its partial
unique index on `(conversation_id, source_event_id)`), so no new table or
index exists.

- **Key format**: 1–128 characters from `[A-Za-z0-9._:~-]` (a UUID is the
  recommendation); anything else → 422 `idempotency_key_invalid`. No header = the
  phase 0 behaviour (no deduplication). A `Idempotency-Key` on other routes is
  ignored (phase 3 covers message creation only; `send-now`, `abort` and the
  channel/webhook doors are already idempotent in their own way).
- **Scope**: the key is scoped to **(conversation, acting user)**. It is stored as
  `source_event_id = "api:" ‖ <user_id> ‖ ":" ‖ <key>` (`api:` is a reserved prefix:
  `13 §5.1` rejects connector event ids starting with it), where `<user_id>` is the
  person the request acts as (the session user, or the owner of an API key — the
  same person has one key space across the dashboard, CLI and keys). One user can
  therefore neither replay, collide with nor probe another user's keys.
- **Order of evaluation**: authentication, rate limiting (a replay costs one
  message-bucket token like any request), authorization and conversation visibility
  (`can_see_conversation`, direct-conversation writer rule) **first** — a replay
  never bypasses any check; then the idempotency lookup; then the freeze check
  (423 `frozen`) and the remaining validations for a *new* message only.
- **First use**: the turn (or the `followup` row, which carries the key too) is
  inserted with the key in the same transaction as today. **Replay** (key already
  stored for this conversation and user): nothing is created; the response has the
  **same status code and shape as the original** — `{ turn }` or `{ followup }`, the
  resource as it is **now** (its status may have advanced; a promoted follow-up
  shows `promoted` and its new turn's id in `Followup.promoted_turn_id`, phase 3, D-107) — plus the header `Idempotent-Replayed:
  true`. A replay after the turn finished, failed or was aborted returns that state;
  it never re-runs it.
- **Mismatch**: a replay whose `text` or set of `attachment_ids` differs from the
  stored turn's → 422 `idempotency_key_reused` (`details.turn_id` or, for a stored
  follow-up, `details.followup_id`; phase 3, D-107), nothing created.
- **Concurrency**: simultaneous requests with one key race on the unique index; the
  loser's transaction fails with the unique violation, re-reads the winner's row
  and answers as a replay (exactly one turn exists).
- **Lifetime**: as long as the turn row exists (no separate expiry): conversation
  deletion or retention (`10 §Archive, delete, redaction, retention, export`)
  frees the key. Clients use a fresh key per logical message.
- Never logged in clear beyond a 12-character prefix; not returned in transcripts
  or events (the `source_event_id` column is internal).

## Event bus (Postgres `LISTEN/NOTIFY`)

Every process that needs signals holds one dedicated LISTEN connection.
Notifications are **hints**: a consumer that (re)connects its listener MUST
resynchronize from the database (SSE re-reads from last seq, gateways flush
caches — phase 3, RFE-014: *after* re-subscribing, keeping the caches meanwhile,
`06 §Operation during a Postgres outage` — waiters return early). Payloads are JSON ≤ 7 000 bytes.

| Channel | Payload | Producer → consumers |
|---------|---------|----------------------|
| `aurora_conversation` | `{ "c": conversation_id, "s": seq }` | API (after commit) → API SSE handlers |
| `aurora_delta` | `{ "c", "t": turn_id, "type": "text.delta"|"thinking.delta"|"activity", "d": string }` | API (on runner events) → API SSE handlers; (phase 1, RFE-016) this is the `notify` implementation of the `DeltaBus` seam below |
| `aurora_work` | `{ "sandbox_id"?: uuid }` | API → API runner long-polls |
| `aurora_ops` | `{ "connector": id }` | API → API connector long-polls |
| `aurora_inbox` | `{}` | API → API inbox workers |
| `aurora_op_result` | `{ "op_id" }` | API (on connector results) → API callers waiting on correlated operations |
| `aurora_approval` | `{ "id", "workspace_id", "event": "created"|"decided"|"expired" }` | gateway & API → gateways, API (cards, SSE, long-polls) |
| `aurora_gateway_invalidate` | `{ "scope": "org"|"workspace"|"agent"|"credential"|"connection"|"all", "id"?: uuid }` | API → gateways |
| `aurora_registry` | `{ "hash": content_hash, "contract": int }` | API → gateways, API replicas (hook hosts have no DB connection and poll `/internal/v1/registry/active` every 60 s and on cache miss) |
| `aurora_secret_origin` | `{ "secret_id", "status" }` | gateway (origin preflight/backstop) → API (audit, owner notice, dashboard) |
| `aurora_auth_revoked` | `{ "kind": "session"|"api_key"|"user", "id" }` | API → API replicas, gateways |

Deltas are coalesced by the API before notifying (≥100 ms or ≥2 000 chars per
turn per notification) so a busy stream does not flood the channel.

### The delta bus seam (phase 1, RFE-016, D-48)

Control signals (`aurora_conversation`, `aurora_work`, `aurora_approval`, …) stay
on NOTIFY (D-03). The **streaming text** (`aurora_delta`) is different: it is
high-volume, ephemeral, and — because Postgres serializes NOTIFY delivery in the
commit path and every API replica receives every notification — the first shared
bottleneck. From phase 1 deltas therefore flow through a named seam, not through
NOTIFY calls scattered in handlers:

```rust
/// aurora-db::bus — registered by id; phase 1 implementation: `notify`.
#[async_trait]
pub trait DeltaBus: Send + Sync + 'static {
    /// Publish one coalesced delta. Best effort: an error is logged and counted,
    /// never fails the turn or the durable event write.
    async fn publish(&self, delta: Delta) -> Result<()>;
    /// One stream of every delta from every replica (including this one). A new
    /// subscription (first or after a loss) makes the consumer resynchronize from
    /// the database; the bus replays nothing.
    async fn subscribe(&self) -> Result<BoxStream<'static, Delta>>;
}
pub struct Delta { pub conversation_id: Uuid, pub turn_id: Uuid,
                   pub kind: DeltaKind /* text | thinking | activity */, pub data: String }
```

Contract: **at-most-once, lossy allowed** (a lost delta is repaired when the
durable `text` event arrives; consumers discard a turn's deltas on it);
per-`(conversation, turn)` order preserved; payload ≤ 7 000 bytes (JSON-encoded
`Delta`); coalescing (≥100 ms or ≥2 000 chars, `DELTA_COALESCE`) happens in the
publisher *above* the trait so every implementation sees the same load; no
durability, no ack. Selection: `events.delta_bus` (default `notify`); an unknown id
fails startup naming the key (registry rule, `01 §Design principles`). The phase 1
`notify` implementation is exactly the `aurora_delta` channel described above;
later implementations (Redis pub/sub, direct replica gossip) are registry
entries, not a refactor. A conformance suite (`18`) runs against every
implementation. Metrics: `aurora_delta_published_total`, `…_dropped_total`,
`aurora_delta_publish_seconds`, `aurora_delta_subscriber_lag_seconds`, and for the
`notify` implementation `aurora_pg_notify_queue_usage` (`pg_notification_queue_usage()`).

## Background jobs (every API replica)

Singleton jobs take `pg_try_advisory_lock(<job hash>)` per run; others are
claim-based and safe to run everywhere.

| Job | Trigger | Kind |
|-----|---------|------|
| Inbox processor (`13`) | `aurora_inbox` + every 5 s | claims |
| Approval card fan-out (`13`) | `aurora_approval created` + every 10 s catch-up | claims (`channel_ops.dedupe_key`) |
| Approval e-mail notification (phase 2, RFE-011a, `07 §Approvals`): pending approvals older than `APPROVAL_NOTIFY_DELAY` without a live surface → e-mail to deciders | every 10 s | claims (`approval_notifications`) |
| Approval watcher heartbeat (phase 2, RFE-011a): each replica upserts `approval_watchers` for its open approvals SSE/long-poll connections; stale rows pruned | every 15 s | idempotent |
| Approval expiry: `pending` past `deadline + 30 s` → `expired`; pending rows of gateway instances unseen for 60 s → `abandoned` | every 10 s | idempotent updates |
| Action-approval expiry | every 60 s | idempotent |
| Attachment pending sweep (`ATTACH_PENDING_TTL`) | hourly | idempotent |
| Conversation retention sweep (phase 2, RFE-010): delete conversations past the org window through the delete path; blob-orphan sweep | daily | singleton |
| Attachment sweep (phase 2, RFE-015): purge attachment bytes past the org window, remove orphan blobs, reconcile `org_storage_usage`, quota notices | daily | singleton |
| Request-log partition maintenance and retention | daily | singleton |
| Retention pruning (inbox/ops/cards > `CHANNEL_LEDGER_RETENTION`, used action tokens, expired email/CLI tokens, sessions, expired context credentials) | daily | singleton |
| Runtime daily rollup | hourly | singleton |
| Plugin directory scan | startup + 60 s | singleton |
| Snapshot compile & activate | on package state change | singleton |
| Peer-task and reach expiry | every 60 s | idempotent |

## The due-work engine

There is **no scheduler loop**. A runner's `POST /v1/runner/work` long-poll is
both its heartbeat and the moment the API computes and *claims* work for the
sandboxes that runner can serve. If nothing is due the request waits on
`aurora_work` (or `RUNNER_POLL_WAIT`) and recomputes once before returning.

Requirements (MUST): (a) claim-with-lease semantics so concurrent pollers never
receive the same item; (b) one lifecycle step at a time per sandbox on its
owning replica; (c) timestamps are the truth of liveness; (d) every arm is a
bounded query (LIMIT) so a poll is cheap.

Arms, evaluated in this order per poll (detail of each in `10`/`11`):

1. **Replica bookkeeping**: upsert `last_seen_at`; reassign ownership of
   sandboxes whose owner replica is not live.
2. **Stop**: running sandboxes with `stop_requested_at`/`restart_requested_at`,
   idle-park candidates (no keep-awake reason within `IDLE_STOP`), background
   cap expiry, eviction over the held-awake ceiling → `sandbox.stop`.
3. **Start**: sandboxes that must exist (an active turn, a follow-up, a due
   schedule/webhook/watch turn) and are `stopped`/`unprovisioned`/`failed`
   (failed only after backoff), placed on this runner → `sandbox.start` (mints
   `boot_id`, boot token, bootstrap token).
4. **Abort / send-now** for turns on sandboxes whose channel this replica holds.
5. **Home sync**: running sandboxes with desired generation > applied →
   `home.sync` parts.
6. **Dispatch**: queued turns whose sandbox is running with a live channel on
   this replica and whose home is synced → `turn.deliver` (mints the context
   credential).
7. **Steer**: `joining` follow-ups of running turns → `turn.message`.
8. **Stale-turn recovery**: never-started dispatches (`STALE_DISPATCH`), stalls
   (`TURN_STALL`; phase 2, RFE-011a: not for turns with a `pending` approval,
   `07 §Approval deadlines`), ceiling warnings and ceiling failures (`TURN_CEILING`).
9. **Schedules**: claim due schedules (`fire_lease_until`), create turns (`10`).
10. **Watches**: convert watches of lost/parked processes; expire watches.
11. **Housekeeping slices**: close runtime intervals for stopped boots; mark
    processes of ended boots `lost`/`parked`.

(phase 2, RFE-006) Arms 3 (start), 6 (dispatch), 7 (steer), 9 (schedules) and 10
(watches) **skip effectively frozen agents** (agent, workspace or org flag,
`04 §Freeze`); arm 2 (stop) parks a running frozen sandbox with
`stop_reason = frozen` and ignores its keep-awake reasons.

Work items and reports are defined in `16 §W5`.

## Turn and transcript persistence rules

- Durable events: `UPDATE conversations SET last_seq = last_seq + 1 … RETURNING
  last_seq` and the `transcript_events` insert happen in one transaction, so
  commit order equals seq order and "history then tail from last seq" is
  lossless. `NOTIFY aurora_conversation` after commit.
- Ephemeral events (`text.delta`, `thinking.delta`, `activity`) are never
  stored; they go out on `aurora_delta` only.
- One active turn per conversation is enforced by the partial unique index.
  Creating a turn when one is active creates a `followup` row instead
  (`10 §Turns`); the unique violation is the race arbiter.

## Gateway cache invalidation

Any write that can change a gateway resolution MUST `NOTIFY
aurora_gateway_invalidate` after commit with the narrowest scope:

| Write | Scope |
|-------|-------|
| Secret create/update/delete/value change | `workspace` (or `org` for org secrets) |
| Connection create/update/delete/status change; OAuth client change | `connection` + `workspace`/`org` |
| Grant change, derived-rule rematerialization | `agent` |
| Policy publish/rollback | `workspace` or `org` |
| Agent delete, token regeneration, credential revocation | `agent` / `credential` |
| Workspace access or membership change | `workspace` / `org` |
| Freeze / unfreeze (phase 2, RFE-006) | `agent` / `workspace` / `org` |
| App availability change | `org` |
| Org governance change (and the stored-origin re-evaluation it triggers) | `org` |
| (phase 2, RFE-025) Workspace BYO posture change | `workspace` |
| (phase 2, RFE-025) BYO source list / require-context change of an agent | `agent` |
| Snapshot activation | (`aurora_registry` instead) |

## Route catalogue

Auth column: **S** session, **K** API key (workspace or org), **SO** session
only, **IA** instance admin (session), **A** agent token, **R** runner, **C**
connector, **P** public capability. Permission checks per `04`.

### Health and instance
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/instance` | none | `{ version, signup_mode, oidc_providers:[{id,display_name}], features:{hosted, channels}, default_agent_name, external_url }`; (phase 1, RFE-004) `oidc_providers` is the complete list of ways to sign in (may include the development provider `dev`, `04`) |
| GET `/v1/resolve` | S/K | slug → ids |

### Auth and account
| Method & path | Auth | Purpose |
|---|---|---|
| POST `/v1/auth/signup` | none | email, password, display_name — **phase 0 only; removed in phase 1 (RFE-004)** |
| POST `/v1/auth/login` | none | **phase 0 only; removed in phase 1 (RFE-004)** |
| POST `/v1/auth/logout` | S | (all phases) |
| POST `/v1/auth/verify-email`, `/v1/auth/verify-email/resend` | none / S | **phase 0 only; removed in phase 1** |
| POST `/v1/auth/password/forgot`, `/v1/auth/password/reset` | none | **phase 0 only; removed in phase 1** |
| POST `/v1/auth/password/change` | SO | **phase 0 only; removed in phase 1** |
| POST `/v1/auth/sessions/revoke-others` | SO | |
| GET `/v1/auth/oidc/{provider}/start`, `/callback` | none | `04 §OIDC`; (phase 1, RFE-004) the only sign-in; `start` accepts `reauth=1` |
| POST `/v1/auth/cli`, `/v1/auth/cli/token` | none | device flow |
| POST `/v1/auth/cli/approve` | SO | `{ user_code, workspace_id }` |
| GET/PATCH/DELETE `/v1/me` | S (DELETE: SO) | (phase 1, RFE-004: DELETE needs a recent OIDC re-authentication, 412 `reauth_required`, no body) profile, memberships (phase 2, RFE-012: with the effective role per workspace), default org; (phase 2, RFE-011a) `PATCH { approval_email: bool }`; (phase 2, C-195) `client_address: string \| null`, the caller's address as the API sees it behind `server.trusted_proxies` |

### Organizations, members, keys
| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/orgs` | S | list mine / create |
| GET/PATCH/DELETE `/v1/orgs/{org}` | S/K | |
| GET `/v1/orgs/{org}/members`; PATCH/DELETE `/v1/orgs/{org}/members/{user}` | S/K | role, suspend; (phase 2, RFE-021) PATCH (suspend) and DELETE accept `reassign_to` (user id or `"workspace"`) to reassign the person's automations atomically |
| GET `/v1/orgs/{org}/members/{user}/offboarding`; GET `/v1/workspaces/{ws}/members/{user}/offboarding` | S/K | (phase 2, RFE-021) what removing the person would disable: `{ schedules: [Item], webhooks: [Item], peer_tasks: [Item], api_keys: n }`, `Item = { id, name, agent, workspace, enabled, anchor, next_fire_at?, would_disable }` |
| GET/POST `/v1/orgs/{org}/invitations`; DELETE `/v1/invitations/{id}` | S/K | (phase 2, RFE-012) body may carry `approver_workspace_ids` besides `workspace_ids` |
| GET `/v1/invitations/preview?token=`; POST `/v1/invitations/accept` | none / SO | |
| GET/POST `/v1/orgs/{org}/api-keys`; DELETE `/v1/api-keys/{id}` | S (POST: SO) | org keys |
| GET `/v1/orgs/{org}/audit` | S/K | (phase 2, RFE-008) filters `actor_id`, `action`, `critical=true` (C-290); rows carry `critical` (C-291) |
| POST `/v1/orgs/{org}/freeze` `{ reason }`; POST `/v1/orgs/{org}/unfreeze` | S/K | (phase 2, RFE-006) org `owner` |
| GET/PUT `/v1/orgs/{org}/app-availability` | S/K | posture + allow-list |
| GET/PUT `/v1/orgs/{org}/governance` | S/K | org governance document (`08 §11.2`) with `revision`; PUT validates against the instance ceiling (422 `governance_invalid` / `governance_below_instance_minimum`) |
| GET `/v1/orgs/{org}/governance/effective` | S/K (any org member) | effective LLM and egress governance (instance ∩ org), each value marked `source: instance|org` (phase 1, RFE-003: includes `egress.open_lane`; phase 2, RFE-025: includes `byo.{posture, source_cidrs, require_context_credentials}`; phase 3, RFE-013: `agent_tokens.max_age_days`); non-compliant LLM secrets of the org |
| GET `/v1/governance/instance` | S | the instance ceiling (config, without secrets) — read-only |

### Workspaces
| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/orgs/{org}/workspaces` | S/K | |
| GET/PATCH/DELETE `/v1/workspaces/{ws}` | S/K | name, timezone, memory_language; (phase 2, RFE-025) PATCH also `byo_posture` (`enabled`\|`owners_only`\|`disabled`\|null = inherit; `can_own`; tighten-only, 422 `governance_below_instance_minimum`; audited `workspace.byo_posture.update`; invalidation scope `workspace`); GET shows `byo_posture` and `effective_byo_posture` with its `level` |
| GET `/v1/workspaces/{ws}/access`; PUT/DELETE `/v1/workspaces/{ws}/access/{user}` | S/K | bindings (phase 2, RFE-021: DELETE accepts `reassign_to`); (phase 2, RFE-012) `PUT { role: "owner"\|"approver"\|"member" }`; `can_own` |
| GET/POST `/v1/workspaces/{ws}/api-keys` | S (POST: SO) | workspace keys |
| GET `/v1/workspaces/{ws}/audit` | S/K | same filters |
| GET `/v1/orgs/{org}/storage` | S/K | (phase 2, RFE-015) org `owner`/`admin`: usage vs quota, breakdown by workspace and direction |
| GET/PUT `/v1/orgs/{org}/retention` | S/K | (phase 2, RFE-010/015) org `owner`/`admin`; `{ conversations_days?, attachments_days?, attachment_quota_mib? }`, `null` = none; 422 `retention_invalid` / `retention_above_instance_maximum` |
| GET `/v1/workspaces/{ws}/activity` | S/K | request log, filters: agent, conversation, decision, host, app |
| GET `/v1/workspaces/{ws}/usage` | S/K | runtime + tokens |
| POST `/v1/workspaces/{ws}/freeze` `{ reason }`; POST `/v1/workspaces/{ws}/unfreeze` | S/K | (phase 2, RFE-006) `can_own`; as for agents |

### Secrets and LLM
| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/workspaces/{ws}/secrets`, `/v1/orgs/{org}/secrets` | S/K | create: name, kind, llm fields or host/path/injection, `value` or `reference` |
| GET/PATCH/DELETE `/v1/secrets/{id}` | S/K | metadata only; values never returned |
| PUT `/v1/secrets/{id}/value` | S/K | replace value |
| POST `/v1/secrets/{id}/accept` | S | agent proposal: `{ value }` → secret `active` + grant to the proposing agent |
| POST `/v1/secrets/{id}/reject` | S/K | agent proposal → `rejected`; a notice goes to the proposing conversation |
| GET `/v1/secret-stores` | S/K | configured stores: id, kind, capabilities (no config) |
| GET `/v1/llm/providers` | S/K | registry + configured endpoints |
| GET `/v1/secrets/{id}/models` | S/K | model catalogue for an LLM secret (only allowed models) |
| POST `/v1/secrets/{id}/verify` | S/K | re-run key-origin verification (`08 §11.3`) and return `origin` |

### Apps, connections, OAuth clients
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/apps?workspace_id=` | S/K | catalogue with availability and connection counts |
| GET `/v1/apps/{app}` | S/K | about, connect methods (form schemas), scopes, tools, agent docs |
| GET `/v1/apps/{app}/icon?variant=light|dark`, `/v1/apps/{app}/guide` | S/K | sanitized PNG; setup guide markdown |
| POST `/v1/apps/{app}/connect` | S | `{ scope, workspace_id?, method_id, fields?, oauth_client_id?, access_level?, reconnect_connection_id? }` → `{ "kind": "redirect", "url" }` \| `{ "kind": "connected", "connection" }` |
| GET `/v1/oauth/callback` | P | state-bound; renders a tiny HTML page that posts to the opener and closes |
| GET `/v1/workspaces/{ws}/connections`, `/v1/orgs/{org}/connections` | S/K | |
| GET/PATCH/DELETE `/v1/connections/{id}` | S/K | label, status (`disabled`↔`active`) |
| GET/POST `/v1/workspaces/{ws}/oauth-clients`, `/v1/orgs/{org}/oauth-clients`; PATCH/DELETE `/v1/oauth-clients/{id}` | S/K | BYOC; (phase 2, D-74) optional `instance_id` binds the client to a self-hosted instance |

### App instances (self-hosted apps; phase 2, D-74)
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/orgs/{org}/app-instances` | S/K | instances usable in the org (instance-wide + the org's own); any member |
| POST `/v1/orgs/{org}/app-instances` | S/K | `{ app_id, host, label }` → `AppInstance`; org owner/admin; validated per `09 §3.4 Instance-bound hosts` (probe included) |
| GET/POST `/v1/admin/app-instances` | S | instance-wide instances; instance admins |
| PATCH/DELETE `/v1/app-instances/{id}` | S/K | label, `status`; delete disables its connections; owner/admin of its scope (instance admin for instance-wide) |

`POST /v1/apps/{app}/connect` takes `instance_id` for an instance-bound app (absent = `default_host`, refused when the app has none). The connect form and the dashboard list the instances visible to the org.

### Grants and effective views
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/agents/{agent}/grants` | S/K | |
| PUT/DELETE `/v1/agents/{agent}/grants/connections/{conn}` | S/K | `{ mode, allow_tools, ask_tools }`; (phase 2, RFE-011a) optional `ask_timeout_secs` |
| PUT/DELETE `/v1/agents/{agent}/grants/secrets/{secret}` | S/K | |
| GET `/v1/connections/{id}/grants` | S/K | agents granted |
| GET `/v1/agents/{agent}/effective` | S/K | effective credentials and per-tool decisions with `org_ceiling`; (phase 2, RFE-005) optional `?speaker=<user_id>\|none` |

### Policy
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/workspaces/{ws}/policy`, `/v1/orgs/{org}/policy` | S/K | published + draft + revision |
| PUT `…/policy/draft` | S/K | whole-draft replace with `revision`; (phase 1, RFE-003) workspace scope also carries `open_lane_posture` (`enabled` \| `allowlist_only` \| `disabled`; a workspace can only tighten, so `enabled` simply adds no restriction beyond the org and instance) |
| POST `…/policy/publish`, `…/policy/rollback` | S/K | `{ revision }` / `{ generation }` |
| GET `…/policy/generations` | S/K | history |
| GET `…/policy/warnings` | S/K | overlap/inert-rule warnings for the draft; (phase 2, RFE-025) includes `rule_applies_to_all_agents` (`07 §Injection selection`) |
| POST `/v1/workspaces/{ws}/policy/simulate` | S/K | `{ agent_id, method, url, body?, use_draft?, lane?, speaker_user_id? }` → decision trace (phase 2, RFE-005: `speaker_user_id` omitted = no speaker); (phase 1, RFE-003) `lane: "open"` evaluates CONNECT-time semantics (`07`) and the open-lane posture |

### Approvals
(phase 2, RFE-012) Every route in this section requires **`can_approve`** on the
approval's workspace (`04`), otherwise 403 `approver_required` (404 where the
workspace/approval is not visible); in phase 0/1 the requirement was `can_use`.

| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/workspaces/{ws}/approvals?status=` | S/K | |
| GET `/v1/approvals/pending?wait=` | S/K | long-poll across what the caller may decide |
| GET `/v1/approvals/stream` | S | SSE of created/decided; (phase 2, RFE-011a) `created` carries `{ id, workspace, agent, title, kind: "gateway"\|"action", expires_at }` so the dashboard can raise a browser notification; counts as a live surface (`approval_watchers`) |
| POST `/v1/approvals/{id}/decision` | S/K | `{ "decision": "approve"|"deny" }`; first decision wins (409 `already_decided`) |
| POST `/v1/approvals/batches/{batch_id}/decision` | S/K | all pending of the batch the caller may decide |
| GET `/v1/workspaces/{ws}/action-approvals`; POST `/v1/action-approvals/{id}/decision` | S/K | |

### Agents and sandboxes
| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/workspaces/{ws}/agents` | S/K | create returns `aug_` once for BYO; (phase 2, RFE-025) creating `kind = byo` is subject to the effective BYO posture: 403 `byo_not_permitted` (`owners_only`, caller lacks `can_own`) or 403 `byo_disabled`; the list/get shape carries `byo: { posture, posture_level, source_cidrs, require_context, effective_source_cidrs, effective_require_context }` for BYO agents |
| GET/PATCH/DELETE `/v1/agents/{agent}` | S/K | name, brief, harness, model, effort, resource overrides (IA) |
| POST `/v1/agents/{agent}/token` | S | BYO: regenerate, returns token once with `expires_at` (phase 3, RFE-013: null unless an agent-token maximum age applies, `08 §7`); (phase 2, RFE-025) same posture check as creation (`byo_not_permitted` / `byo_disabled`); audited `agent.token_regenerate` |
| PUT `/v1/agents/{agent}/byo` | S/K | (phase 2, RFE-025) BYO only (hosted → 422 `not_byo`): `{ source_cidrs: [cidr..] \| null, require_context: bool }`; `can_own`; 422 `byo_cidr_invalid` (bad CIDR, `/0`, empty array, >32 entries) / `byo_cidr_outside_default`; live credentials are re-evaluated by invalidation scope `agent`; audited `agent.byo_settings.update` |
| GET `/v1/agents/{agent}/container-config` | S/K | BYO bootstrap (`08 §BYO bootstrap`) |
| GET `/v1/workspaces/{ws}/agent-tokens`; GET `/v1/orgs/{org}/agent-tokens` | S/K | (phase 3, RFE-013) BYO agent tokens without material: `Page<{ agent, workspace, created_at, expires_at, last_used_at, stale, expiring, display_prefix }>` (phase 3, D-107: paginated, C-313), filters `?stale=true`, `?expiring_within_days=n`; workspace route `can_own`, org route org `owner`/`admin` |
| GET `/v1/agent/container-config`; POST `/v1/agent/context-credentials` | A | BYO self-service (`08`) |
| PUT/GET `/v1/agents/{agent}/avatar`; GET `/v1/public/avatars/{key}` | S/K; P | |
| POST `/v1/agents/{agent}/restart` | S/K | restart sandbox |
| POST `/v1/agents/{agent}/freeze` `{ reason }`; POST `/v1/agents/{agent}/unfreeze` | S/K | (phase 2, RFE-006) `can_own`; 200 with `frozen: {at, by_label, reason, authority, level}`; 403 `freeze_authority_insufficient` when lifting a higher authority's freeze; audited atomically |
| GET `/v1/agents/{agent}/sandbox` | S/K | state, keep-awake reasons, processes, ports |
| GET `/v1/agents/{agent}/models` | S/K | models available from granted LLM keys |
| GET `/v1/gateway/ca.pem` | S/K | gateway CA certificate |

### Conversations and turns
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/agents/{agent}/conversations` | S/K | filters: kind; (phase 2, RFE-010) `archived=true\|false` (default false) |
| POST `/v1/agents/{agent}/conversations/direct` | S | get-or-create caller's direct conversation |
| POST `/v1/agents/{agent}/conversations` | K | `kind = api`, `external_ref` (get-or-create) |
| GET `/v1/conversations/{id}` | S/K | |
| POST `/v1/conversations/{id}/archive`, `/unarchive`; DELETE `/v1/conversations/{id}` | S/K | (phase 2, RFE-010) `10 §Archive, delete, redaction, retention, export`; delete 409 `conversation_active` |
| POST `/v1/conversations/{id}/events/{seq}/redact`; POST `/v1/turns/{id}/redact` | S/K | (phase 2, RFE-010) `{ reason?, reset_session? }`; irreversible, audited |
| GET `/v1/conversations/{id}/export` | S/K | (phase 2, RFE-010) JSON `aurora.conversation.v1` (`attachments=zip` → zip); 422 `export_too_large` |
| GET `/v1/conversations/{id}/events?after_seq=&limit=` | S/K | durable history |
| GET `/v1/conversations/{id}/stream` | S/K | SSE |
| POST `/v1/conversations/{id}/messages` | S/K | `{ text, attachment_ids? }` → `{ turn }` or `{ followup }`; (phase 3, RFE-018) optional header `Idempotency-Key` — a retry with the same key returns the original result (`Idempotent-Replayed: true`) and creates nothing; 422 `idempotency_key_invalid` / `idempotency_key_reused` (`### Idempotency keys`) |
| POST `/v1/turns/{id}/abort`, `/v1/turns/{id}/send-now` | S/K | |
| POST `/v1/conversations/{id}/attachments` | S/K | multipart, returns pending attachment; (phase 2, RFE-015) 422 `attachment_quota_exceeded` when the org quota would be exceeded |
| GET `/v1/attachments/{id}/content` | S/K | `Content-Disposition: attachment` always; (phase 2, RFE-015) 410 `attachment_purged` after retention/delete |
| DELETE `/v1/attachments/{id}` | S/K | (phase 2, RFE-015) purge the bytes now; uploader or `can_own` |

### Memory, skills, schedules, webhooks, links
| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/agents/{agent}/memories`; GET/PUT/DELETE `/v1/agents/{agent}/memories/{key}` | S/K | |
| GET `…/memories/{key}/revisions`; POST `…/memories/{key}/restore`; POST `…/memories/{key}/revisions/{rev}/redact` | S/K | |
| GET `/v1/agents/{agent}/memories/search?q=` | S/K | |
| GET/POST `/v1/agents/{agent}/skills`, `/v1/orgs/{org}/skills`; GET/PATCH/DELETE `/v1/skills/{id}`; PUT/DELETE `/v1/skills/{id}/files/{path}` | S/K | the item routes work for every tier and authorize by the skill's tier (agent: `can_use`; org: org `owner`/`admin`; workspace, phase 3 RFE-020: `can_own`); items carry `tier: "agent"\|"workspace"\|"org"` (phase 3, D-107: next to the existing `scope`, which gains `"workspace"`) |
| GET/POST `/v1/workspaces/{ws}/skills` | S/K | (phase 3, RFE-020) list (`can_use`) / create (`can_own`) workspace-tier skills; same body as the other tiers; 409 `skill_exists` in the workspace tier, 400 `invalid_request` for reserved/invalid names and 422 `skill_too_large` for size limits, as elsewhere (phase 3, D-107) |
| GET `/v1/agents/{agent}/skills/effective` | S/K | (phase 3, RFE-020) the effective set the agent receives: `[{ skill, tier, enabled, shadows: [{ id, tier }] }]` plus the skills that lose (`shadowed_by`); `can_use` |
| GET/POST `/v1/agents/{agent}/schedules`; GET/PATCH/DELETE `/v1/schedules/{id}`; GET `…/runs`; POST `…/run-now` | S/K | (phase 2, RFE-021) items expose `creator`, `anchor` (`creator`\|`workspace`); POST/PATCH accept `anchor` (`can_own` only; tool-created items are always `creator`) |
| PUT `/v1/schedules/{id}/creator`, PUT `/v1/webhooks/{id}/creator` | S/K | (phase 2, RFE-021) `{ user_id, re_enable? }`; `can_own`; 422 `new_creator_unauthorized`; audited `automation.reassign` |
| POST `/v1/workspaces/{ws}/automations/reassign` | S/K | (phase 2, RFE-021) `{ from_user_id, to: <user_id> \| "workspace", kinds?: [schedule,webhook], ids? }` → `{ reassigned: n }`; `can_own`; one transaction, one audit row per item |
| GET/POST `/v1/agents/{agent}/webhooks`; GET/PATCH/DELETE `/v1/webhooks/{id}`; POST `…/rotate`; GET `…/deliveries` | S/K | rotate returns new URL once; (phase 2, RFE-021) same `creator`/`anchor` fields as schedules |
| POST `/v1/hooks/{token}` | P | webhook receive |
| GET `/v1/agents/{agent}/links`; PUT `/v1/agent-links/{id}/policy`; GET `/v1/agents/{agent}/peer-tasks` | S/K | |

### Channels (management)
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/connectors` | S/K | active connectors with setup schemas, capabilities |
| GET/POST `/v1/orgs/{org}/channel-integrations`; GET/PATCH/DELETE `/v1/channel-integrations/{id}` | S/K | create validates via `credentials.validate` op; (phase 1, RFE-001) `config.verified_email_autolink` (bool, default false) is writable by org owner/admin only and refused with 422 `autolink_unsupported` when the integration lacks `email_verified`; changes are audited (security-critical set from phase 2, RFE-008) |
| GET/POST `/v1/channel-integrations/{id}/user-links`; DELETE `/v1/user-links/{id}` | S/K | admin links; each item carries `method` (`verified_email` \| `sign_in_link` \| `admin`) |
| GET/POST `/v1/agents/{agent}/presences`; DELETE `/v1/presences/{id}`; POST `/v1/presences/{id}/reattach` | S/K | |
| GET `/v1/agents/{agent}/space-bindings`; DELETE `/v1/space-bindings/{id}`; PATCH `/v1/space-bindings/{id}` | S/K | owners only |
| GET `/v1/me/channel-links`; DELETE `/v1/me/channel-links/{id}`; POST `/v1/channel-links/confirm` | S (confirm: SO) | sign-in link flow |
| GET/PUT `/v1/me/dm-defaults` | S | |
| GET `/v1/agents/{agent}/reach`; POST `/v1/reach-grants/{id}/decision` | S/K | |

### Instance administration
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/admin/overview` | IA | versions, gateways, hook hosts, runners, connectors, active snapshot |
| GET/PATCH `/v1/admin/users` | IA | list, instance-admin flag, disable |
| GET/POST `/v1/admin/plugins` (multipart upload); GET `/v1/admin/plugins/{id}`; POST `…/approve`, `…/reject`, `…/disable`, `…/enable`; DELETE `…?force=` | IA | (phase 3, RFE-022) GET `{id}` of a pending major upgrade carries `impact` (`09 §10`: `connections_needing_reauth`, `by_app`); `approve` activates and flips the failing connections to `reauth_required` in one transaction and returns the `PluginPackage` plus `connections_reauth: n` (phase 3, D-107) |
| GET `/v1/admin/host-claims`; PUT `/v1/admin/host-claims/{claim_key}` | IA | conflicts and winners |
| GET/POST/DELETE `/v1/admin/trusted-keys` | IA | |
| GET/POST/DELETE `/v1/admin/connectors`; POST `…/{id}/approve`, `…/{id}/token` | IA | provision token, approve manifest |
| GET/POST/DELETE `/v1/admin/runners`; POST `…/{id}/token` | IA | |
| GET `/v1/admin/storage` | IA | (phase 2, RFE-015) instance storage breakdown (`10 §Attachments`) |
| GET `/v1/admin/registry/snapshot` | IA | active snapshot (JSON) |
| POST `/v1/admin/orgs/{org}/freeze` `{ reason }`; POST `/v1/admin/orgs/{org}/unfreeze` | IA | (phase 2, RFE-006) operator kill-switch; `frozen_authority = instance_admin`; org data stays inaccessible |
| GET `/v1/admin/orgs` (`frozen=true` filter, paginated) | IA | (phase 2, RFE-006, D-76) organizations with their freeze state: id, slug, name only |
| GET/POST `/v1/admin/oauth-clients`; PATCH/DELETE `/v1/oauth-clients/{id}` | IA | instance-level OAuth clients (`08 §OAuth clients`) |

### Planes (machine callers)
Runner plane `/v1/runner/*` and connector plane `/v1/connector/*` are specified
in `16 §W5` and `13 §Connector contract`.
