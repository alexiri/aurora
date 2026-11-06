# 12 — Sandbox Supervisor and Harness

Phase: **0**. Binary: `aurora-supervisor` (in the sandbox image).

## Role

The supervisor is Aurora's only program inside a hosted agent's sandbox — the
platform's agent inside the agent's computer. It:

- speaks the supervisor protocol to the runner (`16 §W6`);
- drives harness sessions (one per conversation) through the vendor-neutral
  **harness adapter** interface;
- projects control-plane truth into the home (skills, memory, instructions,
  tool catalogue) and harvests memory edits;
- serves platform tools to harnesses over MCP and relays them to the API;
- manages background processes, watches and schedule gates;
- runs the **loopback proxies** that attach gateway credentials.

## Process model and startup

1. Runs as PID 1 (uid 1000), reaps children, forwards SIGTERM to process groups.
2. Reads `AURORA_BOOT`, calls `prctl(PR_SET_DUMPABLE, 0)`, scrubs the variable
   from its environment.
3. Writes boot `files` (`only_if_absent_or_sentinel` honoured: never overwrite a
   file lacking the `aurora-managed` sentinel), writes `/run/aurora/ca.pem`.
4. Starts the loopback proxies.
5. Dials the runner with the bootstrap token; on success stores the session
   credential for reconnects; sends `ready { harness_capabilities, version }`.
6. Waits for the first complete home sync before accepting `turn.deliver`
   (deliveries arriving earlier are rejected with `not_ready` and re-dispatched).

Transport seam: **WebSocket** (production) and **stdio** (JSON lines on
stdin/stdout, logs on stderr) for tests and the dev loop. Identical vocabulary.

## Loopback proxies

All listen on 127.0.0.1 only and forward to the gateway proxy address from the
boot document. Each strips any client-supplied `Proxy-Authorization` and adds
its own.

| Listener | Credential added | Lane | Used by |
|----------|------------------|------|---------|
| `127.0.0.1:3128` (machine proxy) | boot token `aub_` | MITM | everything that reads `HTTPS_PROXY` from the container env: shells, background processes, tools started outside harness sessions |
| `127.0.0.1:3129` (open proxy) | boot token, username `open` | open | browsers (configured in the Chromium wrapper); CONNECT only — absolute-form requests get 405 so it can never become an injecting proxy |
| per-session ephemeral port | the session's **current turn context credential** `auc_` (boot token between turns) | MITM | the harness process of that conversation (its `HTTPS_PROXY` points here) |

Context credentials therefore attribute every request made by a harness during
a turn — including LLM calls and tool subprocesses inheriting the harness
environment — to that conversation and turn.

## Harness interface

```rust
pub struct HarnessCapabilities {
    pub resume: bool,             // sessions can be resumed by ref
    pub steer: bool,              // follow-ups can join a live run
    pub background_tool: bool,    // "send now" supported
    pub thinking_deltas: bool,
    pub tool_events: bool,
    pub platform_tools: bool,     // connects to the MCP bridge
    pub dialects: Vec<Dialect>,   // LLM API dialects it can speak (08)
    pub skills_dir: Option<RelPath>,       // home-relative, e.g. ".claude/skills"
    pub instruction_files: Vec<RelPath>,   // e.g. ["CLAUDE.md", "AGENTS.md"]
}

#[async_trait]
pub trait HarnessAdapter: Send + Sync {
    fn id(&self) -> &'static str;                 // display/registry only, never a branch key elsewhere
    fn capabilities(&self) -> HarnessCapabilities; // declared, never probed
    async fn start_session(&self, opts: SessionOptions) -> Result<Box<dyn HarnessSession>, HarnessError>;
}

pub struct SessionOptions {
    pub conversation_id: Uuid,
    pub working_dir: PathBuf,                   // /workspace
    pub resume_ref: Option<String>,
    pub model: ModelChoice,                     // provider, endpoint base URL, dialect, model id, effort, client_env
    pub proxy_url: String,                      // per-session loopback proxy
    pub mcp_bridge: McpBridgeSpec,              // command line for `aurora-supervisor mcp-bridge --session <id>`
    pub env: Vec<(String, String)>,             // app env + LLM placeholders; never secrets
}

#[async_trait]
pub trait HarnessSession: Send {
    fn session_ref(&self) -> Option<String>;
    /// Stream of HarnessEvent that ALWAYS ends with exactly one terminal event (Done or Error), also after abort.
    async fn run_turn(&mut self, input: TurnInput) -> Result<HarnessEventStream, HarnessError>;
    async fn steer(&mut self, input: SteerInput) -> Result<SteerAck, HarnessError>;   // only if capability; MUST refuse between turns
    async fn background_running_tool(&mut self) -> Result<SendNowResult, HarnessError>; // "moved" only when confirmed
    async fn abort(&mut self) -> Result<(), HarnessError>;
    async fn release(self: Box<Self>) -> Result<(), HarnessError>; // let go without ending; a later start with the same ref resumes
}
```

- `TurnInput`: `turn_id`, text, recall context (memory index + snippets,
  delivery-only), attachment paths, speaker label, door, platform notices.
