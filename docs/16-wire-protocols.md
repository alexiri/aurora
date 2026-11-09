# 16 — Wire Protocols

Every inter-component contract. Types live in `aurora-protocol` (serde +
schemars); JSON Schemas are generated into `docs/schemas/` of the
implementation repo and checked in (CI fails on drift).

## Index

| # | Contract | Direction | Auth | Owner doc |
|---|----------|-----------|------|-----------|
| W1 | Proxy protocol | agent → gateway :4102 | `Proxy-Authorization` gateway credential; (phase 1, RFE-002) carried inside a TLS session — TLS-only listener | `06` |
| W2 | Management API | browser/CLI/SDK → API `/v1` | session / API key | `05` |
| W3 | Gateway signals | API → gateways (`NOTIFY`), gateways → DB | DB roles | `05`, `06` |
| W4 | Gateway internal | API/monitoring → gateway :4103 | internal secret (ring from phase 2, `08 §4`) | `06` |
| W5 | Runner plane | runner → API `/v1/runner` | `aur_` + replica | this doc |
| W6 | Supervisor channel | supervisor → runner :4105 (WebSocket) | bootstrap token / HMAC session | this doc |
| W7 | Platform tools | harness → MCP bridge → supervisor → runner → API | stamped identity | this doc, `12` |
| W8 | Connector contract | connector → API `/v1/connector` | `aun_` + replica | `13` |
| W9 | Hook host | API/gateway → hookhost :4104; hookhost → API :4111 | internal secret (ring from phase 2, `08 §4`) | `09` |
| W10 | Platform callbacks | chat platform → connector | connector-defined | `20` |
| W11 | Upstream | gateway → internet | guarded resolver; (phase 4, P4-LLM) LLM providers add gateway-side SigV4 (Bedrock) and gateway-minted OAuth2 access tokens (Vertex), `08 §Phase 4 providers` | `06` |
| W12 | CLI / BYO | CLI → API; BYO agent → gateway | API key / agent token | `05`, `08` |
| W13 | Counter backend (phase 4, P4-CNT) | API, gateway → Postgres or Redis | database role / Redis ACL user over TLS | `15 §Shared rate-limit counters` |
| W14 | Blob store (phase 4, P4-S3) | API → S3-compatible bucket | SigV4 with workload identity or env keys, over TLS | `10 §Attachments`, `15 §S3-compatible blob store` |
| W15 | SCIM 2.0 (phase 4, P4-GRP-2) | IdP → API `/scim/v2` | `aud_` bearer per org | this doc, `04 §SCIM` |

## Cross-cutting conventions

| Convention | Rule |
|------------|------|
| Validation | Every message is parsed into typed structs at the receiver; batch parses are all-or-nothing, so senders MUST bound and validate before sending. |
| Versioning | Machine planes send `X-Aurora-Protocol: <plane>/<n>` (`runner/1`, `supervisor/1`, `connector/1`, `hookhost/1`). The API accepts N and N−1; unknown future versions → 426 with the supported range. |
| Open vocabularies | error codes, reason codes, capability names, connector ids, event/report kinds: unknown values are logged and skipped, never fatal. |
| Sender-side bounds | sizes, counts and string lengths enforced by producers (limits registry). |
| Idempotency | report ids, event sequence numbers, `source_event_id`, op ids, claim leases; (phase 3, RFE-018) the management API's `Idempotency-Key` on message creation (`05 §Idempotency keys`). |
| Time | the API's database clock is truth; timestamps from runners, supervisors and connectors are informational only. |
| Untrusted text | sanitized at the boundary where it enters platform voice. |
| Secrets in logs | never; URLs only in redacted form. |

---

## W5 — Runner plane (`/v1/runner`)

All requests: `Authorization: Bearer aur_…`; after registration also
`X-Aurora-Replica: <replica_id>`.

