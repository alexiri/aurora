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
12. Audit log, request-log reads, runtime usage.

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
| BYO agent | `Authorization: Bearer aug_…` (agent token) | only `POST /v1/agent/context-credentials` and `GET /v1/agent/container-config` |
| Runner | `Authorization: Bearer aur_…` + `X-Aurora-Replica: <id>` (after register) | `/v1/runner/**` |
| Connector | `Authorization: Bearer aun_…` + `X-Aurora-Replica` | `/v1/connector/**` |
| Public capability | token in path | `POST /v1/hooks/{token}`, `GET /v1/public/avatars/{key}`, `GET /v1/oauth/callback`, OIDC callbacks |
| Internal services (hook host) | `Authorization: Bearer <internal secret>` | `/internal/v1/**` |
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
- SSE: `Content-Type: text/event-stream`; durable events carry `id: <seq>`;
  ephemeral events carry no id; keepalive comment every `SSE_KEEPALIVE`;
  resume with `Last-Event-ID` or `?after_seq=`.

## Event bus (Postgres `LISTEN/NOTIFY`)

Every process that needs signals holds one dedicated LISTEN connection.
Notifications are **hints**: a consumer that (re)connects its listener MUST
resynchronize from the database (SSE re-reads from last seq, gateways flush
caches, waiters return early). Payloads are JSON ≤ 7 000 bytes.

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
| Approval expiry: `pending` past `deadline + 30 s` → `expired`; pending rows of gateway instances unseen for 60 s → `abandoned` | every 10 s | idempotent updates |
| Action-approval expiry | every 60 s | idempotent |
| Attachment pending sweep (`ATTACH_PENDING_TTL`) | hourly | idempotent |
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
   (`TURN_STALL`), ceiling warnings and ceiling failures (`TURN_CEILING`).
9. **Schedules**: claim due schedules (`fire_lease_until`), create turns (`10`).
10. **Watches**: convert watches of lost/parked processes; expire watches.
11. **Housekeeping slices**: close runtime intervals for stopped boots; mark
    processes of ended boots `lost`/`parked`.

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
| App availability change | `org` |
| Org governance change (and the stored-origin re-evaluation it triggers) | `org` |
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
| GET/PATCH/DELETE `/v1/me` | S (DELETE: SO) | profile, memberships, default org |

### Organizations, members, keys
| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/orgs` | S | list mine / create |
| GET/PATCH/DELETE `/v1/orgs/{org}` | S/K | |
| GET `/v1/orgs/{org}/members`; PATCH/DELETE `/v1/orgs/{org}/members/{user}` | S/K | role, suspend |
| GET/POST `/v1/orgs/{org}/invitations`; DELETE `/v1/invitations/{id}` | S/K | |
| GET `/v1/invitations/preview?token=`; POST `/v1/invitations/accept` | none / SO | |
| GET/POST `/v1/orgs/{org}/api-keys`; DELETE `/v1/api-keys/{id}` | S (POST: SO) | org keys |
| GET `/v1/orgs/{org}/audit` | S/K | |
| GET/PUT `/v1/orgs/{org}/app-availability` | S/K | posture + allow-list |
| GET/PUT `/v1/orgs/{org}/governance` | S/K | org governance document (`08 §11.2`) with `revision`; PUT validates against the instance ceiling (422 `governance_invalid` / `governance_below_instance_minimum`) |
| GET `/v1/orgs/{org}/governance/effective` | S/K (any org member) | effective LLM and egress governance (instance ∩ org), each value marked `source: instance|org` (phase 1, RFE-003: includes `egress.open_lane`); non-compliant LLM secrets of the org |
| GET `/v1/governance/instance` | S | the instance ceiling (config, without secrets) — read-only |

### Workspaces
| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/orgs/{org}/workspaces` | S/K | |
| GET/PATCH/DELETE `/v1/workspaces/{ws}` | S/K | name, timezone, memory_language |
| GET `/v1/workspaces/{ws}/access`; PUT/DELETE `/v1/workspaces/{ws}/access/{user}` | S/K | bindings |
| GET/POST `/v1/workspaces/{ws}/api-keys` | S (POST: SO) | workspace keys |
| GET `/v1/workspaces/{ws}/audit` | S/K | |
| GET `/v1/workspaces/{ws}/activity` | S/K | request log, filters: agent, conversation, decision, host, app |
| GET `/v1/workspaces/{ws}/usage` | S/K | runtime + tokens |

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
| GET/POST `/v1/workspaces/{ws}/oauth-clients`, `/v1/orgs/{org}/oauth-clients`; PATCH/DELETE `/v1/oauth-clients/{id}` | S/K | BYOC |

### Grants and effective views
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/agents/{agent}/grants` | S/K | |
| PUT/DELETE `/v1/agents/{agent}/grants/connections/{conn}` | S/K | `{ mode, allow_tools, ask_tools }` |
| PUT/DELETE `/v1/agents/{agent}/grants/secrets/{secret}` | S/K | |
| GET `/v1/connections/{id}/grants` | S/K | agents granted |
| GET `/v1/agents/{agent}/effective` | S/K | effective credentials and per-tool decisions with `org_ceiling` |

### Policy
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/workspaces/{ws}/policy`, `/v1/orgs/{org}/policy` | S/K | published + draft + revision |
| PUT `…/policy/draft` | S/K | whole-draft replace with `revision`; (phase 1, RFE-003) workspace scope also carries `open_lane_posture` (`enabled` \| `allowlist_only` \| `disabled`; a workspace can only tighten, so `enabled` simply adds no restriction beyond the org and instance) |
| POST `…/policy/publish`, `…/policy/rollback` | S/K | `{ revision }` / `{ generation }` |
| GET `…/policy/generations` | S/K | history |
| GET `…/policy/warnings` | S/K | overlap/inert-rule warnings for the draft |
| POST `/v1/workspaces/{ws}/policy/simulate` | S/K | `{ agent_id, method, url, body?, use_draft?, lane? }` → decision trace; (phase 1, RFE-003) `lane: "open"` evaluates CONNECT-time semantics (`07`) and the open-lane posture |

