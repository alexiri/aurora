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
| W4 | Gateway internal | API/monitoring → gateway :4103 | internal secret (ring from phase 2, `08 §4`); (phase 4, P4-GSL) adds `POST /internal/v1/library-fetch` | `06`, `06 §Library fetch` |
| W5 | Runner plane | runner → API `/v1/runner` | `aur_` + replica | this doc |
| W6 | Supervisor channel | supervisor → runner :4105 (WebSocket) | bootstrap token / HMAC session | this doc |
| W7 | Platform tools | harness → MCP bridge → supervisor → runner → API | stamped identity | this doc, `12` |
| W8 | Connector contract | connector → API `/v1/connector` | `aun_` + replica | `13` |
| W9 | Hook host | API/gateway → hookhost :4104; hookhost → API :4111 | internal secret (ring from phase 2, `08 §4`) | `09` |
| W10 | Platform callbacks | chat platform → connector | connector-defined | `20` |
| W11 | Upstream | gateway → internet | guarded resolver; (phase 5 (D-138, moved from phase 4), P4-LLM) LLM providers add gateway-side SigV4 (Bedrock) and gateway-minted OAuth2 access tokens (Vertex), `08 §Phase 4 providers`; (phase 5, P4-LLM spec batch, D-261) and gateway-minted Entra tokens (Azure) and STS `AssumeRole` sessions (Bedrock), from one mint service at pinned token endpoints | `06` |
| W12 | CLI / BYO | CLI → API; BYO agent → gateway | API key / agent token | `05`, `08` |
| W13 | Counter backend (phase 4, P4-CNT) | API, gateway → Postgres or Redis | database role / Redis ACL user over TLS | `15 §Shared rate-limit counters` |
| W14 | Blob store (phase 6 (D-318, moved from phase 5), P4-S3) | API → S3-compatible bucket | SigV4 with workload identity or env keys, over TLS | `10 §Attachments`, `15 §S3-compatible blob store` |
| W15 | SCIM 2.0 (phase 4, P4-GRP-2) | IdP → API `/scim/v2` | `aud_` bearer per org | this doc, `04 §SCIM` |
| W16 | Web Push (phase 5 (D-138, moved from phase 4), RFE-011b) | API → browser push service (FCM, Mozilla, Apple, WNS) | VAPID (RFC 8292) JWT signed by the instance key | this doc, `07 §Web Push` |
| W17 | Relay plane (phase 4, P4-PRV-2) | relay → API `/v1/relay`; runner → API `/v1/runner/preview-streams/*` | `aul_` + replica; `aur_` + replica | this doc, `04 §Preview access` |
| W18 | Preview byte streams (phase 4, P4-PRV-2) | relay → runner :4108 (WebSocket per stream); runner ↔ supervisor (stream frames on W6) | stream grant `aux_` (single use); the authenticated W6 channel | this doc, `11`, `12` |
| W19 | Terminal streams (phase 4, P4-TERM) | browser → API `/v1/agents/{agent}/terminal` (WebSocket); API → runner :4108 (grant redemption); runner ↔ supervisor (`pty.*` frames on W6) | terminal grant `auy_` (single use); the authenticated W6 channel | this doc, `11`, `12` |
| W20 | SSH access (phase 5, P4-SSH-2, D-239) | SSH client → terminator (SSH, certificate-only); terminator → API `/v1/terminator`; terminator → runner :4108 (one WebSocket per SSH channel); runner ↔ supervisor (`exec.*` frames on W6) | OpenSSH user certificate; `aut_` + replica; channel grant `auz_` (single use); the authenticated W6 channel | this doc, `04 §SSH access`, `11 §SSH access`, `12 §SSH channels` |
| W21 | Files sessions (phase 5, P5-FILES-1, D-316) | browser → API `/v1/agents/{agent}/files*` (REST); API → runner :4108 (one WebSocket per SFTP session); runner ↔ supervisor (`exec.*` frames on W6, mode `sftp`) | session cookie; files grant `auf_` (single use); the authenticated W6 channel | this doc, `11 §Files`, `12 §SSH channels`, API_CONTRACTS C-898..C-907 |

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
| Trace context | (phase 5, FIX-146, D-232, C-732, SEC-88) HTTP calls between Aurora services carry W3C `traceparent` (and `tracestate` when present), injected by the one outgoing-call helper; receivers adopt it parent-based only on internal listeners and the machine planes (`/v1/runner/*`, `/v1/connector-host/*`, `/v1/relay/*`). Calls to upstream apps, LLM providers, IdPs and chat platforms never carry an Aurora `traceparent`. The supervisor channel (W6) and everything a sandbox sends carry no trace context and are never adopted; work on a turn is parented by `turns.trace_context` (W5 work items below). |

---

## W5 — Runner plane (`/v1/runner`)

All requests: `Authorization: Bearer aur_…`; after registration also
`X-Aurora-Replica: <replica_id>`.

| Call | Request → Response |
|------|--------------------|
| `POST /register` | `{ version, backend, capabilities: RunnerCapabilities (optional `image`: the runner's `runner.image`, which the API reports as the sandbox image and uses in `sandbox.start` specs; optional `held_awake_ceiling`: the runner's `runner.held_awake_ceiling`, used for eviction, default `max(1, max_sandboxes − 1)`; optional `defaults: { cpu_millis, memory_mib, pids, home_gib }`: the runner's `runner.sandbox_cpus × 1000`, `sandbox_memory_mib`, `sandbox_pids`, `home_gib`, the API's resource-profile default, absent from older runners; optional `stream_tls`: the stream listener speaks TLS, default false — the API derives the endpoint scheme it hands the relay from it, `§W18`; (phase 5, P4-SSH-2) optional `ssh`: the runner redeems `kind: "ssh"` grants (`auz_`) and speaks `exec.*` in modes `shell`/`exec`/`sftp` with `tty`, default false), advertised_host }` → `{ runner_id, replica_id, protocol: 1, config: { poll_wait_secs, reconcile_interval_secs, orphan_grace_secs, idle_stop_secs, (optional, false when absent) previews, terminal, (P4-SSH-2) ssh_enabled: `ssh.enabled` and the runner advertised `ssh` — the runner then opens :4108 and redeems `auz_` grants, `§W20` } }`; `advertised_host` is a host name or an IP literal (`[v6]` in brackets) with an optional port and nothing else — a scheme, path, userinfo, query or whitespace is refused with 400 `invalid_request` (the relay dials it with a live stream grant, FIX-090) |
| `POST /work` | `{ wait_secs ≤ 25, capacity: { running, max } }` → `{ items: [WorkItem], reconciler: bool }` (also the heartbeat; `reconciler` is true only for the oldest live replica of this runner, computed at poll time — spec fix S-08) |
| `POST /reports` | `{ reports: [Report] }` (≤ 200, ≤ 1 MiB) → `{ results: [{ report_id, status: "applied"|"duplicate"|"fenced"|"invalid", detail? }] }` |
| `POST /channel/claim` | `{ bootstrap_token }` → `{ sandbox_id, boot_id }` or 409 `bootstrap_spent` / 404; idempotent for the same `(sandbox, boot)` while the boot has never held a channel (`channel_held_at` is NULL), spent afterwards (SEC-25) |
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