| Call | Request → Response |
|------|--------------------|
| `POST /register` | `{ version, backend, capabilities: RunnerCapabilities (optional `image`: the runner's `runner.image`, which the API reports as the sandbox image and uses in `sandbox.start` specs; optional `held_awake_ceiling`: the runner's `runner.held_awake_ceiling`, used for eviction, default `max(1, max_sandboxes − 1)`; optional `defaults: { cpu_millis, memory_mib, pids, home_gib }`: the runner's `runner.sandbox_cpus × 1000`, `sandbox_memory_mib`, `sandbox_pids`, `home_gib`, the API's resource-profile default, absent from older runners), advertised_host }` → `{ runner_id, replica_id, protocol: 1, config: { poll_wait_secs, reconcile_interval_secs, orphan_grace_secs, idle_stop_secs } }` |
| `POST /work` | `{ wait_secs ≤ 25, capacity: { running, max } }` → `{ items: [WorkItem], reconciler: bool }` (also the heartbeat; `reconciler` is true only for the oldest live replica of this runner, computed at poll time — spec fix S-08) |
| `POST /reports` | `{ reports: [Report] }` (≤ 200, ≤ 1 MiB) → `{ results: [{ report_id, status: "applied"|"duplicate"|"fenced"|"invalid", detail? }] }` |
| `POST /channel/claim` | `{ bootstrap_token }` → `{ sandbox_id, boot_id }` or 409 `bootstrap_spent` / 404 |
| `POST /channel/hold` | `{ sandbox_id, boot_id }` → `{ ok }` (records `channel_replica_id`; also used after HMAC-session reconnects) |
| `POST /channel/release` | `{ sandbox_id, boot_id }` |
| `POST /tool-calls` | `ToolCall` → `ToolResult` (synchronous, ≤ `TOOL_CALL_TIMEOUT`) |
| `POST /memory-writes` | `{ sandbox_id, boot_id, key, title?, description?, content }` → `{ revision }` or error |
| `POST /attachments` | headers `X-Aurora-Sandbox`, `X-Aurora-Boot`, `X-Aurora-Conversation`, `X-Aurora-Turn`, `X-Aurora-Filename`, `X-Aurora-Caption?`; body = file bytes → `{ attachment_id }` |
| `GET /attachments/{id}` | inbound file bytes for delivery (only attachments bound to turns delivered to this runner's sandboxes) |
| `GET /sandboxes` | → `{ sandboxes: [{ sandbox_id, agent_id, boot_id?, desired: "running"|"stopped"|"deleted", home_ref }] }` for this runner |
| `POST /sandboxes/check` | `{ sandbox_ids: [] }` → `{ known: [] }` (installation sweep) |
| `POST /strays` | `{ objects: [{ sandbox_id?, boot_id?, container_ref, created_at }] }` |
| `POST /retire` | clean shutdown of this replica |

`RunnerCapabilities`: `{ backend, home_durability: "resident"|"snapshot",
resources: { cpu: {min,max}, memory_mib: {min,max}, pids: {min,max}, home_gib:
{min,max} }, max_sandboxes, harnesses: ["claude-code","fake"], max_attachment_bytes,
image_capabilities?: ["browser"] }` (phase 4, P4-HAR: `harnesses` is read from the image label `org.aurora.harnesses`, absent = `["claude-code","fake"]`; the API places a sandbox only on a replica listing the agent's harness; `codex` is a new open-vocabulary id) (phase 1, RFE-003: the capabilities the
sandbox image declares via its `org.aurora.capabilities` label, read by the
runner when it registers; open vocabulary, absent = none).

### Work items

All carry `{ item_id, kind, sandbox_id, boot_id }`.

| Kind | Body |
|------|------|
| `sandbox.start` | `{ spec: { image, env: [[k,v]], boot_secret: base64url, home_ref, home_gib, resources: { cpu_millis, memory_mib, pids }, labels: {} } }` |
| `sandbox.stop` | `{ reason: "idle"|"cap"|"evict"|"restart"|"delete"|"failure", grace_secs }` |
| `home.sync` | `{ generation, part_index, part_count, part: HomeSyncPart }` |
| `turn.deliver` | `{ conversation_id, turn_id, session: { resume_ref?, epoch }, model: ModelChoice, context_credential: "auc_…", input: TurnInput, attachments: [{ attachment_id, filename, size }], gate?: { command, timeout_secs } }` |
| `turn.message` | `{ turn_id, followup_id, text, speaker_label }` |
| `turn.send_now` | `{ turn_id }` |
| `turn.abort` | `{ turn_id }` |
| `turn.notice` | `{ turn_id, text }` (platform notice steered into the live run, e.g. ceiling warning) |
| `session.release` | `{ conversation_id }` (surface changed; release idle session) |
| `session.purge` | (phase 2, RFE-010) `{ conversation_id, session_ref }` — delete the stored harness session files (`12 §Session purge`) |

`ModelChoice`: `{ provider, endpoint_id, base_url, dialect, model, effort,
client_env: [[k,v]] }`. (phase 4, P4-LLM) `provider` and `dialect` are open
vocabularies: `provider` gains `openai`, `azure_openai`, `bedrock`, `vertex`,
`openai_compatible`; `dialect` gains `anthropic_bedrock` and `anthropic_vertex`
(`openai_chat` and `openai_responses` already exist). A supervisor that does not know
a dialect refuses the turn with `harness_failed` and a clear message, never guessing.
`client_env` never carries an AWS key, a service-account key or a token (placeholders
or "skip auth" switches only). `TurnInput`: `{ text, speaker_label, door, recall: {
index, snippets: [] }, notices: [] }`. `HomeSyncPart`: `{ kind: "skills" |
"memory" | "render_inputs" | "tools" | "manifest" | "final", entries: [...] }`
(`12 §Sync protocol`). (phase 3, RFE-020) `skills` parts carry the agent's **effective** set only; the three tiers and their precedence are resolved by the API, so the wire format is unchanged.

### Reports

All carry `{ report_id (uuid, sender-generated), kind, sandbox_id, boot_id }`.
The API fences lifecycle reports on the current `(boot_id, owner_replica_id)`
and channel reports on `channel_replica_id`; fenced reports answer `fenced` and
are dropped.

| Kind | Body | API effect |
|------|------|------------|
| `sandbox.status` | `{ state: "running"|"stopped"|"failed"|"held", container_ref?, reason_code?, message?, observed?: { cpu_millis, memory_mib } }` | lifecycle transition (`11`) |
| `supervisor.ready` | `{ version, harness: { id, capabilities } }` | full home sync scheduled |
| `home.synced` | `{ generation }` | `home_applied_generation` |
| `turn.started` | `{ turn_id }` | `running` |
| `turn.events` | `{ turn_id, events: [{ n, event: HarnessEventWire }] }` | events with `n ≤ turns.last_event_n` are duplicates; durable ones persisted with seq; ephemeral ones NOTIFY'd |
| `turn.progress` | `{ turn_id }` | `last_progress_at` |
| `turn.result` | `{ turn_id, status: "done"|"failed"|"aborted", error_code?, error_message?, usage? }` | terminal state, context credential expiry, mirror, follow-up promotion |
| `steer.result` | `{ followup_id, outcome: "delivered"|"missed"|"refused" }` | `joining` bookkeeping |
| `send_now.result` | `{ turn_id, outcome: "moved"|"nothing_running"|"unsupported" }` | event |
| `session` | `{ conversation_id, session_ref, epoch }` | `harness_session_ref` (ignored if epoch is stale or the boot was restarted) |
| `process.state` | `{ processes: [ProcessState] }` | `processes` upsert |
| `watch.fired` | `{ watch_id, detail }` | watch turn |
| `gate.result` | `{ turn_id, outcome: "quiet"|"wake"|"failed", output? }` | schedule run outcome |
| `ports.state` | `{ ports: [int] }` | `listening_ports` |
| `session.purged` | (phase 2, RFE-010) `{ conversation_id, session_ref, ok }` | completes the `session_purges` row (`ok = false` keeps it pending) |

`turns.last_event_n` is maintained by the API for event idempotency.

---

## W6 — Supervisor channel (WebSocket)

- URL: `ws://<runner>:4105/v1/supervisor`; first message from the supervisor:
  `{ "hello": { "protocol": 1, "bootstrap_token": "aup_…" } }` or
  `{ "hello": { "protocol": 1, "session": { "sandbox_id", "boot_id", "issued_at", "mac" } } }`.
  Answer: `{ "welcome": { "session": {…new…}, "resume_from": <last acked seq> } }`
  or close with code 4401 (unauthorized) / 4409 (spent).
- Frames: JSON text messages `{ "seq": u64, "ack": u64, "kind": string, "body": {} }`,
  each ≤ `WS_FRAME_MAX`. `seq` is per direction and per connection lineage
  (survives reconnects within a boot); receivers apply each seq at most once
  and acknowledge via `ack` (piggybacked or `{kind:"ack"}` every 1 s). Senders
  keep unacked frames (bounded 4 MiB) and replay them after reconnect from
  `resume_from`.
- Ping/pong every 20 s; 60 s silence = dead connection.

Runner → supervisor kinds: `turn.deliver`, `turn.message`, `turn.send_now`,
`turn.abort`, `turn.notice`, `session.release`, `session.purge` (phase 2, RFE-010), `home.sync`, `tool.result`,
`memory.write.result`, `file.result`, `shutdown`.

Supervisor → runner kinds: `ready`, `home.synced`, `turn.started`, `turn.event`
(`{ turn_id, n, event }`), `turn.progress`, `turn.result`, `steer.result`,
`send_now.result`, `session`, `process.state`, `watch.fired`, `gate.result`,
`ports.state`, `tool.call`, `memory.write`, `file.part` (`{ upload_id, index,
data_b64 }` ≤ 144 KiB decoded, so the base64 frame stays under `WS_FRAME_MAX`; spec fix S-06), `file.commit` (`{ upload_id, conversation_id,
turn_id, filename, caption?, sha256, size }`).

The runner translates frames 1:1 into work items / reports / plane calls,
adding `sandbox_id` and `boot_id` from the authenticated channel (never from the
frame body). Reports derived from supervisor frames use a deterministic
`report_id = UUIDv5(NAMESPACE_AURORA_REPORT, boot_id ‖ ":" ‖ seq)` so a frame
replayed after a runner restart deduplicates at the API (spec fix S-07).
Unknown kinds are logged and dropped.

---

## W7 — Platform tool calls

```
harness ──MCP stdio──► mcp-bridge ──unix socket (JSON lines)──► supervisor
   ──tool.call {call_id, conversation_id, turn_id?, tool, args}──► runner
   ──POST /v1/runner/tool-calls {sandbox_id, boot_id, …}──► API tool registry
```

`ToolCall`: `{ sandbox_id, boot_id, call_id, conversation_id, turn_id?, tool,
args }`. The API verifies: the boot is current; the conversation belongs to the
sandbox's agent; when `turn_id` is set it is an active turn of that
conversation; the tool is available for the agent. `ToolResult`: `{ call_id,
ok: true, content: [{ type: "text", text } | { type: "json", value }] }` or
`{ call_id, ok: false, error: { code, message } }`. Local tools (`process_*`)
never leave the supervisor.

MCP surface: `tools/list` answered from the synced catalogue (+ local tools);
`tools/call` as above; no MCP resources or prompts in phase 0.

---

## W9 — Hook host (summary)

`POST /internal/v1/hooks/{connect.authorize|connect.exchange|token.mint|approval.summarize}`
with `{ package_id, package_digest, app_id, input }`; responses
`{ ok: output }` / `{ error: { code, message } }`; timeouts per `HOOK_WALL`.
Callers treat transport errors and timeouts as `credential_unavailable`
(mint), `connect_hook_failed` (connect) or fall back (summarize). See `09 §8`.

---

## W12 — CLI

`aurora` stores `{ api_url, api_key, workspace_id, agent_id? }` in
`~/.config/aurora/config.toml` (0600). Commands: `login` (device flow),
`logout`, `whoami`, `workspaces`, `agents list|create|token`, `run [--agent]
-- <cmd>` (container config applied to a child process), `approvals watch`
(long-poll, interactive approve/deny), `plugin …` (`09 §12`), `admin …`
(instance-admin helpers: plugins, runners, connectors).

---

## W13 — Counter backend (phase 4, P4-CNT)

Not an Aurora-defined protocol: the API and gateway call the `CounterStore` trait
(`15 §Shared rate-limit counters`), whose `postgres` backend speaks SQL to Postgres
(`03`) and whose `redis` backend speaks RESP over TLS (`rediss://`) with an ACL user
whose credentials come from environment variables. The contract Aurora owns is the
trait semantics (atomic, all-or-nothing, store clock, TTL) and the error mapping:
every transport failure is `Unavailable` and is handled by the class policy
(`local` / `closed`), never read as "allowed". Keys contain ids and keyed hashes only
(`05 §Conventions` *Shared counters*).

---

## W14 — Blob store (phase 4, P4-S3)

The API speaks the S3 REST API (`PutObject` with `x-amz-checksum-sha256`, the configured
`x-amz-server-side-encryption` and `If-None-Match: *`, `GetObject`, `DeleteObject`,
`ListObjectsV2`, `HeadBucket`, `GetBucketVersioning`; the versioned-bucket mode adds
`ListObjectVersions` and `DeleteObject?versionId`) signed with SigV4. Aurora owns only
the object key layout (`10 §Attachments`), the no-metadata / no-pre-signed-URL rules and the
error mapping: every transport, 5xx, throttling or credential error is
`blob_store_unavailable`; a missing object for a live row is `blob_store_corrupt` (logged);
nothing is ever mapped to 404/410, which are decided from the database row.

---

## W15 — SCIM 2.0 (phase 4, P4-GRP-2)

Authority and semantics: `04 §SCIM`. This section is the wire subset Aurora implements
(RFC 7643 schemas, RFC 7644 protocol), chosen from what Entra ID, Okta and Keycloak (a SCIM
extension) actually send. Everything else is declared unsupported in `ServiceProviderConfig`
and answered with the SCIM error below, never silently ignored.

**Transport.** Base `<external>/scim/v2`, TLS only (the external URL is https, or loopback in
development). `Authorization: Bearer aud_…` (`00`); no cookies, no CSRF/`Origin` check, no CORS.
Requests and responses use `application/scim+json` (a request with `application/json` is
accepted); bodies ≤ `SCIM_BODY_MAX`; `Accept` other than JSON → 406. Responses carry
`meta { resourceType, created, lastModified, location }` and no `ETag` (`etag.supported=false`);
resource ids are the SCIM ids of `03 §scim_users` / `groups.id`, UUIDs.

**Discovery** (same auth): `GET /ServiceProviderConfig` (`patch.supported = true`,
`bulk.supported = false`, `filter { supported: true, maxResults: SCIM_PAGE_MAX }`,
`changePassword`, `sort`, `etag` all `false`, `authenticationSchemes: [oauthbearertoken]`),
`GET /ResourceTypes`, `GET /Schemas` (the User core schema with the attributes below, the
Group schema; **no enterprise extension**: its attributes are accepted and ignored).

**Users** `urn:ietf:params:scim:schemas:core:2.0:User`. Attributes read and stored: `userName`
(required), `externalId`, `displayName`, `name.givenName|familyName|formatted`, `emails[]`
(`value`, `type`, `primary`; the primary or first is used, `04 §SCIM`), `active`. Returned:
`id`, `externalId`, `userName`, `displayName`, `name`, `emails` (one, `primary: true`), `active`
(`true` only when the link is `active`), `groups` (read-only: `{ value, display, $ref }` of the
SCIM groups the user is in), `meta`. Unknown attributes are ignored except `roles`,
`entitlements`, `password`, `x509Certificates`, `groups` on input, which are **refused 400
`invalidValue`** (`detail`: "not supported") so an IdP cannot believe it set a role. `active` is
accepted as a boolean or the strings `"True"`/`"False"` (Entra).

| Operation | Behaviour |
|-----------|-----------|
| `POST /Users` | `04 §SCIM` *Users*; 201 with the resource and `Location`; 409 `uniqueness` |
| `GET /Users/{id}` | 200; 404 for an unknown id, another org's id or a removed link |
| `GET /Users?filter=&startIndex=&count=` | list of this org's non-removed links; filter subset below |
| `PUT /Users/{id}` | full replacement of the stored attributes; `active` drives deprovisioning (`04 §SCIM`); owner → 403 |
| `PATCH /Users/{id}` | `Operations`; below |
| `DELETE /Users/{id}` | state `removed` (`04 §SCIM`); 204; repeating it is 204 (idempotent); owner → 403 |

**Groups** `urn:ietf:params:scim:schemas:core:2.0:Group`: `displayName` (required), `externalId`,
`members[] { value, display?, $ref? }` (`value` = SCIM user id; `type` ignored). `POST`, `GET
/Groups/{id}`, `GET /Groups` (the org's `source = scim` groups only; `members` omitted from lists
unless `?attributes=members`), `PUT` (replaces name and the whole member list), `PATCH`, `DELETE`
(204, idempotent). Manual groups are invisible here (404 on their ids). Members beyond
`SCIM_MAX_MEMBERS_PER_REQUEST` in one request → 400 `invalidValue`.

**Filter subset** (`filter=`): a single `<attr> eq "<value>"` (case-insensitive attribute names,
case-sensitive value except `userName`, e-mail and `displayName`, which compare
case-insensitively): Users `userName`, `externalId`, `id`, `emails.value`; Groups `displayName`,
`externalId`, `id`. Anything else (other operators, `and`/`or`/`not`, grouping, value paths in the
filter, an unknown attribute) → 400 `invalidFilter`. No `sortBy`. **Pagination:** `startIndex`
(1-based, default 1), `count` (default `SCIM_PAGE_DEFAULT`, max `SCIM_PAGE_MAX`; larger is
clamped); the response is `ListResponse { totalResults, startIndex, itemsPerPage, Resources }`
ordered by `created_at, id` (stable while the set changes within a sync; an index past the end
returns an empty page). `count=0` returns only `totalResults`.

**PATCH subset** (`urn:ietf:params:scim:api:messages:2.0:PatchOp`, ≤ `SCIM_MAX_PATCH_OPS`
operations, applied atomically in order: any failure applies none). `op` is `add`, `replace`
or `remove`, case-insensitive (Entra sends `Add`/`Replace`/`Remove`). Supported `path`s:
Users `active`, `userName`, `displayName`, `externalId`, `name.givenName`, `name.familyName`,
`name.formatted`, `emails`, `emails[type eq "work"].value` (and the same for `primary eq true`),
or **no path** with an object value naming those attributes (Okta, Entra); Groups
`displayName`, `externalId`, `members`, `members[value eq "<id>"]` (for `remove`), or no path
with an object value. `remove` of `active`, `userName` or a required attribute → 400
`noTarget`/`mutability`; any other path → 400 `invalidPath`; a `members` operation with an
unknown user → 400 `invalidValue`. `replace` on `members` replaces the whole list.

**Errors** (`urn:ietf:params:scim:api:messages:2.0:Error`, `status` a string, `scimType` where
RFC 7644 §3.12 names one, `detail` sanitized and never containing a token, e-mail of another
user or an internal id):

| HTTP | `scimType` | When |
|------|-----------|------|
| 400 | `invalidSyntax`, `invalidValue`, `invalidFilter`, `invalidPath`, `noTarget`, `mutability`, `tooMany` | malformed body or schema, bad value or e-mail domain, unsupported filter, unsupported path, missing target, immutable/unsupported attribute, over a limit |
| 401 | — | missing, malformed, unknown, revoked or expired token, SCIM disabled for the org: **one identical response** (`WWW-Authenticate: Bearer`) |
| 403 | — | owner mutation, seat limit, SCIM suspended by the operator |
| 404 | — | unknown or foreign resource, SCIM off at the instance (the whole surface) |
| 409 | `uniqueness` | duplicate `userName`/e-mail/`externalId`/group name |
| 413 | `tooMany` | body above `SCIM_BODY_MAX` |
| 415 / 406 | — | content type / accept not JSON |
| 429 | — | `SCIM_RATE` exceeded, with `retry-after` |
| 500/503 | — | database or audit failure (`audit_unavailable`: the change is not applied); the IdP retries |

**Idempotency and retries.** `POST` is not idempotent: IdPs probe with `GET …?filter=userName eq`
first, and a repeated `POST` for an existing user is 409 `uniqueness`, which Entra and Okta treat as
"already exists, then update". `PUT`, `PATCH` setting a value, `DELETE` and `members` add/remove of
an already-present/absent member are idempotent and succeed.

**Conformance.** The suite of `18 §SCIM` runs recorded request/response sequences of the three IdPs
(sync, deactivate, group membership churn, token rotation) and a protocol validator against
`/scim/v2`; fixtures contain no real tenants.
