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
4. Starts the loopback proxies (the open proxy only with the `browser` capability, `11`).
5. Dials the runner with the bootstrap token; on success stores the session
   credential for reconnects; sends `ready { harness_capabilities, version }`.
6. Waits for the first complete home sync before accepting `turn.deliver`
   (deliveries arriving earlier are rejected with `not_ready` and re-dispatched).

Transport seam: **WebSocket** (production) and **stdio** (JSON lines on
stdin/stdout, logs on stderr) for tests and the dev loop. Identical vocabulary.

## Loopback proxies

All listen on 127.0.0.1 only and forward to the gateway proxy address from the
boot document. Each strips any client-supplied `Proxy-Authorization` and adds
its own. (phase 1, RFE-002) The upstream connection to the gateway is **TLS**
(rustls): the forwarder verifies the gateway's proxy certificate against the
boot document's `ca_pem` and `gateway.server_name`, refuses to proceed on any
verification failure (the client gets 502 `gateway_tls_failed` with no
credential sent) and never falls back to plaintext. The client-facing side stays
plain HTTP on loopback: it carries no credentials and is unreachable from outside
the sandbox (`11 §The boot document`).

| Listener | Credential added | Lane | Used by |
|----------|------------------|------|---------|
| `127.0.0.1:3128` (machine proxy) | boot token `aub_` | MITM | everything that reads `HTTPS_PROXY` from the container env: shells, background processes, tools started outside harness sessions |
| `127.0.0.1:3129` (open proxy) | boot token, username `open` | open | browsers (configured in the Chromium wrapper); CONNECT only — absolute-form requests get 405 so it can never become an injecting proxy. (phase 1, RFE-003) **Started only when the boot document's `capabilities` contains `browser`** (`11 §The boot document`); otherwise nothing listens on 3129 and the Chromium wrapper reports "browsing is disabled for this workspace". |
| `127.0.0.1:3130` (database relay, phase 2, P2-DBL-2, D-89) | boot token `aub_` as the PostgreSQL **password**, or the turn context credential `auc_` while exactly one turn is active | PostgreSQL | `psql service=<name>` and any libpq client (`PGSERVICEFILE`). Started only while the home sync carries `databases`; see §Database relay |
| per-session ephemeral port | the session's **current turn context credential** `auc_` (boot token between turns) | MITM | the harness process of that conversation (its `HTTPS_PROXY` points here) |

Context credentials therefore attribute every request made by a harness during
a turn — including LLM calls and tool subprocesses inheriting the harness
environment — to that conversation and turn.

## Session purge (phase 2, RFE-010)

