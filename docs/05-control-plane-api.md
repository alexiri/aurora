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
| Connector | `Authorization: Bearer aun_…` + `X-Aurora-Replica` (phase 4, P4-MCON: or the connector host's `auh_…` + `Aurora-Connector: <id>` for a **managed** connector, C-458) | `/v1/connector/**` |
| Connector host (phase 4, P4-MCON) | `Authorization: Bearer auh_…` | `/v1/connector-host/**` (C-455..C-457) |
| SCIM client (IdP; phase 4, P4-GRP-2) | `Authorization: Bearer aud_…` (per org; cookies and `Origin` ignored) | **only** `/scim/v2/**` (`16 §W15`); the edge proxy routes `/scim/v2` to the API in addition to `/v1`; 404 while `scim.enabled = false`; an `aud_` bearer anywhere else is 401 |
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
    (phase 4, P4-GRP-2) `/scim/v2/**` is not an authenticated `/v1` request: it has its own
    per-token bucket `SCIM_RATE` (600/min, burst 120; `key = api:scim:token:<id>`, class
    `management`, `04 §SCIM`) and the rejected-bearer limit for bad tokens.
  - **Token buckets**, in memory per API replica by default (the same
    approximation as the gateway, D-05; N replicas can allow up to N×).
    (phase 4, P4-CNT, D-119) With `ratelimit.counters.backend = postgres` or `redis`
    the buckets are **shared by all API replicas** through the `CounterStore` trait
    (`15 §Shared rate-limit counters`, `06 §Rate limiting`): the limits of this
    section then hold for the whole deployment, not per replica; the table and the
    429 shape are unchanged. See *Shared counters* below. A request must obtain a token from
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
  - **Shared counters (phase 4, P4-CNT, D-119, SEC-62).** Applies only when the
    backend is `postgres` or `redis`; `memory` is exactly the text above.
    - *Which limiters.* The authenticated buckets above (`general`, the class
      buckets, `org`), the unauthenticated per-IP limits (`API_RATE_UNAUTH`, the
      rejected-bearer limit, `OIDC_AUTH_RATE`) and the per-webhook 60/min of
      `POST /v1/hooks/{token}`. **Not shared** (stay per replica, by design): the
      concurrent-stream cap `API_STREAMS_PER_PRINCIPAL` (a concurrency gauge, not
      a rate; sharing it needs leases with heartbeats, which is not worth it) and
      CLI device-code polling (`interval` is enforced against the row in Postgres
      already). Residual: the stream cap can reach N× (`17`).
    - *Semantics.* Each bucket is a GCRA / virtual-scheduling state (one value per
      key, `tat`) so a bucket needs one number per key in the store; refill and
      burst are the table above; time comes from the store's clock. The three
      buckets of one request are taken **all-or-nothing in one store call**
      (`bucket_take`), so a refused request still consumes nothing from the
      buckets that would have allowed it. `retry-after` and `ratelimit-*` come
      from the blocking bucket as before.
    - *Local pre-filter.* To keep refused floods off the store, each replica keeps
      its own in-memory bucket per key with the **full** configured limit and checks
      it first; a refusal there is final (a replica alone can never serve more than
      the global limit, so the global bucket would refuse as well). A store refusal
      refunds the local token. The pre-filter only ever refuses what the shared
      state would also refuse, so it cannot loosen or tighten the shared limit.
    - *Keys* (`api:<kind>:…`) use ids (`key:<id>`, `user:<id>`, `org:<id>`,
      `agent:<id>`, webhook id). Keys derived from an **e-mail or an IP address**
      (the unauthenticated limits) use `HMAC-SHA256(hkdf("ratelimit_key"), value)`
      truncated to 128 bits, so a leaked Redis or Postgres dump does not reveal
      e-mail addresses or client IPs and cannot be matched to them offline
      (`08 §4`).
    - *Unavailable backend.* The class decides (`ratelimit.counters.on_unavailable.<class>`,
      default `local` for all): **`security`** (the unauthenticated per-IP limits,
      rejected-bearer limit, `OIDC_AUTH_RATE`): `local` keeps brute-force
      protection at today's strength; `closed` refuses the request with 503
      `counter_unavailable` — these are the limiters that must **never** fail
      open, and `open` is not a value for any class (startup refuses it).
      **`management`** (`general`, class and `org` buckets, webhooks): `local`
      keeps the API usable through a Redis outage with today's per-replica limit;
      `closed` refuses authenticated writes and reads with 503, which turns a
      cache outage into an API outage, so it is for operators who prefer that.
      Health, metrics, the machine planes and the exempt routes are never
      affected. The 503 carries `code = counter_unavailable`, `retry-after: 5`,
      `details.limiter`. The refusal is never returned as 429 (a client must not
      mistake an outage for its own rate).
    - *Metrics:* `aurora_api_rate_limited_total{class,scope}` unchanged; plus the
      `aurora_counters_*` series of `15 §Shared rate-limit counters`.
    - *Cost.* Every authenticated request costs one store round-trip (three
      buckets in one call). Redis is the recommended backend for the API limiter;
      Postgres works for small deployments (≈ hundreds of requests per second in
      total) and is the default shared choice because it adds no infrastructure.
      Operators can keep the API limiter local and share only the gateway with
      `ratelimit.counters.api = "memory"` (`15`).
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
| `aurora_ops` | `{ "connector": id }`; (phase 4, P4-MCON) `{ "assignments": true }` when a managed connector changed | API → API connector long-polls; the connector-host `assignments` long poll |
| `aurora_inbox` | `{}` | API → API inbox workers |
| `aurora_op_result` | `{ "op_id" }` | API (on connector results) → API callers waiting on correlated operations |
| `aurora_approval` | `{ "id", "workspace_id", "event": "created"|"decided"|"expired" }` | gateway & API → gateways, API (cards, SSE, long-polls) |
| `aurora_gateway_invalidate` | `{ "scope": "org"|"workspace"|"agent"|"credential"|"connection"|"user"|"all", "id"?: uuid }` (phase 4, P4-GRP-1: `user`, id = the user; drops the cached identities of that speaker) | API → gateways |
| `aurora_registry` | `{ "hash": content_hash, "contract": int }` | API → gateways, API replicas (hook hosts have no DB connection and poll `/internal/v1/registry/active` every 60 s and on cache miss) |
| `aurora_secret_origin` | `{ "secret_id", "status" }` | gateway (origin preflight/backstop) → API (audit, owner notice, dashboard) |
| `aurora_auth_revoked` | `{ "kind": "session"|"api_key"|"user", "id" }`; (phase 4, P4-GRP-1) `{ "kind": "access", "id": user_id \| null, "org_id" }`: the user's (or, with `id` null, every user of the org's) effective access changed (`04 §Groups`) | API → API replicas, gateways (replicas re-authorize that user's open SSE streams and long-polls; a stream the user may no longer open is closed with a final `access.changed`; gateways ignore it, they use the `user`/`workspace`/`org` scopes) |

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
| Approval expiry: `pending` past `deadline + 30 s` → `expired` (phase 4, RFE-011b: a **parked** approval expires at `deadline`, nobody holds it); pending rows of gateway instances unseen for 60 s → `abandoned` (phase 4: **never** `parked` rows) | every 10 s | idempotent updates |
| Approval resume window (phase 4, RFE-011b): `resume_state = 'ready'` past `resume_expires_at` → `expired` (audit best-effort, transcript notice, no turn); parked approvals at 50 % of their time → one reminder | every 10 s | idempotent updates, claims (`approval_push_claims`) |
| Web Push delivery (phase 4, RFE-011b, `07 §Web Push`): on `aurora_approval created` + `APPROVAL_PUSH_DELAY`, deciders with a live subscription and no watcher; failure handling, subscription disabling, retired-VAPID-key destruction (`PUSH_VAPID_RETIRE`) | event + every 10 s catch-up; daily | claims (`approval_push_claims`) |
| Action-approval expiry | every 60 s | idempotent |
| Attachment pending sweep (`ATTACH_PENDING_TTL`) | hourly | idempotent |
| Conversation retention sweep (phase 2, RFE-010): delete conversations past the org window through the delete path; blob-orphan sweep | daily | singleton |
| Attachment sweep (phase 2, RFE-015): purge attachment bytes past the org window, remove orphan blobs, reconcile `org_storage_usage`, quota notices; (phase 4, P4-S3) with the `s3` backend the orphan step lists the bucket prefix and honours `S3_ORPHAN_GRACE` (`10 §Attachments`) | daily | singleton |
| SCIM token expiry (phase 4, P4-GRP-2): a notice to the org owners and an audit row 14 days and 1 day before a token expires; expired tokens are simply dead (read-time check) | daily | singleton |
| Counter pruning (phase 4, P4-CNT, `postgres` backend only): delete dead `rate_limit_windows` / `rate_limit_buckets` rows | every `COUNTER_PRUNE_INTERVAL` (60 s) | `SKIP LOCKED` batches |
| Blob store probe (phase 4, P4-S3, `s3` backend only): `HeadBucket` + versioning/encryption state, sets `aurora_blob_store_up` | every 30 s | per replica |
| Skill library refresh (phase 4, P4-GSL, `10 §Skills`): libraries with at least one added skill and `next_refresh_at <= now()` and `status = active` are claimed with `FOR UPDATE SKIP LOCKED` and a per-library lease (`LIBRARY_LEASE` 5 min, `refresh_lease_until`/`lease_owner`), at most `LIBRARY_REFRESH_CONCURRENCY` (4) per replica; resolve → compare head → tree → blob comparison → proposed updates through the gateway's library fetch (`06 §Library fetch`); errors back off 15 min → 1 h → 6 h; never writes skill content (only `skill_library_updates`, `skill_libraries`) and never bumps a home-sync generation | every 60 s | claims (lease) |
| Skill library retention (phase 4, P4-GSL): prune decided `skill_library_updates` older than 90 days; expire leases; mark libraries whose connection vanished `error` | daily | singleton |
| Preview housekeeping (phase 4, P4-PRV-1/2): delete expired `preview_sessions`, `preview_grant_uses` and `preview_stream_grants`; clear stale `sandboxes.preview_wake_at`/`preview_start_requested_at`; count shares per org for `aurora_preview_shares` | every 60 s | idempotent (`SKIP LOCKED` batches) |
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

(phase 4, P4-PRV-1, D-145) Arm 3 (start) also starts a stopped sandbox that has a **preview wake
demand** (`sandboxes.preview_wake_at` younger than `PREVIEW_WAKE_WINDOW`, `11 §Port previews`),
only for a hosted agent that is not effectively frozen, whose org has Port previews on, and
that has an active share; arm 2 (stop) counts `preview` among the keep-awake reasons; arm 5
(home sync) carries the `previews` part; after the sync is applied, arm 5 also emits
`preview.start { share_id }` for a share with a start command whose port is not listening and
for which a request is waiting (`preview_start_requested_at` younger than `PREVIEW_WAKE_WAIT`),
at most `PREVIEW_START_ATTEMPTS` per `PREVIEW_START_WINDOW`.

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
| (phase 4, P4-GRP-1) Group member added/removed | `user` (+ `aurora_auth_revoked` kind `access`) |
| (phase 4, P4-GRP-1) Group binding change; group deleted | `workspace` per affected workspace (`org` above 20) (+ `access`, org-wide) |
| (phase 4, P4-GRP-1) Group org-role mapping change | `org` (+ `access`, org-wide) |
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
| GET/PATCH `/v1/me/onboarding` | S | (phase 4, P4-ONB-2, D-141) the caller's onboarding progress: `{ welcome_seen, guide_dismissed: [workspace_id], tour_completed_at }`; guide step completion itself is computed by the client from real state (model readiness, grants, conversations), never stored |
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
| GET `/v1/workspaces/{ws}/access?expand=groups` | S/K | (phase 4, P4-GRP-1) opt-in expansion for `can_own`: adds `group_bindings: [{ group: {id, name, source, member_count}, role }]` and `derived_members: Page<{ user, role, sources: [{ kind: "direct"\|"group"\|"org_role", group? }] }>` (the effective role of everyone who has access through a group or an org role; a person with several sources appears once). Without `expand` the response is unchanged |
| PUT/DELETE `/v1/workspaces/{ws}/access/groups/{group}` | S/K | (phase 4, P4-GRP-1) `PUT { role: "owner"\|"approver"\|"member" }` binds or changes the role of a group; `can_own`; 404 for a group of another org, 422 `group_binding_limit_reached`; DELETE is idempotent; both audited (`group_access.set\|remove`), invalidation scope `workspace` |
| GET/POST `/v1/workspaces/{ws}/api-keys` | S (POST: SO) | workspace keys |
| GET `/v1/workspaces/{ws}/audit` | S/K | same filters |
| GET `/v1/orgs/{org}/storage` | S/K | (phase 2, RFE-015) org `owner`/`admin`: usage vs quota, breakdown by workspace and direction |
| GET/PUT `/v1/orgs/{org}/retention` | S/K | (phase 2, RFE-010/015) org `owner`/`admin`; `{ conversations_days?, attachments_days?, attachment_quota_mib? }`, `null` = none; 422 `retention_invalid` / `retention_above_instance_maximum` |
| GET `/v1/workspaces/{ws}/activity` | S/K | request log, filters: agent, conversation, decision, host, app |
| GET `/v1/workspaces/{ws}/usage` | S/K | runtime + tokens |
| POST `/v1/workspaces/{ws}/freeze` `{ reason }`; POST `/v1/workspaces/{ws}/unfreeze` | S/K | (phase 2, RFE-006) `can_own`; as for agents |

### Groups (phase 4, P4-GRP-1, D-130)
Permissions per `04 §Permission matrix`; every write is audited and security-critical
(`04 §Audit`) and invalidates as in *Gateway cache invalidation*; lists are paginated
(`Page`). A group of another org is 404. Errors: 409 `group_name_taken`, 422
`group_limit_reached`, `group_member_limit_reached`, `group_user_limit_reached`
(`GROUPS_PER_USER`), `not_an_org_member`, `group_org_role_invalid` (`owner` or unknown),
409 `group_managed_by_scim` (name, delete and membership of a `source = scim` group are
the IdP's; `DELETE` is allowed only when the org's SCIM is disabled; the mapping and
workspace bindings stay editable), 409 `stale_revision`.

| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/orgs/{org}/groups` | S/K | list (`q`, `source`, `member=<user>`) / create `{ name, description?, org_role? }` → `Group { id, name, description, source, org_role, member_count, workspace_count, revision, created_at }`; org `owner`/`admin` (names and counts for `can_own` of any workspace of the org) |
| GET/PATCH/DELETE `/v1/groups/{id}` | S/K | PATCH `{ name?, description?, org_role?: "admin"\|"member"\|null }` with `If-Match`/`revision`; DELETE removes members and bindings and never rewrites policy rules (`07`) |
| GET `/v1/groups/{id}/members`; PUT/DELETE `/v1/groups/{id}/members/{user}`; POST `/v1/groups/{id}/members:batch` | S/K | org `owner`/`admin`; PUT/DELETE are idempotent; the batch body is `{ add?: [user], remove?: [user] }` (≤ 100 each, one transaction, one audit row per change) |
| GET `/v1/groups/{id}/workspaces` | S/K | the workspaces the group is bound to, with roles (org `owner`/`admin`) |
| GET `/v1/groups/{id}/impact?remove_member=<user>` | S/K | org `owner`/`admin`: what deleting the group (or, with `remove_member`, removing that member) would take away, `{ workspaces: [{ workspace, users_losing_access: [...≤ GROUP_IMPACT_LIST_MAX], total, schedules_disabled, webhooks_disabled, api_keys }] }` |
| GET `/v1/me` | S/K | (phase 4) also `groups: [{ id, name, org_id }]` and, per membership, `effective_role` with `sources` (`role` stays the direct role) |

(phase 4, P4-GRP-1) The offboarding views of `§Organizations, members, keys` add
`via_groups` to each row; `GET /v1/agents/{agent}/effective` and policy `simulate` take
the speaker's groups into account (`07`).

### SCIM management (phase 4, P4-GRP-2, D-132)
Org `owner` only (`04 §SCIM`); every write is security-critical audited. The SCIM plane
itself (`/scim/v2`, token auth, SCIM errors) is specified in `16 §W15`, not here.

| Method & path | Auth | Purpose |
|---|---|---|
| GET/PUT `/v1/orgs/{org}/scim` | S (PUT: SO) | `{ available, enabled, email_domains: [domain], exclusive, base_url }` (`available` = instance `scim.enabled`; `base_url` = `<external>/scim/v2`); PUT validates domains (422 `scim_domain_invalid`, `scim_domain_not_allowed`, `scim_domains_required` when enabling) and needs a recent re-authentication when it turns SCIM on (412 `reauth_required`); 409 `scim_unavailable` when the instance switch is off |
| GET/POST `/v1/orgs/{org}/scim/tokens`; DELETE `/v1/scim/tokens/{id}` | S (POST: SO) | list without material (`name`, `display_prefix`, `created_at`, `expires_at`, `last_used_at`, `expiring` within 14 days); POST `{ name, expires_in_days? }` → the `aud_` token once; 422 `scim_token_limit_reached`, `scim_not_enabled`; 412 `reauth_required`; DELETE is idempotent and effective at once |
| GET `/v1/orgs/{org}/scim/status` | S | Directory sync page: `{ last_request_at, users: {active, suspended, removed}, groups, errors: [≤ SCIM_ERROR_LOG sanitized { at, method, resource, status, scim_type }] }` |

Offboarding by SCIM is visible through the existing offboarding views
(`GET /v1/orgs/{org}/members/{user}/offboarding`, RFE-021), which keep working for a
`suspended` member and now carry `byo_agents_created` (id, name, workspace) next to the
schedules and webhooks; members created or managed by SCIM carry `directory: {
state, external_id? }` in `GET /v1/orgs/{org}/members`.

### Secrets and LLM
| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/workspaces/{ws}/secrets`, `/v1/orgs/{org}/secrets` | S/K | create: name, kind, llm fields or host/path/injection, `value` or `reference`; (phase 4, P4-LLM) for `aws_sigv4` the `value` is the JSON `{ access_key_id, secret_access_key, session_token? }` and for `gcp_service_account` the key-file JSON, validated for shape, `token_uri` and key type (422 `llm_key_invalid`) before any provider call; the same validation, governance and origin steps as every LLM secret (`08 §LLM secrets`) |
| GET/PATCH/DELETE `/v1/secrets/{id}` | S/K | metadata only; values never returned |
| PUT `/v1/secrets/{id}/value` | S/K | replace value |
| POST `/v1/secrets/{id}/accept` | S | agent proposal: `{ value }` → secret `active` + grant to the proposing agent |
| POST `/v1/secrets/{id}/reject` | S/K | agent proposal → `rejected`; a notice goes to the proposing conversation |
| GET `/v1/secret-stores` | S/K | configured stores: id, kind, capabilities (no config) |
| GET `/v1/llm/providers` | S/K | registry + configured endpoints; (phase 4, P4-LLM) each provider lists its `auth_modes` with their value shape (`string`, `aws_sigv4` fields, `gcp_service_account` file), each endpoint its `dialects`, `region`/`project_id`/`location` where they exist and `models_source` (`listed` \| `static`); never credentials or endpoint internals |
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
| PUT/DELETE `/v1/agents/{agent}/grants/connections/{conn}` | S/K | `{ mode, allow_tools, ask_tools }`; (phase 2, RFE-011a) optional `ask_timeout_secs`; (phase 4, P4-GRANTS, D-123) `mode = full` for a connection of an app whose manifest has `grants.custom_only` → 422 `full_grant_forbidden` (`details: { app_id }`); the grant response and `GET …/grants` carry `app_restrictions: { custom_only, block_untooled }` so clients can explain it; a grant converted by an upgrade is audited `grant.converted_by_app` |
| PUT/DELETE `/v1/agents/{agent}/grants/secrets/{secret}` | S/K | |
| GET `/v1/connections/{id}/grants` | S/K | agents granted |
| POST `/v1/connections/{id}/grants` | S/K | (phase 4, FIX-020, D-137) bulk grant from the credential side: `{ agent_ids (1–`GRANT_BULK_MAX`), mode, allow_tools?, ask_tools?, ask_timeout_secs? }`; each agent passes exactly the checks of `PUT /v1/agents/{agent}/grants/connections/{conn}` (same service function, incl. 422 `full_grant_forbidden`); all-or-nothing, one audit row per new grant; an existing grant is left unchanged and reported `existing` (C-504) |
| GET/POST `/v1/secrets/{id}/grants` | S/K | (phase 4, FIX-020, D-137) agents granted (only agents the caller can see) / bulk grant `{ agent_ids }` with exactly the checks of `PUT /v1/agents/{agent}/grants/secrets/{secret}`; all-or-nothing, idempotent (C-503) |
| GET `/v1/agents/{agent}/model-readiness` | S/K | (phase 4, FIX-019, D-136) `can_own`: per granted LLM key `usable` or the failing reason, and the default model selection (`08 §Model readiness`, C-500) |
| GET `/v1/agents/{agent}/effective` | S/K | effective credentials and per-tool decisions with `org_ceiling`; (phase 2, RFE-005) optional `?speaker=<user_id>\|none` |

### Policy
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/workspaces/{ws}/policy`, `/v1/orgs/{org}/policy` | S/K | published + draft + revision; (phase 4, P4-CNT) plus `rate_limit_scope: "replica" \| "shared"` (`shared` when `ratelimit.counters.backend` is `postgres`/`redis` and the gateway share is not `memory`; the editor note of `14`) |
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
| POST `/v1/approvals/{id}/decision` | S/K | `{ "decision": "approve"|"deny" }`; first decision wins (409 `already_decided`); (phase 4, RFE-011b) for a parked approval the response carries `resume: { state, expires_at }` and the DTOs of the approvals feed, long-poll, SSE and history carry `parked: bool`, `resume_state`, `resume_expires_at` (never the fingerprint or salt) |
| POST `/v1/approvals/batches/{batch_id}/decision` | S/K | all pending of the batch the caller may decide |
| GET `/v1/workspaces/{ws}/action-approvals`; POST `/v1/action-approvals/{id}/decision` | S/K | |

### Web Push (phase 4, RFE-011b, D-134)
Own-resources only: every route acts on the caller's subscriptions; another user's subscription
is 404. 404 `push_unavailable` on all of them while `push.enabled = false`. Writes are best-effort
audited (`push_subscription.create|delete`).

| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/push/config` | S | `{ available, vapid_public_key, vapid_key_id, allowed_hosts }` for the browser's `pushManager.subscribe` (the key is public) |
| GET `/v1/me/push-subscriptions` | S | the caller's devices: `{ id, label, show_details, paused, state: "active"\|"paused"\|"disabled"\|"expired"\|"stale_key", this_session, created_at, last_success_at }` — never the endpoint or keys |
| POST `/v1/me/push-subscriptions` | SO | `{ endpoint, keys: { p256dh, auth }, expiration_time?, label?, show_details?, vapid_key_id }` → 201 `PushSubscription`; 422 `push_endpoint_not_allowed` (not https, host not in `push.allowed_endpoint_hosts`, over 2 048 chars), `push_keys_invalid` (`p256dh` not a 65-byte P-256 point, `auth` not 16 bytes), `push_subscription_limit_reached`, 409 `push_key_stale` (the `vapid_key_id` is retired: re-fetch `config`); an endpoint already registered by another user is **moved** to the caller and its session |
| PATCH/DELETE `/v1/me/push-subscriptions/{id}` | SO | `{ label?, show_details?, paused? }`; DELETE is idempotent |
| POST `/v1/me/push-subscriptions/{id}/test` | SO | sends a `test` push (`PUSH_TEST_PER_HOUR` 3 per user); 202, the outcome appears in the device state |

CLI (operator): `aurora-api push rotate-vapid`, `aurora-api push status` (key ids and dates,
subscription counts by state and host; never key material).

### Agents and sandboxes
| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/workspaces/{ws}/agents` | S/K | create returns `aug_` once for BYO; (phase 2, RFE-025) creating `kind = byo` is subject to the effective BYO posture: 403 `byo_not_permitted` (`owners_only`, caller lacks `can_own`) or 403 `byo_disabled`; the list/get shape carries `byo: { posture, posture_level, source_cidrs, require_context, effective_source_cidrs, effective_require_context }` for BYO agents |
| GET/PATCH/DELETE `/v1/agents/{agent}` | S/K | name, brief, harness, model, effort, resource overrides (IA); (phase 4, P4-HAR) `harness` must be in `hosted.enabled_harnesses` (422 `harness_not_enabled`), cannot change while a turn is active (409 `turn_active`), and a change bumps every conversation's `harness_session_epoch`, queues session purges and drops incompatible model overrides with a notice (`12 §Harness selection and adapter contract`); audited |
| GET `/v1/harnesses` | S/K | (phase 4, P4-HAR) enabled harnesses: `{ id, display_name, dialects, available }` where `available` means at least one live runner replica lists it; drives the dashboard picker |
| POST `/v1/agents/{agent}/token` | S | BYO: regenerate, returns token once with `expires_at` (phase 3, RFE-013: null unless an agent-token maximum age applies, `08 §7`); (phase 2, RFE-025) same posture check as creation (`byo_not_permitted` / `byo_disabled`); audited `agent.token_regenerate` |
| PUT `/v1/agents/{agent}/byo` | S/K | (phase 2, RFE-025) BYO only (hosted → 422 `not_byo`): `{ source_cidrs: [cidr..] \| null, require_context: bool }`; `can_own`; 422 `byo_cidr_invalid` (bad CIDR, `/0`, empty array, >32 entries) / `byo_cidr_outside_default`; live credentials are re-evaluated by invalidation scope `agent`; audited `agent.byo_settings.update` |
| GET `/v1/agents/{agent}/container-config` | S/K | BYO bootstrap (`08 §BYO bootstrap`) |
| GET `/v1/workspaces/{ws}/agent-tokens`; GET `/v1/orgs/{org}/agent-tokens` | S/K | (phase 3, RFE-013) BYO agent tokens without material: `Page<{ agent, workspace, created_at, expires_at, last_used_at, stale, expiring, display_prefix }>` (phase 3, D-107: paginated, C-313), filters `?stale=true`, `?expiring_within_days=n`; workspace route `can_own`, org route org `owner`/`admin` |
| GET `/v1/agent/container-config`; POST `/v1/agent/context-credentials` | A | BYO self-service (`08`) |
| PUT/GET `/v1/agents/{agent}/avatar`; GET `/v1/public/avatars/{key}` | S/K; P | |
| POST `/v1/agents/{agent}/restart` | S/K | restart sandbox |
| GET `/v1/agents/{agent}/terminal` | S | (phase 4, P4-TERM, D-161, C-616) WebSocket upgrade to the sandbox's shell (wterm + Ghostty core in the dashboard); session cookie only, `can_use`, hosted only (BYO 422 `terminal_not_supported`), 404 `terminal_disabled` while off, 423 frozen; mints the single-use `auy_` grant and proxies to the runner; best-effort audit `agent.terminal.open`; bytes never logged |
| POST `/v1/agents/{agent}/sandbox/retry` | S/K | (phase 4, FIX-029, D-160, C-610) retry a failed start from `failed` only (409 `sandbox_not_failed`, 423 `frozen`): clears the failure, resets `start_failures`, notifies due work for an ordinary start; audited `agent.sandbox_retry` |
| GET `/v1/agents/{agent}/start-failures` | S/K | (phase 4, FIX-029, C-613) the agent's recent start failures (≤ 20, newest first, C-586 shape) |
| GET `/v1/admin/agents/{agent}/start-failures` | IA | (phase 4, FIX-029, C-614) the same list with `runner_id` |
| POST `/v1/agents/{agent}/freeze` `{ reason }`; POST `/v1/agents/{agent}/unfreeze` | S/K | (phase 2, RFE-006) `can_own`; 200 with `frozen: {at, by_label, reason, authority, level}`; 403 `freeze_authority_insufficient` when lifting a higher authority's freeze; audited atomically |
| GET `/v1/agents/{agent}/sandbox` | S/K | state, keep-awake reasons, processes, ports |
| GET `/v1/agents/{agent}/models` | S/K | models available from granted LLM keys |

### Port previews (phase 4, P4-PRV-1, D-144, D-145; C-540..C-549)

Present on every instance but inert: while `previews.enabled` is false every route below answers
404 `previews_disabled` (except `GET /v1/instance`, which reports `previews: { enabled: false }`),
and while the **org** has not enabled Port previews, creating a share answers 409
`previews_not_enabled_for_org`. Share routes are management routes (session or API key, `can_use`
of the agent's workspace); the browser-side authorization is P4-PRV-2 (`04 §Preview access`).

| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/agents/{agent}/previews` | S/K | `{ shares: [PreviewShare] }`, `can_use`; each `{ id, agent_id, port, hostname, url, visibility: "private", state: "active"\|"paused", status: "listening"\|"not_listening"\|"waking"\|"paused"\|"frozen", start_command, cwd, last_start: { at, exit_code, tail }\|null, last_accessed_at, created_by: { id, label }, created_at }`; `url` is `https://<label>.<previews.domain>/` |
| POST `/v1/agents/{agent}/previews` | S/K | `{ port, start_command?, cwd? }` → 201 `PreviewShare`; `can_use`; errors 422 `preview_not_supported` (BYO), 422 `preview_port_forbidden`, 422 `invalid_request` (command longer than 1 024, multi-line, `cwd` absolute or with `..`), 409 `preview_exists`, 409 `preview_limit`, 409 `previews_not_enabled_for_org`, 423 `frozen`; audit `preview.share.create`; bumps the agent's home-sync generation like every share change |
| GET/PATCH/DELETE `/v1/previews/{id}` | S/K | `can_use` of the share's workspace (404 otherwise); PATCH `{ state?, start_command?, cwd? }` (the port and the label are immutable); DELETE removes the share and its preview sessions; audit `preview.share.update` / `preview.share.delete`; every change bumps the agent's home-sync generation |
| POST `/v1/previews/{id}/start` | S/K | start the app now (enqueue `preview.start`; the sandbox is woken if parked, subject to `PREVIEW_WAKES_PER_SHARE_HOUR`); 202 `{ status }`; 409 `preview_no_start_command`; 423 `frozen` |
| PATCH `/v1/orgs/{org}` (`settings.previews_enabled`) | S | org `owner`/`admin`; audit `org.previews.update` (security-critical); switching off keeps shares, ends access (`11 §Port previews`) |
| GET `/v1/instance` | none | (existing public route) gains `features.previews` and `previews: { enabled: bool, domain: string\|null }` (no secret; the domain is already public in DNS), so the dashboard shows the Previews tab only when it can work |

`GET /v1/agents/{agent}/sandbox` already lists `ports`; (phase 4, P4-PRV-1) it gains `preview_in_use: bool`
(keep-awake reason `preview`) and each `ports` entry gains `shared: <share id>|null`. Audit and
the share lifecycle: `11 §Port previews`.
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
| POST `/v1/conversations/{id}/attachments` | S/K | multipart, returns pending attachment; (phase 2, RFE-015) 422 `attachment_quota_exceeded` when the org quota would be exceeded; (phase 4, P4-S3) with the `s3` blob backend also 503 `blob_store_unavailable` (`retry-after: 5`) and 503 `blob_store_busy`; downloads (`GET /v1/attachments/{id}/content`) add 503 `blob_store_unavailable`, 503 `blob_backend_not_configured` and 502 `blob_store_corrupt`, never 404/410 for a store failure (`10 §Attachments`) |
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

### Skill libraries (phase 4, P4-GSL, D-148, D-149; C-570..C-579)

404 `skill_libraries_disabled` while `skills.library.enabled` is false. Authorization per row;
tier authority = agent `can_use`, workspace `can_own`, org owner/admin (`04 §Permission matrix`).

| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/orgs/{org}/skill-libraries` | S/K | list (org owner/admin; others get `{ id, label }` of active libraries for the browse picker) / create `{ label, connection_id, repo, ref?, path? }` (org owner/admin) → 201 `SkillLibrary { id, label, app_id, connection: { id, label, status }, repo, ref, path, status, last_head_sha, last_refresh_at, last_error, skill_counts: { agent, workspace, org }, pending_updates }`; 422 `library_unreachable`, `connection_not_org_scoped`, `app_without_library`, `invalid_request` (repo/ref/path), 409 `library_exists`, 409 `library_limit`; audit `skill_library.create` |
| GET/PATCH/DELETE `/v1/skill-libraries/{id}` | S/K | org owner/admin; PATCH `{ label?, status?: "active"\|"paused", connection_id?, ref?, path? }` (changing repo is delete + create; changing ref/path/connection re-resolves and never touches stored skills); DELETE detaches every skill of the library (`10 §Skills`) and audits the count; audit `skill_library.update`/`delete` |
| POST `/v1/skill-libraries/{id}/refresh` | S/K | "Check now": org owner/admin; 202; 429 `library_check_too_soon` within `LIBRARY_CHECK_NOW_INTERVAL` |
| GET `/v1/skill-libraries/{id}/skills?q=&cursor=` | S/K | browse the current head live (`10 §Skills`): `{ commit_sha, items: [{ path, name, description, valid, reason?, size, files, added: [{ tier, id }] }] }`; org members who can use any workspace of the org; 502 `library_unavailable` with the sanitized reason; never stores anything; `Cache-Control: no-store` |
| GET `/v1/skill-libraries/{id}/skills/{path}?commit_sha=` | S/K | one skill's full content at a commit (what an add would store, and the diff base for updates); same audience; same validation result |
| POST `/v1/agents/{agent}/skills`, `/v1/workspaces/{ws}/skills`, `/v1/orgs/{org}/skills` | S/K | (existing routes) body may be `{ from_library: { library_id, path, commit_sha } }` instead of `name/description/body/files`; the commit sha is **required** and fetched exactly; 422 `library_skill_invalid`, `library_budget_exceeded`, 409 `skill_exists`; audit `skill.library.add` (critical) |
| PATCH/PUT/DELETE `/v1/skills/{id}` (+ `/files/{path}`) | S/K | for a library skill: body, description, files → 409 `library_skill_readonly`; `enabled` and DELETE as usual (tier authority) |
| GET `/v1/skills/{id}/library` | S/K | `{ library: { id, label, repo, path }, commit_sha, pinned_at, pinned_by, update: null \| { id, kind, commit_sha, created_at, diff, content_sha256, over_budget } }`; the diff is the whole stored-vs-proposed content, not a summary; readable by whoever can read the skill |
| POST `/v1/skills/{id}/library/updates/{uid}/approve` `{ content_sha256 }` | S | tier authority; 409 `update_superseded`, 422 `library_skill_invalid`, 422 `library_budget_exceeded`; audit `skill.library.update_apply` (critical); bumps the home-sync generation of the affected agents |
| POST `/v1/skills/{id}/library/updates/{uid}/reject`; POST `/v1/skills/{id}/library/detach` | S | tier authority; audits `skill.library.update_reject`, `skill.library.detach` (critical) |
| GET `/v1/orgs/{org}/skill-library-updates`, `/v1/workspaces/{ws}/skill-library-updates`, `/v1/agents/{agent}/skill-library-updates` | S/K | pending proposals visible to the caller's authority at that scope (org: org owner/admin sees all of the org; workspace: `can_own`; agent: `can_use`), with `{ skill, library, kind, created_at }` — the dashboard badge and the Skills pages' "Update available" |

Items of every skill route gain `source: null | { kind: "library", library_id|null, label, repo, path, commit_sha, pinned_at, detached: bool, update_pending: bool }`;
`GET /v1/agents/{agent}/skills/effective` shows it per skill. The pages are paginated like every list.

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
| GET `/v1/admin/overview` | IA | versions, gateways, hook hosts, runners, connectors, active snapshot; (phase 4) `counters: { backend, state }` (P4-CNT, `15`) and `blobs: { backend, healthy }` (P4-S3) |
| GET/PATCH `/v1/admin/users` | IA | list, instance-admin flag, disable; (phase 4, P4-IDP-1, D-139) each user carries `idp_roles: { provider, values, synced_at, granted: [...] }`; claim-derived instance-admin flags cannot be changed here (409 `managed_by_idp`) |
| GET `/v1/admin/identity` | IA | (phase 4, P4-IDP-1) per provider the role claim settings and mapping table, read-only (from the Helm values; no secrets) |
| GET/PUT `/v1/admin/placement` | IA | (phase 4, P4-ONB-1, D-140) the new-user placement (`04 §New-user placement`); PUT validates org/workspace ids, roles, template and starter grants (credentials of the default org only) and is audited; `GET …/placement/preview?template=&sample=` renders a template for a typed sample or the caller's profile (D-151) (C-520..C-529) |
| GET `/v1/admin/setup` | IA | (phase 4, P4-ONB-2, D-141) setup-wizard status per step (roles, placement, LLM endpoints incl. `default_model`, runners, gateway), each `ok` \| `warning` \| `todo` with a reason code |
| GET/POST `/v1/admin/plugins` (multipart upload); GET `/v1/admin/plugins/{id}`; POST `…/approve`, `…/reject`, `…/disable`, `…/enable`; DELETE `…?force=` | IA | (phase 3, RFE-022) GET `{id}` of a pending major upgrade carries `impact` (`09 §10`: `connections_needing_reauth`, `by_app`); (phase 4, P4-GRANTS) `impact` also carries `grants_to_convert` (count, by app and workspace) for an upgrade that introduces `grants.custom_only`, and approval then converts them atomically (`09 §10`, reason `converts_grants`); `approve` activates and flips the failing connections to `reauth_required` in one transaction and returns the `PluginPackage` plus `connections_reauth: n` (phase 3, D-107) |
| GET `/v1/admin/host-claims`; PUT `/v1/admin/host-claims/{claim_key}` | IA | conflicts and winners |
| GET/POST/DELETE `/v1/admin/trusted-keys` | IA | |
| GET/POST/DELETE `/v1/admin/connectors`; POST `…/{id}/approve`, `…/{id}/token` | IA | provision token, approve manifest. (phase 4, P4-MCON) `POST` with `managed: true` adds a managed connector (no token; settings, secrets, reviewed digest approved, C-451); PATCH `/v1/admin/connectors/{id}` edits settings, secrets, `enabled` (C-453); `…/token` on a managed connector → 409 `connector_managed` |
| GET `/v1/admin/connector-types` | IA | (phase 4, P4-MCON) catalogue of built-in connector types announced by live connector hosts (C-450) |
| GET/POST/DELETE `/v1/admin/runners`; POST `…/{id}/token` | IA | |
| GET `/v1/admin/storage` | IA | (phase 2, RFE-015) instance storage breakdown (`10 §Attachments`); (phase 4, P4-S3) plus `blobs: { backend, by_backend: [{ backend, attachments, bytes }], s3?: { bucket, prefix, healthy, last_probe_at, sse, versioning } }` (never credentials; counts come from `attachments`, not from listing the bucket) |
| GET `/v1/admin/registry/snapshot` | IA | active snapshot (JSON) |
| POST `/v1/admin/orgs/{org}/freeze` `{ reason }`; POST `/v1/admin/orgs/{org}/unfreeze` | IA | (phase 2, RFE-006) operator kill-switch; `frozen_authority = instance_admin`; org data stays inaccessible |
| GET `/v1/admin/orgs` (`frozen=true` filter, paginated) | IA | (phase 2, RFE-006, D-76) organizations with their freeze state: id, slug, name only |
| GET/POST `/v1/admin/oauth-clients`; PATCH/DELETE `/v1/oauth-clients/{id}` | IA | instance-level OAuth clients (`08 §OAuth clients`) |

### Planes (machine callers)
Runner plane `/v1/runner/*` and connector plane `/v1/connector/*` are specified
in `16 §W5` and `13 §Connector contract`. (phase 4, P4-MCON) Connector-host plane
`/v1/connector-host/*` (`register`, `assignments` long poll, `status`) is specified in
`13 §3.1` and C-455..C-457.

### Relay plane and preview authorization (phase 4, P4-PRV-2, D-146, D-147; C-550..C-559)

All of it is 404 while `previews.enabled` is false.

| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/previews/authorize?host=<label>&state=<nonce>` | S (session cookie, browser navigation) | the first hop of the grant exchange (`04 §Preview access`): uniform 404 page for every refusal, 423 page when frozen, otherwise `302` to `https://<label>.<previews.domain>/_aurora/grant?g=<grant>`; never redirects anywhere else; `Cache-Control: no-store`; no CORS |
| POST `/v1/relay/register`, `/resolve`, `/grants/redeem`, `/streams`, `/validate`, `/report` | `aul_` + replica | the relay plane, `16 §W17` (the API's share/session/stream logic: resolve answers from the same functions as the dashboard's `can_use` and freeze checks, `aurora-db::authz`; **resolve** also sets the wake demand and `preview_start_requested_at`; **report** writes `last_accessed_at` and `preview_active_until`) |
| POST `/v1/runner/preview-streams/open`, `/closed` | `aur_` + replica | redeem a stream grant, `16 §W17` |
| GET `/v1/admin/relays` | S (instance admin) | relay replicas seen (`id`, `version`, `last_seen_at`, `streams_open`), shown on Admin → Overview |

Cookie-authenticated API requests with `Sec-Fetch-Site: same-site|cross-site` that are not a top-level
`GET` navigation are refused 403 `csrf_rejected` while `previews.enabled` (`04 §Preview access`
rule 5); the `/v1/relay`, `/v1/runner` and `/scim/v2` planes use bearer tokens and are not
affected.