- `HarnessEvent`: text delta (with harness `message_id`, optional `reply_to`
  follow-up id), thinking delta, activity line, tool started/finished,
  correction (retract/replace a streamed message), message joined, sent-now
  confirmation, session ref update, usage, `Done`, `Error{code, message}`.
- **Steer** must refuse between turns (never queue for a future turn).
  Consumption is confirmed later by `message joined`; a steer that resolves but
  never joins before the turn ends is reported *missed* (the API promotes it).
- **Release** is used when a session's surface changes (tool catalogue,
  channel presences, instruction inputs) while idle: the next message starts the
  session again with the same `resume_ref`, so conversation memory is kept and
  tools/instructions refresh together. A running turn is never interrupted for
  this.

A **conformance suite** (`tests/conformance/harness`) runs against every adapter:
terminal-event guarantee, abort, resume, steer refusal between turns, missed
steer, answer rule, MCP tool round-trip, correction handling. It runs in CI
against `fake`, and against `claude-code` when an LLM endpoint is configured
(env-gated, through a real gateway).

### Adapter `claude-code` (phase 0)

- Drives the pinned Claude Code CLI in headless streaming-JSON mode, one child
  process per live session, working directory `/workspace`, resumed by session
  id. Exact flags and event mapping are pinned in the adapter next to the
  pinned CLI version and verified by the live conformance suite.
- Capabilities: `resume`, `tool_events`, `thinking_deltas`, `platform_tools`;
  `steer` only if the pinned version passes the steer conformance tests (else
  false: follow-ups become separate turns); `background_tool = false`;
  dialects `[anthropic_messages]`; skills dir `.claude/skills`; instruction
  files `CLAUDE.md` and `AGENTS.md`.
- Environment per session: `HTTPS_PROXY`/`HTTP_PROXY` = the per-session proxy;
  LLM client env from the provider (`08`): for `litellm` endpoints
  `ANTHROPIC_BASE_URL=<endpoint base URL>` and
  `ANTHROPIC_AUTH_TOKEN=aurora-placeholder`; for `anthropic`
  `ANTHROPIC_API_KEY=aurora-placeholder`; `ANTHROPIC_MODEL=<model>`;
  auto-update, telemetry and non-essential traffic disabled.
- MCP configuration: one stdio server `aurora` = `aurora-supervisor mcp-bridge
  --session <conversation_id>`.
- Tool permission prompts are disabled (the sandbox is the isolation boundary;
  the gateway is the policy boundary).
- Effort mapping: a version-pinned table in the adapter (e.g. `low` = no
  extended thinking, `max` = the largest thinking budget the CLI accepts).
- The adapter holds a starting session's first turn until the harness has
  listed MCP tools, so a stale on-disk tool cache is never used.

### Adapter `fake`

In-process, directive-driven (`[[say: …]]`, `[[tool: memory_save {…}]]`,
`[[http: GET https://…]]` via the session proxy, `[[sleep: 2s]]`, `[[fail:
code]]`, `[[steer-accept]]`), no LLM needed. Used by all hosted end-to-end tests
and the conformance suite.

### Provider refusal classification

The supervisor maps terminal harness errors to turn error codes:
`provider_auth`, `provider_rate_limited`, `provider_quota`,
`provider_overloaded`, `provider_refusal`, `harness_failed`. A refusal that
originated at the **gateway** (recognized by the gateway's error envelope or
the `x-should-retry: false` + Aurora error code) is never presented as a
provider refusal; it is reported as `gateway_refused` with the gateway code.

## Platform tools

### Path

Harness → MCP (stdio) → `aurora-supervisor mcp-bridge --session <id>` → Unix
socket `/run/aurora/tools.sock` → supervisor → `tool.call` frame (stamped with
conversation and current turn) → runner (stamps sandbox and boot) →
`POST /v1/runner/tool-calls` → **tool registry** → result back.