All carry `{ item_id, kind, sandbox_id, boot_id }`. (phase 5, FIX-146, D-232, C-732 (3)) Items
that concern a turn also carry `trace_context?: string`: the W3C traceparent
`00-<32 hex>-<16 hex>-<2 hex>` read from the turn's `turns.trace_context` (`03`), absent when the
column is NULL. The runner records its dispatch and sandbox-start spans for that turn as children of
it and treats an absent or malformed value as none (no error, a new root under the sampling rule).
It is never forwarded to the supervisor or the sandbox. Additive: an older runner ignores the field.

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
| `session.purge` | (phase 2, RFE-010) `{ conversation_id, session_ref, harness? }` — delete the stored harness session files (`12 §Session purge`); (phase 4, P4-HAR, D-153) `harness` names the adapter that deletes them (the one that created the session, after a switch); absent = the sandbox's own |
| `preview.start` | (phase 4, P4-PRV-1, D-145) `{ share_id }` — ask the supervisor to start the share's command on demand (`11 §Port previews`); the runner forwards `preview.start { port }` on the supervisor channel and the supervisor resolves the command from its own share list |

`ModelChoice`: `{ provider, endpoint_id, base_url, dialect, model, effort,
client_env: [[k,v]] }`. (phase 5, P5-MODELS-1, D-303, C-877) Optional additions: `roles: { large?, medium?,
small? }` (resolved model ids on the same endpoint), `subagent_role?: "large" | "medium" | "small"`,
`model_meta?: [{ model, context_window?, capabilities? }]`. If they are absent, the behaviour is as before.
A supervisor re-checks every id against `MODEL_ID_PATTERN` and refuses the turn with `harness_failed` on a
mismatch (SEC-122). `model` is the harness's default, not its only model (`08 §Model selection`). (phase 5 (D-138, moved from phase 4), P4-LLM) `provider` and `dialect` are open
vocabularies: `provider` gains `openai`, `azure_openai`, `bedrock`, `vertex`,
`openai_compatible`; `dialect` gains `anthropic_bedrock` and `anthropic_vertex`
(`openai_chat` and `openai_responses` already exist). A supervisor that does not know
a dialect refuses the turn with `harness_failed` and a clear message, never guessing.
`client_env` never carries an AWS key, a service-account key or a token (placeholders
or "skip auth" switches only). `TurnInput`: `{ text, speaker_label, door, recall: {
index, snippets: [] }, notices: [] }`. `HomeSyncPart`: `{ kind: "skills" |
"memory" | "render_inputs" | "tools" | "previews" | "manifest" | "final", entries: [...] }` ((phase 4, P4-PRV-1) a `previews` part has the single entry `{ shares: [{ share_id, port, start_command?, cwd? }] }`; an older supervisor that does not know the kind skips it, which only means no preview works)
(`12 §Sync protocol`). (phase 5, P5-GUIDE-2, C-643) The kind vocabulary gains `"apps"` (between `tools` and `previews`): file entries `{ path, content }` under `.aurora/apps/<app_id>/`, the entry shape of `skills`; an older supervisor skips the kind, which only means no app folders. (phase 3, RFE-020) `skills` parts carry the agent's **effective** set only; the three tiers and their precedence are resolved by the API, so the wire format is unchanged.

### Reports

All carry `{ report_id (uuid, sender-generated), kind, sandbox_id, boot_id }`.
The API fences lifecycle reports on the current `(boot_id, owner_replica_id)`
and channel reports on `channel_replica_id`; fenced reports answer `fenced` and
are dropped.

| Kind | Body | API effect |
|------|------|------------|
| `sandbox.status` | `{ state: "running"|"stopped"|"failed"|"held", container_ref?, reason_code?, message?, observed?: { cpu_millis, memory_mib } }` | lifecycle transition (`11`); (phase 4, P4-HAR, D-153) also sent **by the supervisor** (frame kind `sandbox.status`, forwarded 1:1) as `failed` / `harness_unsupported` instead of `ready` when the boot document's harness is unknown or its runtime is not in the image |
| `supervisor.ready` | `{ version, harness: { id, capabilities } }` | full home sync scheduled |
| `home.synced` | `{ generation }` | `home_applied_generation` |
| `turn.started` | `{ turn_id }` | `running` |
| `turn.events` | `{ turn_id, events: [{ n, event: HarnessEventWire }] }` | events with `n ≤ turns.last_event_n` are duplicates; durable ones persisted with seq; ephemeral ones NOTIFY'd |
| `turn.progress` | `{ turn_id }` | `last_progress_at` |
| `turn.result` | `{ turn_id, status: "done"|"failed"|"aborted", error_code?, error_message?, usage? }`. (phase 5, P5-MODELS-2, C-879) `usage` may carry `by_model` | terminal state, context credential expiry, mirror, follow-up promotion |
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

- URL: `ws://<runner>:4105/v1/supervisor` — `ws://` only until TLS is implemented (FIX-114:
  the supervisor refuses a `wss://` `runner_url` at boot, because the WebSocket client has
  no TLS support); first message from the supervisor:
  `{ "hello": { "protocol": 1, "bootstrap_token": "aup_…" } }` or
  `{ "hello": { "protocol": 1, "session": { "sandbox_id", "boot_id", "issued_at", "mac" } } }`.
  Answer: `{ "welcome": { "session": {…new…}, "resume_from": <last acked seq> } }`
  or close with code 4401 (unauthorized) / 4409 (spent).
- A bootstrap token admits one supervisor connection. Re-presenting it is accepted only while the boot
  has never held a channel (the first admission did not complete: the claim's answer was lost, or the
  runner closed 1013 before the welcome); once a channel was held it is spent (4409). The runner makes
  one `channel/claim` attempt per connection, and when the claim succeeded but `channel/hold` fails with
  a retryable error it admits the connection with the welcome and retries the hold in the background
  (FIX-094). The supervisor port admits at most 256 unadmitted connections per runner replica (further
  ones are closed at once) and gives each `hello_timeout` (10 s) for the upgrade and the hello.
- Frames: JSON text messages `{ "seq": u64, "ack": u64, "kind": string, "body": {} }`,
  each ≤ `WS_FRAME_MAX`. `seq` is per direction and per connection lineage
  (survives reconnects within a boot); receivers apply each seq at most once
  and acknowledge via `ack` (piggybacked or `{kind:"ack"}` every 1 s). Senders
  keep unacked frames (bounded 4 MiB) and replay them after reconnect from
  `resume_from`.
