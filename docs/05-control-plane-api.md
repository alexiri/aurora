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
  member sees their own direct ones; owners see every direct one, marked
  `direct_user: {id, display_name}`). (phase 5, P5-REFINE-7, D-409, C-1073) A
  person may have several direct conversations with an agent; each is private to
  that person exactly as the single one was, and the agent's search scope
  (D-230) is "the speaker's own direct conversations" (all of them, never
  another member's).
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
  `{ at, by_label, by: { id, display_name } | null (C-1102), reason, authority, level: "agent"|"workspace"|"org" }`, `level` =
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
    | `message` | `POST /v1/conversations/{id}/messages`, `/v1/turns/{id}/send-now`, (FIX-225) `POST /v1/followups/{id}/steer`, `/cancel`, `PATCH /v1/followups/{id}` | 30 / min, burst 10 |
    | `upload` | `POST /v1/conversations/{id}/attachments`, avatar/skill-file/plugin uploads | 20 / min, burst 5 |
    | `simulate` | `POST …/policy/simulate`, `GET /v1/agents/{agent}/effective` | 20 / min, burst 5 |
    | `search` | `GET …/memories/search` and any future search route | 30 / min, burst 10 |
    | `export` | `GET /v1/conversations/{id}/export`, `POST /v1/admin/storage/refresh`, `POST /v1/orgs/{org}/storage/refresh` (FIX-126: the Storage *pages* are cached snapshots read in the ordinary buckets; only the deliberate Refresh buttons are strict); (phase 5, SET-1, SET-4) `GET /v1/admin/settings/export`, `POST /v1/admin/settings/email/test` | 6 / hour, burst 2 |
    | `files` | (phase 5, P5-FILES-1, D-316) every `/v1/agents/{agent}/files*` route, reads included (instead of `write`/`upload`) | 300 / min, burst 60 |
    | `telemetry` | (phase 5, FIX-146, D-232) `POST /v1/telemetry/traces` (instead of `write`, so browser telemetry never spends the budget of real writes) | 60 / min, burst 20 |
    | `check` | (phase 5, P5-UX-14, C-943, SEC-129) `POST /v1/accounts/{type}/{id}/test`; (P5-REFINE-7, C-1077) `POST /v1/agents/{agent}/schedules/preview` | 10 / min, burst 3 |
    | `invite` | (phase 5, P5-UX-5, C-919, SEC-128) `POST /v1/workspaces/{ws}/access/by-email` (per principal; the route also keeps a per-workspace cap of 30 / hour) | 10 / min, burst 5 |
    | `setup_request` | (phase 5, P5-POLISH-4, C-1030, SEC-135) `POST /v1/workspaces/{ws}/apps/{app}/setup-request` and (P5-APIGAP-2, C-1047) `POST /v1/workspaces/{ws}/requests` (the 21st ask in an hour, both together, is 429 `rate_limited`) | 20 / hour, burst 20 |
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
      mistake an outage for its own rate). Each class has its own breaker and its
      own connection budget; a failure in one class never changes another class's
      verdict (FIX-087).
    - *Metrics:* `aurora_api_rate_limited_total{class,scope}` unchanged; plus the
      `aurora_counters_*` series of `15 §Shared rate-limit counters`.
    - *Cost.* Every authenticated request costs one store round-trip (three
      buckets in one call). Redis is the recommended backend for the API limiter;
      Postgres works for small deployments (≈ hundreds of requests per second in
      total) and is the default shared choice because it adds no infrastructure.
      Operators can keep the API limiter local and share only the gateway with
      `ratelimit.counters.api = "memory"` (`15`).
- **Request id and trace context (phase 5, FIX-146, D-232, C-732).** Every response of the public
  listener (`/v1/**` and `/scim/v2/**`), errors and the 404 fallback included, carries
  `x-request-id: <32 lowercase hex>` — the W3C trace id of the request, also when no tracing backend
  is configured. It is opaque (shown for support, never parsed by clients, not in the body) and is the
  `request_id` of every server log line of that request (`02 §Coding rules`); the dashboard shows it as
  the "Reference" of an error. A request may send `traceparent`/`tracestate`; sending them is never an
  error, and how they are treated depends on the route (SEC-88): parent-based on the machine planes
  (`/v1/runner/*`, `/v1/connector-host/*`, `/v1/relay/*`), trace id and parent span id kept but the
  sampled flag ignored and `tracestate` dropped on every other `/v1` route (the sampling rule of
  C-732 (7) decides), ignored on `/scim/v2/**`, OAuth/OIDC callbacks and webhooks. `/healthz`,
  `/readyz` and `/metrics` get no span and no access line.
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
- **Mismatch**: a replay whose `text`, set of `attachment_ids` or `reply_to.seq` differs from the
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
| `aurora_approval` | `{ "id", "workspace_id", "event": "created"|"decided"|"expired"|"abandoned" }` (FIX-109: `abandoned` for an approval ended by a freeze, by the revocation of what it depended on (FIX-084) or by a gateway that went away, with the status of the same name (FIX-122), and the gateways wake a held request on `decided`, `expired` and `abandoned`; the dashboard treats it like `expired` and shows its own copy) | gateway & API → gateways, API (cards, SSE, long-polls) |
| `aurora_gateway_invalidate` | `{ "scope": "org"|"workspace"|"agent"|"credential"|"connection"|"user"|"all", "id"?: uuid }` (phase 4, P4-GRP-1: `user`, id = the user; drops the cached identities of that speaker) | API → gateways |
| `aurora_registry` | `{ "hash": content_hash, "contract": int }` | API → gateways, API replicas (hook hosts have no DB connection and poll `/internal/v1/registry/active` every 60 s and on cache miss) |
| `aurora_settings` | (phase 5, SET-1, D-236(f)) `{ "section", "generation" }` | API (after a committed settings write, Helm mirroring or CLI reset) → API replicas, gateways: re-read that section, validate the merged view, swap (`15 §Instance settings`); a gateway then drops the caches the section feeds (scope `all` for `governance`, `llm`, `limits`, `secrets`) |
| `aurora_secret_origin` | `{ "secret_id", "status" }` | gateway (origin preflight/backstop) → API (audit, owner notice, dashboard) |
| `aurora_timeline` | (phase 5, P5-REFINE-9, C-1084) `{ "a": agent_id, "s": "turn"\|"request"\|"approval"\|"action_approval"\|"run"\|"delivery"\|"peer"\|"access", "i": id, "q"?: int }` (≤ 200 bytes, never text) | API (schedule runs, webhook deliveries, peer tasks, action approvals, access requests: in the writer's transaction), gateway (one `request` hint per agent after each request-log flush, `q` = the batch's oldest `ts` in µs) → API replicas (the agent's open timeline streams reload that row) |
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
| Approval expiry: `pending` past `deadline + 30 s` → `expired` (phase 5 (D-138, moved from phase 4), RFE-011b: a **parked** approval expires at `deadline`, nobody holds it); pending rows of gateway instances unseen for 60 s → `abandoned` (phase 4: **never** `parked` rows) | every 10 s | idempotent updates |
| Approval resume window (phase 5 (D-138, moved from phase 4), RFE-011b): `resume_state = 'ready'` past `resume_expires_at` → `expired` (audit best-effort, transcript notice, no turn); parked approvals at 50 % of their time → one reminder (C-828: e-mail claimed in `approval_notifications` with `stage = 'reminder'`, push in `approval_push_claims`); runs whatever `approvals.park.enabled` says now (D-264(b)) | every 10 s | idempotent updates, claims (`approval_notifications`, `approval_push_claims`) |
| Web Push delivery (phase 5 (D-138, moved from phase 4), RFE-011b, `07 §Web Push`): on `aurora_approval created` + `APPROVAL_PUSH_DELAY`, deciders with a live subscription and no watcher; failure handling, subscription disabling, retired-VAPID-key destruction (`PUSH_VAPID_RETIRE`); (phase 5, P5-SCHED-ONCE, C-1095) the same loop pushes `schedule_answer` once to each opted-in device of a schedule's creator when a one-time run that happened in their own chat (C-1093) ends `done` (claim `schedule_runs.answer_pushed_at`; runs older than 1 h never) | event + every 10 s catch-up; daily | claims (`approval_push_claims`, `schedule_runs.answer_pushed_at`) |
| Action-approval expiry | every 60 s | idempotent |
| Emergency-access expiry sweep (phase 5, FIX-236, C-1164): active `workspace_emergency_access` grants past `expires_at` get `ended_at = expires_at`, `end_reason = 'expired'` and one critical `workspace.emergency_access.expire` audit row each, in the same transaction; use never waits for this job (every check evaluates `expires_at > now()` itself); (C-1164) the nightly retention pass also deletes grant rows older than 400 days | every 60 s | idempotent |
| Pending workspace access expiry (phase 5, P5-UX-5, C-920): delete `workspace_pending_access` rows past `expires_at` (`PENDING_ACCESS_TTL`); an expired row is also never listed or applied | every 60 s (with the action-approval sweep) | idempotent |
| Attachment pending sweep (`ATTACH_PENDING_TTL`) | hourly | idempotent |
| Conversation retention sweep (phase 2, RFE-010): delete conversations past the org window through the delete path; blob-orphan sweep | daily | singleton |
| Attachment sweep (phase 2, RFE-015): purge attachment bytes past the org window, remove orphan blobs (referenced by no attachment row and no agent avatar, FIX-223, D-466), reconcile `org_storage_usage`, quota notices; (phase 6 (D-318, moved from phase 5), P4-S3) with the `s3` backend the orphan step lists the bucket prefix and honours `S3_ORPHAN_GRACE` (`10 §Attachments`) | daily | singleton |
| SCIM token expiry (phase 4, P4-GRP-2, FIX-081): an audit row (the in-app notice) and an e-mail to the org's owners and admins — the channel quota notices use, where a mailer is configured — 14 days and 1 day before a token expires, once per token and threshold; expired tokens are simply dead (read-time check) | daily | singleton |
| Counter pruning (phase 4, P4-CNT, `postgres` backend only): delete dead `rate_limit_windows` / `rate_limit_buckets` rows | every `COUNTER_PRUNE_INTERVAL` (60 s) | `SKIP LOCKED` batches |
| Blob store probe (phase 6 (D-318, moved from phase 5), P4-S3, `s3` backend only): `HeadBucket` + versioning/encryption state, sets `aurora_blob_store_up` | every 30 s | per replica |
| Skill library refresh (phase 4, P4-GSL, `10 §Skills`): libraries with at least one added skill and `next_refresh_at <= now()` and `status = active` are claimed with `FOR UPDATE SKIP LOCKED` and a per-library lease (`LIBRARY_LEASE` 5 min, `refresh_lease_until`/`lease_owner`), at most `LIBRARY_REFRESH_CONCURRENCY` (4) per replica; resolve → compare head → tree → blob comparison → proposed updates, read by the API itself over git smart HTTP (P5-SKLIB-1, C-1125; the gateway's library fetch is removed); (P5-SKLIB-2, C-1128) one pass per library, also for an instance library, with the open-proposal budget per organization **of the skill** under that org's lock: an org over budget gets a `skill_library_org_state` row and no proposals, the row is deleted when it is back under, and a library with such a row skips the unchanged-head short-circuit; errors back off 15 min → 1 h → 6 h; never writes skill content (only `skill_library_updates`, `skill_libraries`) and never bumps a home-sync generation | every 60 s | claims (lease) |
| Skill library retention (phase 4, P4-GSL): prune decided `skill_library_updates` older than 90 days; expire leases (P5-SKLIB-1: no longer marks libraries by their connection; libraries have none) | daily | singleton |
| Preview housekeeping (phase 4, P4-PRV-1/2): delete expired `preview_sessions`, `preview_grant_uses` and `preview_stream_grants`; clear stale `sandboxes.preview_wake_at`/`preview_start_requested_at`; count shares per org for `aurora_preview_shares` | every 60 s | idempotent (`SKIP LOCKED` batches) |
| Request-log partition maintenance and retention | daily | singleton |
| Retention pruning (inbox/ops/cards > `CHANNEL_LEDGER_RETENTION`, used action tokens, expired email/CLI tokens, sessions, expired context credentials) (FIX-111: implemented for terminal `channel_inbox`/`channel_ops` rows (a pending deprovision op survives), settled `channel_cards`, expired `link_requests`, sessions and context credentials past `SESSION_PRUNE_GRACE`, `webhook_deliveries` past `WEBHOOK_DELIVERY_RETENTION` or over `WEBHOOK_DELIVERY_MAX_PER_WEBHOOK`, `cli_logins` past `SESSION_PRUNE_GRACE` after expiry, and (phase 5, P5-SCHED-ONCE, C-1092) Done one-time schedules whose `completed_at` is older than `SCHEDULE_DONE_RETENTION`, deleted through the ordinary schedule delete (their run threads are marked as from a deleted automation, C-1074); used action tokens are pruned by the card fan-out, `email_tokens` are no longer written (OIDC-only sign-in); each statement deletes at most `RETENTION_PRUNE_BATCH` rows) | daily | singleton, `SKIP LOCKED` batches |
| Runtime daily rollup (phase 5, D-138; **built in P5-USAGE-1**, D-359, C-1002: `jobs::rollup_runtime_daily`, which also runs at start-up) a watermark run that recomputes every UTC day touched by an open interval or one changed since the last run) | hourly | singleton |
| Eval run due-work arm (phase 5, P4-EVAL, D-290, C-865): claims `queued` and `running` `eval_runs` under `lease_until`, resumes at the next unfinished question (same `source_event_id`), re-checks authority and freeze before each question | every 5 s (with `aurora_work`) | claims |
| Eval schedule fires (phase 5, P4-EVAL, D-293): the schedules engine's due-work arm for `eval_schedules` (`SCHEDULE_FIRE_LEASE`, fire-time authority, `skipped_frozen`, `skipped_busy`) | as the schedules arm | claims |
| Calendar invitation retention (phase 5, P5-CALINV, D-274; part of the retention pass above): `calendar_invitations` 90 days after the event ended or last changed (a repeating series: 5 years after its last change), `calendar_invitation_sends` after two days; batched like the others | daily | singleton |
| Eval retention (phase 5, P4-EVAL, D-294): delete finished `eval_runs` older than `EVAL_RUN_RETENTION` through the conversation delete path (session purge included) | daily | singleton |
| Plugin directory scan | startup + 60 s | singleton |
| Snapshot compile & activate | on package state change | singleton |
| Peer-task and reach expiry (FIX-111: `approved` reach grants past `expires_at`, set by `expires_in` on the decision, become `expired`) | every 60 s | idempotent |

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
- (phase 5, P5-REFINE-20 item 14 follow-up, C-1100) `turn.finished` carries `ai_key: "starter" | "organization" | "workspace"`
  when the dispatcher recorded the key the turn ran on (`turns.model_basis.key_id`): `starter` is an organization key the
  instance's starter grants give every new agent, `organization` any other organization key, `workspace` a key of the agent's
  workspace. A class, never a name or an id, so whoever may read the transcript may read it; absent for a turn dispatched
  before this change, for the scripted `fake` harness and when the key is gone.

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
| (phase 5, SET-1) Instance settings change of `governance`, `llm`, `limits` or `secrets` | (`aurora_settings` instead: each gateway reloads the section and then flushes as for `all`) |
| Snapshot activation | (`aurora_registry` instead) |

## Route catalogue

Auth column: **S** session, **K** API key (workspace or org), **SO** session
only, **IA** instance admin (session), **A** agent token, **R** runner, **C**
connector, **P** public capability. Permission checks per `04`.

**Outside the OpenAPI document (by design).** `web/openapi.json` describes the routes a dashboard or API-key
client calls. These served routes are not in it — machine planes (their own tokens), SCIM, and the browser
WebSocket — and each is listed here so a catalogue row without an OpenAPI entry is either one of them or
unbuilt (phase-tagged). `scripts/guards/docs-routes.sh` checks every catalogue row against this table, the
OpenAPI document and the phase tags (FIX-119).

| Route | Why it is outside |
|---|---|
| GET `/v1/agents/{agent}/terminal` | WebSocket upgrade (C-616): not describable as an OpenAPI operation. Its auth, close codes and refusals are specified only in `API_CONTRACTS` C-616..C-625 and `16 §W19`; the OpenAPI drift test (`openapi_matches_checked_in`) cannot see a change here, so the terminal integration tests (`api it/terminal.rs`) are the only guard |
| `/scim/v2/**` (Users, Groups, ServiceProviderConfig, ResourceTypes, Schemas) | SCIM 2.0 plane (per-org `aud_` bearer token, SCIM error bodies, `16 §W15`, C-470..C-479): a protocol of its own, not the dashboard API. A change to it is not caught by the OpenAPI drift test; the `scim_*` integration tests and the IdP fixtures (`tests/fixtures/scim/`) are the guard. The token and settings *management* routes under `/v1/orgs/{org}/scim…` are ordinary dashboard routes and are in the document |
| GET `/v1/previews/authorize` | browser navigation redirect of the preview origin flow (C-540..C-549) |
| POST `/v1/relay/register` | relay plane (`aul_` token, C-550..C-559) |
| POST `/v1/runner/preview-streams/open` | runner plane (`aur_` token) |
| POST `/v1/terminator/register` | (phase 5, P4-SSH-2) terminator plane (`aut_` token, C-794); also `/sessions`, `/sessions/{id}/channels`, `/sessions/{id}/close`, `/heartbeat` |

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
| GET `/v1/auth/oidc/{provider}/start`, `/callback` | none | `04 §OIDC`; (phase 1, RFE-004) the only sign-in; `start` accepts `reauth=1`; (FIX-152, C-890) a failed re-authentication redirects to its `return_to` with `reauth_error=<code>` instead of `/auth/login?error=<code>` |
| POST `/v1/auth/cli`, `/v1/auth/cli/token` | none | device flow |
| GET `/v1/auth/cli/pending?code=` | SO | (FIX-108) `{ hostname, requested_at, expires_at, address?, key_expires_in_days? }` for the approval screen; 404 unknown/expired, 400 malformed code |
| POST `/v1/auth/cli/approve` | SO | `{ user_code, workspace_id }` |
| GET/PATCH `/v1/me/onboarding` | S | (phase 4, P4-ONB-2, D-141) the caller's onboarding progress: `{ welcome_seen, guide_dismissed: [workspace_id], tour_completed_at }`; guide step completion itself is computed by the client from real state (model readiness, grants, conversations), never stored; (phase 5, P5-IA-1, C-987) `notices_seen: [key]`, PATCH `{ notice_seen: key }` appends one; (phase 5, P5-REFINE-3, C-1056) `more_controls` (the person's Show more controls switch), PATCH `{ more_controls?: bool }`; it never authorizes anything |
| GET/PATCH/DELETE `/v1/me` | S (DELETE: SO) | (phase 1, RFE-004: DELETE takes no body; (phase 5, FIX-157, D-317) the recent OIDC re-authentication it needed is removed, no 412 `reauth_required`) profile, memberships (phase 2, RFE-012: with the effective role per workspace), default org; (phase 2, RFE-011a) `PATCH { approval_email: bool }`; (FIX-167, C-1024) `admin_attention_email: bool` (default `true`; instance admins only use it); (phase 2, C-195) `client_address: string \| null`, the caller's address as the API sees it behind `server.trusted_proxies`; (phase 5, P5-PERSON, D-402, C-1054) `timezone: string \| null` (the person's IANA time zone; the turn context block's clock, C-1053), `PATCH { timezone: string \| null }` (`null` clears; not a `chrono-tz` name → 422 `value_invalid`); (phase 5, P5-REFINE-5, D-407, C-1064) a `PATCH { display_name }` that **changes** the name sets `users.display_name_edited_at`, so the sign-in never overwrites it again; `Me.display_name_source` is `sign_in` (the token's `name`, refreshed at every sign-in) or `you` |