### Approvals
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/workspaces/{ws}/approvals?status=` | S/K | |
| GET `/v1/approvals/pending?wait=` | S/K | long-poll across what the caller may decide |
| GET `/v1/approvals/stream` | S | SSE of created/decided |
| POST `/v1/approvals/{id}/decision` | S/K | `{ "decision": "approve"|"deny" }`; first decision wins (409 `already_decided`) |
| POST `/v1/approvals/batches/{batch_id}/decision` | S/K | all pending of the batch the caller may decide |
| GET `/v1/workspaces/{ws}/action-approvals`; POST `/v1/action-approvals/{id}/decision` | S/K | |

### Agents and sandboxes
| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/workspaces/{ws}/agents` | S/K | create returns `aug_` once for BYO |
| GET/PATCH/DELETE `/v1/agents/{agent}` | S/K | name, brief, harness, model, effort, resource overrides (IA) |
| POST `/v1/agents/{agent}/token` | S | BYO: regenerate, returns token once |
| GET `/v1/agents/{agent}/container-config` | S/K | BYO bootstrap (`08 §BYO bootstrap`) |
| GET `/v1/agent/container-config`; POST `/v1/agent/context-credentials` | A | BYO self-service (`08`) |
| PUT/GET `/v1/agents/{agent}/avatar`; GET `/v1/public/avatars/{key}` | S/K; P | |
| POST `/v1/agents/{agent}/restart` | S/K | restart sandbox |
| GET `/v1/agents/{agent}/sandbox` | S/K | state, keep-awake reasons, processes, ports |
| GET `/v1/agents/{agent}/models` | S/K | models available from granted LLM keys |
| GET `/v1/gateway/ca.pem` | S/K | gateway CA certificate |

### Conversations and turns
| Method & path | Auth | Purpose |
|---|---|---|
| GET `/v1/agents/{agent}/conversations` | S/K | filters: kind |
| POST `/v1/agents/{agent}/conversations/direct` | S | get-or-create caller's direct conversation |
| POST `/v1/agents/{agent}/conversations` | K | `kind = api`, `external_ref` (get-or-create) |
| GET `/v1/conversations/{id}` | S/K | |
| GET `/v1/conversations/{id}/events?after_seq=&limit=` | S/K | durable history |
| GET `/v1/conversations/{id}/stream` | S/K | SSE |
| POST `/v1/conversations/{id}/messages` | S/K | `{ text, attachment_ids? }` → `{ turn }` or `{ followup }` |
| POST `/v1/turns/{id}/abort`, `/v1/turns/{id}/send-now` | S/K | |
| POST `/v1/conversations/{id}/attachments` | S/K | multipart, returns pending attachment |
| GET `/v1/attachments/{id}/content` | S/K | `Content-Disposition: attachment` always |

### Memory, skills, schedules, webhooks, links
| Method & path | Auth | Purpose |
|---|---|---|
| GET/POST `/v1/agents/{agent}/memories`; GET/PUT/DELETE `/v1/agents/{agent}/memories/{key}` | S/K | |
| GET `…/memories/{key}/revisions`; POST `…/memories/{key}/restore`; POST `…/memories/{key}/revisions/{rev}/redact` | S/K | |
| GET `/v1/agents/{agent}/memories/search?q=` | S/K | |
| GET/POST `/v1/agents/{agent}/skills`, `/v1/orgs/{org}/skills`; GET/PATCH/DELETE `/v1/skills/{id}`; PUT/DELETE `/v1/skills/{id}/files/{path}` | S/K | |
| GET/POST `/v1/agents/{agent}/schedules`; GET/PATCH/DELETE `/v1/schedules/{id}`; GET `…/runs`; POST `…/run-now` | S/K | |
| GET/POST `/v1/agents/{agent}/webhooks`; GET/PATCH/DELETE `/v1/webhooks/{id}`; POST `…/rotate`; GET `…/deliveries` | S/K | rotate returns new URL once |
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
| GET/POST `/v1/admin/plugins` (multipart upload); GET `/v1/admin/plugins/{id}`; POST `…/approve`, `…/reject`, `…/disable`, `…/enable`; DELETE `…?force=` | IA | |
| GET `/v1/admin/host-claims`; PUT `/v1/admin/host-claims/{claim_key}` | IA | conflicts and winners |
| GET/POST/DELETE `/v1/admin/trusted-keys` | IA | |
| GET/POST/DELETE `/v1/admin/connectors`; POST `…/{id}/approve`, `…/{id}/token` | IA | provision token, approve manifest |
| GET/POST/DELETE `/v1/admin/runners`; POST `…/{id}/token` | IA | |
| GET `/v1/admin/registry/snapshot` | IA | active snapshot (JSON) |
| GET/POST `/v1/admin/oauth-clients`; PATCH/DELETE `/v1/oauth-clients/{id}` | IA | instance-level OAuth clients (`08 §OAuth clients`) |

### Planes (machine callers)
Runner plane `/v1/runner/*` and connector plane `/v1/connector/*` are specified
in `16 §W5` and `13 §Connector contract`.