- Ping/pong every 20 s; 60 s silence = dead connection.

Runner → supervisor kinds: `turn.deliver`, `turn.message`, `turn.send_now`,
`turn.abort`, `turn.notice`, `session.release`, `session.purge` (phase 2, RFE-010), `home.sync`, `tool.result`,
`memory.write.result`, `file.result`, `shutdown`; (phase 4, P4-PRV-1/2) `preview.start`
(`{ port }`), `stream.open`, `stream.credit`, `stream.close` and **binary data messages**
(`§W18`: stream frames are outside the seq/ack lineage).

Supervisor → runner kinds: `ready`, `home.synced`, `turn.started`, `turn.event`
(`{ turn_id, n, event }`), `turn.progress`, `turn.result`, `steer.result`,
`send_now.result`, `session`, `process.state`, `watch.fired`, `gate.result`,
`ports.state`, `tool.call`, `memory.write`, `file.part` (`{ upload_id, index,
data_b64 }` ≤ 144 KiB decoded, so the base64 frame stays under `WS_FRAME_MAX`; spec fix S-06), `file.commit` (`{ upload_id, conversation_id,
turn_id, filename, caption?, sha256, size }`), (phase 4, P4-PRV-2) `stream.opened`, `stream.refused`,
`stream.credit`, `stream.close` and binary data messages (`§W18`).

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
   ──tool.call {call_id, conversation_id, turn_id, tool, args}──► runner
   ──POST /v1/runner/tool-calls {sandbox_id, boot_id, …}──► API tool registry
```

`ToolCall`: `{ sandbox_id, boot_id, call_id, conversation_id, turn_id, tool,
args }`. The API verifies: the boot is current; the conversation belongs to the
sandbox's agent; (FIX-110) `turn_id` is **required** and must be an active
turn of that conversation of the calling agent — a call without one is refused
`no_active_turn` (403), and the supervisor refuses before sending when no turn
is running, so a process left behind by an earlier turn cannot act, unattributed,
later; an effectively frozen agent's own calls are refused (423 `frozen`); the
tool is available for the agent. `ToolResult`: `{ call_id,
ok: true, content: [{ type: "text", text } | { type: "json", value }] }` or
`{ call_id, ok: false, error: { code, message } }`. Local tools (`process_*`)
never leave the supervisor.

MCP surface: `tools/list` answered from the synced catalogue (+ local tools);
`tools/call` as above; no MCP resources or prompts in phase 0. (phase 5, P5-GUIDE-7, D-297,
C-654) `ToolDescriptor` gains optional `family` and `exposure` (`core` / `on_demand`, omitted =
`on_demand`); in mode `on_demand` `tools/list` returns the core descriptors only, `tools/call`
is unchanged, and the socket's `list` (used by `aurora-agent` for target `aurora`) returns all (`24 §10b`).

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
(FIX-118, D-188) The CLI talks to `https://` unless the host is loopback (`login --url` and
`AURORA_URL` refuse `http://` elsewhere); a login to a different server drops the stored agent
tokens of the previous one; `run --forward` requires a per-run secret from the child (proxy
basic auth, checked before any upstream connection); server-supplied strings are printed with
control characters stripped; `logout` revokes the API key (and stored agent tokens) or says
that it did not.

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

## W14 — Blob store (phase 6 (D-318, moved from phase 5), P4-S3)

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

---

## W16 — Web Push (phase 5 (D-138, moved from phase 4), RFE-011b)

Semantics, payload and key handling: `07 §Web Push`. The wire is the standard Web Push protocol
and Aurora owns only what it sends:

