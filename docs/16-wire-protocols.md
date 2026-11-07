# 16 — Wire Protocols

Every inter-component contract. Types live in `aurora-protocol` (serde +
schemars); JSON Schemas are generated into `docs/schemas/` of the
implementation repo and checked in (CI fails on drift).

## Index

| # | Contract | Direction | Auth | Owner doc |
|---|----------|-----------|------|-----------|
| W1 | Proxy protocol | agent → gateway :4102 | `Proxy-Authorization` gateway credential | `06` |
| W2 | Management API | browser/CLI/SDK → API `/v1` | session / API key | `05` |
| W3 | Gateway signals | API → gateways (`NOTIFY`), gateways → DB | DB roles | `05`, `06` |
| W4 | Gateway internal | API/monitoring → gateway :4103 | internal secret | `06` |
| W5 | Runner plane | runner → API `/v1/runner` | `aur_` + replica | this doc |
| W6 | Supervisor channel | supervisor → runner :4105 (WebSocket) | bootstrap token / HMAC session | this doc |
| W7 | Platform tools | harness → MCP bridge → supervisor → runner → API | stamped identity | this doc, `12` |
| W8 | Connector contract | connector → API `/v1/connector` | `aun_` + replica | `13` |
| W9 | Hook host | API/gateway → hookhost :4104; hookhost → API :4111 | internal secret | `09` |
| W10 | Platform callbacks | chat platform → connector | connector-defined | `20` |
| W11 | Upstream | gateway → internet | guarded resolver | `06` |
| W12 | CLI / BYO | CLI → API; BYO agent → gateway | API key / agent token | `05`, `08` |

## Cross-cutting conventions

| Convention | Rule |
|------------|------|
| Validation | Every message is parsed into typed structs at the receiver; batch parses are all-or-nothing, so senders MUST bound and validate before sending. |
| Versioning | Machine planes send `X-Aurora-Protocol: <plane>/<n>` (`runner/1`, `supervisor/1`, `connector/1`, `hookhost/1`). The API accepts N and N−1; unknown future versions → 426 with the supported range. |
| Open vocabularies | error codes, reason codes, capability names, connector ids, event/report kinds: unknown values are logged and skipped, never fatal. |
| Sender-side bounds | sizes, counts and string lengths enforced by producers (limits registry). |
| Idempotency | report ids, event sequence numbers, `source_event_id`, op ids, claim leases. |
| Time | the API's database clock is truth; timestamps from runners, supervisors and connectors are informational only. |
| Untrusted text | sanitized at the boundary where it enters platform voice. |
| Secrets in logs | never; URLs only in redacted form. |

---

## W5 — Runner plane (`/v1/runner`)

All requests: `Authorization: Bearer aur_…`; after registration also
`X-Aurora-Replica: <replica_id>`.

| Call | Request → Response |
|------|--------------------|
| `POST /register` | `{ version, backend, capabilities: RunnerCapabilities (optional `image`: the runner's `runner.image`, which the API reports as the sandbox image and uses in `sandbox.start` specs; optional `held_awake_ceiling`: the runner's `runner.held_awake_ceiling`, used for eviction, default `max(1, max_sandboxes − 1)`), advertised_host }` → `{ runner_id, replica_id, protocol: 1, config: { poll_wait_secs, reconcile_interval_secs, orphan_grace_secs, idle_stop_secs } }` |
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
{min,max} }, max_sandboxes, harnesses: ["claude-code","fake"], max_attachment_bytes }`.

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

`ModelChoice`: `{ provider, endpoint_id, base_url, dialect, model, effort,
client_env: [[k,v]] }`. `TurnInput`: `{ text, speaker_label, door, recall: {
index, snippets: [] }, notices: [] }`. `HomeSyncPart`: `{ kind: "skills" |
"memory" | "render_inputs" | "tools" | "manifest" | "final", entries: [...] }`
(`12 §Sync protocol`).

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
`turn.abort`, `turn.notice`, `session.release`, `home.sync`, `tool.result`,
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