### Organizations, members, keys
| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/orgs` | S | list mine / create |
| GET/PATCH/DELETE `/v1/orgs/{org}` | S/K | (phase 5, P5-UX-2, D-349, C-981) GET adds `contacts: { owners, admins }` (up to 3 display names each, readable by every member) |
| GET `/v1/orgs/{org}/members`; PATCH/DELETE `/v1/orgs/{org}/members/{user}` | S/K | role, suspend; (phase 2, RFE-021) PATCH (suspend) and DELETE accept `reassign_to` (user id or `"workspace"`) to reassign the person's automations atomically |
| GET `/v1/orgs/{org}/members/{user}/offboarding`; GET `/v1/workspaces/{ws}/members/{user}/offboarding` | S/K | (phase 2, RFE-021) what removing the person would disable: `{ schedules: [Item], webhooks: [Item], peer_tasks: [Item], api_keys: n }`, `Item = { id, name, agent, workspace, enabled, anchor, next_fire_at?, would_disable }`; org view org owner/admin, workspace view `can_administer` (FIX-234, C-1160) |
| GET/POST `/v1/orgs/{org}/invitations`; DELETE `/v1/invitations/{id}` | S/K | (phase 2, RFE-012) body may carry `approver_workspace_ids` besides `workspace_ids`; (phase 5, P5-REFINE-6, C-1069, SEC-140) the `201` carries `link` (only there, never in a list or `GET`) |
| POST `/v1/invitations/{id}/link`; POST `/v1/invitations/{id}/resend` | S/K | (phase 5, P5-REFINE-6, D-418, C-1069, SEC-140) owners/admins of the invitation's organization (another organization's or an unknown id: 404). `link` re-issues the link: `200 { link, expires_at }`, `Cache-Control: no-store`, the old link stops working, the expiry is reset to 7 days. `resend` re-issues and e-mails it: `202 Invitation` (no link); `409 mail_disabled` when mail is log-only. Both: `409 invitation_not_pending` (accepted, cancelled, expired), 20 per organization per hour (429), audited `invitation.link` / `invitation.resend` (id and e-mail only) |
| GET `/v1/invitations/preview?token=`; POST `/v1/invitations/accept` | none / SO | |
| GET/POST `/v1/orgs/{org}/api-keys`; DELETE `/v1/api-keys/{id}` | S (POST: SO) | org keys |
| DELETE `/v1/me/api-key` | K | (FIX-140, C-720) revokes the API key that authenticates this request, and no other: 204; 400 `not_an_api_key` for a session caller; a revoked/expired key never reaches it (401). Audited `api_key.revoke` with `metadata.self = true`. Used by `aurora logout` |
| GET `/v1/orgs/{org}/audit` | S/K | (phase 2, RFE-008) filters `actor_id`, `action`, `critical=true` (C-290); rows carry `critical` (C-291); (phase 5, P5-REFINE-6, C-1088) `include_system=false` leaves out system events (`actor_kind = system`) and answers `hidden_system` (their count); absent or `true` lists them (also on the workspace audit list); (phase 5, P5-REFINE-20, workspace-3) every `AuditEntry` (this route, the workspace route and `/v1/admin/audit`) carries `actor_name: string \| null`: the acting person's current display name when `actor_kind` is `user`, `instance_admin` or `service` (a key acts as its owner) and the user still exists, else `null` (`actor_label` stays the recorded label, an e-mail for a person; the dashboard names people by `actor_name`, never by e-mail) |
| GET `/v1/orgs/{org}/agents` | S/K | (phase 5, P5-REFINE-6, C-1090) org owners/admins: every agent of the organization, personal workspaces included, paged (`limit`, `cursor`): `{ items: [{ id, name, handle, workspace: { id, slug, name, personal_of }, kind: hosted\|byo, state: paused\|needs_key\|failed\|working\|starting\|asleep\|ready, last_active_at }], next_cursor }`; (D-425) `state` is the agent header's state in its precedence, `last_active_at` the newest turn's time |
| POST `/v1/orgs/{org}/freeze` `{ reason }`; POST `/v1/orgs/{org}/unfreeze` | S/K | (phase 2, RFE-006) org `owner`; (FIX-244, D-486) the organization's other effective owners are told after the commit (an idempotent re-freeze tells nobody) |
| GET/PUT `/v1/orgs/{org}/app-availability` | S/K | posture + allow-list |
| GET/PUT `/v1/orgs/{org}/governance` | S/K | org governance document (`08 §11.2`) with `revision`; PUT validates against the instance ceiling (422 `governance_invalid` / `governance_below_instance_minimum`) |
| GET `/v1/orgs/{org}/governance/effective` | S/K (any org member) | effective LLM and egress governance (instance ∩ org), each value marked `source: instance|org` (phase 1, RFE-003: includes `egress.open_lane`; phase 2, RFE-025: includes `byo.{posture, source_cidrs, require_context_credentials}`; phase 3, RFE-013: `agent_tokens.max_age_days`); non-compliant LLM secrets of the org |
| GET `/v1/governance/instance` | S | the instance ceiling (config, without secrets) — read-only; (phase 5, SET-2) the effective `governance` and `llm` settings (helm or dashboard), each clamp naming its setting (C-783) |

### Workspaces
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/orgs/{org}/workspace-overrides` | S/K | (phase 5, P5-IA-2, D-339, C-964) org owner/admin: per workspace, what it tightened in Internet access below the organization (requests without an account, browser connections, default rule, rule count); only differing workspaces unless `?all=true`; paginated. Read from each workspace's **published** generation (an unapplied draft is not a difference); a workspace differs when requests without an account are `block`, browser connections are not `enabled`, or the default rule is not `none`; `rule_count` is informational. `personal_of` is present on a personal workspace |
| GET/POST `/v1/orgs/{org}/workspaces` | S/K | (phase 5, P5-UX-11, C-947) items carry `personal_of?: { id, name }` |
| GET/PATCH/DELETE `/v1/workspaces/{ws}` | S/K | name, timezone, memory_language; (phase 2, RFE-025) PATCH also `byo_posture` (`enabled`\|`owners_only`\|`disabled`\|null = inherit; own `can_own`; tighten-only, 422 `governance_below_instance_minimum`; audited `workspace.byo_posture.update`; invalidation scope `workspace`); GET shows `byo_posture` and `effective_byo_posture` with its `level`; (phase 5, P5-UX-2, D-349, C-981) GET adds `contacts: { owners }` (up to 3 display names, readable by every member); (FIX-234, C-1160, C-1162) GET works for `can_administer` too: `my_role` is `null` without own access, `administers: true` marks an administrator, and `audience: { people, owners (≤5), owners_total }` names the own-right owners; PATCH stays own `can_own`, DELETE `can_administer` |
| GET `/v1/workspaces/{ws}/access`; PUT/DELETE `/v1/workspaces/{ws}/access/{user}` | S/K | bindings (phase 2, RFE-021: DELETE accepts `reassign_to`); (phase 2, RFE-012) `PUT { role: "owner"\|"approver"\|"member" }`; (FIX-234, C-1160) `can_administer` (in a personal workspace the adds and raises are the person's alone, C-1161; the person is never lowered, suspended or removed there: 409 `personal_workspace_person`). (phase 5, P5-APIGAP-4, C-1050, C-1051) every `AccessBinding` (and every `derived_members` row) carries `joined_at` (timestamp or `null`) and `status: "active"\|"suspended"`; a suspended binding is still listed. `PUT` on a suspended binding changes the role and leaves it suspended. (phase 5, P5-REFINE-6, C-1086) every `AccessBinding` carries `signed_in` (`users.last_login_at` is set); (FIX-234) `implicit` is always `false` and no implicit rows are returned |
| PATCH `/v1/workspaces/{ws}/access/{user}` | S/K | (phase 5, P5-APIGAP-4, D-399, C-1051, SEC-138) `can_administer` (C-1160). `{ status: "active"\|"suspended" }` suspends or restores the person's **direct** binding in this workspace only → 200 `AccessBinding`; the same status again is a no-op (200, no audit row). 404 when the person has no direct binding; 409 `cannot_suspend_self`; 409 `personal_workspace_person` (FIX-234, C-1161: a personal workspace's person is never suspended there); 409 `managed_by_idp` (an IdP-claimed binding, suspend only; restore is always allowed); 409 `last_owner` (the only active owner). While suspended the person fails `can_use`/`can_approve`/`can_own` in this workspace on every surface (their direct **and** group-derived access; `04 §The access law`). Audited `workspace_access.suspend` / `workspace_access.restore` (security-critical) |
| POST `/v1/workspaces/{ws}/access/by-email` | S/K | (phase 5, P5-UX-5, D-325, C-919, SEC-128) `can_administer` (C-1160; a personal workspace: only its person, 403 `personal_workspace_owner_only`); `{ email, role }`: an active org member is bound at once, anyone else gets a pending row; one exact address, rate class `invite`; `{ result: "added"\|"pending", org_entry: "member"\|"open_signup"\|"invitation_needed" }` (`member`: added at once; the other two say how a waiting person gets into the organization); 422 `owner_requires_member` (owner for a non-member), 422 `pending_access_limit`; also capped at 30 per hour per workspace |
| GET `/v1/workspaces/{ws}/access/pending`; DELETE `/v1/workspaces/{ws}/access/pending/{id}` | S/K | (phase 5, P5-UX-5, C-920) `can_administer` (C-1160): pending access rows, applied at the person's first sign-in into the org with a verified matching e-mail, 30-day expiry |
| GET `/v1/workspaces/{ws}/access?expand=groups` | S/K | (phase 4, P4-GRP-1) opt-in expansion for `can_administer` (C-1160): adds `group_bindings: [{ group: {id, name, source, member_count}, role }]` and `derived_members: Page<{ user, role, sources: [{ kind: "direct"\|"group", group? }] }>` (the effective role of everyone who has access; only own-right sources appear since FIX-234 — an org role or an `admin` group mapping is no source; a person with several sources appears once). Without `expand` the response is unchanged |
| PUT/DELETE `/v1/workspaces/{ws}/access/groups/{group}` | S/K | (phase 4, P4-GRP-1) `PUT { role: "owner"\|"approver"\|"member" }` binds or changes the role of a group; `can_administer` (C-1160; a personal workspace: adding or raising is its person's alone, C-1161); 404 for a group of another org, 422 `group_binding_limit_reached`; DELETE is idempotent; both audited (`group_access.set\|remove`), invalidation scope `workspace` |
| GET/POST `/v1/workspaces/{ws}/api-keys` | S (POST: SO) | workspace keys |
| GET `/v1/workspaces/{ws}/audit` | S/K | same filters |
| GET `/v1/orgs/{org}/storage` | S/K | (phase 2, RFE-015) org `owner`/`admin`: usage vs quota, breakdown by workspace and direction; (FIX-126) the file counts and breakdown come from a cached snapshot (`computed_at`), `used_bytes`/`notice` stay live; (phase 5, P5-REFINE-6, C-1072) `categories: [{ id: conversations\|files\|activity, bytes }]`, always the three: `files` is `used_bytes`, the other two `null` (not computable per organization) |
| GET `/v1/orgs/{org}/usage` | S/K | (phase 5, P5-USAGE-1, D-360, C-1006) org `owner`/`admin`: the organization's usage, `?from=&to=&workspace_id=` as C-1000 → `{ from, to, totals, by_day, by_workspace (top `USAGE_TOP_MAX`), other_workspaces, by_model, runtime_as_of }`; tokens, turns, turn time and awake time; no cost |
| POST `/v1/orgs/{org}/storage/refresh` | S/K | (FIX-126) org `owner`/`admin`: recompute the org's snapshot now, same body as the GET; `export` class |
| GET/PUT `/v1/orgs/{org}/retention` | S/K | (phase 2, RFE-010/015) org `owner`/`admin`; `{ conversations_days?, attachments_days?, attachment_quota_mib? }`, `null` = none; 422 `retention_invalid` / `retention_above_instance_maximum` |
| GET `/v1/workspaces/{ws}/activity` | S/K | request log, filters: agent, conversation, decision, host, app |
| GET `/v1/workspaces/{ws}/usage` | S/K | (phase 5, D-138; **built in P5-USAGE-1**, with the hourly "Runtime daily rollup" of `§Background jobs` and the token aggregation of `10`) runtime + tokens; (phase 5, P5-USAGE-1, D-359, C-1000) `?from=&to=&agent_id=` (UTC dates, inclusive, default the last 30 days, at most `USAGE_RANGE_MAX_DAYS`; 422 `usage_range_invalid`) → `{ from, to, totals, by_day, by_agent, by_model, runtime_as_of }` with turns, turn time, awake time and the four token counters; read from the `usage_daily` ledger and `runtime_daily`; no cost (D-359(a)); (FIX-234, C-1160) readable by `can_administer` too |
| POST `/v1/workspaces/{ws}/freeze` `{ reason }`; POST `/v1/workspaces/{ws}/unfreeze` | S/K | (phase 2, RFE-006) `can_administer` (FIX-234, C-1160; `frozen_authority` stays `org_admin` for an administrator); as for agents; (FIX-244, D-486) the workspace's other own-right owners — else the organization's other effective owners — are told after the commit |
| POST `/v1/workspaces/{ws}/emergency-access` `{ reason, hours? }` → 201 `EmergencyAccess`; GET `/v1/workspaces/{ws}/emergency-access`; POST `/v1/emergency-access/{id}/end`; GET `/v1/orgs/{org}/emergency-access` | S | (phase 5, FIX-236, D-478, C-1164) break-glass: an active effective org **owner** opens time-limited (1–24 h, default 4) read-only access to one workspace, with a reason (10–500 chars); org admin, member, instance admin and API keys are refused (403 `org_owner_required` / 403 `session_required`), own `can_own` there 409 `already_has_access`, one active per workspace and person 409 `emergency_access_active`; 422 on `/reason` and `/hours`; rate class `invite`; audited `workspace.emergency_access.open` (critical, in the transaction), then notices (C-1165). End: the opener or any active org owner, idempotent, critical `workspace.emergency_access.end`. Lists: own access or `can_administer` (workspace), org `owner`/`admin` (org); metadata only, newest first, paged. Expiry: a minute sweep sets `expired` (critical row), but every use checks `expires_at > now()` itself; an opener who stops being an owner ends with `access_lost` in the same transaction. Rows kept 400 days |
| (the C-1165 read routes: the workspace GET, its agents list and detail, the conversation lists and folders, `GET /v1/conversations/{id}` + events + timeline + read stream, attachment download, export, workspace search, activity, memory, schedules and webhooks (lists, detail, runs, deliveries), skills, `GET …/access`) | S/K | (phase 5, FIX-236, C-1165) each takes `ReadAuth = Own(WorkspaceAuth) | Emergency(EmergencyGrant)` through `Caller::workspace_read`: the caller's own access, or — **session only** — an active emergency grant while its holder is still an active org owner. The grant never satisfies `can_use`, `can_approve`, `can_own`, `can_administer` or `can_ssh`, and no write route accepts it. Every response carrying conversation content writes `conversation.emergency_view` before the body (a failed write is 503 `audit_unavailable`); once per (actor, conversation, hour); an export writes `conversation.export` with `via: "emergency"` too. The full list is enforced by the route-inventory test |

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
| GET `/v1/groups/{id}/impact?remove_member=<user>&unbind_workspace=<workspace>` | S/K | org `owner`/`admin`: what deleting the group (or, with `remove_member`, removing that member; with `unbind_workspace`, removing its binding to that one workspace) would take away, `{ workspaces: [{ workspace, users_losing_access: [...≤ GROUP_IMPACT_LIST_MAX], total, schedules_disabled, webhooks_disabled, api_keys }] }` |
| GET `/v1/me` | S/K | (phase 4) also `groups: [{ id, name, org_id }]` and, per membership, `effective_role` with `sources` (`role` stays the direct role); (phase 5, P5-REFINE-6, C-1072) `can_create_org`: an instance admin, or `auth.org_creation = anyone` (exactly when `POST /v1/orgs` works) |
| GET `/v1/me/chat-platforms` | S/K | (phase 5, P5-REFINE-6, C-1089) the chat platforms the caller can link: `{ platforms: [{ kind, display_name, bot_handle, linked }] }` from the active integrations of their organizations (an API key: its own organization), one per kind; no workspace, channel or token data |

(phase 4, P4-GRP-1) The offboarding views of `§Organizations, members, keys` add
`via_groups` to each row; `GET /v1/agents/{agent}/effective` and policy `simulate` take
the speaker's groups into account (`07`).

### SCIM management (phase 4, P4-GRP-2, D-132)
Org `owner` only (`04 §SCIM`); every write is security-critical audited. The SCIM plane
itself (`/scim/v2`, token auth, SCIM errors) is specified in `16 §W15`, not here.

| Method & path | Auth | Purpose |
|---|---|---|
| GET/PUT `/v1/orgs/{org}/scim` | S (PUT: SO) | `{ available, enabled, email_domains: [domain], exclusive, base_url }` (`available` = instance `scim.enabled`; `base_url` = `<external>/scim/v2`); PUT validates domains (422 `scim_domain_invalid`, `scim_domain_not_allowed`, `scim_domains_required` when enabling) and needs no re-authentication ((phase 5, FIX-157, D-317) the one it needed to turn SCIM on is removed, no 412 `reauth_required`); 409 `scim_unavailable` when the instance switch is off |
| GET/POST `/v1/orgs/{org}/scim/tokens`; DELETE `/v1/scim/tokens/{id}` | S (POST: SO) | list without material (`name`, `display_prefix`, `created_at`, `expires_at`, `last_used_at`, `expiring` within 14 days); POST `{ name, expires_in_days? }` → the `aud_` token once; 422 `scim_token_limit_reached`, `scim_not_enabled`; (phase 5, FIX-157, D-317) no re-authentication, so no 412 `reauth_required`; DELETE is idempotent and effective at once |
| GET `/v1/orgs/{org}/scim/status` | S | Directory sync page: `{ last_request_at, users: {active, suspended, removed}, groups, errors: [≤ SCIM_ERROR_LOG sanitized { at, method, resource, status, scim_type }] }` |

Offboarding by SCIM is visible through the existing offboarding views
(`GET /v1/orgs/{org}/members/{user}/offboarding`, RFE-021), which keep working for a
`suspended` member and now carry `byo_agents_created` (id, name, workspace) next to the
schedules and webhooks; members created or managed by SCIM carry `directory: {
state, external_id? }` in `GET /v1/orgs/{org}/members`.

### Secrets and LLM
| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/workspaces/{ws}/secrets`, `/v1/orgs/{org}/secrets` | S/K | create: name, kind, llm fields or host/path/injection, `value` or `reference`; (phase 5 (D-138, moved from phase 4), P4-LLM) for `aws_sigv4` the `value` is the JSON `{ access_key_id, secret_access_key, session_token? }` and for `gcp_service_account` the key-file JSON, validated for shape, `token_uri` and key type (422 `llm_key_invalid`) before any provider call; the same validation, governance and origin steps as every LLM secret (`08 §LLM secrets`); (phase 5, P4-LLM spec batch, D-261, C-801) `aws_assume_role` takes `{ access_key_id, secret_access_key, role_arn, session_duration_secs? }` and `azure_entra` `{ tenant_id, client_id, client_secret }`, validated by a live `AssumeRole` / Entra mint (`details.reason` names the failing step); (phase 5, P5-UX-2, C-988) POST accepts `grant: { agent_ids }`, written in the same transaction, so a website or an AI key is added with access in one request |
| GET/PATCH/DELETE `/v1/secrets/{id}` | S/K | metadata only; values never returned |
| PUT `/v1/secrets/{id}/value` | S/K | replace value |
| POST `/v1/secrets/{id}/accept` | S | agent proposal: `{ value }` → secret `active` + grant to the proposing agent |
| POST `/v1/secrets/{id}/reject` | S/K | agent proposal → `rejected`; a notice goes to the proposing conversation |
| GET `/v1/secret-stores` | S/K | configured stores: id, kind, capabilities (no config) |
| GET `/v1/llm/providers` | S/K | registry + configured endpoints; (phase 5 (D-138, moved from phase 4), P4-LLM) each provider lists its `auth_modes` with their value shape (`string`, `aws_sigv4` fields, `gcp_service_account` file), each endpoint its `dialects`, `region`/`project_id`/`location` where they exist and `models_source` (`listed` \| `static`); never credentials or endpoint internals. (phase 5, P4-LLM spec batch, C-802) Auth modes gain `aws_assume_role` (the key form shows the org's `external_id` from `Org`, for the role's trust policy) and `azure_entra`; each endpoint gains `harnesses` (the enabled harnesses it serves, D-260(a)) and `recommended_auth_mode`. (FIX-105) An endpoint's `base_url` is **not secret**: it is the address agents' clients already dial through the proxy and the dashboard shows when keys are added; it is returned to every authenticated caller (S/K), and an operator who must keep an internal address private fronts it with a public name |
| GET `/v1/secrets/{id}/models` | S/K | model catalogue for an LLM secret (only allowed models) |
| POST `/v1/secrets/{id}/verify` | S/K | re-run key-origin verification (`08 §11.3`) and return `origin` |

### Apps, connections, OAuth clients
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/apps?workspace_id=` | S/K | catalogue with availability and connection counts; (phase 5, FIX-154, C-895) `credential_free`; (phase 5, P5-POLISH-3, D-388, C-1029) `readiness` (`ready` \| `needs_admin`, with `needs`, `who_can` and, after P5-POLISH-4, `requested`), the caller's givable `accounts`, `summary`; apps switched off are omitted for callers who cannot change availability |
| GET `/v1/apps/{app}` | S/K | about, connect methods (form schemas), scopes, tools, agent docs; (phase 5, FIX-154, C-895) `credential_free`; (phase 5, P5-POLISH-3, D-387, C-1031) `summary`, `first_prompt`, per access level `permission_hints` |
| POST/DELETE `/v1/workspaces/{ws}/apps/{app}/setup-request`; GET `/v1/orgs/{org}/app-setup-requests?status=open`; POST `/v1/orgs/{org}/app-setup-requests/{id}/decline` | S/K | (phase 5, P5-POLISH-4, D-386, C-1030, SEC-135) a person asks the organization's owners and admins to make an app ready (idempotent per person, no free text, 20 asks per person per hour); owners and admins list and decline; requests resolve by themselves when the app becomes ready |
| POST/GET `/v1/workspaces/{ws}/requests`; DELETE `/v1/workspaces/{ws}/requests/{id}` | S/K | (phase 5, P5-APIGAP-2, D-399, C-1047, SEC-135) the same ask with a `subject` (`app`, `chat_platform`, `ai_key`): any active member of the workspace asks (class `setup_request`, 20 an hour), lists their own open asks and withdraws; 409 `already_available`; owners and admins (and, for a chat platform no instance connector serves, instance admins by notice) are told; the org list and decline above cover every subject |
| (phase 5, P5-REFINE-8, D-417, D-407, C-1081, SEC-135 amended) the bodies above | S/K | both asks take an optional `reason` (1..300 characters after trimming, control and invisible formatting characters removed, line breaks kept; 400 `invalid_request` at `/reason` above 300); a repeat ask with a new reason replaces the person's reason (no new notice, no new audit row). The decline takes an optional `{ message }` (same rules); the askers' notice names the role ("An admin isn't setting up <Thing> now"), never the decliner. `GET /v1/orgs/{org}/app-setup-requests?status=history` (closed in the last 30 days, newest first, every level) and `GET /v1/workspaces/{ws}/requests?status=open\|history` (the caller's own, each with only their own ask; never the admin's name). `AppSetupRequest` gains `level`, `message`, `closed_by_role`, `closed_by` (owners, admins, instance admins only); `Asker` gains `reason` |
| GET `/v1/admin/requests?status=open\|history`; POST `/v1/admin/requests/{id}/decline` | IA | (phase 5, P5-REFINE-8, D-417, C-1081) the setup requests whose resolving level (C-1080) is `administration`, of every organization; the decline takes `{ message? }` and stores `closed_by_role: "instance_admin"`. A request of the organization level is 404 here, and an Administration-level one is 404 on the organization's decline route |
| GET `/v1/apps/{app}/icon?variant=light|dark`, `/v1/apps/{app}/guide` | S/K | sanitized PNG; setup guide markdown; (phase 5, P5-UX-13, D-334, C-940) the guide takes `?workspace_id=\|org_id=` and fills `{{aurora.*}}` placeholders for the caller (callback URL, external id for org members, links to pages at the caller's level) |
| GET `/v1/apps/{app}/setup?workspace_id=\|org_id=` | S/K | (phase 5, P5-UX-2, D-330, C-931) `can_own` (workspace view) or org owner/admin (org view): per connect method whether it works here, what is missing and who can fix it (`WhoCan`, C-981); the suggested method; OAuth clients by level (the instance only as set up or not), named by the methods that use them; self-hosted apps; availability; values to copy (callback URL, external id); what the caller can do |
| GET `/v1/apps/{app}/connect-plan?workspace_id=\|org_id=` | S/K | (phase 5, P5-UX-2, D-321, C-955) the projection of the setup status the connect journey needs: methods with readiness and `WhoCan`, the suggested method, scope presets, existing accounts the caller may give |
| POST `/v1/apps/{app}/connect` | S | `{ scope, workspace_id?, method_id, fields?, oauth_client_id?, access_level?, reconnect_connection_id? }` → `{ "kind": "redirect", "url" }` \| `{ "kind": "connected", "connection" }`. (Phase 5: the body also carries `access`, `scopes` and `instance_id`, and the answer carries `missing_scopes`, per C-740/C-741/C-243.) (Phase 5, P5-UX-2, D-322, D-344, C-957, SEC-130) optional `grant: { agent_ids, level: "read"\|"ask"\|"act" }`, which makes the account and the agents' access in one transaction and derives the connection's `access` from the level (C-980) (`connected`: the answer adds `grants`) or carries the grant sealed through the OAuth sign-in and applies it at the callback (`grant_failed` when it cannot) |
| GET `/v1/oauth/callback` | P | state-bound; renders a tiny HTML page that posts to the opener and closes (phase 5, FIX-149: reports over a `BroadcastChannel` instead; P5-UX-2, C-957: the result adds `granted_agent_ids`, `grant_failed?`) |
| GET `/v1/workspaces/{ws}/connections`, `/v1/orgs/{org}/connections` | S/K | |
| GET `/v1/workspaces/{ws}/accounts?agent_id=&kind=&app_id=`, `/v1/orgs/{org}/accounts?kind=&app_id=` | S/K | (phase 5, P5-UX-4, D-345, C-917) the one list of what agents can use: app accounts, websites and AI keys with who can use each (agent, access level and always-allowed changes), the limit of an organization's account, its health, who manages it and what the caller can do; `can_use` (workspace) or org owner/admin (organization); with `agent_id`, the list for that agent's Access tab; (phase 5, P5-APIGAP-3, D-399, C-1048, C-1049) an AI-key row carries `default_model` (the model an agent gets on that key with no override, D-372's rule; null when unknown) and, in the organization list, an organization key carries `starter` (true when the instance's starter grants give it to every new agent of the organization; read-only) |
| GET/PATCH/DELETE `/v1/connections/{id}` | S/K | label, status (`disabled`↔`active`); (phase 5, P5-UX-14, C-944, C-985) `last_check_at`, `last_check_result`, also recorded by the gateway from real use; (phase 5, P5-UX-11, D-353, C-990, SEC-131) PATCH `{ limit: "read"\|"ask"\|"act" }` on an organization's connection (org owner/admin): lowering it lowers the access of the agents above it in the same transaction |
| GET `/v1/connections/{id}/limit-preview?limit=` | S/K | (phase 5, P5-UX-11, C-990) org owner/admin for an org-scope connection, own `can_own` for a workspace-scope one (FIX-234, C-1160): `{ lowered: [{ agent, workspace, from, to }] }`, the agents a lower limit would change |
| GET / PUT `/v1/connections/{id}/webhook` | S/K | (phase 5, P5-WEBHOOK-2, D-320, C-909, C-912) an outgoing webhook's configuration; `can_own` of the connection's workspace (org owner/admin for an organization's). GET: the `WebhookConfig`, `host`, `secret_value_names` and `effective_rate { per_minute, per_day, source }`, never the URL, a secret value or the signing secret. PUT replaces it (`secret_values`: a value sets, `null` deletes, an absent name is kept; `signing_secret.mode` `keep`/`generate`/`provided`/`off`; a generated secret is returned once); a change of secrets bumps `credentials_version`; any change signals the gateway; 422 `webhook_config_invalid { fields: [{ path, code }] }`; audit `connection.webhook.update` (security-critical) with names only |
| POST `/v1/connections/{id}/webhook/test` | S/K | (C-909) `{ values, dry_run? }`: sends (or with `dry_run` renders, masked) the stored webhook through `aurora-webhook` and the guarded client, rate-counted on the gateway's counters; the answer is ≤ 4 KiB and echo-checked; audit `webhook.test_sent` (best effort); 403 `outgoing_webhooks_disabled` when the org switch is off |
| POST `/v1/apps/webhook/webhook-preview` · GET `/v1/apps/{app}/webhook-presets` | S/K | (C-909) the preview renders a configuration for sample values with no network (secrets as `«secret:name»`); the presets are those of the installed package version |
| POST `/v1/accounts/{type}/{id}/test` | S/K | (phase 5, P5-UX-14, D-335, C-943, SEC-129) `type` = `connection` or `secret`; `can_own` (org owner/admin for an organization's account), rate class `check`: run the method's probe, a gateway database login, a hook mint or the AI-key verify; `{ result: ok\|rejected\|unreachable\|unexpected\|not_testable, detail?, checked_at }`, never upstream content; audited |
| GET/POST `/v1/workspaces/{ws}/oauth-clients`, `/v1/orgs/{org}/oauth-clients`; PATCH/DELETE `/v1/oauth-clients/{id}` | S/K | BYOC; (phase 2, D-74) optional `instance_id` binds the client to a self-hosted instance; (phase 5, P5-REFINE-6, C-1070) each client carries `test: { result, at } \| null` and `state: ready_not_tested\|ready\|test_failed\|disabled` |
| POST `/v1/oauth-clients/{id}/test` | S/K | (phase 5, P5-REFINE-6, D-407, C-1070) Test a sign-in app: the callers of `PATCH`; rate class `check`; one token request with a made-up code through the destination guard → `200 { result: ok\|rejected\|unreachable\|unexpected\|not_testable, checked_at, detail?: { host?, status? } }`; no upstream body is returned or stored; audited `oauth_client.test` |

### App instances (self-hosted apps; phase 2, D-74)
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/orgs/{org}/app-instances` | S/K | instances usable in the org (instance-wide + the org's own); any member |
| POST `/v1/orgs/{org}/app-instances` | S/K | `{ app_id, host, label, base_path? }` → `AppInstance`; org owner/admin; validated per `09 §3.4 Instance-bound hosts` (probe included); (phase 5, C-731) `base_path` as below |
| GET/POST `/v1/admin/app-instances` | S | instance admins; GET lists the instance-wide ones, or with `scope=all` (and `org=`) every organization's too, each row carrying `org_name`/`org_slug` (FIX-132, C-639); POST registers instance-wide; a refused host's `details.conflict` names the holder (`static_claim`, `llm_host`, `aurora_address`, `instance` — C-639) |
| POST `/v1/admin/app-instances/{id}/promote` | S | (FIX-132, C-639) make an organization's registration instance-wide (`org_id = NULL`): one transaction, connections keep `instance_id`, audited `app_instance.promote` (critical); 409 `instance_already_wide` / `instance_host_shared` (another registration of the host exists whose base path overlaps, C-731) |
| PATCH/DELETE `/v1/app-instances/{id}` | S/K | label, `status`; delete disables its connections; owner/admin of its scope (instance admin for instance-wide); `base_path` (like `host`) cannot be changed: an unknown field, 400 |

`POST /v1/apps/{app}/connect` takes `instance_id` for an instance-bound app (absent = `default_host`, refused when the app has none). The connect form and the dashboard list the instances visible to the org.

**Base path** (phase 5, FIX-145, D-231, C-731). Both POST bodies take `base_path?: string | null`
for an app served under a path (`https://its.cern.ch/jira` → `host: "its.cern.ch"`,
`base_path: "/jira"`). It is normalized first (a trailing `/` is dropped; `""` and `"/"` mean
`null`; case is kept), then validated: it starts with `/`, has 1–8 segments, each 1–64 characters
from `[A-Za-z0-9._~-]` and neither `.` nor `..`, total ≤ 128 characters; otherwise 422
`instance_base_path_invalid` with `details.fields[].path = "/base_path"`. `AppInstance` gains
`base_path: string | null` (normalized) and `base_url: string` (`https://<host><base_path or "">`,
what clients display); `Connection.instance` gains the same two fields. Conflicts:
`instance_host_conflict` with `details.conflict.kind = "instance"` is raised only when another
registration visible to this one is on the same host **and** the base paths overlap (either is
`null`, they are equal, or one is a segment prefix of the other: `/jira` overlaps `/jira/x`, not
`/jira2`); the conflict then also carries the holder's `base_path` and `base_url`. Registrations on
one host with disjoint base paths are accepted, for one app or different apps. `static_claim`,
`llm_host` and `aurora_address` conflicts are per host and ignore `base_path`. Promote's
`instance_host_shared` uses the same overlap rule. The registration probe, connect probes and OAuth
URLs resolve `{instance}` to `<host><base_path>`; `instance_probe_failed.details.url` is that full
URL. A client that omits `base_path` gets today's behaviour; a client must identify a registration
by `id`, never by `host` alone.

### Grants and effective views
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/agents/{agent}/grants` | S/K | |
| PUT/DELETE `/v1/agents/{agent}/grants/connections/{conn}` | S/K | `{ mode, allow_tools, ask_tools }`; (phase 2, RFE-011a) optional `ask_timeout_secs`; (phase 4, P4-GRANTS, D-123) `mode = full` for a connection of an app whose manifest has `grants.custom_only` → 422 `full_grant_forbidden` (`details: { app_id }`); the grant response and `GET …/grants` carry `app_restrictions: { custom_only, block_untooled }` so clients can explain it; a grant converted by an upgrade is audited `grant.converted_by_app`; (phase 5, P5-UX-4, D-344, C-980, SEC-131) PUT also accepts `{ level: "read"\|"ask"\|"act" }`, expanded by the server; for a workspace's account, the same transaction sets the connection's `access` from all its agents; above the limit of an organization's account → 422 `above_account_limit` with `who_can` |
| PUT/DELETE `/v1/agents/{agent}/grants/secrets/{secret}` | S/K | |
| GET `/v1/connections/{id}/grants` | S/K | agents granted; (FIX-234, C-1160) an org-scope connection's grants are read by its org's owners/admins too; a workspace-scope connection needs own `can_own` |
| POST `/v1/connections/{id}/grants` | S/K | (phase 4, FIX-020, D-137) bulk grant from the credential side: `{ agent_ids (1–`GRANT_BULK_MAX`), mode, allow_tools?, ask_tools?, ask_timeout_secs? }`; each agent passes exactly the checks of `PUT /v1/agents/{agent}/grants/connections/{conn}` (same service function, incl. 422 `full_grant_forbidden`); all-or-nothing, one audit row per new grant; an existing grant is left unchanged and reported `existing` (C-504); (FIX-234, C-1160) an org-scope connection is granted by its org's owners/admins too; a workspace-scope one needs own `can_own` |
| PUT `/v1/agents/{agent}/grants/apps/{app}` | S/K | (phase 5, FIX-154, D-306, C-896) give an agent access to a **credential-free** app (C-895) in one step: body as `PUT …/grants/connections/{conn}`; in one transaction serialized per (workspace, app) it reuses the workspace's oldest active connection of the app or creates one (audited `connection.created`, `via: "agent_grant"`), then upserts the grant; → `{ connection, grant, created }`; 404 `app_not_found`, 403 `app_unavailable`, 422 `app_not_credential_free`, 422 `full_grant_forbidden`. Removing access uses `DELETE …/grants/connections/{conn}` |
| GET/POST `/v1/secrets/{id}/grants` | S/K | (phase 4, FIX-020, D-137) agents granted (only agents the caller can see) / bulk grant `{ agent_ids }` with exactly the checks of `PUT /v1/agents/{agent}/grants/secrets/{secret}`; all-or-nothing, idempotent (C-503) |
| GET `/v1/agents/{agent}/model-readiness` | S/K | (phase 4, FIX-019, D-136) `can_own`: per granted LLM key `usable` or the failing reason, and the default model selection (`08 §Model readiness`, C-500). (phase 5, P5-MODELS-1, C-878) `default` also carries the role resolution and warnings |
| GET `/v1/agents/{agent}/effective` | S/K | effective credentials and per-tool decisions with `org_ceiling`; (phase 2, RFE-005) optional `?speaker=<user_id>\|none`; (phase 5, P5-UX-10, C-934) computed on the shared code of FIX-159 |
| GET `/v1/agents/{agent}/apps/{app}/access` | S/K | (phase 5, P5-UX-10, D-346, C-934) `can_use`, rate class `simulate`: for each account of the app the agent can use, every tool with its decision (allow/ask/block) and, only when something other than the agent's access decided, the reason and a link |

### Policy
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/workspaces/{ws}/policy`, `/v1/orgs/{org}/policy` | S/K | published + draft + revision; (phase 4, P4-CNT) plus `rate_limit_scope: "replica" \| "shared"` (`shared` when `ratelimit.counters.backend` is `postgres`/`redis` and the gateway share is not `memory`; the editor note of `14`) |
| PUT `…/policy/draft` | S/K | whole-draft replace with `revision`; (phase 1, RFE-003) workspace scope also carries `open_lane_posture` (`enabled` \| `allowlist_only` \| `disabled`; a workspace can only tighten, so `enabled` simply adds no restriction beyond the org and instance) |
| POST `…/policy/publish`, `…/policy/rollback` | S/K | `{ revision }` / `{ generation, expect_revision }` (FIX-109: rollback replaces the current draft, so it needs the draft revision the caller last read; 409 `stale_revision` otherwise) |
| GET `…/policy/generations` | S/K | history; (phase 5, P5-REFINE-6, C-1087) each carries `changes: [{ kind: added\|removed\|edited, rule_name }]` (≤ 20) and `more` against the generation before it, and `actor: { display_name } \| null` (the publisher) |
| GET `/v1/policy/approval-limits` | S/K | (phase 5, P4-011B, D-265(d), C-829) any signed-in user: the live approval bounds `{ timeout_secs, max_timeout_secs, park: { enabled, threshold_secs, max_timeout_secs, resume_window_secs } }` for the rule editor and the grant ask timeout; not on the unauthenticated `/v1/instance` |
| GET `…/policy/warnings` | S/K | overlap/inert-rule warnings for the draft; (phase 2, RFE-025) includes `rule_applies_to_all_agents` (`07 §Injection selection`) |
| GET `/v1/workspaces/{ws}/internet-access` | S/K | (FIX-164, C-1020, D-365; amends I-27 for this read) **any member of the workspace** (`can_use`): the **published** generation only, `{ number, default_action, uncredentialed_default, uncredentialed_allowlist, open_lane_posture, rules }`; no draft, revision, history, publisher or warnings; a non-member gets 404 |
| POST `/v1/workspaces/{ws}/policy/simulate` | S/K | `{ agent_id, method, url, body?, use_draft?, lane?, speaker_user_id? }` → decision trace (phase 2, RFE-005: `speaker_user_id` omitted = no speaker); (phase 1, RFE-003) `lane: "open"` evaluates CONNECT-time semantics (`07`) and the open-lane posture |

### Approvals
(phase 2, RFE-012) Every route in this section requires **`can_approve`** on the
approval's workspace (`04`), otherwise 403 `approver_required` (404 where the
workspace/approval is not visible); in phase 0/1 the requirement was `can_use`.

| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/workspaces/{ws}/approvals?status=` | S/K | (phase 5, P5-AGENT-1, C-700) optional `agent_id`: only that agent's approvals, applied in the query with the visibility predicate; another workspace's or an unknown agent gives an empty page (as on `action-approvals`) |
| GET `/v1/approvals/pending?wait=` | S/K | long-poll across what the caller may decide |
| GET `/v1/approvals/stream` | S | SSE of created/decided; (phase 2, RFE-011a) `created` carries `{ id, workspace, agent, title, kind: "gateway"\|"action", expires_at }` so the dashboard can raise a browser notification; counts as a live surface (`approval_watchers`) |
| POST `/v1/approvals/{id}/decision` | S/K | `{ "decision": "approve"|"deny" }`; first decision wins (409 `already_decided`); (phase 5 (D-138, moved from phase 4), RFE-011b) for a parked approval the response carries `resume: { state, expires_at }` and the DTOs of the approvals feed, long-poll, SSE and history carry `parked: bool`, `resume_state`, `resume_expires_at`, `consumed_at` (never the fingerprint or salt; C-822); the decision, its follow-up and its audit row are one transaction (C-823, C-824) || POST `/v1/approvals/batches/{batch_id}/decision` | S/K | all pending of the batch the caller may decide; (phase 5, P5-UX-16, D-348, C-982, SEC-131) optional `always: true` with `approve`: also moves the asked tool to allowed in the agent's access and approves the other waiting requests of the same kind; only for a question asked by the agent's access, `can_own`, and never above the limit of an organization's account, else 422 `always_not_available`; the access stays `ask` with the tool listed as always allowed |
| GET `/v1/workspaces/{ws}/access-requests?status=open`; POST `/v1/access-requests/{id}/dismiss` | S/K | (phase 5, P5-UX-17, D-348, C-984, SEC-132) what agents asked for (an app, or a website with a token): `can_use` to list, `can_own` to dismiss; a request is resolved when access is given |
| GET `/v1/workspaces/{ws}/action-approvals`; POST `/v1/action-approvals/{id}/decision` | S/K | |

### Web Push (phase 5 (D-138, moved from phase 4), RFE-011b, D-134)
Own-resources only: every route acts on the caller's subscriptions; another user's subscription
is 404. 404 `push_unavailable` on all of them while `push.enabled = false`. Writes are best-effort
audited (`push_subscription.create|update|delete|move`, C-838). Contracts C-830..C-834 (P4-011B).

| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/push/config` | S | `{ available, vapid_public_key, allowed_hosts }` for the browser's `pushManager.subscribe` (the key is public; (P4-011B spec batch, D-266(d), C-830) no key id: the browser compares public keys, which works for generated and operator-managed keys alike) |
| GET `/v1/me/push-subscriptions` | S | the caller's devices: `{ id, label, show_details, paused, state: "active"\|"paused"\|"disabled"\|"expired"\|"stale_key", this_session, created_at, last_success_at, last_failure_at }` — never the endpoint or keys (C-831) |
| POST `/v1/me/push-subscriptions` | SO | `{ endpoint, keys: { p256dh, auth }, expiration_time?, label?, show_details?, vapid_public_key, rebind? }` → 201 `PushSubscription` (200 when the endpoint already belonged to the caller: its session is re-bound); 422 `push_endpoint_not_allowed` (`details.reason`: `not_https`, `host_not_allowed`, `too_long` over `PUSH_ENDPOINT_MAX`, `userinfo`, `ip_literal`), `push_keys_invalid` (`p256dh` not a 65-byte P-256 point, `auth` not 16 bytes), 409 `push_subscription_limit_reached`, 409 `push_key_stale` (`vapid_public_key` is not the active key: re-fetch `config`); an endpoint already registered by another user is **moved** to the caller and its session — except with `rebind: true`, which (D-266(c)) only re-binds an endpoint already registered to the caller and otherwise answers 404 `push_subscription_not_found` and creates nothing |
| PATCH/DELETE `/v1/me/push-subscriptions/{id}` | SO | `{ label?, show_details?, paused? }`; DELETE is idempotent |
| POST `/v1/me/push-subscriptions/{id}/test` | SO | sends a `test` push (`PUSH_TEST_PER_HOUR` 3 per user, then 429 `push_test_rate_limited` with `Retry-After`); 202, the outcome appears in the device state |

CLI (operator): `aurora-api push rotate-vapid [--dry-run]`, `aurora-api push status [--json]` (mode,
key ids and dates, subscription counts by state and host; never key material; C-833). (P4-011B spec
batch, D-266(c)) Session revocation (sign-out, sign out others, an administrator's revocation,
deactivation) deletes the session's subscriptions in the same transaction; `GET /v1/instance`
gains `features.push: bool` (C-830).

### Agents and sandboxes
| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/workspaces/{ws}/agents` | S/K | create returns `aug_` once for BYO; (phase 2, RFE-025) creating `kind = byo` is subject to the effective BYO posture: 403 `byo_not_permitted` (`owners_only`, caller lacks `can_own`) or 403 `byo_disabled`; the list/get shape carries `byo: { posture, posture_level, source_cidrs, require_context, effective_source_cidrs, effective_require_context }` for BYO agents |
| GET/PATCH/DELETE `/v1/agents/{agent}` | S/K | name, brief, harness, model, effort, resource overrides (IA); (phase 4, P4-HAR) `harness` must be in `hosted.enabled_harnesses` (422 `harness_not_enabled`), cannot change while a turn is active (409 `turn_active`), and a change bumps every conversation's `harness_session_epoch`, queues session purges and drops incompatible model overrides with a notice (`12 §Harness selection and adapter contract`); audited. (phase 5, P5-MODELS-1, D-303, C-876) `model` is the **default** model. The route also reads and writes `model_roles` (`{ endpoint_id, large?, medium?, small? } \| null`) and `subagent_model`, under the same permission as `model`. 422 `model_id_invalid` (pattern), `llm_model_not_allowed`, `model_role_endpoint_mismatch` (the roles' `endpoint_id` is not the agent's current default endpoint) |
| GET `/v1/harnesses` | S/K | (phase 4, P4-HAR) enabled harnesses: `{ id, display_name, dialects, available }` where `available` means at least one live runner replica lists it; drives the dashboard picker. (phase 5, P5-MODELS-1, C-882) Each item also has `model_roles: string[]` (the roles the harness maps; `[]` for `codex`) |
| POST `/v1/agents/{agent}/token` | S | BYO: regenerate, returns token once with `expires_at` (phase 3, RFE-013: null unless an agent-token maximum age applies, `08 §7`); (phase 2, RFE-025) same posture check as creation (`byo_not_permitted` / `byo_disabled`); audited `agent.token_regenerate` |
| PUT `/v1/agents/{agent}/byo` | S/K | (phase 2, RFE-025) BYO only (hosted → 422 `not_byo`): `{ source_cidrs: [cidr..] \| null, require_context: bool }`; `can_own`; 422 `byo_cidr_invalid` (bad CIDR, `/0`, empty array, >32 entries) / `byo_cidr_outside_default`; live credentials are re-evaluated by invalidation scope `agent`; audited `agent.byo_settings.update` |
| GET `/v1/agents/{agent}/container-config` | S/K | BYO bootstrap (`08 §BYO bootstrap`) |
| GET `/v1/workspaces/{ws}/agent-tokens`; GET `/v1/orgs/{org}/agent-tokens` | S/K | (phase 3, RFE-013) BYO agent tokens without material: `Page<{ agent, workspace, created_at, expires_at, last_used_at, stale, expiring, display_prefix }>` (phase 3, D-107: paginated, C-313), filters `?stale=true`, `?expiring_within_days=n`; workspace route `can_administer` (FIX-234, C-1160), org route org `owner`/`admin` |
| GET `/v1/agent/container-config`; POST `/v1/agent/context-credentials` | A | BYO self-service (`08`) |
| PUT/GET `/v1/agents/{agent}/avatar`; GET `/v1/public/avatars/{key}` | S/K; P | |
| POST `/v1/agents/{agent}/restart` | S/K | restart sandbox (FIX-049, D-164, C-626: from `failed` performs the C-610 recovery — 423 `frozen` applies; from `stopped`/`unprovisioned`/`stopping` 409 `restart_not_applicable`) |
| GET `/v1/agents/{agent}/terminal` | S | (phase 4, P4-TERM, D-161, C-616) WebSocket upgrade to the sandbox's shell (wterm + Ghostty core in the dashboard); session cookie only, `can_use`, hosted only (BYO 422 `terminal_not_supported`), 404 `terminal_disabled` while off, 423 frozen; mints the single-use `auy_` grant and proxies to the runner; best-effort audit `agent.terminal.open`; bytes never logged; (FIX-138) optional query `shell=bash\|zsh` (any other value 422 `invalid_shell`) is stored on the grant and reaches the PTY spawn (`12 §Terminal`); (FIX-218) when absent, the person's stored preference for the agent applies, else `bash` |
| GET/PUT `/v1/agents/{agent}/terminal/shell` | S | (FIX-218, D-459, C-1145) the person's shell choice for this agent: `GET` → `{ shell: "bash"\|"zsh", stored }` (`bash`, `stored: false` when nothing is saved), `PUT { shell }` → `{ shell, stored: true }`, any other value 422 `invalid_shell`; session cookie, `can_use` (404/403), hosted only (BYO 422 `terminal_not_supported`), readable and writable while frozen; used by the web terminal when `?shell=` is absent and by SSH (`16 §W20`) |
| GET `/v1/agents/{agent}/files?path=` | S | (phase 5, P5-FILES-1, D-316, C-898) list a directory under `/workspace` (`FileEntry[]`, ≤ 5,000, `truncated`); the terminal's availability (404 `terminal_disabled`, BYO 422 `files_not_supported`, frozen 423), parked sandbox 503 `sandbox_waking` (wake demand), rate class `files`; paths per C-899 |
| GET `/v1/agents/{agent}/files/content?path=` | S | (phase 5, P5-FILES-1, C-898, C-903) download a file, streamed, always `attachment` + `application/octet-stream` + `nosniff` + sandbox CSP; audit `agent.files.download` |
| GET `/v1/agents/{agent}/files/text?path=` | S | (phase 5, P5-FILES-1, C-898) a UTF-8 text file ≤ 1 MiB for the editor: `{ content, etag, size, modified_at, line_ending, truncated }`; 422 `file_not_editable`. (P5-REFINE-4, C-1060) `&preview=1`: a text file over 1 MiB answers its first 256 KiB cut at a line end with `truncated: true`, `size` the whole file's, `etag: null` (never saveable), audited `agent.files.download` `via: "preview"`; binary / not-UTF-8 stay 422; without `preview` nothing changes |
| PUT `/v1/agents/{agent}/files/content?path=` | S | (phase 5, P5-FILES-1, C-898) upload or save, raw body ≤ 512 MiB, `If-Match` (409 `file_changed`) or `If-None-Match: *` (409 `file_exists`); temp file + `posix-rename`; audit `agent.files.write` |
| POST `/v1/agents/{agent}/files/ops` | S | (phase 5, P5-FILES-1, C-898) `mkdir` (with `parents`), `rename` (no replace), `delete` (with `recursive`, ≤ 10,000 entries); `/workspace/.aurora` 403 `path_read_only`; audit per op. (C-1060) `rename` takes a full destination and moves across folders (a folder into itself is 422 `invalid_path`): the dashboard's Move to… is `rename` with a full destination |
| POST `/v1/agents/{agent}/sandbox/retry` | S/K | (phase 4, FIX-029, D-160, C-610; amended FIX-049/D-164) retry a failed start from `failed` only (409 `sandbox_not_failed`, 423 `frozen`; a live failure-backoff lease never refuses): clears the failure, resets `start_failures`, notifies due work for an ordinary start; audited `agent.sandbox_retry` |
| POST `/v1/agents/{agent}/sandbox/wake` | S/K | (P5-APIGAP-1, D-399, C-1045) start the agent's computer when it is asleep, without a message: `can_use`; 202 `SandboxView` when a start was requested (`stopped`, `unprovisioned`, `stopping`), 200 `SandboxView` unchanged when it is already `starting` or `running` (idempotent, no second start), 409 `sandbox_failed` (retry is the way out), 423 `frozen`, 409 `not_hosted` (BYO agent), 404 as the sibling routes; 429 `rate_limited` past the shared wake budget (the terminal's, C-621); audited `agent.sandbox_wake`. The same demand a terminal open sets, so no second start path. (C-1046) `SandboxView` gains `idle_stop_after_secs` |
| POST `/v1/agents/{agent}/sandbox/sleep` | S/K | (P5-REFINE-4, D-408, C-1060) Sleep now, `can_use` (the wake rule): sets `stop_requested_at` with `stop_reason = 'idle'` on a `starting`/`running` computer no work holds awake (the stop arm stops it). 202 `SandboxView` when requested; 200 `SandboxView` when it is already `stopping`, `stopped`, `unprovisioned` or a stop is already ordered (nothing written); 409 `sandbox_busy` `details { reasons }` while `turn`, `process` or `watch` holds it; 409 `sandbox_failed`; 409 `not_hosted`; 404 as the sibling routes; 429 past the shared wake budget (spent only on an awake computer). Open terminal, SSH, Files and preview sessions do not block it. A message that arrives after the request brings the computer back through the start arm. Audited `agent.sandbox_sleep` (`previous_state`, `reasons`) |
| GET `/v1/agents/{agent}/start-failures` | S/K | (phase 4, FIX-029, C-613) the agent's recent start failures (≤ 20, newest first, C-586 shape); the runner's wording and `events` are for instance admins only, owners and members get the class and the member hint (FIX-196, D-424) |
| GET `/v1/admin/agents/{agent}/start-failures` | IA | (phase 4, FIX-029, C-614) the same list with `runner_id` |
| POST `/v1/agents/{agent}/freeze` `{ reason }`; POST `/v1/agents/{agent}/unfreeze` | S/K | (phase 2, RFE-006) `can_administer` of the agent's workspace (FIX-234, C-1160; `frozen_authority` stays `org_admin` for an administrator); 200 with `frozen: {at, by_label, by, reason, authority, level}` (C-1102: `by` is `{ id, display_name }`, null when the user is gone; `by_label` stays the e-mail for compatibility, and the dashboard shows `by`); 403 `freeze_authority_insufficient` when lifting a higher authority's freeze; audited atomically |
| GET `/v1/agents/{agent}/sandbox` | S/K | state, keep-awake reasons, processes, ports. (P5-REFINE-4, C-1059, D-408, D-412) `computer { word, ready, working, asleep, phase, sleeps_in_secs, started_at }` for every caller (`word` one of `off`, `starting`, `on`, `stopping`, `failed`; `ready` only once the supervisor connected and the harness reported ready; `sleeps_in_secs` while on, idle and idle stop is on). **Who sees what (SEC-141, amends D-178 Q-R5-5):** `image`, `runner_id`, `start_attempt.detail`/`events` and `start_log[].detail` only for instance admins; workspace owners and members get null / `[]` there and the failure's member hint as `failure_message`; `boot_id`, the phases and the times for everyone. The runner's `running` report never moves `start_phase` back from `booting`/`ready` |
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
| PATCH `/v1/orgs/{org}` (`settings.outgoing_webhooks_enabled`, `outgoing_webhooks_per_minute_max`, `outgoing_webhooks_per_day_max`) | S | (phase 5, P5-WEBHOOK-2, D-320, C-912) org `owner`; stored as `organizations.settings.outgoing_webhooks { enabled, per_minute_max, per_day_max }` like the calendar switch below (switch default `true`; ceilings `null` = the instance's, above the instance's → 422 `above_instance_ceiling`); audit `org.update` with `metadata.outgoing_webhooks_enabled`. The connect of a webhook and every send are refused while it is off |
| PATCH `/v1/orgs/{org}` (`settings.calendar_invitations_enabled`) | S | (phase 5, P5-CALINV, C-844) org `owner`; stored as `organizations.settings.calendar_invitations.enabled` (default `true`; `Org.settings.calendar_invitations_enabled`); audit `org.update` with `metadata.calendar_invitations_enabled`. **Calendar invitations have no HTTP route of their own**: they are agent tools only (`12 §Phase 0 catalogue`) |
| PATCH `/v1/orgs/{org}` (`settings.previews_enabled`) | S | org `owner`/`admin`; audit `org.previews.update` (security-critical); switching off keeps shares, ends access (`11 §Port previews`) |
| GET `/v1/instance` | none | (existing public route) gains `features.previews` and `previews: { enabled: bool, domain: string\|null }` (no secret; the domain is already public in DNS), so the dashboard shows the Previews tab only when it can work |

`GET /v1/agents/{agent}/sandbox` already lists `ports`; (phase 4, P4-PRV-1) it gains `preview_in_use: bool`
(keep-awake reason `preview`) and each `ports` entry gains `shared: <share id>|null`. Audit and
the share lifecycle: `11 §Port previews`.
| GET `/v1/gateway/ca.pem` | S/K | gateway CA certificate |

### SSH access (phase 5, P4-SSH-1, P4-SSH-2, D-239; C-790..C-799)

Present on every instance but inert: while `ssh.enabled` is false every route below answers 404
`ssh_disabled` (the key routes included: no key can be registered on an instance without SSH), and
`GET /v1/instance` reports `features.ssh: false`. Authority: `04 §SSH access`. Errors use the
standard body.

| Method & path | Auth | Purpose |
|---|---|---|
| PATCH `/v1/agents/{agent}` (`narration_cards`) | S/K | (phase 5, P4-NARR, C-846) boolean, default `false`; the agent's other settings' rules apply |
| GET `/v1/me/user-tokens` | S/K | (phase 5, P4-UTOK, C-853) `{ tokens: UserToken[] }` |
| POST `/v1/me/user-tokens` | SO | (phase 5, P4-UTOK, C-853) `{ integration_id, token, label? }` → 201 `UserToken`; the token is sealed and verified (`user_token.verify`) before the response; never returned or logged |
| PATCH `/v1/me/user-tokens/{id}` | SO | (phase 5, P4-UTOK, C-855) `{ label }`; 404 for another person's token |
| DELETE `/v1/me/user-tokens/{id}` | SO | (phase 5, P4-UTOK, C-855) 204; revokes, discards the sealed value, cancels pending sends (`credential_revoked`); audit `user_token.revoke` (critical); 404 for a token that is not yours or already revoked |
| PUT / DELETE `/v1/me/user-tokens/{id}/agents/{agent}` | SO | (phase 5, P4-UTOK, C-855) grant / remove one agent; idempotent, 204; grant needs `can_use` on the agent (404 for an agent you cannot see or one in another organization than the token's integration, 409 `token_inactive` for a token that is not `active`); removing a grant cancels that agent's pending posts as the person (the token stays) |
| GET `/v1/me/user-token-approvals` | SO | (phase 5, P4-UTOK, C-856) `?status=` → `{ items: [{ id, workspace_id, agent_id, token_id, summary, text, status, error, expires_at, created_at }] }`: the caller's own proposed posts with the full text, newest first, at most 100; nobody else's. Decided with `POST /v1/action-approvals/{id}/decision` (owner only: anyone else, even with `can_approve`, gets 403 `not_token_owner`, or 404 where the conversation is a direct one they cannot see; an API key never decides) |
| GET `/v1/me/ssh-keys` | S/K | (phase 5, P4-SSH-1, C-790) the caller's keys: `{ keys: [{ id, name, algorithm: "ssh-ed25519", public_key, fingerprint, created_at, last_used_at }] }` |
| POST `/v1/me/ssh-keys` | SO | (phase 5, P4-SSH-1, C-790) `{ public_key, name? }` → 201 key; `public_key` is one OpenSSH line (`ssh-ed25519 AAAA… [comment]`, ≤ 4 KiB, no options prefix); 422 `ssh_key_invalid` (unparsable, options, a certificate, private key material), 422 `ssh_key_unsupported_type` (anything but `ssh-ed25519`), 409 `ssh_key_exists`, 409 `ssh_key_limit` (`SSH_KEYS_PER_USER`); audit `ssh_key.create` (critical) |
| PATCH `/v1/me/ssh-keys/{id}` | SO | (phase 5, P4-SSH-1, C-790) `{ name }`; 404 for another person's key |
| DELETE `/v1/me/ssh-keys/{id}` | SO | (phase 5, P4-SSH-1, C-790) 204; open sessions authenticated with it are closed at the next heartbeat (`key_deleted`); audit `ssh_key.delete` (critical) |
| POST `/v1/agents/{agent}/ssh-certificate` | S/K | (phase 5, P4-SSH-1, C-791) `{ public_key }` (an OpenSSH line or the canonical form; must be one of the caller's registered keys) → 201 `{ certificate, serial, principal, valid_after, valid_before, username, host, port, known_hosts: [string] }` (`certificate` is one OpenSSH line `ssh-ed25519-cert-v01@openssh.com AAAA… aurora:<agent id>` with principals `[<agent id>, <handle>]`; `principal` is the agent id and `username` the handle, the SSH user name to connect as (C-1121, D-445); `known_hosts` may be empty until P4-SSH-2 registers a terminator); session or the caller's own API key (service keys and agent credentials 403); `can_ssh`: 404 unknown or invisible agent, 422 `ssh_not_supported` (BYO), 423 `frozen`, 422 `ssh_key_not_registered`, 429 `ssh_mint_rate_limited` (`Retry-After`); audit `agent.ssh_certificate.mint` |
| GET `/v1/ssh` | S/K | (phase 5, P4-SSH-1, C-792) connection facts for the "Connect with SSH" panel and the CLI: `{ host, port, cert_ttl_secs, host_keys: [{ public_key, fingerprint }], known_hosts: ["[host]:port ssh-ed25519 AAAA…"] }` (the distinct host keys of live terminator replicas; empty with a `terminators_live: 0` flag when none is registered) |
| GET `/v1/me/ssh-sessions` | S/K | (phase 5, P4-SSH-2, C-793) the caller's open sessions: `{ sessions: [SshSession] }`, `SshSession = { id, agent: { id, name }, workspace_id, key: { id\|null, name\|null, fingerprint }, client_address, opened_at, last_activity_at, channels_total }` (coarse address) |
| GET `/v1/agents/{agent}/ssh-sessions` | S/K | (phase 5, P4-SSH-2, C-793) open sessions on the agent: `can_own` sees all (each with `user: { id, label }`), a `can_use` caller only their own |
| DELETE `/v1/ssh-sessions/{id}` | S/K | (phase 5, P4-SSH-2, C-793) disconnect: the session's person or `can_own` of its workspace (404 otherwise); sets `revoke_requested_at`; 202 — the terminator ends the connection within one heartbeat (≤ 30 s); audit `ssh_session.disconnect` |
| GET `/v1/admin/terminators` | IA | (phase 5, P4-SSH-2, C-793) terminator replicas seen: `{ id, version, host_key_fingerprint, sessions_open, last_seen_at, live }`, shown on Admin › Overview |
| GET `/v1/instance` | none | (phase 5, P4-SSH-1; existing public route) gains `features.ssh: bool` so the dashboard shows the SSH surfaces only when they can work |

(Phase 5, P5-FILES-1, D-316) `keep_awake_reasons` also gains `files` ("Files in use"). Files error
codes (C-898..C-901): `invalid_path` (400), `path_outside_workspace`, `path_read_only`,
`permission_denied` (403), `path_not_found` (404), `file_exists`, `file_changed`,
`directory_not_empty`, `sandbox_unavailable` (409), `length_required` (411), `file_too_large` (413),
`not_a_directory`, `is_directory`, `file_not_editable`, `too_many_entries`, `files_operation_failed`,
`files_not_supported` (422), `too_many_transfers`, `wake_limited` (429), `sandbox_waking`,
`files_unavailable` (503, `retry-after`).

`GET /v1/agents/{agent}/sandbox`'s `keep_awake_reasons` gains `ssh` (phase 5, P4-SSH-2; "SSH session in
use"). Error codes added: `ssh_disabled` (404), `ssh_not_supported` (422), `ssh_key_invalid` (422),
`ssh_key_unsupported_type` (422), `ssh_key_exists` (409), `ssh_key_limit` (409),
`ssh_key_not_registered` (422), `ssh_mint_rate_limited` (429), `ssh_session_not_found` (404).

### Evals (phase 5, P4-EVAL, D-289..D-294; C-860..C-874)

Hosted agents only (BYO: 422 `eval_not_supported`). Reads and writes of questions, settings, runs and schedules need `can_use` of the agent's workspace unless the row says otherwise; setting the judge key and deleting a run need `can_own`; no route is an agent tool. Errors use the standard body (`docs/05 §Conventions`).

| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/agents/{agent}/eval-questions` | S/K | (phase 5, P4-EVAL, C-861) list in position order / create one `EvalQuestion` (422 `invalid`, 422 `eval_question_limit`) |
| GET/PATCH/DELETE `/v1/eval-questions/{id}` | S/K | (phase 5, P4-EVAL, C-861) `PATCH` bumps `version`; the expected-answer fields follow C-862 |
| GET/PUT `/v1/agents/{agent}/eval-settings` | S/K | (phase 5, P4-EVAL, C-863) `{ judge: { secret_id, model_id } \| null, regression_notify: bool }` → `{ judge, regression_notify, test: { result, took_ms, reason, tested_at } \| null }` (a changed judge is tested with one minimal model call within `EVAL_JUDGE_TEST_TIMEOUT` and saved either way; the last test is kept and `GET` returns it, C-1101; `reason` `rejected` \| `unreachable` \| `rate_limited`); a judge change needs `can_own`; 422 `eval_judge_not_granted`, `eval_judge_not_usable` (`details.cause` is `"model"` when the key works but its endpoint does not list the judge model), `eval_judge_not_permitted` |
| GET `/v1/agents/{agent}/eval-fingerprint` | S/K | (phase 5, P4-EVAL, C-866) the configuration fingerprint a run started now would record: `{ fingerprint, fingerprint_hash, question_set_version }` |
| POST `/v1/agents/{agent}/eval-runs` | S/K | (phase 5, P4-EVAL, C-864) start a run (optional body `{ question_ids?: uuid[] }`, 1..=20, runs only those) → 202 `EvalRun` (`queued`; `counts` is live while the run is active); 409 `eval_run_active`, 429 `eval_runs_limited`, 423 `frozen`, 422 `eval_no_questions` |
| GET `/v1/agents/{agent}/eval-runs` | S/K | (phase 5, P4-EVAL, C-864) runs newest first, cursor page (`05` list convention), filters `status`, `trigger` |
| GET `/v1/eval-runs/{id}` | S/K | (phase 5, P4-EVAL, C-864/C-867) the run with counts, fingerprint, regression and its results (`?results_after=` cursor, ≤ 100 per page) |
| POST `/v1/eval-runs/{id}/cancel` | S/K | (phase 5, P4-EVAL, C-865) sets `cancel_requested_at`; 409 `eval_run_finished` for a finished run |
| DELETE `/v1/eval-runs/{id}` | S/K | (phase 5, P4-EVAL, C-864) `can_own`; a running run is refused (409 `eval_run_active`; cancel first); deletes its eval conversations (`10 §Archive, delete`) |
| GET/POST `/v1/agents/{agent}/eval-schedules` | S/K | (phase 5, P4-EVAL, C-870) list / create (`{ name, cron, timezone, anchor? }`; interval below `EVAL_SCHEDULE_MIN_INTERVAL` → 422 `invalid`; `anchor` `can_own` only) |
| GET/PATCH/DELETE `/v1/eval-schedules/{id}` | S/K | (phase 5, P4-EVAL, C-870) same authority as the schedules of `10 §Schedules`; audit `eval.schedule.*` |

Phase 5 rows, each answered with the standard error body; the set of routes and their statuses is the one C-860..C-874 define.

### Conversations and turns
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/agents/{agent}/conversations` | S/K | filters: kind; (phase 2, RFE-010) `archived=true\|false` (default false); (phase 5, P5-REFINE-7, C-1074) `automation=all\|none\|only` (default `all`) and `automation_id=<uuid>` (only with `automation=only`, else 400 `invalid_request`); items carry `automation: { kind, id, name, deleted, previous_name } \| null` (C-1098: `previous_name` is the automation's name before its last rename, else null), `run_at` and `title_source` (`auto`\|`person`\|`automation`) |
| POST `/v1/agents/{agent}/conversations/direct` | S | get-or-create the caller's **most recently active non-archived** direct conversation (phase 5, P5-REFINE-7, C-1073: never an archived one; creates the first, `user:<uid>`, when there is none) |
| POST `/v1/agents/{agent}/conversations/direct/new` | S | (phase 5, P5-REFINE-7, C-1073, D-409) a fresh direct conversation (`can_use`): 200 with the caller's latest direct conversation when it has no events yet, 201 with a new one (`user:<uid>:<uuid>`) otherwise; 422 `conversation_limit` past 500 per person and agent |
| GET `/v1/agents/{agent}/conversation-folders?cursor=&limit=` | S/K | (phase 5, P5-REFINE-7, C-1074) `can_use`; one item per automation with threads, newest activity first: `{ kind, id, name, deleted, thread_count, last_activity_at, last_result }` |
| POST `/v1/agents/{agent}/conversations` | K | `kind = api`, `external_ref` (get-or-create) |
| GET `/v1/conversations/{id}` | S/K | |
| PATCH `/v1/conversations/{id}` | S/K | (phase 5, P5-REFINE-7, C-1073) `{ title: string \| null }`: 1–200 characters trimmed, no control characters (422 `invalid_title`, `details.fields`); `null` re-derives the auto title. A `direct` conversation: its person only (403 `direct_conversation_forbidden` for anyone else, owners included); `group`/`api`/other kinds: `can_own` (403 `forbidden`); `schedule`/`webhook` threads 409 `title_managed`. Audited `conversation.rename` (ids only) |
| POST `/v1/conversations/{id}/archive`, `/unarchive`; DELETE `/v1/conversations/{id}` | S/K | (phase 2, RFE-010) `10 §Archive, delete, redaction, retention, export`; delete 409 `conversation_active` |
| POST `/v1/conversations/{id}/events/{seq}/redact`; POST `/v1/turns/{id}/redact` | S/K | (phase 2, RFE-010) `{ reason?, reset_session? }`; irreversible, audited |
| GET `/v1/conversations/{id}/export` | S/K | (phase 2, RFE-010) JSON `aurora.conversation.v1` (`attachments=zip` → zip); 422 `export_too_large` |
| GET `/v1/conversations/{id}/events?after_seq=&limit=` | S/K | durable history |
| GET `/v1/conversations/{id}/stream` | S/K | SSE |
| GET `/v1/workspaces/{ws}/messages/search?q=&agent_id=&conversation_id=&author=&has=&after=&before=&archived=&sort=&limit=&cursor=` | S/K | (phase 5, P5-CHAT-4, D-229, C-727; `search` rate class) workspace member. Searches the text people and agents said in the conversations the caller can see — `turn.queued` text, agent `text`, `attachment` file names and captions, file names of a person's attachments (`10 §Search`). `q` 1–200 chars after trim, ≤ 12 terms: words (the last prefix-matched), `"phrases"`, `-word`, `OR`. `author` `me`\|`agent`\|`others`; `has=file`; `after`/`before` RFC 3339 or `YYYY-MM-DD` (UTC, half-open); `archived` `include` (default)\|`exclude`\|`only`; `sort` `recent` (default)\|`relevance`; `limit` 1–50 (default 20). → `{ items: [{ conversation: { id, title, kind, archived }, agent: { id, name }, seq, type, author: { kind, label }, at, snippet, highlights: [[start, end]] }], next_cursor, index_complete }`; `snippet` is plain text ≤ 300 chars, `highlights` UTF-16 offsets into it; `index_complete: false` while older messages are still being indexed. Visibility (`can_see_conversation`) is applied inside the query. 422 `search_query_invalid` (`details.reason` `empty`\|`too_long`\|`too_many_terms`), 400 `validation_error`, 404 for a `conversation_id` the caller cannot see, 503 `search_timeout` (the 2 s statement bound) |
| POST `/v1/conversations/{id}/messages` | S/K | `{ text?, attachment_ids?, reply_to?: { seq } }` → `{ turn }` or `{ followup }`; (phase 5, P5-CHAT-1, C-725/C-726) `text` may be empty or absent when `attachment_ids` is non-empty (both empty: 400 `validation_error` on `/text`); `reply_to.seq` quotes an earlier `text`, `turn.queued` or `attachment` event of this conversation — anything else (unknown or another conversation's seq → `not_found`, another type → `not_replyable`, a redacted event → `redacted`) is 422 `reply_target_invalid` with `details.reason`; (phase 3, RFE-018) optional header `Idempotency-Key` — a retry with the same key returns the original result (`Idempotent-Replayed: true`) and creates nothing; 422 `idempotency_key_invalid` / `idempotency_key_reused` (`### Idempotency keys`) |
| POST `/v1/turns/{id}/abort`, `/v1/turns/{id}/send-now` | S/K | (FIX-225, C-1156) send-now: 409 `send_now_unsupported` when the harness lacks `background_tool` or steers |
| GET `/v1/conversations/{id}/followups`; POST `/v1/followups/{id}/steer`, `/v1/followups/{id}/cancel`; PATCH `/v1/followups/{id}` | S/K | (phase 5, FIX-225, C-1154, C-1155) waiting follow-ups with per-caller `controls`; steer (Stop authority), cancel (author or Stop authority), edit (author only); 403 `followup_forbidden`, 409 `followup_not_waiting`, `steer_unsupported` |
| POST `/v1/conversations/{id}/attachments` | S/K | multipart, returns pending attachment; (phase 2, RFE-015) 422 `attachment_quota_exceeded` when the org quota would be exceeded; (phase 6 (D-318, moved from phase 5), P4-S3) with the `s3` blob backend also 503 `blob_store_unavailable` (`retry-after: 5`) and 503 `blob_store_busy`; downloads (`GET /v1/attachments/{id}/content`) add 503 `blob_store_unavailable`, 503 `blob_backend_not_configured` and 502 `blob_store_corrupt`, never 404/410 for a store failure (`10 §Attachments`) |
| GET `/v1/attachments/{id}/content` | S/K | `Content-Disposition: attachment` always; (phase 2, RFE-015) 410 `attachment_purged` after retention/delete |
| DELETE `/v1/attachments/{id}` | S/K | (phase 2, RFE-015) purge the bytes now; uploader or `can_own` |

### Memory, skills, schedules, webhooks, links
| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/agents/{agent}/memories`; GET/PUT/DELETE `/v1/agents/{agent}/memories/{key}`; (P6-MEMLOCK, C-1148) PUT `…/memories/{key}/locked` (`setMemoryLocked`, idempotent: the same value changes nothing, a change is audited `memory.lock`/`memory.unlock` and bumps the home-sync generation, no revision; 409 `memory_locked` for any agent-authored write or delete of a locked memory) | S/K | |
| GET `…/memories/{key}/revisions`; POST `…/memories/{key}/restore`; POST `…/memories/{key}/revisions/{rev}/redact` | S/K | |
| GET `/v1/agents/{agent}/memories/search?q=` | S/K | |
| GET `/v1/agents/{agent}/helpers` (`listAgentHelpers`); GET `/v1/agents/{agent}/helpers/content?path=` (`getAgentHelper`); DELETE `/v1/agents/{agent}/helpers?path=` (`deleteAgentHelper`; without `path`, every helper file → 204) | S/K | (phase 6, P6-TPL-6, C-1143) the template helper files stored with the agent and projected read-only at `.aurora/helpers/<path>` (`12 §Home projection`); `can_use` of the agent's workspace (404 otherwise); list: `{ items: [{ path, size, executable, sha256 }] }` by path; content: the bytes with the C-903 headers and `X-Helper-Executable: true|false`; 400 `invalid_request` without `path`, 404 for an unknown path; delete: 204, audited `agent.helpers_remove` (ordinary; metadata: paths) only when something was removed; **no write route and no agent tool**: helpers are written only by the template create/update (P6-TPL-3, `agent_helpers::replace_all`) |
| GET/POST `/v1/agents/{agent}/skills`, `/v1/orgs/{org}/skills`; GET/PATCH/DELETE `/v1/skills/{id}`; GET/PUT/DELETE `/v1/skills/{id}/files/{path}` (GET: the stored bytes, C-1099; always `application/octet-stream` with `Content-Disposition: attachment` and `nosniff`) | S/K | the item routes work for every tier and authorize by the skill's tier (agent: `can_use`; org: org `owner`/`admin`; workspace, phase 3 RFE-020: the workspace's `ReadAuth` for the GET (FIX-236, review LOW-9: a C-1165 read), `can_own` for PATCH/DELETE and the files (own access; review LOW-9 names the two reads only)); items carry `tier: "agent"\|"workspace"\|"org"` (phase 3, D-107: next to the existing `scope`, which gains `"workspace"`) |
| GET/POST `/v1/workspaces/{ws}/skills` | S/K | (phase 3, RFE-020) list (FIX-236, review LOW-9: the workspace's `ReadAuth` — a C-1165 read) / create (`can_own`) workspace-tier skills; same body as the other tiers; 409 `skill_exists` in the workspace tier, 400 `invalid_request` for reserved/invalid names and 422 `skill_too_large` for size limits, as elsewhere (phase 3, D-107) |
| GET `/v1/agents/{agent}/skills/effective` | S/K | (phase 3, RFE-020) the effective set the agent receives: `[{ skill, tier, enabled, shadows: [{ id, tier }] }]` plus the skills that lose (`shadowed_by`); `can_use` |
| GET `/v1/agents/{agent}/template` (`getAgentTemplate`) | S/K | (P6-TPL-3, C-1112) `can_use`; `{ template: { template_id, name, version }\|null, available, update: null \| { version, content_sha256, readme, changelog: null, inputs_needed, apps_added, parts: [{ kind, key, state, current, proposed, skill_ref }] } }`; an agent not from a template answers `{ template: null, available: false, notes: [] }`; parts that differ get `state` (`unchanged_here`, `edited_here`, `removed_here`, `new`, `removed_upstream`); the texts are rendered with the stored inputs; a library skill's content is never a part |
| POST `/v1/agents/{agent}/template/apply` (`applyAgentTemplate`) | S/K | (P6-TPL-3, C-1112) `can_use`; `{ version, content_sha256, inputs?, skill_refs?, parts: [{ kind, key }] }` → 200 the new status; only the listed parts change, in one transaction (library fetches before it) through the same writers; 409 `template_version_changed` on a stale version or hash, 422 `template_part_unknown`, 409 `library_skill_readonly`; writes the parts rows and moves `template_version`; audit `agent.template_apply` (ordinary; metadata: from, to, parts) |
| DELETE `/v1/agents/{agent}/template` (`detachAgentTemplate`) | S/K | (P6-TPL-3, C-1112) `can_use`; forgets the provenance (columns null, parts deleted, 204); no further offers; helper files and library skills stay; 404 for an agent not from a template; audit `agent.template_detach` (ordinary) |
| GET/POST `/v1/agents/{agent}/schedules`; GET/PATCH/DELETE `/v1/schedules/{id}`; GET `…/runs`; POST `…/run-now` | S/K | (phase 2, RFE-021) items expose `creator`, `anchor` (`creator`\|`workspace`); POST/PATCH accept `anchor` (`can_own` only; tool-created items are always `creator`); (phase 5, P5-REFINE-7, C-1074, C-1077) every run gets its own conversation (`schedule:<id>:<run>`); `Schedule` gains `next_runs` (three instants, empty when paused) and `last_result`; `ScheduleRun` gains `conversation_id`, `result` (`running`\|`answered`\|`skipped_by_gate`\|`failed`\|`skipped`) and `reason`; `run-now` → 201 with `result: "running"` and the new `conversation_id`, 409 `schedule_busy` while the latest run's turn is active; (phase 5, P5-SCHED-ONCE, D-421, C-1092) POST/PATCH take `run_at` (a local `YYYY-MM-DDTHH:MM[:SS]` without an offset, read in `timezone`) instead of `cron`: exactly one of them on POST, at most one on PATCH (400 `invalid_request` at `/run_at` "set either cron or run_at, not both", at `/cron` when neither; 400 like every other schedule field error, the brief said 422); an offset, a daylight-saving gap, a time before the current minute or more than `SCHEDULE_ONCE_MAX_AHEAD` ahead → 400 `invalid_request` at `/run_at`; `Schedule.cron` and `run_at` (local, the request's format) are nullable, `completed_at` is set once a one-time schedule ran (Done; deleted after `SCHEDULE_DONE_RETENTION`); `enabled` on a Done one → 409 `schedule_completed`; a `timezone`-only PATCH keeps the local wall time; the 50-per-agent limit counts only rows that are not Done; `ScheduleRun.outcome` may be `waiting_paused` (`result: "skipped"`, `reason: "paused"`); run-now on a one-time schedule changes neither `run_at` nor `completed_at`; (P5-REFINE-14, C-1096) `Schedule.paused_by: { id, display_name } \| null`, set while a person's own pause stands |
| POST `/v1/agents/{agent}/schedules/preview` | S/K | (phase 5, P5-REFINE-7, C-1077) `can_use`, rate class `check`; `{ cron, timezone? }` → `{ next (three instants), interval_secs, too_frequent, min_interval_secs }`; 422 `invalid_cron` / `invalid_timezone` (`details.fields` names the field); saves nothing; (phase 5, P5-SCHED-ONCE, C-1092) also `{ run_at, timezone? }` (exactly one of `cron` / `run_at`, else 422 `invalid_run_at`) → `{ next: [instant], interval_secs: null, too_frequent: false, min_interval_secs }`, or 422 `invalid_run_at` with the create messages |
| PUT `/v1/schedules/{id}/creator`, PUT `/v1/webhooks/{id}/creator` | S/K | (phase 2, RFE-021) `{ user_id, re_enable? }`; `can_administer` (FIX-234, C-1160); the new creator must hold own `can_use` (422 `new_creator_unauthorized`); audited `automation.reassign` |
| POST `/v1/workspaces/{ws}/automations/reassign` | S/K | (phase 2, RFE-021) `{ from_user_id, to: <user_id> \| "workspace", kinds?: [schedule,webhook], ids? }` → `{ reassigned: n }`; `can_administer` (FIX-234, C-1160); the new creator must hold own `can_use`; one transaction, one audit row per item |
| GET/POST `/v1/agents/{agent}/webhooks`; GET/PATCH/DELETE `/v1/webhooks/{id}`; POST `…/rotate`; GET `…/deliveries` | S/K | rotate returns new URL once; (phase 2, RFE-021) same `creator`/`anchor` fields as schedules; (phase 5, P5-REFINE-7, C-1078, C-1079) create without `verification` means `{ kind: hmac_sha256, generate_secret: true }` and returns the generated `secret` once; `verification` shows the effective `header` and `format`; every accepted delivery gets its own conversation (`webhook:<id>:<delivery>`); deliveries carry `result`, `conversation_id`, `duplicate_of`, and a repeated delivery is recorded as `duplicate`; (P5-REFINE-14, C-1096) `Webhook.paused_by` as on schedules; (C-1097) the turn's `display` carries `verification` (`signed`\|`token`\|`none`) |
| POST `/v1/webhooks/{id}/rotate-secret` | S/K | (phase 5, P5-REFINE-7, C-1078) same callers as `rotate`; `hmac_sha256`/`bearer` only (422 `verification_not_signed`); → `{ webhook, secret }` once, the old secret stops working at once; audited `webhook.rotate_secret` |
| POST `/v1/hooks/{token}` | P | webhook receive (D-178 Q-R5-20: replay protection of a signed delivery without a delivery id is the sender's; 60 s dedup only) |
| GET `/v1/agents/{agent}/links`; PUT `/v1/agent-links/{id}/policy`; GET `/v1/agents/{agent}/peer-tasks` | S/K | (phase 5, C-1085) each `AgentLink` also carries `direction` (`both`\|`outgoing`\|`incoming`, from the asked agent's side), `created_by` and `peer.workspace_name` |
| GET `/v1/agents/{agent}/timeline` | S/K | (phase 5, P5-REFINE-9, C-1083, SEC-143) `can_use` of the agent's workspace (404 otherwise) → `TimelinePage { items: TimelineItem[], next_cursor }`; `limit` 1..100 (default 50), `cursor`, `kind` and `outcome` comma-separated; rows of a conversation the caller cannot see (another person's direct chat, unless `can_own`) are omitted, never counted; structured kinds and names, never sentences |
| GET `/v1/agents/{agent}/timeline/stream` | S | (phase 5, P5-REFINE-9, C-1084, SEC-143) server-sent events `row` (`{op: "upsert", item}`), `request` (`TimelineRequestEvent`: id, time, method, decision, status, app and tool, the host only with a visible conversation, no path, only with `?technical=true`) and `resync`; no SSE ids; `can_use`, re-checked before every batch (losing it, or being unable to check it, ends the stream); at most `TIMELINE_STREAMS_PER_USER` (5) per person per API replica (429 `rate_limited`); fed by `aurora_timeline`, `aurora_conversation` and `aurora_approval` hints |
| GET `/v1/agents/{agent}/linkable` | S/K | (phase 5, P5-REFINE-10, C-1085, SEC-142) `can_use` of the agent's workspace → `{ items: { agent_id, name, handle, workspace_id, workspace_name, linked, hosted, frozen }[] }`: same-organization agents of workspaces the caller can use (workspace key: its workspace), not the agent itself; at most 200, by workspace name then name |
| POST `/v1/agents/{agent}/links` | S/K | (phase 5, P5-REFINE-10, C-1085, SEC-142) `{ peer_agent_id, direction: both\|to_peer\|from_peer }` → 201 `AgentLink` (200 when already linked the same way); `can_use` of **both** agents' workspaces (else 404); 409 `link_self` \| `not_hosted` \| `link_exists` (`details.link_id`); 423 `frozen`; each side starts `allow` when the caller `can_approve` it, else `ask`; audited `agent_link.create` (critical) |
| PUT `/v1/agent-links/{id}/direction` | S/K | (phase 5, P5-REFINE-10, C-1085) `{ agent_id, direction, revision }` → 200 `AgentLink`; `can_use` of `agent_id`'s workspace; widening needs `can_approve` on both sides (403 `approver_required`); 409 `stale_revision`; audited `agent_link.direction` (critical) |
| DELETE `/v1/agent-links/{id}` | S/K | (phase 5, P5-REFINE-10, C-1085) → 204; `can_use` of either side; cancels the pair's open and queued tasks, expires pending `message_agent` approvals; audited `agent_link.delete` (critical) |

### Skill libraries (phase 4, P4-GSL, D-148, D-149; C-570..C-579; phase 5, P5-SKLIB-1/2, D-448, D-449, C-1122..C-1128)

404 `skill_libraries_disabled` while `skills.library.enabled` is false. Authorization per row;
tier authority = agent `can_use`, workspace `can_own`, org owner/admin (`04 §Permission matrix`).
(P5-SKLIB-1, D-448) A library is an **https git URL** read by the API itself (git smart HTTP,
C-1125) with an optional sealed read-only token (C-1126); no app connection and no gateway route.
(P5-SKLIB-2, D-449) A library with `org_id` null is an **instance library**, managed by instance
admins and visible to every organization (C-1128).

| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/orgs/{org}/skill-libraries` | S/K | list (org owner/admin; others get `{ id, label, status, scope }` of active libraries for the browse picker): the org's libraries by label, then (P5-SKLIB-2) the instance libraries by label (owners/admins: every status, so a paused one shows as Paused; members: only the `active` ones) with `scope: "instance"`, counts, `pending_updates` and `status` (`over_budget` from this org's `skill_library_org_state` row) of **this org only**, `token_updated_at` null / create `{ label, url, ref?, path?, token?, token_username? }` (org owner/admin; C-1122: the URL normalized by C-1123, a page URL fills `ref`/`path` when absent, one read of the repository before saving, the token sealed) → 201 `SkillLibrary { id, scope, org_id, label, status, url, repo, ref, path, token_set, token_updated_at, last_head_sha, last_refresh_at, last_error, skill_counts: { agent, workspace, org }, pending_updates, created_at }` (never the token); 422 `library_url_invalid` (`details.reason`: `not_https`, `ssh_url`, `credentials_in_url`, `query_or_fragment`, `not_a_repository_path`, `too_long`), 422 `library_unreachable` (sanitized reason), 422 `library_tree_too_large`, 400 `invalid_request` (label/ref/path/token/`token_username` without `token`, field errors as everywhere in this API), 409 `library_exists` (same label, or same normalized url + ref + path in the org), 409 `library_limit`; 429 `rate_limited` (a create is a repository read: it shares the check's 10 per minute per person); 502 `library_unavailable` when this replica is reading too many repositories (`LIBRARY_READS_CONCURRENT`) or the read passed its 90 s deadline; audit `skill_library.create` (critical; `url`, `ref`, `path`, `token: "set"` when one was given) |
| POST `/v1/orgs/{org}/skill-libraries/check` | S/K | (P5-SKLIB-1, C-1124) org owner/admin: the create body without `label`; reads the repository like a create and answers 200 `LibraryCheck { url, repo, ref, resolved: { kind: "branch"\|"tag"\|"commit", name, default }, commit_sha, path, skills: { found, valid, names }, truncated, suggested_label }`; stores nothing (no row, no head, no audit); a token in the body is used once; same errors as create; 429 `rate_limited` after 10 per minute per person; `Cache-Control: no-store` |
| GET/POST `/v1/admin/skill-libraries`; POST `/v1/admin/skill-libraries/check` | S (IA) | (P5-SKLIB-2, C-1128) instance admins, **session only**: list every instance library (`scope: "instance"`, full `SkillLibrary` with totals over every organization and `orgs_using`) / create as the org route with `scope = "instance"`, at most `LIBRARIES_PER_INSTANCE` (409 `library_limit`), labels unique among instance libraries; audit instance row `skill_library.create` / check as C-1124 |
| GET/PATCH/DELETE `/v1/skill-libraries/{id}` | S/K | org library: its org owner/admin; instance library (P5-SKLIB-2): instance admin, session only, and GET also by an org owner/admin with `?org=<their org>` (that org's view, as in the org list); anyone else 404. PATCH `{ label?, status?: "active"\|"paused", ref? ("" clears), path?, token?: string\|null, token_username?, url? }` (C-1122: a change of url, ref, path or token re-reads the repository first and never touches stored skills; `token: null` removes it; `url` only while the stored one is null, else 409 `library_url_fixed`; a patch that re-reads also draws on the check's 10 per minute per person, 429 `rate_limited`, and on the replica's read permits, 502 `library_unavailable`); DELETE detaches every skill of the library (`10 §Skills`), deletes the sealed token and audits the count (an instance library: the instance row and one `skill_library.delete` row in each organization whose skills were detached); audit `skill_library.update` (`change`, `url`, `ref`, `path`, `token: "set"\|"replaced"\|"removed"`)/`delete` |
| POST `/v1/skill-libraries/{id}/refresh` | S/K | "Check now": who may manage the library (as PATCH); 202; 429 `library_check_too_soon` within `LIBRARY_CHECK_NOW_INTERVAL` |
| GET `/v1/skill-libraries/{id}/skills?q=&cursor=&org=` | S/K | browse the current head live (`10 §Skills`): `{ commit_sha, items: [{ path, name, description, valid, reason?, size, files, added: [{ tier, id }] }] }`; org members who can use any workspace of the org; an instance library (P5-SKLIB-2) needs `org` (400 `invalid_request` without it, 404 when the caller cannot use a workspace of it) and `added` lists only that org's skills the caller can see; for an org library `org` is optional and must be its org; 502 `library_unavailable` with the sanitized reason; never stores anything; `Cache-Control: no-store` |
| GET `/v1/skill-libraries/{id}/skills/{path}?commit_sha=&org=` | S/K | one skill's full content at a commit (what an add would store, and the diff base for updates); same audience and `org` rule; same validation result |
| POST `/v1/agents/{agent}/skills`, `/v1/workspaces/{ws}/skills`, `/v1/orgs/{org}/skills` | S/K | (existing routes) body may be `{ from_library: { library_id, path, commit_sha } }` instead of `name/description/body/files`; the commit sha is **required** and fetched exactly; 422 `library_skill_invalid`, `library_budget_exceeded`, 409 `skill_exists`; audit `skill.library.add` (critical) |
| PATCH/PUT/DELETE `/v1/skills/{id}` (+ `/files/{path}`) | S/K | for a library skill: body, description, files → 409 `library_skill_readonly`; `enabled` and DELETE as usual (tier authority) |
| GET `/v1/skills/{id}/library` | S/K | `{ library: { id, label, repo, path }, commit_sha, pinned_at, pinned_by, update: null \| { id, kind, commit_sha, created_at, diff, content_sha256, over_budget } }`; the diff is the whole stored-vs-proposed content, not a summary; readable by whoever can read the skill |
| POST `/v1/skills/{id}/library/updates/{uid}/approve` `{ content_sha256 }` | S | tier authority; 409 `update_superseded`, 422 `library_skill_invalid`, 422 `library_budget_exceeded`; audit `skill.library.update_apply` (critical); bumps the home-sync generation of the affected agents |
| POST `/v1/skills/{id}/library/updates/{uid}/reject`; POST `/v1/skills/{id}/library/detach` | S | tier authority; audits `skill.library.update_reject`, `skill.library.detach` (critical) |
| GET `/v1/orgs/{org}/skill-library-updates`, `/v1/workspaces/{ws}/skill-library-updates`, `/v1/agents/{agent}/skill-library-updates` | S/K | pending proposals visible to the caller's authority at that scope (org: org owner/admin sees all of the org; workspace: `can_own`; agent: `can_use`), with `{ skill, library, kind, created_at }` — the dashboard badge and the Skills pages' "Update available" |

Items of every skill route gain `source: null | { kind: "library", library_id|null, label, repo, path, commit_sha, pinned_at, detached: bool, update_pending: bool }`;
`GET /v1/agents/{agent}/skills/effective` shows it per skill. The pages are paginated like every list.

### Agent templates (phase 6, P6-TPL-2, D-435, D-436; C-1106..C-1110, C-1113, C-1142)

Instance administrators add template repositories by their https git address (C-1123's parser, the
skill library's git reader). A check reads the tracked branch, downloads the package every new
version waits for (`awaiting_package`), and accepts it only when its content equals the source at
the recorded commit; the version then waits for an admin's approval. A version still
`awaiting_package` past `TEMPLATE_PACKAGE_WAIT` from its creation — measured by the database's
clock, so replica clock skew cannot move the expiry — becomes `invalid` with the reason it last
failed with, and that version string is never tried again (a new version supersedes it). A
failure that can resolve on its own is **transient**: the package download (`download_failed`),
the repository read (`source_unreadable`) and this replica's avatar re-encode
(`avatar_reencode_failed`). A version whose last reason is transient gets one more retry window
before it expires, so a network failure in the very last attempt does not retire a version that
nothing is wrong with; `package_not_found` (the package URL answered 404) and every content
verdict are not transient. The offered version is the
newest `approved` while the template is offered (one rule, C-1108). 404 `templates_disabled` on the
organization and workspace routes while `agents.templates.enabled` is false; the admin routes keep
working.

**Template sources (C-1107)** — instance administrators, **session only**.

| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/admin/agent-template-sources` | S (IA) | list (creation order: the stored manifest, `status`, `check_failed_at`/`reason` (never the token, never a read detail), `last_checked_at`, `next_check_at`, `token_set`/`token_updated_at`) / add `{ url, ref?, path?, token?, token_username?, homepage?, contact?, update_interval? }`: the URL normalized by C-1123 (a page URL fills `ref`/`path` when absent), one read of the repository before saving, the token sealed; 422 `template_url_invalid` (`details.reason` as `library_url_invalid`), 422 `template_unreachable` (sanitized reason), 422 `template_manifest_invalid`, 409 `template_source_exists` (same normalized url + ref + path), 409 `template_source_limit` (`TEMPLATE_SOURCES_MAX`); 429 `rate_limited` (an add is a repository read: 10 preview/add per minute per person); 502 `template_unavailable` when this replica reads too many repositories; audit `agent_template_source.add` (critical; `url`, `ref`, `path`, `token: "set"` when one was given) |
| POST `/v1/admin/agent-template-sources/preview` | S (IA) | (C-1124's shape) the add body without the label semantics: reads the repository once and answers 200 `AgentTemplateSourcePreview { url, repo, ref, resolved, commit_sha, path, manifest: { name, description, packages_url (the manifest's `packages.url` pattern, as text) }, templates: [{ template_id, dir, name, summary, version, state, problems }], truncated }`; stores nothing, no audit row; a token in the body is used once; same errors as add; `Cache-Control: no-store` |
| GET/PATCH/DELETE `/v1/admin/agent-template-sources/{id}` | S (IA) | one source from the stored rows (never a live read) / patch `{ update_interval?, ref?, path?, token?, token_username?, url? }` — a change of url, ref, path or token re-reads the repository first and on failure the old address parts stay; `token: null` removes it; the `never` interval unschedules; DELETE removes the source with its templates and versions (agents made from them are copies and do not change), the organizations' switches stay; audit `agent_template_source.update`/`remove` (critical) |
| POST `/v1/admin/agent-template-sources/{id}/check` | S (IA) | "Check now": the full C-1108 path, synchronous, under the same lease as the poll; 409 `template_check_in_progress` when busy; a read failure is a 200 result (`status: "failed"`), not an error |
| POST `/v1/admin/agent-template-sources/check-all` | S (IA) | every source sequentially; a failing or busy source never blocks the others |

**The templates, ingest and review (C-1108)** — instance administrators, **session only**. The
poll job checks due sources under their lease (`agents.templates.update_interval`, default 1 h;
a source with an `awaiting_package` version is due again within `TEMPLATE_PACKAGE_RETRY`).

| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/admin/agent-templates` | S (IA) | every template of every source (creation order): `name`, `summary`, `source: { id, name }`, `offered`, `removed_upstream`, `head_version`/`head_state`/`head_problems`, `current_version` (the offered version), `versions` (newest first: `status`, `status_reason`, `commit_sha`, `size_bytes`, `decided_by`/`decided_at`, `last_attempt_at`), `needs_apps` (installed on the instance?) and `needs_libraries` (a matching library added here? the C-1142 address rule) of the current or newest pending version |
| GET `/v1/admin/agent-templates/{template_id}/versions/{version}` | S (IA) | the review, everything in full: `TemplateVersionDetail { template_id, version, status, status_reason, commit_sha, source: { id, name, repo, ref, path }, content_sha256, archive_sha256, size_bytes, content: TemplateContentView }`; the content is the stored package's, never a live read |
| GET `/v1/admin/agent-templates/{template_id}/versions/{version}/avatar` | S (IA) | `image/png` from the stored content; 404 without one |
| POST `…/versions/{version}/approve` `{ content_sha256 }` | S (IA) | only from `pending_approval` and only when the hash is still the stored one; 409 `template_content_changed` (stale review or lost race), 409 `invalid_state`; audit `agent_template.approve` (critical, in the write's transaction) |
| POST `…/versions/{version}/reject` | S (IA) | only from `pending_approval`; that version string is never ingested again; 409 `invalid_state`; audit `agent_template.reject` (critical) |
| PATCH `/v1/admin/agent-templates/{template_id}` `{ offered }` | S (IA) | withdraw or offer again; the stored versions are untouched (the offered version is a rule over them); people keep using the current version while it is offered; audit `agent_template.update` (critical) |

**Organization switches (C-1109)** — org owner/admin.

| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/orgs/{org}/agent-templates` | S/K | every offered template, by name: `{ template_id, name, summary, version (the offered version, null when none is approved), enabled }`; every template is on by default |
| PUT `/v1/orgs/{org}/agent-templates/{template_id}` `{ enabled }` | S/K | writes or deletes the `org_disabled_agent_templates` row (keyed by the template id, so a source removed and added again keeps the choice); 404 for an unknown template (no row stored); audit `org.template_availability` (ordinary) |

**Workspace catalogue (C-1110)** — `can_use` on the workspace, S/K; everything about the offered
version. Availability: the app is active on the instance and available to the workspace under
`07 §App availability`; a template is available when it is offered and not turned off for the
workspace's organization (a not-available template is 404 `not_found`).

| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/workspaces/{ws}/agent-templates` | S/K | the available templates, by name: `TemplateCard { template_id, name, summary, readme (in full, D-458), version, source_name (the source's display name), has_avatar, apps: [{ app_ids, required, available, skills (the available apps' app skills) }], skill_refs: [{ index, kind, name, required, available, repo (the reference's library address in C-1142's display form; null for `organization`) }], has_helpers, automations: [{ kind, name }], ready }`; `ready` = every required app entry has an available app and every required skill reference is available |
| GET `/v1/workspaces/{ws}/agent-templates/{template_id}` | S/K | `TemplatePlan` = the card's fields + `content_sha256`, `content: TemplateContentView`, `entries: [{ index, required, why, choices: [{ app_id, app_name, available, unavailable_reason: "not_installed"\|"not_available_here"\|null, level (the template's; `act` on a `custom_only` app becomes `ask` with `level_note: "custom_only"`), custom: { allow_tools, ask_tools, dropped_tools } from the app's current catalogue\|null, accounts: [Account] (C-917 shape, the C-504 check, with `limit`) }] }]`, `skill_refs: [SkillRefPlan]` (C-1142's resolution: `available`, `unavailable_reason` (`library_not_added`, `skill_not_in_library`, `library_unreadable`, `libraries_disabled`, `not_in_organization`), `repo` (the reference's library address in C-1142's display form; null for `organization`), `library: { id, label, scope, repo, ref, path, commit_sha }\|null`, `content: { use_it_when, body, files }\|null`) |
| GET `…/{template_id}/avatar` | S/K | the offered version's avatar, `image/png`; 404 `not_found` when the template is not available or has none |
| POST `/v1/workspaces/{ws}/agents/from-template` (`createAgentFromTemplate`) | S/K | (P6-TPL-3, C-1111) `can_use`; `{ template_id, version, content_sha256, name, handle?, inputs, accounts: [{ entry, app_id, connection_id, level?, custom? }], skill_refs: [{ index, library_id, commit_sha }], automations: [{ kind: "schedule", name, enabled, timezone? }] }` → 201 `{ agent, grants, skills, webhooks (secret once), notes }`; every check of C-1111 before any write (409 `template_version_changed` with the fresh plan in `details.plan`, 422 with `details.fields` naming entries/indexes, 403 as C-504 for an account the plan did not list or that is not `active`), then **one transaction** through the existing writers: the hosted agent (effort, brief, avatar), starter grants (D-414), own skills, chosen library skills pinned through the add path (422 `library_budget_exceeded` rolls back), helper files, memories (locked ones locked by the caller), schedules with the caller as creator, always-paused webhooks, each account's access through the one grant writer with `via: "template"` (`grant.create` critical rows), and the C-1112 provenance; notes report `tools_dropped`, `access_not_given`, `skill_skipped`; any failure rolls everything back; audit `agent.create` (ordinary) with metadata `template: "<id>@<version>"`; never writes rules, secrets, channels, presences, resource overrides, BYO settings or libraries (SEC-146) |

### Channels (management)
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/connectors` | S/K | active connectors with setup schemas, capabilities; (FIX-127, C-715) setup fields may carry `advanced` and `choices: "from_validate"` |
| GET `/v1/connector-availability` | S/K | (FIX-127, C-712, D-218) `{ items: [{ type_id, name, icon_url, setup_state: active\|not_added\|waiting_for_host\|disabled\|needs_approval, connector_id }], instance_admins: [{ name }] }` — what an org admin sees when no connector is usable: one item per type announced by a live host plus unannounced connector rows; no secrets, no settings; admin names only (≤ 10); (phase 5, P5-REFINE-6, C-1068) `setup_state` may also be `registering` (a live host announces the type, the first registration is pending; it resolves within one host poll) and `waiting_for_host` means no live host announces the type; each item gains `host_running`, `can_connect` and `next_step: connect\|owner_connects\|start_host\|approve\|enable\|add` (`connect` for a caller who is an organization owner/admin), all from the one readiness function the admin list uses |
| POST `/v1/orgs/{org}/channel-integrations:probe` `{ connector_id, config, credentials }` | S/K | (FIX-127, C-711, D-217) org owner/admin, `write` class: one correlated `credentials.validate` with `mode: "probe"`; 200 `{ choices: { <field>: [{ value, label }] }, tenant_label, warnings[] }`; stores and logs nothing; errors (API-wide conventions) 400 `invalid_request` (body shape, unknown or inactive `connector_id`; `details.fields`), 403, 404 `not_found` (org the caller is not in), 422 `credentials_invalid`/`probe_unsupported`, 502 `connector_error`, 503 `connector_unavailable` |
| GET/POST `/v1/orgs/{org}/channel-integrations`; GET/PATCH/DELETE `/v1/channel-integrations/{id}` | S/K | create validates via `credentials.validate` op; (phase 1, RFE-001) `config.verified_email_autolink` (bool, default false) is writable by org owner/admin only and refused with 422 `autolink_unsupported` when the integration lacks `email_verified`; changes are audited (security-critical set from phase 2, RFE-008) |
| GET/POST `/v1/channel-integrations/{id}/user-links`; DELETE `/v1/user-links/{id}` | S/K | admin links; each item carries `method` (`verified_email` \| `sign_in_link` \| `admin`) |
| GET/POST `/v1/agents/{agent}/presences`; DELETE `/v1/presences/{id}`; POST `/v1/presences/{id}/reattach` | S/K | |
| GET `/v1/agents/{agent}/space-bindings`; DELETE `/v1/space-bindings/{id}`; PATCH `/v1/space-bindings/{id}` | S/K | owners only |
| POST `/v1/agents/{agent}/space-bindings` `{ integration_id, space_ref, listen? }` | S/K | (FIX-127, C-713, D-218) `can_own`; 201 `SpaceBinding`; only for a space the caller's **linked chat account is a member of** and the shared bot is already in; 422 `no_shared_presence`; 409 `link_required`, `bot_not_in_space` (with `hint`), `already_bound`, `space_already_bound`; 403 `not_a_member`; 422 `space_not_bindable` (direct conversation); 404 `integration_not_found`; 503 `connector_unavailable`; audit `space_binding.create`, the bot announces itself as `bind` does |
| GET `/v1/integrations/{id}/spaces?mine=true&query=&limit=` | S/K | (FIX-127, C-714) org owner/admin or `can_own` in the org: `{ items: [{ space_ref, label, kind: "channel"\|"private", bot_present: true }] }` (the bot's own `spaces.list` view, so `bot_present` is always true) — `spaces.list` ∩ the caller's memberships, DMs excluded; 409 `link_required`, 404 `integration_not_found`, 503 `connector_unavailable`, 400 without `mine=true` |
| GET `/v1/me/channel-links`; DELETE `/v1/me/channel-links/{id}`; GET `/v1/channel-links/preview?token=`; POST `/v1/channel-links/confirm` | S (preview, confirm: SO) | sign-in link flow; the preview shows the chat account, confirm repeats its reference (FIX-112); (FIX-220, C-36) confirm takes an optional `agent_id`: same rule and same transaction as the DM-default PUT, refused (422 `agent_not_reachable`) with nothing linked and the token unused |
| GET/PUT `/v1/me/dm-defaults` | S | (FIX-220, C-1147) the PUT attaches an own agent (FIX-230, C-1157: not another member's agent through an admin's implicit access; 422 `agent_not_reachable`) that is not on the shared bot yet, in one transaction with the default |
| GET `/v1/me/dm-default-options?integration_id=` | S | (FIX-220, C-1147; FIX-230, C-1157) the agents of the integration's organization that are the caller's own (access in their own right, not an admin's implicit access), `attached` per agent, and the shared bot; empty with `shared_bot: null` when the integration offers no shared mode; caller must be an active member of the org |
| GET `/v1/agents/{agent}/reach`; POST `/v1/reach-grants/{id}/decision` | S/K | |

### Instance administration
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/admin/overview` | IA | versions, gateways, hook hosts, runners, connectors, active snapshot; (phase 4) `counters: { backend, state }` (P4-CNT, `15`) and `blobs: { backend, healthy }` (phase 6, D-318, P4-S3) |
| GET `/v1/admin/attention` | IA | (FIX-167 Part B, D-370, C-1022) what needs an instance admin: `{ items: [{ kind, subject_id, title, detail, since, href, decision }] }`; `kind` `connector_approval`, `package_approval`, `addon_key_changed`, `host_claim_conflict` (decisions), `connector_down`, `ownerless_org`; computed on read, decisions first; 403 for anyone else. (phase 5, P5-REFINE-8, D-417, C-1080) `?state=open\|done`; the items gain the fields of C-1080 (`id`, `level`, `topic`, `actors`, `actors_total`, `reason`, `own`, `action`, `can_decline`, `state`, `closed_at`, `message`, `closed_by_role`, `closed_by`) and the list gains `count`; new kinds `app_request` and `chat_platform_request` for the requests whose resolving level is `administration` |
| GET `/v1/workspaces/{ws}/attention?state=open\|done`; GET `/v1/orgs/{org}/attention?state=open\|done` | S/K | (phase 5, P5-REFINE-8, D-417, D-407, C-1080) the same `AttentionList` per level. Workspace (any active member; 404 otherwise): `approval` (the caller can decide), `access_request` (the caller can own the workspace), `broken_account` (fix for whoever can, "Ask <role>" for everyone else) `agent_cannot_answer` (P5-REFINE-20 batch 4, C-1080: a hosted agent that has no usable AI key or model; owners of the workspace only, counted, topic `agents`, action "Fix" → the agent's Access tab; `since` is when the agent last lost a key — its newest `grant.delete` of a secret grant — else when it was created) and the caller's own `my_request` (`own: true`, never counted). Organization (owners and admins; 404 for a non-member, 403 for a member): `app_request`, `chat_platform_request`, `ai_key_request` whose resolving level is `organization`. `done`: closed in the last 30 days |
| GET `/v1/attention/counts?org=&workspace=` | S/K | (phase 5, P5-REFINE-8, D-417, C-1082) `{ administration, organization, workspace }`: per scope the caller can act on and asked for, `{ count, topics }` equal to the matching feed's `count`; `null` otherwise (never 403/404). Cheap: no sentences, no names |
| GET/PATCH `/v1/admin/users` | IA | list, instance-admin flag, disable; (phase 4, P4-IDP-1, D-139) each user carries `idp_roles: { provider, values, synced_at, granted: [...] }`; claim-derived instance-admin flags cannot be changed here (409 `managed_by_idp`) |
| GET `/v1/admin/settings` | IA | (phase 5, SET-1, D-236, C-771) the settings index: per section its generation, last writer (`updated_by_label` and, C-1102, `updated_by: { id, display_name } | null`) and counts of locked / dashboard keys; plus every locked key with its Helm key (the Admin › Settings banner) |
| GET `/v1/admin/settings/{section}` | IA | (phase 5, SET-1, C-772) one section: every key's effective value, source (`helm` \| `dashboard` \| `default`), lock, Helm key and default; keyed-list entries with their own source; secrets only as "set"; 404 `setting_section_unknown`. `export` and `import` are not section ids; (phase 5, P5-IA-5, D-341, C-972, C-973) adds `fields[]` (label, help, kind, choices with labels, default in words, unit, operator doc) from the field registry, and for a section not yet editable `editable: false` with `current[]` (read-only values and their source) |
| PUT `/v1/admin/settings/{section}` | IA | (phase 5, SET-1, C-773) replace the section's dashboard values (`{ generation, values }`); session only ((phase 5, FIX-157, D-317) the re-authentication within 600 s it needed is removed, no 412 `reauth_required`); 409 `stale_revision` / `setting_locked` (a locked key or whole list), 400 `setting_unknown`, 422 `settings_invalid` (including `entry_id_in_helm` when a dashboard entry of a keyed list — OIDC providers, secret stores, LLM endpoints — reuses a Helm entry's id) / `setting_requires_deployment`; security-critical audit `instance_settings.update` with the diff; notifies `aurora_settings` and e-mails every instance admin |
| GET `/v1/admin/secret-stores` | IA | (phase 5, FIX-124, C-746) the store registry: every store's source (built in, Helm, dashboard), settings without credentials, health (cached), value counts, reference scopes, the default, and the migration state |
| PUT / DELETE `/v1/admin/secret-stores/{id}` | IA | (phase 5, FIX-124, C-746) add or replace / remove a dashboard-defined Vault store: C-773 on the `secrets` section (session only, CAS, critical audit; (phase 5, FIX-157, D-317) no re-authentication); a store that still holds values or is the default is not removed (409) |
| PUT `/v1/admin/secret-store-default`, PUT `/v1/admin/secret-stores/{id}/reference-scopes` | IA | (phase 5, FIX-124, C-746) choose where new managed values go; edit a store's per-organization reference scopes (SEC-61); (FIX-244, D-486) a real change of the default tells every organization's owners after the commit (from which store to which store, by the dashboard's names; never a secret value, endpoint or path; the actor, an instance administrator, is not named and not told) |
| POST `/v1/admin/secret-store-test` | IA | (phase 5, FIX-124, C-746) Test connection: build the store from a definition, sign in, write/read/remove a probe value; nothing saved |
| POST `/v1/admin/secret-stores/migrations`, GET `/v1/admin/secret-stores/migrations/current` | IA | (phase 5, FIX-124, C-747) dry run, or the background job that moves every managed value to a store (one at a time, shared with the CLI); the running or last report |
| POST `/v1/admin/settings/{section}/preview` | IA | (phase 5, SET-1, C-774) the same body as PUT, nothing written: the diff, the keys that loosen, the organizations whose effective values change, warnings |
| GET `/v1/admin/settings/export` | IA | (phase 5, SET-1, C-775) the dashboard values of every (or the listed) section as YAML or JSON, no secret values; `export` rate class |
| POST `/v1/admin/settings/import` | IA | (phase 5, SET-1, C-776) a settings document: `dry_run` returns the per-section diff, otherwise all its sections are applied in one transaction (generations checked, session only, `instance_settings.import` critical audit; (phase 5, FIX-157, D-317) no re-authentication) |
| GET `/v1/admin/settings/governance/orgs` | IA | (phase 5, SET-2, C-778) where organizations stand against the instance floors: effective LLM / egress / open-lane / BYO postures, stricter-than-minimum and clamped keys per org (metadata only, paginated) |
| POST `/v1/admin/settings/llm/endpoint-test` | IA | (phase 5, SET-3, C-779) reachability probe of one endpoint entry as typed (no credential sent); (phase 5, P4-LLM, C-803) the probe path is the provider's own and any answer below 500 is reachable |
| POST `/v1/admin/settings/sign_in/provider-test` | IA | (phase 5, SET-4, C-780) OIDC discovery of one provider entry as typed: issuer match and endpoints, no sign-in |
| POST `/v1/admin/settings/email/test` | IA | (phase 5, SET-4, C-781) send a test e-mail to the caller with the effective or the proposed e-mail settings; `export` rate class |
| GET/PUT `/v1/admin/placement` | IA | (phase 4, P4-ONB-1, D-140) the new-user placement (`04 §New-user placement`; (phase 5, SET-4) the sign-up mode it shows read-only is the `sign_in` setting, linked); PUT validates org/workspace ids, roles, template and starter grants (credentials of the default org only) and is audited; `GET …/placement/preview?template=&sample=` renders a template for a typed sample or the caller's profile (D-151) (C-520..C-529); (phase 5, P5-UX-6, C-923, D-353) a new personal workspace's name is the person's own by a fixed rule; no setting is added |
| POST `/v1/admin/placement/starter-grants/backfill` | IA | (phase 5, P5-UX-6, D-326, C-922) `{ credential_ids? }`: give the starter grants to the default agent of every existing personal workspace of the default org that lacks them; idempotent, audited per grant; `{ granted, skipped }` |
| GET `/v1/admin/setup` | IA | (phase 4, P4-ONB-2, D-141) setup-wizard status per step (roles, placement, LLM endpoints incl. `default_model`, runners, gateway), each `ok` \| `warning` \| `todo` with a reason code; (phase 5, P5-UX-6, D-326, C-922, C-924) also `personal_workspaces_without_starter` and the reasons `app_needs_oauth_client` (`app_id`, `package`, `client_key`), `app_token_only` and `email_not_configured` |
| GET/POST `/v1/admin/plugins` (multipart upload); GET `/v1/admin/plugins/{id}`; POST `…/approve`, `…/reject`, `…/disable`, `…/enable`; DELETE `…?force=` | IA | (phase 3, RFE-022) GET `{id}` of a pending major upgrade carries `impact` (`09 §10`: `connections_needing_reauth`, `by_app`); (phase 4, P4-GRANTS) `impact` also carries `grants_to_convert` (count, by app and workspace) for an upgrade that introduces `grants.custom_only`, and approval then converts them atomically (`09 §10`, reason `converts_grants`); `approve` activates and flips the failing connections to `reauth_required` in one transaction and returns the `PluginPackage` plus `connections_reauth: n` (phase 3, D-107); (phase 5, P5-GUIDE-2, C-648; P5-APPSKILL-1/2, C-1136, C-1137) `PluginPackage` carries `agent` (per app: `summary`, `instructions`, `operations`, `skills[]` with their files and executable bits, `changed` against the active version; replaces `agent_guides`), and the audit rows `plugin.approve` / `plugin.activate` / `plugin.auto_approved` carry `agent_changed` (replaces `agent_guide_changed`); (phase 5, P5-POLISH-3, D-381, C-1032, SEC-136) list items carry `origin` (`aurora` \| `uploaded` \| `directory` \| `addon`), `summary` and `reaches`; project-signed builtins (and, with `plugins.allow_unsigned` in dev, unsigned ones in `plugins.builtin_dir`) are approved at install **and upgrade**, audit `plugin.auto_approved`; (phase 5, P5-REFINE-6, C-1071) a refused upload (`422 package_invalid`) says so in words: `message` is a sentence ("That isn't an app package. Use a `.tar.gz` made with `aurora plugin pack`.", or "That app package has N problems. The first: …") and each `details.violations[]` gains `sentence` next to the raw `code`, `path`, `message`; the 413 says "That package is too large (limit 20 MB)."; (phase 5, FIX-201, D-438, C-1120) the list takes `?package_id=` (exact match, with or without `?status`; no match is an empty page) |
| GET `/v1/admin/llm-endpoint-warnings` | IA | (phase 5, P5-POLISH-3, D-385, C-1034) per endpoint with a warning, the model roles agents on it may ask for that it does not map or allow (`role_unset` \| `role_not_allowed`; `role_not_served` needs a key's model listing and stays on `model-readiness`), from the endpoint's defaults |
| GET `/v1/admin/plugins/{id}/apps/{app_id}/skills/{name}/file?path=` | IA | (phase 5, P5-APPSKILL-2, C-1136, SEC-150) one file of an app skill of the package (`path` absent = `SKILL.md` as shipped) as `text/plain; charset=utf-8`, `nosniff`, `Content-Disposition: attachment`, `Cache-Control: private, no-store`; 404 for an unknown package, app, skill or file, 400 `invalid_request` for a path outside the skill path grammar (`24 §9`); replaces `…/agent-guides/{app_id}/{file}` |
| GET `/v1/admin/plugins/{id}/apps/{app_id}/operations` | IA | (phase 5, P5-APPSKILL-2, C-1136) the operations file of the app, same headers; 404 when the package, app or file is unknown |
| GET `/v1/admin/host-claims`; PUT `/v1/admin/host-claims/{claim_key}` | IA | conflicts and winners |
| GET/POST/DELETE `/v1/admin/trusted-keys` | IA | |
| GET/POST `/v1/admin/addon-sources`; POST `…/preview`, `…/check-all`; GET/PATCH/DELETE `/v1/admin/addon-sources/{id}`; POST `…/{id}/check`, `…/{id}/confirm-key` | IA | (phase 5, P5-ASRC, D-250, C-590..C-609) third-party app-package repositories: registration is TOFU over the manifest's signing key (`09 §10`, format in `22-addon-repos.md`); checks (manual and scheduled) share one lease-protected path; a changed signing key flags the source for re-confirm and never auto-trusts (SEC-86) |
| GET `/v1/admin/audit` | IA | (FIX-125, C-638) the instance audit log: `scope=instance` (default; rows with `org_id` null) or `all`; `org` (id, implies `all`), `actor_id`, `action` (prefix), `critical`, `since`/`until` (RFC 3339, `until` exclusive); keyset cursor as the org route; rows are `AuditEntry` plus `org_slug`. Unknown `scope` → 400; (phase 5, P5-REFINE-6, C-1088) `include_system` and `hidden_system` as on the org route |
| GET `/v1/admin/usage` | IA | (phase 5, P5-USAGE-1, D-360, C-1003) the instance's usage: `?from=&to=&org_id=` as C-1000 → `{ from, to, totals, by_day, by_org, by_workspace (top `USAGE_TOP_MAX`), other_workspaces, by_model, runtime_as_of }`; tokens, turns, turn time and awake time; no cost |
| GET/POST/DELETE `/v1/admin/connectors`; POST `…/{id}/approve`, `…/{id}/token` | IA | provision token, approve manifest. (phase 4, P4-MCON) `POST` with `managed: true` adds a managed connector (no token; settings, secrets, reviewed digest approved, C-451); PATCH `/v1/admin/connectors/{id}` edits settings, secrets, `enabled` (C-453); `…/token` on a managed connector → 409 `connector_managed`; (FIX-167, D-369, C-1021) each connector carries `update_pending`, `pending_version` and, while an update waits, its `widening`; `…/approve` approves the pending update (the connector keeps running at the approved level meanwhile); (FIX-183, D-398, C-1044) each connector carries `has_icon: bool`, true exactly when `GET /v1/connectors/{id}/icon` would serve a PNG (the connector is `active` and its manifest carries an icon); the icon route is unchanged and still 404s without one; (phase 5, P5-REFINE-6, C-1068) each connector carries `setup_state`, `host_running` and `next_step` from the same readiness function as `/v1/connector-availability` (`next_step` here: `owner_connects` once usable) |
| GET `/v1/admin/connector-types` | IA | (phase 4, P4-MCON) catalogue of built-in connector types announced by live connector hosts (C-450) |
| GET/POST/DELETE `/v1/admin/runners`; POST `…/{id}/token` | IA | (FIX-121, D-211) the list carries each runner's capacity (C-694); the rotation takes `?grace_secs=` and keeps the replaced token working for that long (C-695) |
| POST `/v1/admin/storage/refresh` | IA | (FIX-126) recompute the instance snapshot now, same body as the GET; `export` class |
| GET `/v1/admin/storage` | IA | (phase 2, RFE-015) instance storage breakdown (`10 §Attachments`); (FIX-126) served from a snapshot shared by every replica, `computed_at` says how old (`STORAGE_SNAPSHOT_MAX_AGE`, recomputed by the daily sweep, by a read past the bound — once, under a lock — and by the refresh route); (phase 6 (D-318, moved from phase 5), P4-S3) plus `blobs: { backend, by_backend: [{ backend, attachments, bytes }], s3?: { bucket, prefix, healthy, last_probe_at, sse, versioning } }` (never credentials; counts come from `attachments`, not from listing the bucket); (phase 5, P5-REFINE-6, C-1072) `categories: [{ id: conversations\|files\|activity, bytes }]` (always the three, summed from `tables`: `transcript_events` + `transcript_search`, `blobs` + `attachments`, `request_log` + `audit_log`; empty in a snapshot written before them); each `tables` row carries its `category` |
| GET `/v1/admin/registry/snapshot` | IA | active snapshot (JSON) |
| POST `/v1/admin/orgs/{org}/freeze` `{ reason }`; POST `/v1/admin/orgs/{org}/unfreeze` | IA | (phase 2, RFE-006) operator kill-switch; `frozen_authority = instance_admin`; org data stays inaccessible |
| GET `/v1/admin/orgs` (`frozen=true` filter, paginated) | IA | (phase 2, RFE-006, D-76) organizations with their freeze state: id, slug, name only; (phase 5, P5-ADMDEL, C-733) also `ownerless`, `member_count`, `workspace_count`, `agent_count`, `created_at` |
| GET `/v1/admin/orgs/{org}`; GET `/v1/admin/orgs/{org}/workspaces` (paginated) | IA | (phase 5, P5-ADMDEL, D-234, C-733) one organization's row as above; its workspaces: `id`, `slug`, `name`, `personal`, `agent_count`, `created_at` — metadata only |
| DELETE `/v1/admin/orgs/{org}`; DELETE `/v1/admin/workspaces/{ws}` `{ confirm }` | IA (SO) | (phase 5, P5-ADMDEL, D-234, C-733) delete any organization / workspace: `confirm` = its slug (400 `confirmation_mismatch`; (phase 5, FIX-157, D-317) the recent OIDC re-authentication, 412 `reauth_required`, is removed), works on frozen orgs; the owner paths' services; audit `admin.org.delete` / `admin.workspace.delete` (security-critical, instance-level) |
| GET/POST `/v1/admin/oauth-clients`; PATCH/DELETE `/v1/oauth-clients/{id}` | IA | instance-level OAuth clients (`08 §OAuth clients`) |

### Telemetry (phase 5, FIX-146, D-232; C-732)
| Method & path | Auth | Purpose |
|---|---|---|
| POST `/v1/telemetry/traces` | SO | (phase 5, FIX-146) browser span ingestion, C-732 (6): OTLP/HTTP JSON (`ExportTraceServiceRequest`, `content-type: application/json`) from the dashboard; CSRF and the `telemetry` rate-limit class apply; limits `TELEMETRY_INGEST_BODY_MAX` (256 KiB, 413), `TELEMETRY_INGEST_SPANS_MAX` (512), `TELEMETRY_SPAN_ATTRIBUTES_MAX` (32), `TELEMETRY_SPAN_EVENTS_MAX` (8). The API never trusts the payload: it rebuilds every span (resource replaced, spans failing the sampling rule dropped, names outside the C-732 shapes renamed `browser`, browser attribute allowlist, id-like route/page segments dropped, links and `tracestate` removed, timestamps clamped, SEC-88) and exports through its own exporter. 204 on success (also when every span was dropped); 400 `invalid_request` on malformed input; 401 without a session; 429 over the rate limit; **404 when `telemetry.otlp_endpoint` is not set** (like an unknown route) |

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
rule 5); the `/v1/relay`, `/v1/runner`, `/v1/terminator` and `/scim/v2` planes use bearer tokens and
are not affected.

### Terminator plane (phase 5, P4-SSH-2, D-239; C-794)

All of it is 404 while `ssh.enabled` is false. Bearer `aut_…` (`AURORA_BOOTSTRAP_TERMINATOR_TOKEN`,
compared by hash in constant time; an unknown or malformed token is 401 with one body for every
cause) and, after registration, `X-Aurora-Replica: <replica_id>`. Its own rate class
(`SSH_API_RATE`, per replica), exempt from the management limiter. Wire shapes: `16 §W20`.

| Method & path | Auth | Purpose |
|---|---|---|
| POST `/v1/terminator/register` | `aut_` | `{ version, host_public_key }` → `{ replica_id, protocol: 1, config: { ca_public_keys, heartbeat_secs, lease_secs, wake_wait_secs, idle_timeout_secs, max_lifetime_secs, limits } }`; upserts `ssh_terminator_replicas` |
| POST `/v1/terminator/sessions` | `aut_` + replica | open a session after the client's signature verified (`04 §SSH access` point 2); `{ certificate, username, client_address }` → 201 `{ session_id, agent_id, sandbox: "running"\|"waking"\|"stopped", lease_secs, expires_at }` or 403 `{ reason }` — a **bare body**, not the error envelope (`denied` for every access refusal; `limit` over a cap) — the terminator answers both with the same hint-free failure; a parked sandbox gets the wake demand; audit `agent.ssh.open` |
| POST `/v1/terminator/sessions/{id}/channels` | `aut_` + replica | `{ mode: "shell"\|"exec"\|"sftp", wait_secs?: ≤ 45 }` → `{ grant: "auz_…", expires_in: 30, runner_endpoint }`; 403 `session_closed` (closed, lease passed or `can_ssh` fails: the row is closed at detection); 409 `not_running` with `retry_after_secs` while the sandbox wakes after `wait_secs` (a stopped sandbox gets the wake demand, within the shared budget; over it 429 `wake_limited`); 429 `ssh_channel_limit` |
| POST `/v1/terminator/heartbeat` | `aut_` + replica | `{ sessions: [{ id, last_activity_at?, channels_open, channels_total, bytes_in, bytes_out }] ≤ 500 }` → `{ revoked: [{ id, reason }], config }`; renews each open session's lease, re-runs `04 §SSH access` point 4, refreshes `ssh_terminator_replicas.last_seen_at`; a listed session that is unknown or already closed comes back revoked |
| POST `/v1/terminator/sessions/{id}/close` | `aut_` + replica | `{ reason, channels_total, channels_by_mode, bytes_in, bytes_out }` → `{ ok }`; idempotent: the first close wins (close-exactly-once), a repeat answers `{ ok }` and changes nothing |
| POST `/v1/runner/preview-streams/open` (`kind: "ssh"`) | `aur_` + replica | (runner plane) redeem an `auz_` grant: `16 §W20` |
| POST `/v1/runner/terminal/keep-awake` (`kind: "ssh"`) | `aur_` + replica | (runner plane) the keep-awake stamp of a byte-carrying SSH channel: `ssh_active_until`, `16 §W20` |

A stale-session sweep (every API replica, every 30 s, claims with `SKIP LOCKED`) closes `open` rows whose
`lease_until` passed (`lease_expired`) through the same guarded transition.