| Part | Rule |
|------|------|
| Request | `POST <endpoint>` over TLS to a host in `push.allowed_endpoint_hosts`, resolved through the guarded resolver, no redirects followed, body `Content-Encoding: aes128gcm` (RFC 8291; record size 4096; per-message ephemeral key and salt; `auth` and `p256dh` from the subscription), `Content-Length` ≤ the push service's 4 096-byte limit (Aurora's plaintext is ≤ `PUSH_PAYLOAD_MAX`, 1 KiB) |
| Authorization | `Authorization: vapid t=<JWT>, k=<application server public key>` (RFC 8292): ES256, claims `aud` = the endpoint's origin, `exp` ≤ now + 1 h (never the 24 h maximum), `sub` = `push.vapid.subject`; the key is the one named by the subscription's `vapid_key_id` |
| Headers | `TTL` (remaining approval time capped at `PUSH_TTL_MAX`, ≥ 60 s), `Urgency: high` for `approval.pending` and `approval.reminder`, `normal` for digest and test, `Topic` (32 URL-safe characters, so a newer push for the same approval replaces the older one; (P4-011B spec batch, D-266(e), C-832) the first 32 base64url characters of SHA-256(subscription id ‖ approval id), or of SHA-256(subscription id ‖ "digest") for a digest: the push service learns neither the approval id nor which pushes to one person's devices concern the same approval) |
| Payload (plaintext JSON, `v: 1`) | `{ v, type, approval_id?, workspace_id?, expires_at?, parked?, url?, count?, agent?, workspace? }`; no other field may be added without a version bump; `agent`/`workspace` present only when the subscription has `show_details`; **never** a title, summary, rule name, host, path, method, query, body, token, credential, approve/deny link, e-mail address or user name (`07 §Web Push`) |
| Responses | 201 success; 404/410 → delete the subscription; 400/403/413 → failure (a 403 after a rotation is a key mismatch); 429/5xx → retry with backoff, at most 3 tries in 60 s; any other status → failure; bodies of responses are logged only as a status and a truncated, sanitized reason |
| Client side | the service worker of `14` receives the decrypted payload, shows a notification and on click opens a same-origin path; it never calls the API |

Conformance: the RFC 8291 Appendix A and RFC 8292 examples are test vectors for the encryption and
the JWT; a mock push service in the test kit verifies VAPID, decrypts with a test subscription key
pair and asserts the payload invariants (`18 §Approval park-and-resume and Web Push`). (P4-011B spec
batch, D-267(b), C-839) The mock is reached only through the development key
`push.dev_endpoint_overrides` (loopback targets on a loopback instance); the request it receives is
byte-for-byte what a push service would receive, including the JWT `aud` of the real origin.

---

## W17 — Relay plane (`/v1/relay`; phase 4, P4-PRV-2, D-146, D-147)

Served by the API on the public port like the runner plane (routed by the edge under `/v1/*`;
only the relay holds a credential). Present only when `previews.enabled` (else 404). Every
request: `Authorization: Bearer aul_…` and, after registration, `X-Aurora-Replica: <replica_id>`
and `X-Aurora-Protocol: relay/1`. The anchor token is generated by the chart into the instance Secret
(`AURORA_BOOTSTRAP_RELAY_TOKEN`, handed to the API as `AURORA_BOOTSTRAP_RELAY_TOKEN` and stored hashed
in `relay_tokens`, like `auh_`); it opens **only** these routes. The relay needs no database and no key.

| Call | Request → Response |
|------|--------------------|
| `POST /register` | `{ version }` → `{ replica_id, protocol: 1, config: { previews_domain, auth_cache_secs, wake_wait_secs, limits: { … } } }`; the relay refuses to serve if its own `previews.domain` differs from the API's |
| `POST /resolve` | `{ label, session_hash?: hex(SHA-256 of the cookie token), wait_secs?: ≤ 45, navigation: bool }` → `{ decision: "allow"\|"login"\|"deny", reason?, share?: { id, port }, route?: { sandbox: "running"\|"waking"\|"stopped"\|"unavailable", listening: bool, start: "none"\|"starting"\|"exited"\|"limited", runner_endpoint?, retry_after_secs? } }`. `deny` reasons are the **uniform** `not_found` (unknown label, previews or org off, no access, paused-for-others is not distinguished from "no such preview" for a caller without a valid session; with a valid session of the share: `paused` 403, `frozen` 423, `wake_limited` 429, `unreachable` 502); a stopped sandbox sets the wake demand (`11 §Port previews`), **recorded at most once per request, however long the long-poll runs** (and not at all while a demand of the current `PREVIEW_WAKE_WINDOW` is outstanding); **over the wake budget the answer is `deny` `wake_limited` with `retry_after_secs`**; `wait_secs` long-polls until `listening` or the timeout, and only while a wake or a start is under way (a running sandbox with nothing starting answers at once) |
| `POST /grants/redeem` | `{ grant, label, nonce }` → `{ token: "auv_…", expires_at, idle_secs }` (shown once) or 400 `grant_invalid` (`04 §Preview access`) |
| `POST /streams` | `{ label, session_hash }` → `{ grant: "aux_…", expires_in: 30, runner_endpoint }` — mints one **stream grant** bound to (share, sandbox, boot, port) after the same checks as `resolve`; 409 `not_running` |
| `POST /validate` | `{ sessions: [{ label, session_hash }] ≤ 500 }` → `{ revoked: [{ label, session_hash }] }`, the entries that no longer pass; the relay closes their streams with close code 4403 |
| `POST /report` | `{ shares: [{ share_id, streams_open, streams_total, bytes_to_app, bytes_from_app, last_request_at }] }` → `{ ok }`; also the heartbeat (every 15 s): updates `relay_replicas.last_seen_at`, `preview_shares.last_accessed_at`, `sandboxes.preview_active_until` |

Machine-plane conventions apply (typed bodies, bounded batches, idempotent where repeated);
the relay-plane routes are exempt from the management rate limiter and have their own per-replica
class (`PREVIEW_API_RATE`, 100 req/s/replica, burst 200): a live `aul_` is charged to that class only,
keyed by the registered replica (by the token before registration). An unknown, expired, rotated-away
or malformed `aul_` is 401 with the same body for every cause and is charged like any rejected bearer.
**Rotation** (FIX-090): the token presented at bootstrap is the only valid anchor — bootstrapping with
a new token deletes every other `relay_tokens` row and its replicas, so the old `aul_` answers 401.
The relay retries `validate` and `report` with backoff on 429 and 5xx and answers a visitor's request with
a try-later page (503, `Retry-After`) when `resolve` fails the same way, never a bare 502.

### Runner plane additions (`/v1/runner`)

| Call | Request → Response |
|------|--------------------|
| `POST /preview-streams/open` | `{ grant: "aux_…" }` → `{ sandbox_id, boot_id, port, share_id, window, idle_secs, max_lifetime_secs }`; the grant is **consumed** (single use, ≤ 30 s, hashed in `preview_stream_grants`); 403 `grant_invalid`; 409 `not_current` (boot is no longer current, share paused, agent frozen, previews off); the runner uses **only** the values of this answer, never anything the relay sent. (phase 4, P4-TERM, C-617) the body gains an optional `kind: "port" | "terminal"` (absent = `"port"`); a terminal request carries a `auy_` grant hashed in `terminal_stream_grants` and the answer omits `port`/`share_id`; a runner that does not know `kind` (older version) refuses unknown kinds — the API closes the browser WebSocket 4502; a runner that knows it answers either kind on the same listener |
| `POST /preview-streams/closed` | `{ share_id, bytes_to_app, bytes_from_app, reason }` (advisory; accounting) |

## W18 — Preview byte streams (phase 4, P4-PRV-2, D-147, SEC-82)

**Leg 1, relay → runner.** One WebSocket per upstream connection:
`GET /v1/preview-stream` on the runner's **stream listener** (`runner.stream_bind`, port **4108**,
a separate listener from the sandbox-facing 4105 and the ops 4106; reachable from the relay
only, `15 §Port previews`), `Authorization: Bearer aux_…` (the stream grant), no other parameter:
**the relay cannot name a sandbox or a port**. The runner redeems the grant (`/preview-streams/open`),
checks it holds that sandbox's live supervisor channel for that boot (404 `not_here` otherwise; the
relay re-resolves once), asks the supervisor to open the stream, and only then completes the
WebSocket handshake (101). After that: **binary messages = raw bytes** (≤ `PREVIEW_CHUNK`
16 KiB each), in both directions, no framing of its own; text messages are ignored; ping/pong every
20 s. Backpressure on this leg is TCP's: the runner stops reading a stream's WebSocket when
its supervisor-side credit is exhausted and its buffer (`PREVIEW_STREAM_WINDOW`) is full.
Closing: either side sends a WebSocket close; codes `1000` normal, `4403` not allowed
(grant, access revoked), `4404` not found, `4409` limit reached (`PREVIEW_STREAMS_PER_SANDBOX`,
`PREVIEW_STREAMS_PER_SHARE`), `4502` the supervisor could not connect to the port or refused it,
`4503` channel lost or sandbox stopped, `4504` timeout (connect, idle, lifetime). TLS on this leg
is optional (`runner.stream_tls`, default off in-cluster, residual risk in `17`): the runner
registers a `stream_tls` capability at registration and the API derives the endpoint scheme it
hands the relay from it (`ws://` off, `wss://` on).

**Leg 2, runner ↔ supervisor, over the existing W6 channel (wire change).** W6 gains **stream
frames** that are **outside the seq/ack lineage**: JSON text frames of kind `stream.*` carry
`seq: 0, ack: 0`, are never buffered for replay and are not acknowledged; binary WebSocket messages
(new in W6; before this, all messages were JSON text) carry data: `[0x01][stream_id: u32 BE][payload ≤ 16 KiB]`.
A W6 reconnect ends **every** stream (the runner closes each leg-1 WebSocket with 4503 and the supervisor
closes each loopback socket); streams are never resumed. A receiver that does not know the kinds
(older supervisor) ignores them, so the runner's `stream.open` times out (5 s) → `4502` and no
stream works: harmless.