On `session.purge { conversation_id, session_ref }` the supervisor aborts and
releases any live session for that conversation, calls `HarnessAdapter::
purge_session`, and reports `session.purged { conversation_id, session_ref, ok }`.
Pending purges are also delivered in the **home sync** (`purges` entries of the
`render_inputs` part) and executed before the supervisor accepts `turn.deliver`,
so a parked sandbox cannot resume a deleted conversation. A failed purge is
retried with the next sync and surfaced on the agent's Computer tab.

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
    /// (phase 2, RFE-010) Delete the files of one stored session (no session may be live for it); idempotent.
    async fn purge_session(&self, working_dir: &Path, session_ref: &str) -> Result<(), HarnessError>;
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
- (phase 4, P4-LLM, D-121) The adapter's `dialects` become `[anthropic_messages,
  anthropic_bedrock, anthropic_vertex]` (declared, never probed; a harness version
  that cannot speak one removes it). Client environment for the new dialects, all
  **without credentials** because the gateway signs or mints: `anthropic_bedrock` →
  `CLAUDE_CODE_USE_BEDROCK=1`, `AWS_REGION=<region>`, `ANTHROPIC_BEDROCK_BASE_URL=<endpoint base URL>`,
  `CLAUDE_CODE_SKIP_BEDROCK_AUTH=1`, `ANTHROPIC_MODEL=<model>` (no AWS key variables at
  all, so nothing in the sandbox can be mistaken for a credential); `anthropic_vertex` →
  `CLAUDE_CODE_USE_VERTEX=1`, `CLOUD_ML_REGION=<location>`, `ANTHROPIC_VERTEX_PROJECT_ID=<project_id>`,
  `ANTHROPIC_VERTEX_BASE_URL=<endpoint base URL>`, `CLAUDE_CODE_SKIP_VERTEX_AUTH=1`,
  `ANTHROPIC_MODEL=<model>`. The exact variable names are pinned next to the CLI version
  and verified by the live conformance suite (env-gated, per provider), like the phase-0
  ones. A pinned version that lacks the "skip auth" switch for a provider does not declare
  that dialect.
- MCP configuration: one stdio server `aurora` = `aurora-supervisor mcp-bridge
  --session <conversation_id>`.
- Tool permission prompts are disabled (the sandbox is the isolation boundary;
  the gateway is the policy boundary).
- Effort mapping: a version-pinned table in the adapter (e.g. `low` = no
  extended thinking, `max` = the largest thinking budget the CLI accepts).
- The adapter holds a starting session's first turn until the harness has
  listed MCP tools, so a stale on-disk tool cache is never used.

### Harness selection and adapter contract (phase 4, P4-HAR, D-122, SEC-66)

Phase 0–3 shipped one real adapter, so selection was implicit. From phase 4
(implemented **after P4-LLM**, because a harness is only useful with a provider that
speaks its dialect) the rules are explicit; nothing changes for a deployment that
does not enable a second harness.

- **Who chooses.** The harness is a property of the **agent** (`agents.harness`, `10`),
  set at creation (default `hosted.default_harness`) and changeable by whoever may edit
  the agent's instructions, and only among `hosted.enabled_harnesses` (default
  `["claude-code"]`, so Codex is opt-in). The API stamps it into the boot document
  (`harness.id`, `11 §The boot document`) and the supervisor instantiates the adapter
  **by that id from a compile-time registry** (`claude-code`, `codex`, `fake`; `fake`
  only with `hosted.allow_fake_harness`). Nothing inside the sandbox (an agent, a
  tool, a file) can change it. An id the supervisor does not know, or whose runtime is
  not in the image, fails the boot (`sandbox.status failed`, `reason_code =
  harness_unsupported`) before `ready`, never a fallback to another harness.
- **Placement.** The API places a sandbox only on a runner replica whose registered
  `harnesses` list contains the agent's harness (`16 §W5`, taken from the image label
  `org.aurora.harnesses`, `11 §Sandbox image`); with none, the turn fails
  `harness_unavailable` with the dashboard hint to enable the image or runner, before any
  start.
- **What an adapter declares.** `HarnessCapabilities` (above) is the whole contract between
  a harness and the rest of the platform; capabilities and `dialects` are declared per
  pinned version, never probed. **Dialects, in preference order**:

  | Harness | Declared dialects | Notes |
  |---------|-------------------|-------|
  | `claude-code` | `anthropic_messages`, `anthropic_bedrock`, `anthropic_vertex` | `08 §Phase 4 providers` |
  | `codex` | `openai_responses`, `openai_chat` | `openai_chat` only while the pinned version supports the chat wire API; a version that drops it removes it here |
  | `fake` | none (needs no model key) | tests |

- **Dialect choice.** For an agent, a usable key's endpoint serves the harness when the
  endpoint's dialects intersect the harness's; the dialect used is the **first of the
  harness's own list** that the endpoint also serves (`08 §Model selection`). Switching an
  agent's harness re-runs that selection: a model override whose endpoint no longer serves
  the new harness is dropped with a notice (the existing rule); with no compatible granted
  key the next turn fails `no_model_key`.
- **Switching the harness of an agent that has history.** Sessions are adapter-owned
  files in the agent's home, so the change (a) bumps `harness_session_epoch` of every
  conversation (the next turn starts a fresh harness session; the transcript in Aurora
  is untouched, the new harness does not see the old harness's private context),
  (b) queues `session_purges` for the old sessions so the previous adapter's files are
  deleted (`§Session purge`, run by the **old** adapter's `purge_session`, which therefore stays
  in the image while any purge is pending), and (c) re-renders instructions and skills for
  the new harness's `instruction_files` and `skills_dir` on the next home sync. An
  active turn refuses the change (409 `turn_active`).
- **Conformance.** Every adapter must pass `tests/conformance/harness` (`§Harness
  interface`). `codex` runs it against the mock Responses upstream of `aurora-testkit`
  (CI) and against a real OpenAI endpoint (env-gated).

### Adapter `codex` (phase 4, P4-HAR)

Drives the pinned OpenAI **Codex CLI** non-interactively (`codex exec`, JSON event
stream, one child process per live session, working directory `/workspace`), resumed by
the session id it reports. Exact flags, config keys and event mapping are pinned in the
adapter next to the pinned CLI version and verified by the conformance suites; what is
specified here is the contract.

- **Capabilities.** `resume`, `tool_events`, `platform_tools`; `thinking_deltas` only if
  the pinned version streams reasoning summaries; `steer = false` (a running `codex exec`
  cannot take a follow-up, so follow-ups become separate turns, `10 §Turns`);
  `background_tool = false`; dialects `[openai_responses, openai_chat]`;
  `instruction_files = ["AGENTS.md"]` (the platform preamble, brief and capability
  fragments are rendered there only; `CLAUDE.md` is rendered for `claude-code` only);
  `skills_dir` = the pinned version's skills directory when it has one, else none (skills
  are then listed in `AGENTS.md`).
- **State.** `CODEX_HOME` points into the agent's home (`~/.codex`); sessions live there
  and are what `purge_session` deletes (idempotent: the rollout files of one session id).
  No `auth.json` is ever written and ChatGPT-account sign-in is never offered.
- **Configuration the adapter renders** (per session start, into `CODEX_HOME/config.toml`
  owned by Aurora and overwritten on every start): a model provider whose `base_url` is the
  endpoint base URL of `ModelChoice`, `wire_api` from the chosen dialect, `env_key =
  OPENAI_API_KEY`; `model`; the reasoning effort from a version-pinned table (`low`,
  `medium`, `high`; `max` maps to the highest the CLI accepts); approvals set to "never"
  and Codex's own sandbox set to full access, **because the Aurora sandbox and the
  gateway are the isolation and policy boundaries** (Codex's inner sandbox cannot nest in
  the container and would only fail), exactly as tool prompts are disabled for
  `claude-code`; update checks, analytics and any non-essential network use disabled; one
  MCP server `aurora` = `aurora-supervisor mcp-bridge --session <conversation_id>`.
  The file contains **no secret**: `OPENAI_API_KEY=aurora-placeholder` is in the process
  environment and the gateway injects the real key (`08 §Phase 4 providers`, SEC-64).
- **Environment per session:** `HTTPS_PROXY`/`HTTP_PROXY` = the per-session proxy (so
  every call carries the turn's context credential), the CA bundle variables of the boot
  document, `CODEX_HOME`, `OPENAI_API_KEY=aurora-placeholder`; no `OPENAI_BASE_URL`
  outside the rendered provider. Azure OpenAI and OpenAI-compatible endpoints use the same
  provider block with their `base_url`.
- **Events.** The adapter maps Codex's JSON events to `HarnessEvent`: agent messages →
  text (message boundaries per the harness's framing, `10 §Answer rule`), reasoning
  summaries → thinking, command/file/MCP tool items → tool started/finished and activity
  lines, token usage → `usage`, turn completion/failure → exactly one terminal `Done` or
  `Error`. Provider errors map through the shared `aurora-llm` classification
  (`provider_auth` … `provider_refusal`); anything else is `harness_failed` with a
  sanitized message. Usage `provider`/`model` fields are overwritten by the API from the
  `ModelChoice` (`08`).
- **Not supported:** ChatGPT-subscription sign-in (the host `chatgpt.com` stays a known
  LLM host, refused under `sanctioned_only`), Codex cloud/remote features, IDE/app-server
  modes, its own MCP-server catalogue beyond Aurora's bridge.

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
(phase 4, P4-LLM) The classes for the new providers come from the shared
`aurora-llm` table (`08 §Phase 4 providers`, *Error classification*), so a Bedrock
`ThrottlingException`, an OpenAI `insufficient_quota` or a Vertex `PERMISSION_DENIED`
map to `provider_rate_limited`, `provider_quota` and `provider_auth` the same way in
every adapter. The new gateway codes `llm_path_not_permitted` and
`llm_request_too_large` are gateway refusals (`gateway_refused`).
(phase 4, RFE-011b) `approval_parked` is a gateway refusal that is **not a failure**: the supervisor
shows it as an activity line ("waiting for approval") and never maps it to `gateway_refused` as a
terminal error; a turn that ends normally after it is `done`. No adapter changes: the refusal
arrives as the HTTP response of the agent's own tool call, and the platform preamble
(`10 §Agent instructions`) tells the agent how to behave. Harness-level tool-failure counters
(`TOOL_CONSECUTIVE_FAILURES`) apply to platform tools only and do not see it. Resuming is a new
turn, so no harness needs a "suspend" capability and none is added to the adapter contract.

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
- Bridge ⇄ supervisor socket protocol (spec fix S-09): newline-delimited JSON.
  Requests `{ "id": u64, "session": string, "method": "list" }` or
  `{ "id", "session", "method": "call", "tool": string, "arguments": object }`;
  responses `{ "id", "result": <list: [ToolDescriptor] | call: ToolResult> }` or
  `{ "id", "error": { "code", "message" } }` (types from `aurora-protocol::tools`).

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
| Skills | `skill_list {}`, `skill_get {name}`, `skill_create {name, description, body, files?}`, `skill_update {name, …}`, `skill_delete {name}` (agent tier only; phase 3, RFE-020: `skill_list`/`skill_get` return the effective set of all three tiers with `tier` and `shadows`, writes touch the agent tier only) |
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
  org-skill generation (the API computes a combined monotonic value; phase 3, RFE-020: workspace-tier skill changes bump the workspace generation); any change
  to skills, memory, brief, name, presences, peers, grants/connected apps,
  memory language, timezone or tool availability bumps it. A bump never wakes a
  parked sandbox.

### Memory harvest

On a 10 s timer and before acking a sync, the supervisor scans `memory/*.md`;
files whose checksum trailer fails (or that are new) are uploaded with
`memory.write {key, content}` (bounded; titles/descriptions parsed from
optional YAML front matter). Invalid keys are reported back as a notice to the
next turn. Files stay in place until superseded by the canonical render.

## Database relay and files (phase 2, P2-DBL-2, D-89)

For each database connection the agent is granted (an app with a `database` section) the API puts
`databases: [{service, connection_id}]` in the `render_inputs` entry of the home sync (`API_CONTRACTS`
C-231), and the instruction file names the service under the account (`psql service=<name>`). The
supervisor turns that list into one file, `.aurora/pg/pg_service.conf` (`PGSERVICEFILE`, 0644,
`aurora-supervisor::pg`, C-232): a section per service with `host=127.0.0.1`, `port=3130` (the
relay), `user=<connection id>` and `sslmode=disable`. **No password file, no CA file, no credential
of any kind is in the sandbox's files or environment**: agent processes never see a gateway
credential (`11 §The boot document`, SEC-01). A `pgpass` or `root.crt` of an earlier version is
removed on the next sync, and the directory goes with the last database grant.

**Database relay** (`aurora-supervisor::pgrelay`, `127.0.0.1:3130`, started while the installed home
carries databases and stopped when none remain). Per client connection:

1. `SSLRequest` and `GSSENCRequest` from the client are answered `N`: the loopback leg is plaintext,
   the same documented exception as the HTTP forwarders.
2. The relay validates that the user is a UUID, dials the gateway proxy address of the boot document,
   sends `SSLRequest` and verifies TLS against `ca_pem` with the server name
   `<connection_id>.db.aurora.internal` (the gateway presents a CA leaf for it, `06 §Database lane`).
   Verification is never relaxed; on failure the client gets a FATAL `08006` and **no credential is
   sent**.
3. The client's `StartupMessage` (user = connection id) is forwarded. When the gateway asks for the
   cleartext password the relay answers it itself with the credential the HTTP forwarders use: the boot
   token, overridden by the turn context credential while exactly one turn is active (with several
   concurrent turns a process cannot be attributed to one, so the boot token is used). The
   authentication request is never passed to the client.
4. From `AuthenticationOk` on, bytes are relayed untouched. A `CancelRequest` is forwarded over the
   same verified TLS (the gateway maps its own keys).

Every agent process (harness session, `process_start`, gates) is started with `PGSERVICEFILE`, so
`psql service=<name> -c 'select 1'` works with nothing on the command line. A failure to write the
file is logged and does not fail the sync. The sandbox image ships the `psql` client.

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

`send_file` (supervisor-local tool, S-11): the supervisor resolves the path
(symlinks and `..` included) and validates it is a regular file inside
`/workspace` and **not** inside the platform-managed `/workspace/.aurora/`
(tool catalogue cache, inbox attachments, home projection state), size ≤
`ATTACH_OUT_FILE_MAX`, then uploads in `file.part` chunks (≤ 144 KiB (S-06)) with a
final `file.commit`; the API enforces per-turn and per-conversation caps.

## Logging

JSON lines to stderr (collected by the substrate), rate-limited (100 lines/s),
credential-looking values redacted; never logs tool arguments or results
verbatim.