- End-to-end budget `TOOL_CALL_TIMEOUT` (below typical harness MCP timeouts so
  the model sees Aurora's error text). After `TOOL_CONSECUTIVE_FAILURES`
  consecutive transport failures in a session the bridge answers immediately
  with a "platform tools temporarily unavailable" error until a call succeeds.
- The socket is **not a trust boundary**; identity is attached downstream from
  authenticated facts (runner-stamped sandbox → agent; API-validated turn ∈
  agent's active turns).
- Results are bounded (≤ 64 KiB serialized; larger results are truncated with
  a note).

### Tool registry (API)

Each tool is a registry entry in the API:

```rust
pub trait PlatformTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self, ctx: &ToolContext) -> String;        // may depend on memory language, platform…
    fn input_schema(&self) -> serde_json::Value;               // JSON Schema
    fn available(&self, ctx: &ToolContext) -> bool;            // e.g. messaging tools need an active presence
    async fn call(&self, ctx: &ToolCallContext, args: serde_json::Value) -> Result<ToolResult, ToolError>;
}
```

The **catalogue** (names, descriptions, schemas of available tools) is part of
the home sync render inputs, so adding a tool is an API-only change. When the
catalogue changes for an agent, idle sessions are released and pick it up on the
next message. A tool that becomes unavailable while a stale session still
lists it answers with an explanation (e.g. "your presence on Mattermost was
removed; ask an owner to re-attach it").

**Local tools** execute in the supervisor (physics in the container): the
`process_*` family. The supervisor is the enforcement authority for their
arguments; state changes are reported to the API.

### Phase 0 catalogue

| Family | Tools |
|--------|-------|
| Memory | `memory_save {key, title?, description?, content}`, `memory_get {key}`, `memory_list {}`, `memory_search {query}`, `memory_delete {key}` |
| Skills | `skill_list {}`, `skill_get {name}`, `skill_create {name, description, body, files?}`, `skill_update {name, …}`, `skill_delete {name}` (agent tier only) |
| Schedules | `schedule_create {name, prompt, cron, timezone?, gate_command?, gate_description?}`, `schedule_list {}`, `schedule_update {id, …}`, `schedule_delete {id}` |
| Webhooks | `webhook_list {}`, `webhook_deliveries {id}`, `webhook_update {id, instructions?, enabled?}` |
| Processes (local) | `process_start {name, command, cwd?}`, `process_status {id?}`, `process_stop {id}`, `process_watch {id, kind, pattern?, silence_secs?}` |
| Files | `send_file {path, caption?}` (to the current conversation) |
| Messaging (available with an active presence) | `find_recipient {query, platform?}`, `send_message {to, text, files?, platform?}`, `read_conversation {target?, limit?}`, `list_my_messages {}`, `edit_my_message {message_id, text}`, `delete_my_message {message_id}` (`13 §Messaging tools`) |
| Agents | `list_agents {}`, `message_agent {agent, text}`, `complete_task {report}`, `list_peer_tasks {}`, `cancel_peer_task {id}` |
| Apps | `list_apps {}`, `request_app_connection {app_id, reason}`, `propose_custom_connection {name, host, path?, recipe, reason}` |

## Home projection

The home (`/workspace`) holds:

1. **Agent-owned** free-form files (never touched by the platform).
2. **Projection** (managed roots only): `<skills_dir>/` (0444/0555),
   `memory/` (`*.md` 0644 + `index.md` 0444), instruction files (0444) at the
   paths the harness declares, `.aurora/` (tool catalogue cache, inbox
   attachments).
3. **Ephemeral** `/run/aurora`: CA, tools socket, boot-time stubs outside home.

### Sync protocol

- The API sends `home.sync` in ordered **parts** of one **generation**, each ≤
  `HOME_SYNC_PART_MAX` UTF-8 bytes (part count sized for worst-case encodings:
  CJK ×3, JSON-escaped controls ×6).
- Part kinds: `skills` (full files), `memory` (rendered files), `render_inputs`
  (preamble data, brief, capability fragment inputs: apps with snippets and
  docs links, presences, peers, memory language, schedules summary, timezone),
  `tools` (catalogue), `manifest` (the complete list of managed paths), `final`.
- The supervisor applies each part idempotently without buffering; only the
  **final** part prunes managed roots to the manifest, re-renders instruction
  files, and acks `home.synced { generation }`. A crash mid-generation leaves
  stale bytes and no ack; the next boot starts at generation 0 with a full sync.
- Paths outside managed roots are refused (warning), although traversal is
  already unrepresentable on the wire.
- Desired generation = `agents.home_sync_generation` + workspace generation +
  org-skill generation (the API computes a combined monotonic value); any change
  to skills, memory, brief, name, presences, peers, grants/connected apps,
  memory language, timezone or tool availability bumps it. A bump never wakes a
  parked sandbox.

### Memory harvest

On a 10 s timer and before acking a sync, the supervisor scans `memory/*.md`;
files whose checksum trailer fails (or that are new) are uploaded with
`memory.write {key, content}` (bounded; titles/descriptions parsed from
optional YAML front matter). Invalid keys are reported back as a notice to the
next turn. Files stay in place until superseded by the canonical render.

## Process manager

- Background processes run in their own process group as uid 1000 with the
  container env (machine proxy); stdout/stderr interleaved into a ring buffer
  (`PROCESS_TAIL`); busy = output within 60 s or CPU > 5 % over 30 s.
- `process.state` frames on change (debounced 2 s).
- Watches are evaluated in memory; a fired watch sends `watch.fired`.
- **Gates** for schedules run with the same posture and `GATE_TIMEOUT` (well
  below `STALE_DISPATCH`, since a gated turn stays `dispatched` while its gate
  runs); stdout is captured (≤ 16 KiB).
- Harness-run background tasks (if the adapter exposes them) are mirrored
  through the same `process.state` path so the platform knows about all
  background work.

## Outbound files

`send_file`: the supervisor validates the path is inside `/workspace`, size ≤
`ATTACH_OUT_FILE_MAX`, then uploads in `file.part` chunks (≤ 144 KiB (S-06)) with a
final `file.commit`; the API enforces per-turn and per-conversation caps.

## Logging

JSON lines to stderr (collected by the substrate), rate-limited (100 lines/s),
credential-looking values redacted; never logs tool arguments or results
verbatim.