| Direction | Frame | Body |
|---|---|---|
| runner → supervisor | `stream.open` | `{ stream_id: u32, port: u16, window: bytes }` (stream ids are allocated by the runner, increasing per channel connection) |
| supervisor → runner | `stream.opened` / `stream.refused` | `{ stream_id, window }` / `{ stream_id, reason: "not_shared"\|"refused"\|"timeout"\|"limit" }` |
| both | `stream.credit` | `{ stream_id, bytes }` |
| both | `stream.close` | `{ stream_id, reason: "eof"\|"reset"\|"idle"\|"lifetime"\|"channel_lost"\|"error", bytes_in?, bytes_out? }` |

**Credit-based flow control.** Each direction of each stream has a window (`PREVIEW_STREAM_WINDOW`,
**128 KiB**, announced in `stream.open`/`opened`). A sender may have at most `window` unacknowledged
bytes outstanding and stops at zero; a receiver returns credit with `stream.credit` after the
bytes were *written onward* (to the loopback socket or the leg-1 WebSocket), at least every
`window/4`. Control frames are never blocked by data. The sender interleaves streams round-robin
one chunk at a time, so one stream cannot starve the channel. With at most
`PREVIEW_STREAMS_PER_SANDBOX` (64) streams the bytes in flight per sandbox are bounded by
`64 × 128 KiB = 8 MiB` per direction; a credit that would exceed a window, a data frame beyond the window, an unknown
`stream_id` or a second `stream.open` for a live id is a protocol
violation: the receiver sends `stream.close { reason: "error" }` for that stream. Data for a stream
id this side closed, or was told closed, in the current channel connection is dropped silently and is
not a violation (a legal close race). A violation is data beyond the window or for an id never
opened. **Four violations within 60 s** on one channel connection close it with 4400; the count
resets when the connection changes. A supervisor's stream and PTY pumps never wait on a peer
(an app that stopped reading, a program that stopped reading its stdin): they hand data to
per-stream queues with `try_send` and a stream whose queue is full is closed with `error`.

**Allow-list, enforced at both ends (SEC-82).**
1. **Runner**: the port and sandbox of every stream come from the API's answer to the stream grant
   (never from the relay); `stream.open` is sent only for that pair and only on the channel of that
   `boot_id`.
2. **Supervisor**: `stream.open { port }` is accepted only if `port` is in its in-memory share
   list (`12 §Previews`, from the API's `previews` sync part), is not one of its own listeners and is
   in 1024..65535; it connects **only** to `127.0.0.1:<port>` (then `[::1]:<port>`), with a
   5 s connect timeout, never to any other address, never through a proxy, and answers
   `stream.refused { not_shared }` otherwise. An empty list refuses everything.
Both must agree: a stolen grant opens only its own share's port; a compromised relay cannot name
another; a compromised runner cannot make the supervisor dial a port that is not shared; the
agent cannot add to the list.

**Limits** (`02 §Limits`): concurrent streams per share 32, per sandbox 64, per user and per relay
replica 64 (**the relay's caps count per replica**: with N relay replicas an operator should expect up
to N × the cap in total; the runner and supervisor caps below are the global bound per sandbox); idle 120 s for a plain HTTP stream, 600 s for an upgraded one (a WebSocket's pings count);
lifetime 4 h; connect 10 s; response header timeout 60 s (an app that has not sent its response head
by then gets 504 and the stream is closed); request body 100 MiB, counted as the bytes stream, so a
chunked upload past it gets 413 and the leg is closed; bytes per share per day
(`PREVIEW_BYTES_PER_SHARE_DAY`, 5 GiB, then 429 `preview_quota` until the next UTC day).
**The relay's pump never awaits a peer** (FIX-088): every send toward the runner or toward the viewer
is raced against revocation and the idle and lifetime timers, so a viewer that stopped reading or a
runner that stopped reading cannot hold a stream open past revocation (SEC-81: ≤ 15 s); the closing
`Close` frame to the runner is bounded to 2 s and the stream slot is released whether or not it was
taken. The relay asks the app to close after each plain exchange (`Connection: close` on the
forwarded head; an upgrade is exempt) and ends a response that is cut short (malformed chunk framing,
or a leg that ends before the terminal chunk or the declared `Content-Length`) with a transport error
toward the browser, never a clean end.
**Runner-side bounds** (FIX-092): the stream listener closes a connection that has not delivered its
whole request head within 10 s (a total deadline, not a per-read one) and holds at most 256
unauthenticated connections per runner replica (further ones are closed at once; a connection leaves the
count when its grant is redeemed). The runner caps live legs per sandbox at the supervisor's own limits
(64 streams, 2 PTYs; 429 beyond). A leg's idle clock resets on any byte **or ping/pong** from the peer, but
only bytes stamp keep-awake, so an idle open tab does not hold a sandbox awake. The runner's outbound queue
per supervisor connection is bounded: when the supervisor stops reading, the offending leg (for a sequenced
frame, the connection — the replay buffer redelivers on reconnect) is closed instead of queued without
limit. `pty.resize` from the browser is coalesced to the latest size.
**Enforcement** (FIX-072, C-636/C-637): the relay reserves a per-share and a per-user slot (per replica;
`relay.max_streams_per_user` may lower the per-user value) before it asks the API for a stream grant and
releases it on every exit; over a cap a plain request answers 429 `preview_streams_limit` and a WebSocket
upgrade is accepted and closed `4409`. Grant redemptions at `/_aurora/grant` are limited to
`PREVIEW_GRANT_RATE` (30/min per client address, 10/min per share; fixed window per replica) with 429
`grant_rate_limited` and `Retry-After`; the check runs only for a request that carries a well-formed
state cookie (a request without one never reaches the API and spends no budget). The **client address**
is the socket peer, or — when the peer is inside `relay.trusted_proxies` (same semantics and default
as `server.trusted_proxies`, FIX-090) — the rightmost untrusted `X-Forwarded-For` entry; the same
address is the outbound `X-Forwarded-For`. The relay also caps the **negative resolves** per client
address (30/min): past it, a request whose credential is not already cached is answered locally
(login redirect, or 401 for a subrequest) without asking the API, so a flood of random cookies cannot
spend the relay plane's rate class. The daily quota is one **shared counter per share and UTC day**
(`aurora-counters`, class `management`; does not fail open — `local` counts per API replica, so under
`local` the effective limit is up to replicas × the limit; `closed` answers 503 `counter_unavailable`): relays add the bytes they moved (both directions, batched per stream in
units of ≥ 64 KiB and sent with each 15 s heartbeat) and `resolve` answers `deny`/`preview_quota` once the
counter reaches the limit, so the relay answers 429 with `Retry-After` to the next UTC midnight. A stream
that has started may finish its response; no new stream opens. The quota can be exceeded by at most what the
relays move between two heartbeats plus the 15 s resolve cache plus responses already open. Refusals count in
`aurora_relay_refused_total{reason}` (`share_streams`, `user_streams`, `quota`, `grant_rate_address`,
`grant_rate_share`; no ids in labels). WebSockets
are carried as ordinary upgraded streams: the relay forwards the `Upgrade` handshake over the stream
after checking `Origin` (`04 §Preview access`) and then relays bytes; it does not inspect frames.

## W19 — Terminal streams (phase 4, P4-TERM, D-161, SEC-87)

A web terminal (wterm + Ghostty core in the dashboard, `14 §Terminal tab`)
into the sandbox's shell, over the W18 machinery with a **PTY target**: the
leg-1 listener, the grant pattern, the credit windows, the chunking and the
close-code table are `16 §W18`'s, unchanged; this section owns only the
differences. Off unless `terminal.enabled` (operator opt-in); no relay, no
previews domain — the browser talks to the **API** (`GET
/v1/agents/{agent}/terminal` WebSocket upgrade, session cookie, `Origin`
checked, C-616), and the **API** is the one redeeming grants at the runner
(`kind: "terminal"`, C-617). The grant `auy_` (hashed in
`terminal_stream_grants`, single use, ≤ 30 s) is bound to `(agent, sandbox,
boot)` and never reaches the browser; a stolen grant opens only its bound
sandbox's terminal.

**Leg 2, runner → supervisor — `pty.*` frames on the W6 channel**, beside the
`stream.*` frames (same conventions: outside seq/ack, control as JSON text
`{ seq: 0, ack: 0 }`, data as binary, one pty per `pty_id` allocated by the
runner increasing per channel connection, interleaved round-robin):

| Direction | Frame | Body |
|---|---|---|
| runner → supervisor | `pty.open` | `{ pty_id: u32, window: bytes, cols: u16, rows: u16, shell?: "bash"\|"zsh" }` — (FIX-138) `shell` is the person's enumerated choice carried from the grant (the redemption answer of `/preview-streams/open` gains `shell`); absent = `bash`; the supervisor maps it to a fixed binary (`12 §Terminal`) |
| supervisor → runner | `pty.opened` / `pty.refused` | `{ pty_id, window }` / `{ pty_id, reason: "disabled"\|"limit"\|"error" }` |
| runner → supervisor | `pty.resize` | `{ pty_id, cols, rows }` |
| both | `pty.close` | `{ pty_id, reason: "eof"\|"hangup"\|"idle"\|"lifetime"\|"channel_lost"\|"error", bytes_in?, bytes_out? }` |
| both | binary data | `[0x02][pty_id: u32 BE][payload ≤ 16 KiB]` — the tag `0x02` distinguishes terminal data from W18 port-stream data (`0x01`) on the same channel; both may coexist |

Credit-based flow control exactly as W18 (`TERMINAL_STREAM_WINDOW` 128 KiB per
direction per PTY, credit returned after the bytes were written onward, at
least every `window/4`, control frames never blocked, round-robin one chunk at
a time; a violation closes that PTY with `pty.close { reason: "error" }`; data for a PTY id closed on
this connection is dropped silently, and four violations within 60 s on one connection close the
channel with 4400, exactly as W18).
A W6 reconnect ends every PTY (leg 1 closes 4503, nothing is resumed); a
supervisor that does not know the kinds ignores them (the runner's `pty.open`
times out 5 s → 4502): harmless. Limits: `TERMINALS_PER_SANDBOX` 2,
`TERMINALS_PER_AGENT` 4, `TERMINALS_PER_USER` 2 (per API replica), idle 600 s
(pings count: the API pings the browser every 20 s and any frame from it, a pong included, resets the
idle clock, which the API enforces with 4504), lifetime 4 h per session. A browser text message above
256 bytes (a `pty.resize` is a few dozen) closes the leg 4502 without being forwarded, and a write toward
the browser or the runner that stalls for 30 s ends the leg and frees its slot. Terminal bytes are never logged or
stored; the supervisor's PTY posture (environment minus every `AURORA_*`
variable and the boot document, agent uid, `/workspace`, `TIOCSWINSZ` resize)
is `12 §Terminal`, the wake/keep-awake semantics `11 §Web terminal`.

**Leg 1 framing, browser → PTY (FIX-054)**: the dashboard sends terminal
input — keystrokes and pastes — as **Binary** frames, UTF-8 encoded, at most
`TERMINAL_CHUNK` (16 KiB) per frame (a paste longer than one chunk is sent as
several ordered frames); the PTY leg reads PTY bytes from Binary frames only,
so a Text keystroke would be silently dropped. The **only** legal Text frame
browser → runner is the `pty.resize` control (`{"kind":"pty.resize",
"pty_id":0,...}` — the runner stamps the leg's real id); any other Text is
ignored, never parsed as bytes (C-619). In the other direction the PTY's
output rides Binary frames end to end.

## W20 — SSH access (phase 5, P4-SSH-1, P4-SSH-2, D-239, SEC-92..SEC-96)

Four hops: the SSH protocol between the client and `aurora-terminator`; the terminator plane
(`05 §Terminator plane`, C-794); leg 1, terminator → runner :4108, one WebSocket per SSH channel;
leg 2, `exec.*` frames on W6. Legs 1 and 2 are W18/W19's machinery (listener, grant pattern, credit
windows, chunking, close codes, violation counting) with an **exec target**; this section owns only
the differences. Off unless `ssh.enabled`.

**SSH protocol (terminator, `russh`).**
- *Transport*: SSH-2 only; host key `ssh-ed25519` (from `AURORA_SSH_HOST_KEY`, the same key on every
  replica); key exchange `curve25519-sha256` (and `@libssh.org`), `sntrup761x25519-sha512@openssh.com`
  where the library offers it; ciphers `chacha20-poly1305@openssh.com`, `aes256-gcm@openssh.com`,
  `aes128-gcm@openssh.com`; MACs only for non-AEAD ciphers (none offered); no SHA-1, CBC, DSA or RSA.
  Server keepalive every 30 s (keeps load-balancer idle timers from cutting a session); identification
  string `SSH-2.0-aurora` without a version number.
- *Authentication*: the only method advertised and accepted is `publickey` with an OpenSSH **user
  certificate** of type `ssh-ed25519-cert-v01@openssh.com`. In the *query* phase (no signature) the
  terminator answers OK to any `ssh-ed25519` offer and refuses every other algorithm (D-356: the
  SSH library shows the handler only the subject key there, and a certificate offer and a plain
  ed25519 key look the same, so every ed25519 offer gets the same answer whoever holds it). In the
  *signature* phase the library verifies possession of the key; a plain key is then refused, and for
  a certificate, **before any API call**,
  the terminator applies the whole certificate law offline: CA signature by one of the
  `ca_public_keys` of its registration, type `user`, inside the validity window (60 s skew),
  principal list exactly `[<username>]` and the username a UUID, **no critical options**, the key id
  in the `aurora:u=…:k=…:a=…:w=…` shape. Only when it passes does the terminator call `POST /v1/terminator/sessions`, and
  authentication succeeds only if the API answers 201. Every other outcome — plain keys, `password`,
  `keyboard-interactive`, `hostbased`, a failing certificate, an API refusal or error — is the same
  `USERAUTH_FAILURE` listing `publickey` only, after the same 1 s delay, no banner (hint-free). The
  `none` probe is answered without counting; `SSH_AUTH_ATTEMPTS` (6) other attempts end the connection.
- *Pre-auth limits* (per replica): `SSH_PREAUTH_MAX` (256) unauthenticated connections in total and
  `SSH_PREAUTH_PER_IP` (8) per client address, `SSH_CONNECT_RATE` (30/min, burst 10) per address,
  `SSH_AUTH_TIMEOUT` (20 s) from accept to success; a refusal closes the socket without a message.
  The client address is the socket peer (the chart's LoadBalancer keeps it with
  `externalTrafficPolicy: Local`, `15 §SSH access`).
- *Channels* (D-239(g)): only `session` channels, at most `SSH_CHANNELS_PER_SESSION` (4) at once. On a
  session channel the terminator accepts `pty-req` (needs `permit-pty` in the certificate; `term` ≤ 64
  printable ASCII, cols/rows 1…1000 — a zero dimension is "unspecified" (RFC 4254 §6.2; OpenSSH sends 0×0 when its stdin is not a terminal) and becomes 80×24 —, terminal modes ignored), `env` (`LANG`, `LC_*` only, ≤ 16 of
  ≤ 256 bytes; other names get a failure reply), `window-change`, and exactly one of `shell`, `exec`
  (`SSH_EXEC_COMMAND_MAX`, no NUL), `subsystem sftp`. Refused with a failure reply: `x11-req`,
  `auth-agent-req@openssh.com`, `signal`, `break`, any other subsystem or request. Refused channel opens
  (`SSH_MSG_CHANNEL_OPEN_FAILURE`, `administratively prohibited`): `direct-tcpip` (`-L`, `-D`),
  `direct-streamlocal@openssh.com`, `x11`, `auth-agent@openssh.com`, every unknown type. Refused global
  requests: `tcpip-forward` and `cancel-tcpip-forward` (`-R`), `streamlocal-forward@openssh.com`;
  `keepalive@openssh.com` gets the standard failure reply; `no-more-sessions@openssh.com` is honoured.
- *Ending*: the child's exit status becomes `exit-status` (or `exit-signal` with the signal name),
  then `EOF` and `CLOSE`; a leg that ends before an exit status is `relay_error` (`exit-status` 255 and
  a one-line stderr notice). Notices are written to the channel's extended data (stderr) and the
  terminator waits for the client's `CHANNEL_CLOSE` (or 2 s) before `DISCONNECT`, so OpenSSH prints
  them (the banner/goodbye ordering of OneCLI `docs/11`).

**Terminator plane** (routes in `05 §Terminator plane`). `register` is repeated after any 401 or restart;
`config` in the register and heartbeat answers carries `ca_public_keys` (the active CA key and
`ssh.ca.previous_public_keys`), so a CA rotation reaches the terminators within one heartbeat. The
terminator sends the heartbeat every `SSH_HEARTBEAT` (30 s) with every session it holds (≤ 500 per call,
batched); after `SSH_HEARTBEAT_FAILURES` (3) consecutive failures it closes every session
(`lease_expired`, they have expired server-side anyway) and refuses new authentications until a
heartbeat succeeds. The terminator reports `close` once per session with its reason; the API's
guarded transition makes a second close, a sweep close or a heartbeat revocation a no-op.

**Leg 1, terminator → runner.** `GET /v1/preview-stream` on the runner's stream listener, `Authorization:
Bearer auz_…` — the prefix selects the kind, as `auy_` does. The runner redeems the grant (`POST
/v1/runner/preview-streams/open { kind: "ssh", grant }` → `{ sandbox_id, boot_id, window, idle_secs,
max_lifetime_secs }`; 403 `grant_invalid`; 409 `not_current` when the boot is stale, the agent frozen,
`ssh.enabled` off or the grant's session no longer open and lease-current), checks it holds the
sandbox's channel for that boot (404 `not_here`), and completes the handshake (101). The terminator's
**first message** must then be one Text frame, within 5 s (else close 4504):
`{"kind":"exec.open","mode":"shell"|"exec"|"sftp","command"?:string,"tty"?:{"term","cols","rows"},"env":{…}}`
(≤ 12 KiB); the runner allocates the `pty_id`, sends leg 2's `exec.open`, and answers
`{"kind":"exec.opened"}` or closes 4502 (refused, unsupported, timeout) / 4409 (limit). After that:

| Frame on leg 1 | Direction | Meaning |
|---|---|---|
| Binary `[0x02][payload ≤ 16 KiB]` | both | stdin (terminator → runner); stdout or the PTY's output (runner → terminator) |
| Binary `[0x03][payload ≤ 16 KiB]` | runner → terminator | stderr (only without `tty`) |
| Text `{"kind":"pty.resize","cols","rows"}` | terminator → runner | `window-change` (coalesced to the latest size) |
| Text `{"kind":"exec.eof"}` | terminator → runner | the client sent `EOF` on stdin |
| Text `{"kind":"exec.exit","status"?:int,"signal"?:string}` | runner → terminator | the child ended; a `1000` close follows |

Any other Text frame, a Text frame over 256 bytes after `exec.open`, or a Binary frame with another tag
closes the leg 4502 without forwarding. Close codes, ping/pong, the 30 s stalled-write rule and
backpressure are W18 leg 1's; the runner refreshes keep-awake on bytes only (`POST
/v1/runner/terminal/keep-awake { sandbox_id, kind: "ssh" }` → `ssh_active_until`, at most one per
`SSH_AWAKE_GRACE` per sandbox; an older API ignores `kind` and stamps the terminal column, harmless).

**Leg 2, runner ↔ supervisor — `exec.*` frames on W6**, beside `pty.*` and `stream.*` (same conventions:
outside seq/ack, control as JSON text `{ seq: 0, ack: 0 }`, one id space with `pty.*`, data as binary,
interleaved round-robin, credit with `pty.credit` exactly as W19):

| Direction | Frame | Body |
|---|---|---|
| runner → supervisor | `exec.open` | `{ pty_id: u32, window: bytes, mode: "shell"\|"exec"\|"sftp", command?: string, tty?: { term, cols: u16, rows: u16 }, env: { name: value } }` |
| supervisor → runner | `exec.opened` / `exec.refused` | `{ pty_id, window }` / `{ pty_id, reason: "disabled"\|"limit"\|"unsupported"\|"error" }` |
| runner → supervisor | `pty.resize` | as W19 (only for an exec with `tty`) |
| runner → supervisor | `exec.eof` | `{ pty_id }` — close the child's stdin |
| supervisor → runner | `exec.exit` | `{ pty_id, status?: i32, signal?: string }` — exactly one, before `pty.close { reason: "eof" }` |
| both | `pty.credit`, `pty.close` | as W19 |
| both | binary data | `[0x02][pty_id: u32 BE][payload ≤ 16 KiB]` stdin/stdout/PTY; `[0x03][pty_id: u32 BE][payload ≤ 16 KiB]` stderr (supervisor → runner only); both count against the one window of that `pty_id` |

A W6 reconnect ends every exec (leg 1 closes 4503); a supervisor that does not know `exec.*` ignores it
(the runner's `exec.open` times out 5 s → 4502). Supervisor semantics: `12 §SSH channels`. Channel bytes
and commands are never logged or stored at any hop (byte totals only).

**Allow-list at every hop (SEC-95).** The terminator accepts only the channel types and requests above;
the API mints a grant only for a lease-current open session that passes `can_ssh`, bound to (session,
agent, sandbox, boot); the runner takes sandbox and boot only from the API's answer and sends `exec.open`
only on that boot's channel; the supervisor accepts only the three modes, its fixed `sftp-server` path
and the `LANG`/`LC_*` environment. No hop dials a TCP port or a socket for SSH: forwarding does not exist
below the terminator either.

(Phase 5, P5-FILES-1, D-316) The `exec.*` leg 2 and the supervisor's `sftp` mode land first with the
files sessions of `§W21`; P4-SSH-2 adds `shell`, `exec` and `tty` and the `auz_` leg 1 on top. The
supervisor's exec ceiling is `EXECS_PER_SANDBOX` (12), the runner's per-kind caps
`SSH_CHANNELS_PER_SANDBOX` (8) and `FILE_SESSIONS_PER_SANDBOX` (4).

## W21 — Files sessions (phase 5, P5-FILES-1, D-316, SEC-125, SEC-126)

The dashboard's Files page (`14 §Files`) reads and writes the sandbox's `/workspace` through REST
routes on the API (`05`, API_CONTRACTS C-898). The **API** is the SFTP client: it speaks SFTP
version 3 to the image's `sftp-server`, which the supervisor starts as an `exec` in mode `sftp`. The
hops are W20's legs 1 and 2 with a different grant and no terminator; this section owns only the
differences. No gate of its own: the routes and the runner kind exist where `terminal.enabled` is on.

**Leg 0, browser → API.** Plain HTTPS with the dashboard session cookie (session only, CSRF on
mutations). The browser never sees a grant, a runner address or an SFTP byte.

**Leg 1, API → runner.** `GET /v1/preview-stream` on the runner's stream listener :4108,
`Authorization: Bearer auf_…` — the prefix selects the kind, as `auy_` and `auz_` do. The runner
redeems the grant (`POST /v1/runner/preview-streams/open { kind: "files", grant }` → `{ sandbox_id,
boot_id, window, idle_secs, max_lifetime_secs }`; 403 `grant_invalid`; 409 `not_current` when the boot
is stale, the agent frozen or `terminal.enabled` off), checks it holds that boot's channel (404
`not_here`), enforces `FILE_SESSIONS_PER_SANDBOX` (4) live `auf_` legs (4409) and completes the
handshake (101). The API's **first message** must be one Text frame within 5 s (else 4504):
`{"kind":"exec.open","mode":"sftp"}` — on an `auf_` leg any other mode, a `command`, a `tty` or a
non-empty `env` closes the leg 4502 without reaching the supervisor. The runner answers
`{"kind":"exec.opened"}` or closes 4502 (refused, unsupported, timeout) / 4409 (limit). After that the
frames are W20 leg 1's minus `pty.resize`: Binary `[0x02][payload ≤ 16 KiB]` both ways (the SFTP
stream), Binary `[0x03]` runner → API (the server's stderr; the API discards it and counts bytes),
Text `exec.eof` API → runner, Text `exec.exit` runner → API then `1000`. Close codes, ping/pong, the
30 s stalled-write rule, credit windows (`TERMINAL_STREAM_WINDOW` 128 KiB) and violation counting are
W18/W20's. The runner stamps **no** keep-awake for `auf_` legs: the API stamps `files_active_until`
itself on each request (`11 §Files`).

**Leg 2, runner ↔ supervisor.** W20's `exec.*` frames, unchanged, with `mode: "sftp"`, `env: {}` and no
`tty`; the supervisor cannot tell a files session from an SFTP channel of an SSH session, by design:
both get exactly the same process (`12 §SSH channels`).

**API side.** One SFTP session per (person, agent) per API replica, multiplexing that person's
requests by SFTP request id; idle `FILES_SESSION_IDLE` 60 s, lifetime `FILES_SESSION_LIFETIME` 1 h
(the next request opens a new one); any leg close drops it and fails in-flight requests 503
`files_unavailable`. Writes are pipelined at most 4 requests of 32 KiB less 512 B of data outstanding per
transfer (so four whole requests, headers included, stay inside the leg's 128 KiB credit window: the
runner closes the leg 4502 when the API sends more than the window unreplied, found on kind with
1 MiB uploads, P5-FILES-1), so a transfer holds a bounded buffer; the leg's credit window bounds the rest. Only these SFTP requests are ever
sent: `INIT`, `REALPATH`, `STAT`, `LSTAT`, `FSTAT`, `OPENDIR`, `READDIR`, `CLOSE`, `OPEN`, `READ`,
`WRITE`, `FSETSTAT` (mode only), `MKDIR`, `RMDIR`, `REMOVE`, `READLINK` and the extension
`posix-rename@openssh.com` (checked in the server's `VERSION` extensions; without it the session is
`files_not_supported`, `details.reason: "image"`). Bytes and file contents are never logged or stored
at any hop (byte totals in metrics only); paths appear only in the audit rows of C-906.

**Allow-list at every hop.** The API mints an `auf_` grant only for a request that passed C-900's
authority, bound to (agent, sandbox, boot, person); the runner opens only an `sftp` exec for it and
never a PTY, a shell, a command or a port; the supervisor starts only its fixed `sftp-server` path.
An `auf_` grant redeemed with another kind, or an `auy_`/`auz_`/`aux_` grant redeemed as `files`, is
403 `grant_invalid`.
