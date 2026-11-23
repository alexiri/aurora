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
   (FIX-114) Tokio reaps the children it spawned; a SIGCHLD/1 s loop reaps the zombie
   children of PID 1 that are still there on the next scan (orphans reparented to it:
   tool subprocesses of an aborted turn, `sh -c 'server &'`), so they cannot eat the
   sandbox's pid budget; shutdown (the runner's `shutdown` frame or SIGTERM) sends
   SIGTERM to every registered process group first.
2. Reads `AURORA_BOOT`, makes the process non-dumpable (`prctl(PR_SET_DUMPABLE, 0)`) so
   `/proc/<pid>/environ` is unreadable to the agent, and never passes the variable to a
   child (every child starts from a rebuilt environment, `SEC-34`). The supervisor cannot
   remove the variable from its own environment (no `unsafe`).
   (FIX-114) The child-environment filter drops values with any platform token prefix
   (`aug_ aub_ auc_ auk_ auo_ aur_ aun_ aut_ auw_ aus_ aui_ aup_ auh_ aud_ aul_ auv_ aux_
   auy_`) and secret-shaped names (`*_TOKEN *_SECRET *_PASSWORD *_KEY *_CREDENTIAL(S) *_DSN
   *_PASS *_PWD *_AUTH *_APIKEY`, `DATABASE_URL`) unless the value is the placeholder.
   Reads of agent-writable files are bounded (memory files at 4 × the content limit, with a
   notice; `tools.json`, staged sync parts and `render_inputs.json` at a few MiB), the
   process table keeps at most 64 exited records and reserves a slot under the same lock
   as the `max_running` check, and each tools-socket connection and MCP bridge has an
   in-flight cap (32; over it `tools_unavailable`) and a request-line bound (1 MiB).
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
    pub skills_dir: RelPath,               // required (P5-SKILLDIR, D-454): home-relative, e.g. ".claude/skills"; exported as SKILLS_DIR
    pub instruction_files: Vec<RelPath>,   // e.g. ["CLAUDE.md", "AGENTS.md"]
    pub model_roles: Vec<ModelRole>,       // (phase 5, P5-MODELS-1, D-303) roles it can map: large/medium/small; [] = none
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

### Turn controls by harness (FIX-225, D-469(d), C-1152)

What each pinned harness supports **in the mode its adapter uses**, tested against the
`aurora-testkit` mock upstreams. `aurora_protocol::runner::harness_turn_controls` is this
table; a supervisor test pins every adapter's declaration to it, and the dashboard and
chat cards show Steer / Send now from it (D-469(b), D-470).

| Harness | Pinned version | `steer` | `background_tool` | Evidence | Other modes that could (not built) |
|---|---|---|---|---|---|
| `claude-code` | 2.1.319 (also 2.1.318) | **yes** | no | `claude_code_steer_reaches_the_model_at_the_next_tool_boundary_and_the_run_ends_once` and the `steerable` case of `claude_code_conforms_against_the_mock_messages_upstream` (real CLI, mock Messages upstream); scripted-child tests `claude_code_steer_*` in `adapters/claude_code/tests.rs` run in CI without the CLI | `claude -p --input-format stream-json` has no command that moves a running tool to the background (none in `--help` or the stream-json input; the model's own `run_in_background` Bash option is chosen before the call, not a control) |
| `codex` | 0.167.0 | no | no | `codex_cannot_take_a_steer_or_background_a_tool_in_exec_mode`: `codex exec -` reads its whole prompt from stdin and runs one child per turn; a steer during the run is refused and its text never reaches the model | `codex app-server` (JSON-RPC: `turn/steer`, `turn/interrupt` in its generated schema) would steer; not built (D-469(d)) |
| `goose` | 1.59.2 | no | no | `goose_cannot_take_a_steer_or_background_a_tool_in_run_mode` (`goose run -i -`: the prompt is stdin read to its end) | `goose acp` (ACP over stdio) and `goose serve` (ACP over HTTP/WebSocket) have live sessions; not built |
| `opencode` | 1.19.6 | no | no | `opencode_cannot_take_a_steer_or_background_a_tool_in_run_mode` (`opencode run --format json`, prompt on stdin, no `--attach`) | `opencode serve` (HTTP; `POST /session/:id/prompt_async` while a run is live) and `opencode acp`; not built, and `--attach` would break the MCP bridge's parent chain (D-301) |
| `pi` | 1.5.2 | **yes** | no | the `steerable` case of `pi_conforms_against_the_mock_upstream_of_every_dialect_it_declares` (a `prompt` with `streamingBehavior: "steer"` is queued, joins before the next model call, the run ends once) | RPC mode is the mode in use; its commands include no way to background a running tool (none found) |

Probed 2026-11-22 against the pinned binaries (the sandbox image's own versions, downloaded and
checksum-checked) and the mock upstreams of `aurora-testkit`, no real keys. Findings for
`claude-code`: a second `{"type":"user",…}` line written during a run is taken **at the next tool
boundary** (the model's next request carries it) and the run continues to **one** `result` line; with
`--replay-user-messages` the CLI echoes the line it took (`"isReplay":true`, the same text), which is
the join and the only acknowledgement there is; without the flag nothing is echoed. A line that reaches
an **idle** CLI starts a run of its own with its own `result`, so the adapter refuses a steer between
turns and kills a CLI that still holds an unechoed steer when the turn ends. `harness_turn_controls` in
`aurora-protocol` is this table; `every_adapter_declares_what_the_turn_controls_table_says` pins the
adapters to it (the scripted `fake` harness declares both, as a test double).

### Adapter `claude-code` (phase 0)

- Drives the pinned Claude Code CLI in headless streaming-JSON mode, one child
  process per live session, working directory `/workspace`, resumed by session
  id. Exact flags and event mapping are pinned in the adapter next to the
  pinned CLI version and verified by the live conformance suite.
- Capabilities: `resume`, `tool_events`, `thinking_deltas`, `platform_tools`;
  `steer = true` and `background_tool = false` (FIX-225, §Turn controls by harness: a
  second user line takes effect at the next tool boundary, confirmed by the CLI's
  `--replay-user-messages` echo; the turn's end kills a CLI that still holds an unechoed
  steer, so a stray run never leaks into the next turn);
  dialects `[anthropic_messages]`; skills dir `.claude/skills`; instruction
  files `CLAUDE.md` and `AGENTS.md`.
- Environment per session: `HTTPS_PROXY`/`HTTP_PROXY` = the per-session proxy;
  LLM client env from the provider (`08`): for `litellm` endpoints
  `ANTHROPIC_BASE_URL=<endpoint base URL>` and
  `ANTHROPIC_AUTH_TOKEN=aurora-placeholder`; for `anthropic`
  `ANTHROPIC_API_KEY=aurora-placeholder`; `ANTHROPIC_MODEL=<model>`;
  auto-update, telemetry and non-essential traffic disabled.
- (phase 5 (D-138, moved from phase 4), P4-LLM, D-121) The adapter's `dialects` become `[anthropic_messages,
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
  that dialect. (phase 5, P4-LLM spec batch, D-261) The environment is the same for every
  auth mode of the endpoint (`aws_assume_role`, `aws_sigv4` and a Bedrock `api_key` alike:
  no `AWS_BEARER_TOKEN_BEDROCK`, no AWS profile, no `GOOGLE_APPLICATION_CREDENTIALS`); with
  the "skip auth" switches the CLI sends unsigned requests and the gateway signs or injects.
  The adapter also sets `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` for these dialects as
  for `anthropic_messages`, so the only calls on the endpoint host are inference calls
  (the inference path sets of `08` refuse anything else).
- MCP configuration: one stdio server `aurora` = `aurora-supervisor mcp-bridge
  --session <conversation_id>`.
- Tool permission prompts are disabled (the sandbox is the isolation boundary;
  the gateway is the policy boundary).
- Effort mapping: a version-pinned table in the adapter (e.g. `low` = no
  extended thinking, `max` = the largest thinking budget the CLI accepts).
- The adapter holds a starting session's first turn until the harness has
  listed MCP tools, so a stale on-disk tool cache is never used.
- (phase 5, P5-MODELS-1, D-302, D-303, C-877) **Models.** `ANTHROPIC_MODEL` is the **default** model
  (`08 §Model selection`). The adapter declares `model_roles = [large, medium, small]` and maps
  `ModelChoice` as follows:
  - `roles.large` → `ANTHROPIC_DEFAULT_OPUS_MODEL`, `roles.medium` → `ANTHROPIC_DEFAULT_SONNET_MODEL`,
    `roles.small` → `ANTHROPIC_DEFAULT_HAIKU_MODEL`. An unset role sets nothing. The CLI's other aliases
    (`fable`) stay unmapped.
  - `subagent_role` → `CLAUDE_CODE_SUBAGENT_MODEL=<alias of the role>` (`opus`/`sonnet`/`haiku`). It is
    never `CLAUDE_CODE_SUBAGENT_MODEL_FORCE`, so a subagent definition's own `model:` still wins.
  - `model_meta` → for each role whose model has `capabilities`, the CLI's per-alias capabilities
    variable (`ANTHROPIC_DEFAULT_<ALIAS>_MODEL_SUPPORTED_CAPABILITIES`, comma-joined). The default model's
    `context_window` → the CLI's context-window override (`CLAUDE_CODE_MAX_CONTEXT_TOKENS`). If the
    pinned version accepts a per-alias override, each role's `context_window` is mapped to it as well.

  The exact variable names are pinned next to the CLI version, like the `client_env` names (C-809). The
  live conformance suite verifies that a subagent with `model: haiku` and the CLI's background requests
  reach `roles.small` (seen in `request_log.llm_model`). Every value is re-checked against
  `MODEL_ID_PATTERN` (or as an integer, or the capability vocabulary) before it enters the environment
  (SEC-122). The adapter never writes `.claude/agents/`: subagent definitions are the agent's (D-302(d)).
  *Verified against the pinned 2.1.319 binary (2026-11-17, strings of the native `claude`):* every variable
  above exists (`ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU}_MODEL`, their `_SUPPORTED_CAPABILITIES`,
  `CLAUDE_CODE_SUBAGENT_MODEL`, `CLAUDE_CODE_MAX_CONTEXT_TOKENS`). The CLI has **no per-alias context
  variable**: `CLAUDE_CODE_MAX_CONTEXT_TOKENS` is one value applied to every model the CLI does not
  recognise, so only the default model's `context_window` is mapped. The binary also knows
  `ANTHROPIC_SMALL_FAST_MODEL` (the legacy background-model variable) and a `fable` alias, both left
  unmapped. The live conformance run (a subagent with `model: haiku` and the background requests reach
  `roles.small`) needs LiteLLM serving two models and is recorded in the brief's Completion report.
- (phase 5, P5-MODELS-2, D-304, C-879) **Usage.** `Done { usage }` is built from the result's
  per-model report (`modelUsage`; the key is pinned with the CLI version). Each entry becomes a
  `by_model` entry (`inputTokens` → `input_tokens`, `outputTokens` → `output_tokens`,
  `cacheReadInputTokens` → `cache_read_tokens`, `cacheCreationInputTokens` → `cache_write_tokens`), and
  the totals are their sum. The result's top-level `usage` covers the main loop only, and is used only
  when the per-model report is absent. `costUSD` and `total_cost_usd` are dropped. Streaming `Usage`
  events stay main-loop progress counters. The terminal `Done` is authoritative.

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
  only with `hosted.allow_fake_harness`; phase 5, P5-HARN, adds `pi`, `goose` and `opencode`, D-361). Nothing inside the sandbox (an agent, a
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
  | `codex` | `openai_responses` | `openai_chat` only while the pinned version supports the chat wire API; a version that drops it removes it here. The pinned 0.167.0 refuses `wire_api = "chat"`, so it is not declared (D-153) |
  | `pi` | `anthropic_messages`, `openai_responses`, `openai_chat` | phase 5, P5-HARN (D-362); all three verified against Pi 1.5.2 by its conformance suite |
  | `goose` | `anthropic_messages`, `openai_chat` | phase 5, P5-HARN; **not** `openai_responses`: goose 1.59.2's `openai` provider picks the Responses API by model name, so the path cannot be promised (`§Adapters pi, goose and opencode`) |
  | `opencode` | `anthropic_messages`, `openai_responses`, `openai_chat` | phase 5, P5-HARN (D-363, v1.19.6); all three verified by its conformance suite |
  | `fake` | none (needs no model key) | tests |

- **Dialect choice.** For an agent, a usable key's endpoint serves the harness when the
  endpoint's dialects intersect the harness's; the dialect used is the **first of the
  harness's own list** that the endpoint also serves (`08 §Model selection`). Switching an
  agent's harness re-runs that selection: a model override whose endpoint no longer serves
  the new harness is dropped with a notice (the existing rule); with no compatible granted
  key the next turn fails `no_model_key`.
- **Provider reach per harness** (phase 5, P4-LLM spec batch, D-260). The gateway never
  translates between wire formats, so a harness reaches a provider only through a shared
  dialect (the full table is `08 §Phase 4 providers`, *Wire formats and harness reach*):
  `claude-code` reaches `anthropic`, `bedrock` (`anthropic_bedrock`), `vertex`
  (`anthropic_vertex`), and `litellm`/`openai_compatible` endpoints declaring
  `anthropic_messages`; `codex` reaches `openai`, `azure_openai`, and `litellm`/
  `openai_compatible`/`bedrock` endpoints declaring `openai_responses`; (P5-HARN) `pi` and
  `opencode` reach `anthropic`, `openai`, `azure_openai` and any `litellm`/`openai_compatible`
  endpoint declaring one of their three dialects, and `goose` reaches `anthropic` and endpoints
  declaring `anthropic_messages` or `openai_chat` (none of the three reaches Bedrock or Vertex
  except through `litellm`). Any other pairing
  goes through a `litellm` endpoint (LiteLLM translates) or is refused as
  `no_shared_dialect`. The **auth mode never reaches the harness**: the supervisor gets
  the same `client_env` for every auth mode of a provider (placeholders and "skip auth"
  switches, C-809), because the gateway removes whatever the client sends and injects,
  signs or mints itself; the API passes the key's real auth mode to `client_env` (not a
  fixed `api_key`).
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
  `skills_dir = ".codex/skills/aurora"` (since P4-HAR; required, D-454: skills are never
  listed in `AGENTS.md` instead).
- **State.** `CODEX_HOME` points into the agent's home (`~/.codex`); sessions live there
  and are what `purge_session` deletes (idempotent: exactly the rollout files named
  `rollout-<timestamp>-<id>.jsonl` of one session id). Codex keeps its rollouts and its
  configuration under one home, so the file is **re-rendered and verified (read back) immediately
  before every `codex exec`**: what the agent wrote there between turns is gone (SEC-66). A thread
  id from `thread.started` is kept only when it is a UUID; stdout lines are read with a 16 MiB
  bound and a longer line fails the turn (`harness_failed`); the release supervisor reads no
  binary override from the environment.
  No `auth.json` is ever written and ChatGPT-account sign-in is never offered.
- **Configuration the adapter renders** (before every turn, into `CODEX_HOME/config.toml`
  owned by Aurora and overwritten each time): a model provider whose `base_url` is the
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
  provider block with their `base_url`. (phase 5, P4-LLM spec batch, C-809) The block's
  `base_url` is the provider's `OPENAI_BASE_URL` from `client_env` (as for `litellm`
  today): `openai` `https://api.openai.com/v1`, `azure_openai` `<base>/openai/v1` (the
  v1 API needs no `api-version`; the placeholder `Authorization` Codex sends is removed and
  replaced by `api-key` or an Entra token at the gateway), `openai_compatible` `<base>/v1`,
  `bedrock` with `openai_responses` `<runtime base>/openai/v1`. No Azure- or AWS-specific
  Codex setting is rendered.
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
- (phase 5, P5-MODELS, D-303, D-304) **Models.** `model_roles = []`: `roles`, `subagent_role` and
  `model_meta` are received and ignored, and the dashboard says "Not used by Codex". A later pinned
  version that can choose a model per task declares roles in its own spec change. The rendered `model` is
  the default. Usage reports one `by_model` entry, for the default model, unless the pinned version
  reports per-model token counts.

### Adapters `pi`, `goose` and `opencode` (phase 5, P5-HARN, D-361, D-362, SEC-133)

These three harnesses get the same treatment as `claude-code` (owner, 2026-11-17). Selection, placement,
switching, purge, the conformance suite and the provider-error mapping are exactly as above. This section
gives the contract each adapter adds. **It was written from the projects' documentation and then checked
against the pinned binaries (P5-HARN Part A, 2026-11-18; each item marked *verified* was run against a mock
Messages, Responses and Chat upstream with a logging proxy, the others are still the documented behaviour
and are proved by the adapter's tests).** A flag, variable, dialect or role the pinned version does not
have is removed here, never emulated. Exact names are pinned in each adapter next to its `PINNED_VERSION`,
as for `claude-code` and `codex`.

**Pins (D-362 amendment, Part A):** `pi` **1.5.2** (npm `@earendil-works/pi-coding-agent`, tree pinned by
`images/sandbox/harness/pi/package-lock.json`, needs Node.js ≥ 22.19; the image's AlmaLinux 10 `nodejs` is
22.23.4), `goose` **1.59.2** (`goose-<arch>-unknown-linux-gnu.tar.gz`; 1.60.0 was under a day old),
`opencode` **1.19.6** (`opencode-linux-<x64|arm64>.tar.gz`, the latest v1.19.x, D-363). SHA-256: opencode
x64 `47734e08968b30cc22a0bffb6d9feeed76ad4aa743213a12122040392d09a057`, arm64
`5b68c1f4c5e75b4134e664cc8f36d634d64208d4fcfe5bcf3b48bab699c2dbac`; goose x86_64
`c63cf82953e968d84b7542eb38a621dfdb5050b967728ed363517be5cdc666ea`, aarch64
`ec2e0aded1e2aba8d310727a5138f2e3cce0769f2611fbe196ed014c31ce370a`. **Choices left to the pinned version
(D-362 amendment):** goose is driven with `goose run --output-format stream-json` (its stream has tool
events and usage, so `goose acp` is not used); all three use `.agents/skills` as the skills directory; the
usage keys are the ones named per adapter below. Image (Part A.3, measured with `podman`): the default
`sandbox` image is 3.07 GB, against 2.49 GB with `claude-code,codex` (+0.58 GB: Pi 154 MB, goose 199 MB,
OpenCode 206 MB) and 2.16 GB with `SANDBOX_HARNESSES=claude-code`.

**Common to all three** (D-362):

- **State.** Each conversation's harness state lives in its own directory,
  `<home>/.aurora/harness/<id>/sessions/<conversation_id>/`. `purge_session` removes that directory, which
  is idempotent and exact. Any configuration the adapter renders lives in
  `<home>/.aurora/harness/<id>/config/`, owned by Aurora, and is **re-rendered and read back before every
  start** (SEC-66). The harness's credential stores are deleted before every start: `auth.json` (Pi,
  OpenCode), `secrets.yaml` (goose), and any keyring use, which is switched off.
- **Credential.** Rendered files refer to the placeholder **through an environment variable**
  (`ANTHROPIC_API_KEY` or `OPENAI_API_KEY` = `aurora-placeholder`), never as a literal. The gateway removes
  it and injects the key. All traffic uses the per-session proxy and the boot document's CA bundle.
- **Dialects** in preference order (a dialect the pinned version cannot use without a credential is not
  declared). `pi`: `anthropic_messages`, `openai_responses`, `openai_chat`. `goose`: `anthropic_messages`,
  `openai_chat` (**not** `openai_responses`: its `openai` provider picks the Responses API by model name,
  Part C). `opencode`: `anthropic_messages`, `openai_responses`, `openai_chat`. None declares
  `anthropic_bedrock` or `anthropic_vertex`: their SDK paths sign with cloud credentials and have no
  skip-auth switch, so Bedrock and Vertex reach them only through a `litellm` endpoint (D-260(a)).
- **No vendor network**: telemetry, update checks, model catalogues, share links, session-naming calls and
  background services are switched off. The live suite asserts that a turn calls only the endpoint host,
  and only its inference path set. Where the pinned version has no switch (goose's model catalogue fetch,
  below) the attempt is refused by the session proxy and the suite asserts that it never reaches the
  endpoint host.
- **Approvals** are off: the Aurora sandbox and the gateway are the boundaries, as for `claude-code` and
  `codex`.
- **Instructions**: `instruction_files = ["AGENTS.md"]`. **Platform tools**: one stdio MCP server `aurora` =
  `aurora-supervisor mcp-bridge --session <conversation_id>`, with tools **exposed directly**, not
  through the harness's own code mode. The bridge must run as a descendant of the harness process the
  supervisor started (D-301).
- **Built-in tools**: the harness's own file, edit and shell tools stay on. Built-ins that duplicate a
  platform feature with their own storage, or that call a vendor service, are switched off.
- `background_tool = false`. **Usage**: `by_model` entries come from the per-message usage and its
  model (input, output, cache read, cache write); totals are their sum; cost fields are dropped (D-304).
  A version without per-model data reports one entry, for the default model. **Effort**: a version-pinned
  table (`low`, `medium`, `high`; `max` = the highest level the CLI accepts for the model).
- **Models** (D-303): a role is declared in `model_roles` only when the harness has a native slot that
  receives it. `model_meta`'s `context_window` for the default model goes to the harness's context
  setting. Values are re-checked against `MODEL_ID_PATTERN` before they are rendered (SEC-122).

**`pi`** (Node.js, the npm tree pinned by a lockfile):
- **Drive**: one long-lived `pi --mode rpc` child per live session, with JSONL commands on stdin and events
  on stdout (16 MiB line bound, like `codex`). `--session-id` is the session ref: the resume reference, or
  the conversation's id for a new session, so `--session-dir` = `<home>/.aurora/harness/pi/sessions/<ref>/`
  and a purge by session ref removes exactly that directory. `PI_CODING_AGENT_DIR` is the config directory.
  The configuration is rendered and read back at every start **and before every turn** (an agent can write
  it while the child lives), with `auth.json` removed each time. The session ref is reported at the
  first turn's acknowledgement.
- **Config** (*verified*): `<config>/models.json` = `{"providers": {"<id>": {"baseUrl", "api", "apiKey",
  "models": [{"id", "name", "contextWindow", "maxTokens"}]}}}` with `api` `anthropic-messages`,
  `openai-responses` or `openai-completions` from the dialect; all three work against the mock upstreams.
  `apiKey` is a **`$NAME` reference** to the placeholder variable (`"$ANTHROPIC_API_KEY"` or
  `"$OPENAI_API_KEY"`), never a literal. (*Corrected in Part B:* Part A wrote "the name of the variable",
  but Pi resolves `apiKey` as a literal, `$NAME` or `!command`, so the bare name was sent as the key
  itself; the conformance suite's credential check caught it.) For `anthropic-messages` the `baseUrl` is the endpoint host without `/v1` (the SDK appends
  `/v1/messages?beta=true`); for the OpenAI dialects it includes `/v1`. `contextWindow` comes from
  `model_meta`. `<config>/mcp.json` = `{"mcpServers": {"aurora": {"command", "args", "env",
  "exposure": "direct"}}}`: **`exposure: "direct"`** is required (the default `codemode` hides every MCP
  tool behind one `codemode` tool); the tools are then named `mcp__aurora__<tool>`. Pi also writes
  `auth.json` (`{}`) and `models-store.json` into the config directory; `auth.json` is deleted and
  re-rendered empty before every start. The documented "project trust" setting is **not** used: a
  project's `.pi/` and `.agents/skills` are protected resources that only load with `--approve`, and the
  adapter never passes it (and passes `--no-approve`), so a `.pi/` written in the workspace cannot replace
  the rendered provider or MCP configuration. `AGENTS.md` in the working directory still loads.
  Environment (*verified*: with these a turn makes only the inference request, no other connection):
  `PI_CODING_AGENT_DIR`, `PI_OFFLINE=1`, `PI_TELEMETRY=0`, `PI_SKIP_VERSION_CHECK=1`.
- **Capabilities**: `resume`, `tool_events`, `thinking_deltas`, `platform_tools`, **`steer = true`**
  (*verified*, see Events). `skills_dir = ".agents/skills"` (*verified*: `$HOME/.agents/skills/` loads
  without trust and with `--no-approve`). `model_roles = []`, because Pi has no subagents.
- **Drive** (*verified*): `pi --mode rpc --provider <id> --model <model> --session-dir <dir> --session-id
  <uuid> --no-approve` (a new session id logs a "creating a new session" warning on stderr; the session file
  is `<timestamp>_<id>.jsonl` in the directory). The first stdout line after a `prompt` command is
  `{"type":"response","command":"prompt","success":true,"data":{"disposition":"started"|"queued"}}`.
- **Events** (*verified*): `message_update` with `assistantMessageEvent` `text_delta` / `thinking_delta` →
  text and thinking deltas; `tool_execution_start/end` (`toolCallId`, `toolName`, `args`, `result`,
  `isError`) → tool started and finished; `agent_settled {aborted}` is the **terminal** event → exactly one
  `Done`, or `Error` when the last assistant message has `stopReason = "error"` and the preceding
  `agent_end` had `willRetry = false` (`errorMessage` is `"<status> <response body>"`, classified through
  `provider_error.rs` with the status prefixed; the adapter's `<config>/settings.json` sets
  `retry {enabled: true, maxRetries: 2, baseDelayMs: 2000}`, the same two retries as `codex`
  (`request_max_retries = 2`), so a refusal arrives after **about 6 s** instead of Pi's default 14 s
  (*verified*: `auto_retry_start` 1/2 and 2/2, then `agent_settled`; each retry shows as an activity line).
  `settings.json` also sets `defaultProjectTrust: "never"`, `enableInstallTelemetry: false` and
  `quietStartup`). `abort` → RPC `abort`, answered by `agent_settled {aborted: true}` within about 30 ms
  (*verified*), then `{"type":"response","command":"abort"}`; queued steers are dropped. The adapter waits
  5 s for the settle after an abort and then kills the child; the next turn starts a new one with the same
  session id and directory (the session is resumed from its file).
  **Steer**: a `prompt` command with `streamingBehavior: "steer"` during a run is answered `queued` and is
  delivered after the in-flight tool result and before the next model call; it joins when a `message_start`
  with `role: "user"` carries its text (the `queue_update` event empties). **Sent while idle it starts a new
  run (`started`)**, so the adapter refuses a steer between turns itself and never sends it. It also
  refuses one until Pi has acknowledged the turn's own prompt (`disposition: "started"`; Pi takes a few
  hundred milliseconds to start, and a steer sent before the acknowledgement would itself start a run).
  A steer that still races the end of the turn and is answered `started` is refused, the run it started is
  aborted, and the next turn first waits for that run to settle; the supervisor then sends the text as a
  turn of its own. The steer's text is `<speaker>: <text>` and the join is matched on it.
  Effort → `--thinking` (`off`, `low`, `medium`, `high`, `max`).
- **Usage** (*verified*): every assistant `message_end` carries `model` and `usage {input, output,
  cacheRead, cacheWrite, totalTokens, cost}`; `input` already excludes cached tokens. `by_model` sums them
  per `model` over the turn (`cost` dropped). Unknown models fold into the default (D-304).

**`goose`** (Rust release binary, checksum verified):
- **Drive** (*verified*): one `goose run -i - --output-format stream-json --name <ref> [--resume]
  --provider <p> --model <m>` child per turn, prompt on stdin. **`stream-json` is used, not `goose acp`**:
  its stream carries tool events and usage. stdout also has a banner (parse only lines that start with
  `{`). **`--no-profile` is not used** (Part C correction): it makes goose ignore the extensions in
  `config.yaml` ("only use CLI-specified extensions"), so the rendered `developer` and `aurora` extensions
  would not load; without it goose loads exactly the extensions of the rendered file.
  `GOOSE_PATH_ROOT` is the conversation's directory `<home>/.aurora/harness/goose/sessions/<ref>/` and
  holds **everything**, rendered config included (goose has one root, so unlike `pi` the rendered
  `config/config.yaml` lives inside the session directory; it is rendered and read back before every turn
  and `config/secrets.yaml` is removed): `config/config.yaml`, `data/sessions/sessions.db`, the model
  catalogue under `data/model_catalog/`, `state/logs/` (including `llm_request.*.jsonl`); nothing in
  `HOME`. A purge by session ref removes that directory. The session name is the ref (the conversation's id,
  or the resume reference); **`--resume` of a name that does not exist is an error** ("No session found with
  name …", exit 1, nothing on stdout), so the adapter passes `--resume` only once a turn has produced a goose
  event (or when it was given a resume reference), and a start that produced no event is a failed
  `harness_failed` turn carrying goose's stderr. **A resumed session keeps its stored model and ignores
  `GOOSE_MODEL`**, so the model is also passed as `--provider`/`--model` (*verified* by the conformance
  suite: a restart that resumes the stored session with another model sends the new model, with the
  earlier turns in the request).
- **Events** (*verified*): `{"type":"message","message":{id,role,content:[…]}}` with content items `text`
  (deltas under one message id), `toolRequest` (`id`, `toolCall.value {name, arguments}`), `toolResponse`
  (`id`, `toolResult`) and `error`; `{"type":"complete", total_tokens, input_tokens, output_tokens,
  cache_read_input_tokens, cache_write_input_tokens}` → `Done` (`input_tokens` **includes** cached tokens,
  so Aurora's input = `input_tokens − cache_read − cache_write`); `{"type":"error","error":"…"}` and exit
  status 1 → `Error` (text such as `Authentication error: … Status: 401` or `Rate limit exceeded: …`,
  classified through `provider_error.rs`; goose retries 429 and 5xx three times with backoff, so a refusal
  arrives after **about 7 s** (*verified*, no setting; a 401 arrives at once). goose reports every 429 as
  "Rate limit exceeded", so a quota or billing phrase in the text is read first and maps to
  `provider_quota`). **No per-model usage**: one `by_model` entry, for the model the turn ran on. The
  platform tools are reported as `mcp__aurora__<tool>`, like the other harnesses.
- **Config and environment** (*verified*): **providers** (Part C correction). `GOOSE_PROVIDER=anthropic`
  (`ANTHROPIC_HOST` = the endpoint host without `/v1`, `ANTHROPIC_API_KEY` = the placeholder) serves
  `anthropic_messages`; `GOOSE_PROVIDER=litellm` (`LITELLM_HOST` = the host without `/v1`,
  `LITELLM_BASE_PATH=v1/chat/completions`, `LITELLM_API_KEY` = the placeholder) serves `openai_chat`,
  always on the Chat Completions path. **The `openai` provider is not used**: it chooses the Responses API
  by model name (`o*`, `gpt-5*`, `*codex*` go to `/v1/responses` whatever `OPENAI_BASE_PATH` says, every
  other model to chat), which an endpoint's declared dialect cannot promise, so **`openai_responses` is not
  declared**. Also `GOOSE_MODEL`, `GOOSE_MODE=auto`, `GOOSE_TELEMETRY_ENABLED=false`,
  `GOOSE_DISABLE_KEYRING=1`, `GOOSE_DISABLE_SESSION_NAMING=true`, `CONTEXT_FILE_NAMES=["AGENTS.md"]`,
  `GOOSE_CA_CERT_PATH` = the boot CA bundle, `GOOSE_CONTEXT_LIMIT` from `model_meta`, and the effort table:
  `GOOSE_THINKING_EFFORT` = `low`, `medium`, `high`, `max` (thinking budgets of 4000, 10000, 16000 and 32000
  tokens on a Claude model). `config/config.yaml` (rendered): `developer` on (`type: builtin`), `aurora` as
  `type: stdio` (`cmd`, `args`, `envs`; tools are named `aurora__<tool>`), and the platform extensions
  `apps` and `extensionmanager` **off** (`type: platform`, `enabled: false`; goose enables them by default.
  The old `memory` and `computercontroller` builtins are not in 1.59.2's default set and are not listed).
  The agent's tools are then `analyze`, `edit`, `read_image`, `shell`, `tree`, `write` (developer), `load`,
  `load_skill`, `delegate` (subagents and skills) and `aurora__<tool>`. The stdio server must answer goose's
  first request `server/discover` with a JSON-RPC error (`-32601`) or a result; the bridge does, and goose
  then falls back to `initialize`.
- **No vendor network** (Part C): goose fetches its model catalogue from `https://models.dev/api.json`
  **once per start** and has no switch for it, **but a pre-seeded cache stops the fetch** (*verified*): goose
  reads `<root>/data/model_catalog/models_dev_api.json` and its `models_dev_api.etag`, and when both exist
  and are fresh it makes no connection to models.dev. The adapter renders both before every turn (the
  catalogue holds only the session's models, the default and the resolved roles, with their context
  window; **the etag file must end with a newline**, otherwise goose fetches anyway: the conformance suite's
  "the proxy saw no request" assertion caught that). So there is nothing to refuse and **no row per turn**;
  without the seed the attempt would be one refused `CONNECT models.dev:443` per start (about 0.2 s here,
  against a proxy that refuses at once) that the gateway records as a denied egress request in the
  workspace's request log. No decision is left to the owner for it. **A second discovery request remains
  for the `litellm` provider** (`openai_chat`): goose asks `GET /model/info` for the model's context
  window at every start (the `anthropic` provider makes no such call; `GOOSE_CONTEXT_LIMIT` does not stop it
  for `litellm`). **(D-367, owner 2026-11-18) The gateway forwards it**: `GET /model/info` is part of the
  default inference-path set of endpoints of kind `litellm` only (that one path, `GET` only, below the
  endpoint's base path), forwarded like an inference path with the placeholder replaced by the endpoint's
  key (`08 §Inference paths`); `POST` to it, any other metadata or management route (`/model/group/info`,
  `/model/new`, `/key/info`) and any other endpoint kind stay `403 llm_path_not_permitted`. So there is no
  refused row per turn any more. goose still tolerates a refusal (it falls back to the rendered
  `GOOSE_CONTEXT_LIMIT`: "Context-limit discovery failed") when an endpoint kind or an old gateway refuses it.
- **Capabilities**: `resume`, `tool_events`, `platform_tools`; `thinking_deltas = false` (no thinking
  content in the stream); `steer = false`. `skills_dir = ".agents/skills"` (*verified*: found in the home
  and in the project, with `AGENTS.md` read by default and `CLAUDE.md` not). **Roles**: `subagent_role` →
  `GOOSE_SUBAGENT_PROVIDER` and `GOOSE_SUBAGENT_MODEL` = the model of the role it names (*verified*: the
  conformance suite's delegated subagent sends its requests to that model), so
  `model_roles = [large, medium, small]` (`harness_model_roles("goose")`); with no role the subagent uses the
  main model.

**`opencode`** (release binary, checksum verified; **pinned on the v1.19.x line**, D-363, so the v1 CLI
and config schema of `opencode.ai/v1/docs` apply, not the v2 docs):
- **Drive** (*verified*): `opencode run --format json -m <provider>/<model> [--session <ses_…>]
  [--variant <effort>]` per turn, **without `--attach`**: the run is in-process, and its MCP
  child's parent is the `opencode run` process (D-301; the conformance suite checks the ancestry).
  **Prompt size rule (Part D, *verified*):** the message is **not** an argument (Linux limits a single
  argument to `MAX_ARG_STRLEN`, 128 KiB, far below the turn's input bound). With no positional message,
  `opencode run` reads the whole of **stdin**, which the adapter writes and closes; a 400 KB prompt
  (three times the limit) reaches the model request complete, the last line included, on Messages and Chat
  (a conformance test, and a unit test that no argument carries the prompt). So there is no oversize
  failure to map: no prompt is truncated and none is refused for its size. `--auto` exists in 1.19.6 but
  the `permission` block does the job; `--thinking` is not used (text arrives whole and `thinking_deltas`
  is false). SIGTERM or SIGKILL mid-turn ends the process without a terminal event; the session stays
  resumable. **Session ref:** the adapter reports the **conversation's id** as the session ref (its
  directory is named after it, so a purge by ref removes exactly that state, as for `pi` and `goose`);
  OpenCode's own `ses_…` id comes from the first event's `sessionID`, is kept by the live session, and
  after a restart is read again with `opencode db "select id from session where parent_id is null order
  by time_created desc limit 1"`; if the data is gone (a purge) the turn starts a new session instead of
  failing. The first event's `sessionID` is the turn's own; events of any other session are ignored.
- **Events** (*verified*, one JSON object per line, all with `sessionID`): `step_start`; `text` (the whole
  part, `part.text`; **no streaming deltas**, thinking only with `--thinking`); `tool_use` (`part.tool`,
  `part.callID`, `part.state {status, input, output}`, already completed); `step_finish`
  (`part.reason` `stop`|`tool-calls`, `part.tokens {total, input, output, reasoning, cache {write, read}}`;
  `input` excludes cached tokens); `error` (`error.name = "APIError"`, `error.data {message, statusCode,
  isRetryable, responseBody}`) with exit status 1. The run ends when the process exits: exit 0 → `Done`,
  non-zero or an `error` event → `Error`, classified from `statusCode` and `responseBody` through
  `provider_error.rs` (the adapter hands the table `status <code> <body>`). The answer is complete at the
  first `step_finish` whose `reason` is not `tool-calls`; the process normally exits at once, and a process
  that lingers (a child of the agent's own command holding the output pipe) is stopped after 10 s and the
  turn is `Done`. **Retries (Part D, *verified*):** OpenCode retries a retryable error (`isRetryable`: 429
  and 5xx) with a growing delay and reports it after the **sixth request, about 72 s** for a 429 on every
  request (`fail-429` against the mock); a 401 or a 402 is reported at once. **1.19.6 has no setting for
  it** (no config key, no variable: the strings of the binary were searched), so it cannot be aligned with
  `codex` (two retries, about 6 s) or `pi`; instead **the delay is written into the mapping**: the message
  of a retryable error ends with "(OpenCode retried the request for about 70 seconds before giving up)", so
  the dashboard's wait is explained. The conformance suite's refusal case uses the mock's `402` billing
  error (→ `provider_quota`), which OpenCode reports at once; the 429 text is covered by a unit test over a
  recorded event, because 72 s is longer than the suite's stall bound.
- **Usage** (*verified*): events carry no model. After a `Done` the adapter runs `opencode db "<SQL>"
  --format json` (the same no-network environment and directories as the turn): the assistant messages
  of the turn's session **and every session below it** (a recursive query over `session.parent_id`, so
  subagents are included) created since the child started (`message.time_created >=` the start in
  milliseconds), JSON column `data` with `modelID` and `tokens {input, output, cache {read, write}}`,
  `sum`med and grouped by `modelID`. `input` excludes cached tokens on every dialect; cost is not read.
  If the query fails or returns no row, the usage is the `step_finish` tokens summed into **one entry for
  the default model** (D-304: the API folds any model it does not accept into the default's entry anyway).
  `opencode db` needs no `sqlite3` binary and no network (it ran in the image with `--network=none`).
  Its numbers are a sandbox claim under D-304's trust rule, like every other harness's usage. The
  conformance suite checks the four counters on all three dialects and that a delegated subagent's tokens
  are a second entry under its own model.
- **Config** (*verified*, v1 schema), `opencode.json` through `OPENCODE_CONFIG`: `autoupdate: false`,
  `share: "disabled"`, `permission: {"*": "allow"}`, `agent.title.disable: true` (otherwise a title request
  goes to the small model on every new session), `instructions: ["/workspace/AGENTS.md"]`, `model`,
  `small_model` and `agent.general.model` (`"<provider>/<model>"`, both *verified*: the subagent's request
  used the configured model), one `provider.<id>` per dialect: `npm` `@ai-sdk/anthropic` (`/v1/messages`),
  `@ai-sdk/openai` (`/v1/responses`) or `@ai-sdk/openai-compatible` (`/v1/chat/completions`), all bundled
  in the binary, `options.baseURL` from `ModelChoice`, `options.apiKey: "{env:ANTHROPIC_API_KEY}"` (or
  `OPENAI_API_KEY`), `models.<id>.limit {context, output}` from `model_meta` (omitted when the window is
  unknown) and `models.<id>.variants.{low,medium,high,max}` carrying the provider's reasoning options (a
  `--variant` that the config does not define is silently ignored, so all four are always rendered).
  **Effort table (Part D, *verified*: the request carries it):** `@ai-sdk/anthropic` →
  `thinking {type: "enabled", budgetTokens: 4000 | 10000 | 16000 | 32000}` (the request's
  `thinking.budget_tokens`); the OpenAI packages → `reasoningEffort` `low`, `medium`, `high` (`max` is
  `high`, the highest level the API accepts for every model; the request's `reasoning_effort`). An empty or
  unknown effort passes no `--variant`. `options.baseURL`: the Anthropic package wants the host **with**
  `/v1` (it appends `/messages`), so the adapter adds `/v1` to the endpoint's base URL; the OpenAI packages
  take the OpenAI form (`…/v1`). `models` holds the default model and every resolved role's model.
  `mcp.aurora = {"type": "local", "command": [...],
  "enabled": true, "environment": {}}`; the tools are exposed directly as `aurora_<tool>` next to the
  built-ins (`bash`, `edit`, `glob`, `grep`, `read`, `skill`, `task`, `todowrite`, `webfetch`, `write`);
  the adapter names the platform tools `mcp__aurora__<tool>` in events, like the other harnesses.
- **State layout (Part D, as goose's one root):** the conversation's directory
  `<home>/.aurora/harness/opencode/sessions/<ref>/` holds everything, so a purge by ref removes it whole:
  `config/` (`XDG_CONFIG_HOME`; `opencode.json` through `OPENCODE_CONFIG`, plus the files below), `data/`
  (`XDG_DATA_HOME`: `opencode/opencode.db`, `log/`, `snapshot/`), `cache/` and `state/`
  (`XDG_CACHE_HOME`, `XDG_STATE_HOME`). Before every start the adapter **deletes `config/` whole** (an
  agent's own `agents/`, `plugin/` or a second `opencode.json` there are gone), renders it again with
  read-back, checks `data`, `data/opencode`, `cache` and `state` one by one (a link planted at `data` is
  replaced, never followed) and deletes `data/opencode/auth.json` and `mcp-auth.json`.
- **No vendor network** (*verified*: with all of these a turn makes only the inference request):
  environment `OPENCODE_DISABLE_MODELS_FETCH=1` (models.opencode.ai), `OPENCODE_DISABLE_DEFAULT_PLUGINS=1`,
  `OPENCODE_DISABLE_LSP_DOWNLOAD=1`, `OPENCODE_DISABLE_AUTOUPDATE=1`, `OPENCODE_DISABLE_PROJECT_CONFIG=1`
  (a workspace `.opencode/` or `opencode.json` cannot replace the rendered configuration; it also stops
  project `AGENTS.md` discovery, hence `instructions` above) and `OPENCODE_DISABLE_CLAUDE_CODE=1`
  (`CLAUDE.md` and `.claude/skills` are not read). **The background `npm install @opencode-ai/plugin`
  into the global configuration directory** (registry.npmjs.org) returns at once when the directory is
  **not writable by the running user** or already holds `node_modules` and a `package-lock.json` listing the
  dependency (*both verified in Part D*: with either, a start makes no request to the registry).
  **Uid model and the choice:** the supervisor, `opencode` and the agent's own commands all run as uid 1000
  (`§Process model`), so there is **no uid that can write the directory and the agent cannot**: a
  read-only directory (mode 0555 set right before every start) would only stop the install until the agent
  `chmod`s it back, and it is a mode bit the render and read-back cannot assert. The adapter therefore
  renders the second form, which needs no mode bits and no uid: `config/opencode/package.json`,
  `package-lock.json` (`packages[""].dependencies` = `@opencode-ai/plugin` at the pinned version) and
  `node_modules/@opencode-ai/plugin/package.json`, written and read back before every start with the rest
  of the configuration (the whole directory is rebuilt, so a deleted file is back at the next start). An
  agent that deletes them in a turn only re-enables the install attempt, and **the backstop is the session
  proxy, which refuses `registry.npmjs.org`**: the conformance suite asserts that no request to the npm
  registry, models.opencode.ai or any other host reaches the proxy (fail-before check: without the rendered
  files six `CONNECT registry.npmjs.org:443` per turn reach it). `webfetch` stays (it goes through the proxy
  like any agent traffic); the optional web search (`OPENCODE_ENABLE_EXA`) is never enabled. Also
  `XDG_*` and `OPENCODE_CONFIG` point into the root; variables the caller supplies for OpenCode, the
  providers or the proxy are dropped (`OPENCODE_*` including `OPENCODE_CONFIG_CONTENT`, `XDG_*`,
  `ANTHROPIC_*`, `OPENAI_*`, `NODE_EXTRA_CA_CERTS`); the CA bundle is passed as `SSL_CERT_FILE` and
  `NODE_EXTRA_CA_CERTS`.
- **Capabilities**: `resume`, `tool_events`, `platform_tools`; `thinking_deltas = false` (text arrives
  whole); `steer = false`. `skills_dir = ".agents/skills"` (*verified*, the project's and the home's;
  `OPENCODE_DISABLE_EXTERNAL_SKILLS` stays unset). **Roles**: `roles.small` → `small_model`, and
  `subagent_role` → `agent.general.model` (the model of the role it names, any of the three), so
  `model_roles = [large, medium, small]`; `large` and `medium` are used only when `subagent_role` names
  them.

The web terminal gets `pi`, `goose` and `opencode` wrappers like `claude` and `codex` (D-247, C-1015).

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
(phase 5 (D-138, moved from phase 4), P4-LLM) The classes for the new providers come from the shared
`aurora-llm` table (`08 §Phase 4 providers`, *Error classification*), so a Bedrock
`ThrottlingException`, an OpenAI `insufficient_quota` or a Vertex `PERMISSION_DENIED`
map to `provider_rate_limited`, `provider_quota` and `provider_auth` the same way in
every adapter. The new gateway codes `llm_path_not_permitted` and
`llm_request_too_large` are gateway refusals (`gateway_refused`). (phase 5, P4-LLM spec
batch, D-261) So is a 502 `credential_unavailable` with `details.kind = "llm_mint"`; when
its `reason` is `mint_rejected` the adapter reports `provider_auth` instead (the
credential, not the gateway, is at fault), with the gateway code kept in the message.
Errors inside a 200 stream (an event-stream exception, an SSE `error` event) are classified
by the adapter from the event's error type with the same table (D-260(d)).
(P4-LLM-5, implemented) The shared table lives in `aurora-core` (`llm_classify`, re-exported as `aurora_llm::classify`) because the
supervisor does not depend on `aurora-llm`; both adapters call it through `adapters/provider_error.rs`. The `claude-code` adapter
drops every `AWS_*` credential variable, `AWS_BEARER_TOKEN_BEDROCK`, `GOOGLE_APPLICATION_CREDENTIALS` and profile variable from
`client_env` and refuses an unknown dialect with `harness_failed`; the environment is identical for the three Bedrock auth modes.
(phase 5 (D-138, moved from phase 4), RFE-011b) `approval_parked` is a gateway refusal that is **not a failure**: a turn
that ends normally after it is `done`. (P4-011B spec batch, D-264(a)(c); corrects the earlier text,
which had the supervisor turn it into an activity line) The supervisor never sees it: the refusal
arrives as the HTTP response of the agent's own tool call, inside the harness, and the platform
preamble (`10 §Agent instructions`) tells the agent how to behave; the person sees the
`approval.pending { parked: true }` transcript event the API appends. The harness's **own** model
requests never park (requests to governed LLM endpoints are held, `07` *Which approvals park* 6), so
`approval_parked` can never reach the provider-error mapping above and needs no entry in it. No
adapter changes. Harness-level tool-failure counters
(`TOOL_CONSECUTIVE_FAILURES`) apply to platform tools only and do not see it. Resuming is a new
turn, so no harness needs a "suspend" capability and none is added to the adapter contract.

## Platform tools

### Path

Harness → MCP (stdio) → `aurora-supervisor mcp-bridge --session <id>` → Unix
socket `/run/aurora/tools.sock` → supervisor → `tool.call` frame (stamped with
conversation and current turn) → runner (stamps sandbox and boot) →
`POST /v1/runner/tool-calls` → **tool registry** → result back.

(phase 5, FIX-151, D-301, SEC-121) **Who may call.** The socket is mode 0666 and the sandbox runs one uid, so the
`session` a request names proves nothing. The supervisor reads the connecting process's pid with `SO_PEERCRED`
(with its start time, at accept) and, for **every** request (`list` and `call`), walks that process's parent chain in
`/proc/<pid>/stat` (at most 64 steps, `comm` parsed from the last `)`) until it reaches a pid in the **harness
registry**. Every adapter's launcher registers the process it starts for a conversation (`proc_ancestry`:
pid, conversation, `/proc` start time) and the entry goes when that process exits, so a recycled pid never matches. The
request is served only when that harness's conversation equals the request's `session`; any other outcome (no
harness above the caller, another conversation's harness, a vanished or recycled pid) answers `session_not_caller`
with one fixed text that names no conversation, bumps a refusal counter, and logs no arguments. The check cannot be defeated by tracing the harness: the sandbox's seccomp profile refuses `ptrace` and `process_vm_readv/writev` to every process in it (D-394, SEC-137), and no adapter, the terminal or the language servers need them. `session` stays on the wire
as a cross-check; it is never corrected. The turn is still checked as before: a harness that runs while its
conversation has no turn gets `no_active_turn`. The MCP bridge and `aurora-agent` run from the harness's shell pass; a web-terminal
or SSH shell, a `process_start` child (a child of the supervisor, not of the harness), another conversation's harness
and a process that daemonized away from its harness (reparented to the subreaper) are refused.

(phase 5, P5-GUIDE-7, D-297, SEC-119, `24 §10b`) A second client takes the same path from the
socket on: **`aurora-agent`** with the target `aurora` (`aurora-supervisor agent`, wrapper
`/opt/aurora/bin/aurora-agent`, `24 §10a`),
run by the agent from its shell, with the conversation from `AURORA_SESSION`, which the
supervisor sets in the environment of every harness process it starts (not for the web
terminal, SSH sessions or `process_start` children). Each `ToolDescriptor` carries `family` and
`exposure` (`core` / `on_demand`); in mode `on_demand` (`hosted.platform_tools`, carried in the
render inputs as `platform_tools`) the bridge's `tools/list` returns the core tools only, its
`tools/call` still forwards any name, and the socket's `list` returns every available tool. A
supervisor without the command (no wrappers, `agent_cli` false, C-653) behaves as mode `all`.
Idle sessions are released on a catalogue change only when the core descriptors or the rendered
instruction files change (C-654).

- (FIX-110) `turn_id` is required and must be an active turn of the calling agent;
  a call without one is refused `no_active_turn` (the supervisor refuses it locally
  when no turn is running in the conversation, and the API refuses it again).
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
| Skills | `skill_list {}`, `skill_get {name}`, `skill_create {name, description, body, files?}`, `skill_update {name, …}`, `skill_delete {name}` (agent tier only; phase 3, RFE-020: `skill_list`/`skill_get` return the effective set of all three tiers with `tier` and `shadows`, writes touch the agent tier only; phase 4, P4-GSL: `skill_list`/`skill_get` also return `source` for a library skill, and `skill_update`/`skill_delete` on a library skill are refused with `library_skill_readonly` — only people change what a library pinned, `10 §Skills`; `skill_create` cannot take a library source) |
| Schedules | `schedule_create {name, prompt, cron?, run_at?, timezone?, gate_command?, gate_description?}` (exactly one of `cron` / `run_at`; phase 5, P5-SCHED-ONCE, C-1094: `run_at` is a local `YYYY-MM-DDTHH:MM` that runs once; `timezone` defaults to the speaker's `users.timezone`, else the workspace's; needs a person speaking, so a schedule run cannot schedule again; one made in a person's own chat runs in that chat), `schedule_list {}` (items carry `run_at` (local) and `completed_at`; `cron` may be null), `schedule_update {id, …, run_at?}` (a new `cron` / `run_at` without `timezone` is read in the speaker's zone), `schedule_delete {id}` |
| Webhooks | `webhook_list {}`, `webhook_deliveries {id}`, `webhook_update {id, instructions?, enabled?}` |
| Processes (local) | `process_start {name, command, cwd?}`, `process_status {id?}`, `process_stop {id}`, `process_watch {id, kind, pattern?, silence_secs?}` |
| Files | `send_file {path, caption?}` (to the current conversation) |
| Conversations (phase 5, P5-CHAT-4 part C, D-230, C-728; always available to hosted agents) | `conversation_search {query, where?, author?, after?, before?, limit?}`, `conversation_read {conversation_id, seq, before?, after?}` — the agent's own memory of what was said in its conversations, on the message-search index (`10 §Search`). Scope computed at call time from the stamped agent and turn: the current conversation, the speaker's own direct conversation, and non-direct conversations the speaker can see; never another person's direct conversation, even for an owner speaker. Results are wrapped as untrusted quoted data; the descriptions repeat the direct-conversation privacy rule of `10 §Visibility of direct conversations` |
| Messaging (available with an active presence) | `find_recipient {query, platform?}`, `send_message {to, text, files?, platform?}` (`to`: `me` (phase 5, P5-PERSON, C-1055: the person the turn runs for, in a DM, no approval), `here`, an alias or id from `find_recipient`, or an exact handle / space name), (phase 5, P4-UTOK) `send_as_me {to, text}` (only for an agent holding a grant of a person's token; a held approval of that person, `13 §5.11`), `read_conversation {target?, limit?}`, `list_my_messages {}`, `edit_my_message {message_id, text}`, `delete_my_message {message_id}` (`13 §Messaging tools`) |
| Agents | `list_agents {}`, `message_agent {agent, text}`, `complete_task {report}`, `list_peer_tasks {}`, `cancel_peer_task {id}` |
| Apps | `list_apps {}`, `request_app_connection {app_id, reason}`, `propose_custom_connection {name, host, path?, recipe, reason}` |
| Calendar (phase 5, P5-CALINV, D-274, C-844, SEC-108; present only while the instance mailer is configured (`email.smtp_url` set, not log-only) and the org switch `calendar_invitations.enabled` (default on) allows them) | `calendar_invite {title, start, end, timezone?, all_day?, location?, description?, repeat?, reminder_minutes?}` → `{ id, status: "sent" }`, `calendar_invite_update {id, …the same fields}` (the full event again: same `UID`, `SEQUENCE + 1`), `calendar_invite_cancel {id}` (`METHOD:CANCEL`), `calendar_invite_list {}` → `{ items: [{ id, title, start, end, timezone, all_day, status }] }` (this agent's invitations to the current person, newest first, at most 50). The agent e-mails the **person** a standard invitation (iMIP); their Accept puts it in the calendar, so no calendar access or credential exists. **The only recipient is the verified account e-mail of the person the turn runs for** (the speaker of the triggering message, or a schedule's creator; webhook, peer and creator-less schedule turns get `no_person`); there is no address or attendee argument, and unknown arguments are refused. `start`/`end` are local date-times with `timezone` (an IANA name; default: the workspace's, the person has no profile zone) or dates with `all_day` (`end` is then the last day, inclusive); `end` after `start`, at most 14 days long, the start within 5 years of now; `repeat {freq: daily\|weekly\|monthly\|yearly, interval?: 1–12, by_day?: [MO…SU] (weekly only), count?: 1–200, until?: date}` needs exactly one of `count`/`until` (`until` at most 10 years after the start); `reminder_minutes` 0–10 080; `title` and `location` at most 200, `description` at most 4 000 characters (plain text). Errors (the tool's `code`): `invalid_arguments` (the message starts with the field), `no_person`, `no_verified_email`, `not_found` (another agent's or person's id), `rate_limited` (the message carries `retry_after_secs`; 20 sends per person per agent per hour and 60 per person per day across agents, every invite, update and cancel counting), `mail_unavailable` (nothing is recorded), `tool_unavailable`. The three writes are in the eval-refusal set (`eval_read_only`, C-868); `calendar_invite_list` is a read. The mail: `15 §Calendar invitations`; what agents can and cannot do: `10 §Calendar invitations` |

## Home projection

The home (`/workspace`) holds:

1. **Agent-owned** free-form files (never touched by the platform).
2. **Projection** (managed roots only): `<skills_dir>/` (0444/0555),
   `memory/` (`*.md` 0644, (P6-MEMLOCK, C-1149) a locked memory `0444`, +
   `index.md` 0444), instruction files (0444) at the
   paths the harness declares, `.aurora/` (tool catalogue cache, inbox
   attachments), (phase 5, P5-GUIDE-2) `.aurora/apps/<app_id>/` (the files Aurora writes for each
   connected app: `app.md`, `tools.md`, `api.json`, 0444 in 0555 directories; `24 §5`, `§7`);
   (P5-APPSKILL-2, C-1134) the app skills of the agent are ordinary entries of the `skills`
   projection (lowest tier, `0555` for a script, else `0444`).
   (P5-SKILLDIR, D-454, C-1140) The skills folder is exported as `SKILLS_DIR`
   (`<home>/<skills_dir>`, absolute, no trailing slash) to the harness process, `process_start`
   processes, terminal shells and SSH execs (`§Process manager`, `§Terminal`, `§SSH channels`);
   the projection itself is unchanged.
3. **Ephemeral** `/run/aurora`: CA, tools socket, boot-time stubs outside home.

### Sync protocol

- The API sends `home.sync` in ordered **parts** of one **generation**, each ≤
  `HOME_SYNC_PART_MAX` UTF-8 bytes (part count sized for worst-case encodings:
  CJK ×3, JSON-escaped controls ×6).
- Part kinds: `skills` (full files), `memory` (rendered files; (P6-MEMLOCK,
  C-1149) a locked memory's entry carries `locked: true`, omitted when false,
  and installs `0444`), `render_inputs`
  (preamble data, brief, capability fragment inputs: apps with snippets and
  docs links, presences, peers, memory language, (P6-MEMLOCK, C-1151) `memory_locked` when the
  agent holds at least one locked memory, schedules summary, timezone),
  `tools` (catalogue), (phase 5, P5-GUIDE-2) `apps` (the app folders' generated files, below), (phase 4, P4-PRV-1) `previews` (the active preview shares, held in memory only, `§Previews`), `manifest` (the complete list of managed paths), `final`.
- (phase 5, P5-GUIDE-2, D-171, SEC-117, `24 §7`) **`apps` parts** carry file entries
  `{ path, content }` (the entry shape of `skills`) for `.aurora/apps/<app_id>/<file>`: the
  files the API renders per connected app (`app.md`, `tools.md`, and from P5-GUIDE-6
  `api.json`), nothing else; the app's skills travel in the `skills` part, up to
  `APP_SKILLS_SYNC_MAX` per sync (apps beyond the budget, in `app_id` order, project no skills and
  their `app.md` says so; metric `aurora_home_app_skills_budget_exceeded_total`).
  `.aurora/apps/` is a managed root: an entry is accepted only at exactly
  `.aurora/apps/<id>/<name>` with both segments matching their grammars (`24 §3.1`), files are
  installed 0444, and the root is pruned to the manifest at commit, so a folder disappears with
  the grant, connection or availability that put it there. Each `render_inputs` app entry gains
  `app_id`, `summary` and `guide_path`; `name`, `accounts`, `docs_links` and `snippet` are
  unchanged. A supervisor that predates the kind skips the part and renders as before; a
  supervisor that gets no `guide_path` (an older API) renders as before. A sync that changes
  only files under `.aurora/apps/` leaves the instruction files byte-identical and therefore
  releases no idle session.
- The supervisor applies each part idempotently without buffering; only the
  **final** part prunes managed roots to the manifest, re-renders instruction
  files, and acks `home.synced { generation }`. A crash mid-generation leaves
  stale bytes and no ack; the next boot starts at generation 0 with a full sync.
- Paths outside managed roots are refused (warning), although traversal is
  already unrepresentable on the wire.
- Desired generation = `agents.home_sync_generation` + workspace generation +
  org-skill generation (the API computes a combined monotonic value; phase 3, RFE-020: workspace-tier skill changes bump the workspace generation); any change
  to skills, memory, brief, name, presences, peers, grants/connected apps,
  memory language, timezone, (P6-MEMLOCK, C-1148) a memory's lock or tool availability bumps it. A bump never wakes a
  parked sandbox.

### Memory harvest

On a 10 s timer and before acking a sync, the supervisor scans `memory/*.md`;
files whose checksum trailer fails (or that are new) are uploaded with
`memory.write {key, content}` (bounded; titles/descriptions parsed from
optional YAML front matter). Invalid keys are reported back as a notice to the
next turn. Files stay in place until superseded by the canonical render.
(P6-MEMLOCK, C-1149) A **locked memory is never uploaded**: the supervisor
installs its file `0444`, records the exact bytes it installed (in memory
only), and on a failed checksum, a replaced file or a missing one writes those
bytes back `0444` and gives the next turn one notice per changed digest —
"memory `<key>` is locked: a person set it, and you can read it but not change
or delete it. Your change was undone." An older supervisor that does upload
gets the API's 409, which reaches the turn through the existing
`memory.write.result` notice.

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

**Calendar links file** (P5-ICS, D-273, C-843). For each active connection of a secret-link app the agent
is granted, the `render_inputs` entry carries `links: [{name, app_id, connection_id}]`, and the
supervisor writes `.aurora/links.json` (`aurora-supervisor::links`, `0600`):
`[{ "name", "app", "url": "https://<connection_id>.link.aurora.internal/" }]`. The `url` is the
connection's **synthetic address** that only the gateway resolves to the real link: no secret is in the
sandbox, and two links with one name both appear, told apart by their address. The file is removed
when the list is empty. It does not depend on the database relay.

**Servers file** (P5-CALDAV, D-275, C-845). For each active connection of an `own_server` app the agent
is granted, the `render_inputs` entry carries `servers: [{name, app_id, connection_id, url}]` (generic:
a later own-server app reuses it), and the supervisor writes `.aurora/servers.json`
(`aurora-supervisor::servers`, `0600`): `[{ "name", "app", "url" }]`, `url` being the calendar home
discovered at connect. The address is visible to the agent on purpose; no credential is in the sandbox
or the sync. Only plain `https` addresses without userinfo are written; the file is removed when the
list is empty.

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
  container env (machine proxy) plus `SKILLS_DIR` (P5-SKILLDIR, D-454: the adapter's skills
  folder, set last so an image value does not win); stdout/stderr interleaved into a ring buffer
  (`PROCESS_TAIL`); busy = output within 60 s or CPU > 5 % over 30 s.
- `process.state` frames on change (debounced 2 s).
- Watches are evaluated in memory; a fired watch sends `watch.fired`.
- **Gates** for schedules run with the same posture and `GATE_TIMEOUT` (well
  below `STALE_DISPATCH`, since a gated turn stays `dispatched` while its gate
  runs); stdout is captured (≤ 16 KiB).
- Harness-run background tasks (if the adapter exposes them) are mirrored
  through the same `process.state` path so the platform knows about all
  background work.

### Previews (phase 4, P4-PRV-1, D-144, D-145; streams in P4-PRV-2)

- The supervisor keeps the **share list** in memory only: the `previews` part of the home sync
  (`{ shares: [{ share_id, port, start_command?, cwd? }] }`, replaced as a whole by each full
  sync, empty when previews are off, the org has them off, the share is paused or the agent is
  frozen). Nothing is read back from the home (the agent can write it) and nothing is persisted.
- `ports.state` discovery is informational only: on the 10 s harvest tick (the first fires at
  boot) the supervisor scans `/proc/net/tcp{,6}` for TCP LISTEN rows (st `0A`) on loopback
  addresses (127.0.0.0/8, ::1, wildcard binds) and reports `ports.state { ports }` only when the
  set changed.
  Port 0 is never reported. The allow-list for streams is the share list, **never** the
  discovery result (`12 §Streams`).
- **On-demand start**: the frame `preview.start { port }` (from the work item `preview.start`)
  looks the port up in the share list; with a `start_command` and the port not listening, the
  supervisor starts it as a background process `preview:<port>` (kind `preview` in
  `process.state`, `/workspace` or the share's `cwd`, container environment, own process group,
  counted in the ordinary process limits, output in the ring buffer like any process). An
  unknown port, a share without a command or a port that is already listening is a no-op.
  A start never carries command text on the wire and never runs while the agent is frozen
  (the share list is empty).
- **Streams** (P4-PRV-2, `16 §W18`): on `stream.open { stream_id, port, window }` the supervisor
  accepts only a port of its **share list**, 1024..65535 and not one of its own listeners;
  connects to `127.0.0.1:<port>` (then `[::1]`) only, 5 s timeout, no proxy, no other address;
  answers `stream.opened`/`stream.refused`; relays bytes with credit-based flow control (128 KiB
  windows, 16 KiB chunks, round-robin between streams, control frames first); at most
  `PREVIEW_STREAMS_PER_SANDBOX` (64) concurrently, more → `refused { limit }`; closes everything on
  channel loss, `shutdown` or a changed share list that removed the port. Stream bytes never pass
  through the harness, the gateway or the loopback proxies and are never logged or stored (only
  counts). With an empty list the supervisor refuses every `stream.open`: that is the state of every
  sandbox of an instance, org or agent without previews.
- A preview-started process is an ordinary process: it is *busy* by the usual rule, parks and
  is marked `parked` with the boot, and keeps the sandbox awake only through the ordinary
  background rule; the `preview` keep-awake reason comes from stream activity alone.

### Terminal (phase 4, P4-TERM, D-161; wire in `16 §W19`)

- On `pty.open { pty_id, window, cols, rows, shell? }` from its channel — which only the runner sends,
  and only for an API-redeemed terminal grant of the current boot — the supervisor spawns the
  person's chosen shell under a pseudo-terminal as the agent user (FIX-138, D-246: `shell` is
  the enumeration `bash` (default, also when absent) or `zsh`, never a path; the supervisor maps
  it to the fixed binaries `/bin/bash` and `/usr/bin/zsh` and starts them as an interactive
  **login** shell, `-l`. A missing zsh falls back to bash; a missing bash — or neither — to
  `/bin/sh` without `-l`; each fallback prints one line in the pane ("zsh is not installed in
  this sandbox image; starting bash instead." / "bash and zsh are not installed in this sandbox
  image; starting sh (no completion or history)."). `$SHELL` of the supervisor is no longer
  consulted; the PTY's `SHELL` is the chosen binary)
  (uid/gid as the process manager uses), cwd `/workspace`, own session with the
  pseudo-terminal as its controlling terminal (job control works); the supervisor opens both
  ends with `O_NOCTTY` and never acquires a terminal itself (it is PID 1 and a session leader
  in the sandbox), and answers `pty.opened`. Closing a terminal hangs the session up
  (SIGHUP), then after a short grace kills its process group (SIGKILL): a shell that ignores
  HUP does not outlive its terminal. The environment is the container environment **minus every `AURORA_*`
  variable** and minus the boot document: no platform credential, no token, no supervisor
  handle ever appears in a terminal's environment (SEC-01/SEC-34 posture). (P5-SKILLDIR, D-454)
  `SKILLS_DIR` is added after the sweep, beside `TERM` (the adapter's skills folder; the `AURORA_*`
  sweep has no exception, which is why the name has no prefix) — the shell has
  exactly the privilege a `process_start` command already has, in a PTY.
- **`PATH` and interactive defaults (FIX-138).** One `PATH` for both shells, a turn and the
  terminal: `/workspace/.local/bin:/opt/aurora/bin:/usr/local/bin:/usr/bin:/bin` (the image's
  `ENV PATH`, `/etc/profile.d/aurora.sh` for bash login shells, `/etc/zsh/zshenv` for zsh, the PTY
  spawn's environment and the supervisor's spawn default `spawn::IMAGE_PATH`). `/opt/aurora/bin`
  is the directory of the `claude` and `codex` wrappers (below). System-wide **interactive** defaults (aliases `ls`,
  `ll`, `la`, `l`, `grep`, colours, a prompt `user@<agent handle>:cwd`, history size and
  de-duplication, zsh `compinit`) live only in the image's system files — `/etc/profile.d/aurora.sh`
  (read by a **login** bash through `/etc/profile`; the terminal and SSH `shell` both start one) and
  `/etc/zshrc` (zsh) — not in the agent's home, so a person's own `~/.zshrc` still wins — and are
  read by interactive shells only: agent turns and `bash -c` never see the aliases. (FIX-218,
  D-459(d)) A login bash reads `~/.bash_profile`, else `~/.bash_login`, else `~/.profile`, never
  `~/.bashrc`; so `/etc/profile.d/aurora.sh` also sources the person's own `~/.bashrc` when the home
  has none of those three files (guarded against a double sourcing), and a person's `~/.bashrc` is
  honoured by the web terminal and SSH alike. The person's shell choice is stored per person and
  agent (`terminal_shell_preferences`, C-1145) and applies to both entry points.
- **Harnesses in the terminal (FIX-138 item 1b, D-247).** `claude` and `codex` on `PATH` are the
  wrappers `/opt/aurora/bin/claude` and `/opt/aurora/bin/codex` (image files, `images/sandbox/bin`),
  not the pinned binaries (`/opt/aurora/harness/…`, off `PATH`). At boot, and again with the agent's
  model at every session start, the supervisor writes **`/run/aurora/harness.env`** (mode 0644,
  **no secret**; **(FIX-155)** the wrappers *parse* it as data — blank lines, `# …` and
  `NAME='VALUE'` with `'\''` escaping only; any other line makes them print "… is not in the
  expected format" and exit 1 with nothing applied; they never source it and call `mkdir` and the
  pinned binary by absolute path. The terminal is the agent's own environment (`17`, "`/run/aurora`
  is the agent's"), so this removes one surprise, not a boundary): the harnesses the instance enables (boot document
  `enabled_harnesses`, from `hosted.enabled_harnesses`; absent = only the booted one), the machine
  proxy `http://127.0.0.1:3128` (`HTTPS_PROXY`…: the loopback forwarder adds the boot token, the
  terminal never holds it), the CA bundle (`SSL_CERT_FILE`, `NODE_EXTRA_CA_CERTS`,
  `CODEX_CA_CERTIFICATE`), the placeholder credential `aurora-placeholder` that the gateway
  replaces with the agent's governed LLM key (SEC-01/SEC-34: a real key is never in a terminal's
  environment or on disk), the CLIs' telemetry/auto-update switches, and — once a turn has chosen
  one — the model, and for a non-Anthropic endpoint its base URL, exactly the values the adapters
  put into a turn's environment (Codex takes its provider from `-c` overrides the wrapper builds,
  not from a config file). **(Owner 2026-11-14, C-750 (4))** Every terminal open also brings the
  models: the API resolves, at grant redemption, the model a turn of **each** enabled terminal
  harness (`claude-code`, `codex`) would use (the same `model::select`), and the runner hands them
  to the supervisor in `pty.open.models`; the supervisor rewrites the file before it spawns the
  shell. So `claude`/`codex` reach the agent's endpoint from the first terminal of a fresh boot,
  not only after a turn (before this, a fresh boot sent `claude` to `api.anthropic.com`, where an
  agent whose key is for another endpoint has no credential). The file keeps one section per
  harness; a turn's session start replaces only its own harness's section. A harness with no
  usable model gets `AURORA_<HARNESS>_UNAVAILABLE` (a fixed reason: no usable LLM key, or no model
  chosen) and its wrapper prints that and exits 1 instead of calling a host with no credential.
  An older runner or API (no `models`) keeps the previous behaviour. Each wrapper refuses with "`<harness>`: not
  enabled on this instance" (exit 127) when its binary is not in the image or its harness is not
  in `HARNESSES_ENABLED`; otherwise it sources the file, sets its **own state directory**
  (`CLAUDE_CONFIG_DIR=/workspace/.aurora/terminal/claude`, `CODEX_HOME=/workspace/.aurora/terminal/codex`,
  created by the supervisor and owned by the agent user) so an interactive session never reads or
  changes the agent harness's configuration, sessions or memory (FIX-095), and `exec`s the binary.
  **(P5-HARN, C-1015)** `pi`, `goose` and `opencode` are wrappers of the same kind, with sections
  of their own: `AURORA_<HARNESS>_MODEL` plus what the CLI needs, rendered by the adapter's own
  config code so a terminal and a turn agree: **Pi** gets `AURORA_PI_MODELS_JSON` (the `models.json`
  that the wrapper writes into `PI_CODING_AGENT_DIR=/workspace/.aurora/terminal/pi`) and
  `AURORA_PI_KEY_VAR`, with `PI_OFFLINE`, `PI_TELEMETRY=0`, `PI_SKIP_VERSION_CHECK` set. **(FIX-190,
  D-404)** Pi also gets `AURORA_PI_SETTINGS_JSON`, a turn's `settings.json` (`adapters/pi/config.rs`
  `terminal_settings`: the keys and values of `render_settings`, i.e. the retry bound,
  `defaultProjectTrust: "never"` (no project-trust question), no install telemetry, `quietStartup`,
  plus `collapseChangelog: true`, which only the interface reads: one line instead of the "What's
  New" page after an image update bumps Pi). Pi has no other first-run screen when a model is
  configured: login, provider and model choice open only on `/login` and `/model`, and the update
  check is off (`PI_SKIP_VERSION_CHECK`, `PI_OFFLINE`). Pi writes that file itself on its first start and
  the person may edit it, so the wrapper merges rather than replaces: it adds the keys the file lacks
  (`jq`, Aurora's values under the file's) and keeps every value already there; a file that has them
  all, is not a JSON object, or is a link is not touched. `models.json` is still written on every
  start (it holds only the gateway provider). With no model for Pi and no arguments, the wrapper says
  so ("no model is set up for this agent's terminal; check the agent's model on its Behaviour page",
  exit 1) instead of opening Pi's provider setup; arguments still pass through. Pi runs on the
  image's system Node.js, which needs the full ICU data (`docs/11 §Sandbox image`, FIX-190); **goose**
  gets `AURORA_GOOSE_PROVIDER` (`anthropic` or `litellm`) and `AURORA_GOOSE_HOST`, which the wrapper
  turns into goose's own variables, with `GOOSE_PATH_ROOT=/workspace/.aurora/terminal/goose`,
  telemetry, keyring and session naming off. **(FIX-189, D-404)** A bare `goose` opens its setup
  wizard whenever its config file does not exist, whatever the environment says, so the wrapper
  creates a minimal `$GOOSE_PATH_ROOT/config/config.yaml` (mode 0600, non-secret keys only:
  `GOOSE_PROVIDER`, `GOOSE_MODEL`, the provider's host keys and `GOOSE_TELEMETRY_ENABLED: false`, each
  a YAML string; never the placeholder key) when there is none. An existing file is the person's and
  is left alone; the environment still sets the provider and model, because goose reads the
  environment before the file. With no model for goose and no arguments, the wrapper says so ("no
  model is set up for this agent's terminal; check the agent's model on its Behaviour page", exit 1)
  instead of opening the wizard; arguments (`goose configure`, `--version`) still pass through.
  **OpenCode** gets `AURORA_OPENCODE_CONFIG` (the rendered
  `opencode.json` without the platform tools, passed as `OPENCODE_CONFIG_CONTENT`), with its XDG
  directories under `/workspace/.aurora/terminal/opencode` and the vendor switches of `§Adapters`
  set. **(FIX-191)** It also gets `AURORA_OPENCODE_PLUGIN_VERSION` (the pinned version), from which
  the wrapper writes a turn's `plugin_files` under `$XDG_CONFIG_HOME/opencode` when they are missing,
  so the start does not ask the npm registry for OpenCode's plugin package (a file already there is
  kept: OpenCode updates them for the person's own plugins). OpenCode runs as the wrapper's child, not
  `exec`ed, and after a non-zero exit the wrapper prints "opencode: exited with status N; see
  `<XDG_DATA_HOME>/opencode/log` for details", because its errors go to that log, behind its
  full-screen interface. A model the harness cannot use (an id or role that fails the supervisor's re-check, or a
  dialect it does not speak, for example goose on a Responses-only endpoint) becomes
  `AURORA_<HARNESS>_UNAVAILABLE` ("this agent's model endpoint cannot be used with …"), never a
  half-configured CLI. The wrappers name the placeholder only through the variable the CLI's config
  refers to. The terminal session does not read the agent's instructions or skills. **Attribution:** the
  traffic goes through the same machine proxy and boot-token credential as a turn's model calls, so
  the gateway attributes it to the **agent** (lane `mitm`, no turn or conversation), counts it
  against the same budgets and rate limits and shows it in the request log (`07`) like any other
  model call of that agent; the person at the terminal is not recorded as a speaker.
  **(FIX-169, D-371, C-1025) Approvals.** A wrapper starts its CLI with the approval and sandbox
  settings a turn uses (the Aurora sandbox and gateway are the boundary; the CLI's per-action prompts
  and its own sandbox protect nothing the terminal does not already allow). It is a default: whatever
  the person passes keeps winning. `claude` gets `--permission-mode bypassPermissions` in front of
  the person's arguments, unless they pass `--permission-mode` (either spelling),
  `--dangerously-skip-permissions` or `--allow-dangerously-skip-permissions`, or give `--` first; a
  root flag before a subcommand is accepted by `mcp`, `doctor`, `update`, `--version` and `--help`
  (checked on the 2.1.318 CLI next to the pinned 2.1.319), so subcommands need no special case, and
  the wrapper never writes `settings.json`. `codex` gets `-c approval_policy="never"` and
  `-c sandbox_mode="danger-full-access"` ahead of the person's arguments in both exec branches
  (a later `-c` of theirs wins); the wrapper leaves out the default a dedicated flag covers
  (`-a/--ask-for-approval`, `-s/--sandbox`, `--full-auto`, `--dangerously-bypass-approvals-and-sandbox`).
  `goose` gets `GOOSE_MODE=auto` unless `GOOSE_MODE` was in the environment the wrapper received
  (taken before `harness.env` is read, and the file has no such key) or a mode is saved in
  `$GOOSE_PATH_ROOT/config/config.yaml`. `opencode` (`"permission": {"*": "allow"}` in the terminal
  config) and `pi` (never asks) already behave this way.
- `pty.resize { cols, rows }` applies `TIOCSWINSZ`; bytes are relayed with the W18 credit
  machinery (`16 §W19`, tag `0x02`), never logged, never counted beyond byte totals, never
  visible to the harness, the gateway or the loopback proxies. A terminal session is **not**
  a process row (it is not background work), never a turn, never a transcript event and never
  stamps `last_engaged_at`; the keep-awake reason `terminal` comes from session byte activity
  alone (`11 §Web terminal`).
- Limits: at most `TERMINALS_PER_SANDBOX` (2) concurrently, more → `pty.refused { limit }`;
  idle and lifetime enforcement as the runner instructs (`pty.close` reasons `idle`,
  `lifetime`). Everything closes on `shutdown` (PTYs and preview streams, within the shutdown
  grace — a wedged engine cannot hold shutdown), channel loss, a stop, a restart or a freeze
  (the runner closes leg 1 with 4503/4403); nothing is resumed.

### SSH channels (phase 5, P4-SSH-2, D-239, SEC-95; wire in `16 §W20`)

An SSH session channel becomes one **exec** in the sandbox: the terminal's PTY posture plus the
non-interactive modes SSH clients need. Only the runner sends `exec.open`, and only for an
API-redeemed `auz_` grant of the current boot — or (phase 5, P5-FILES-1, D-316) an `auf_` files grant,
for which the runner only ever asks for mode `sftp` (`11 §Files`, `16 §W21`); agent tools can never open,
signal or read one. P5-FILES-1 builds this engine with the pipes path and the `sftp` mode; P4-SSH-2 adds
`shell`, `exec` and `tty`. The supervisor cannot tell a files session from an SSH `sftp` channel and
treats both identically.

- **Modes** (`exec.open { pty_id, window, mode, command?, tty?, env }`, the `pty_id` space and the
  `0x02` data tag shared with terminals):
  - `shell` — (FIX-218, D-459) the person's chosen shell (`exec.open.shell`: `bash` or `zsh`, absent =
    bash; the enumeration resolves through the same `ShellPaths::resolve` as the terminal, so the
    same fixed binaries and fallbacks) as an interactive **login** shell (`-l`, as OpenSSH's `sshd`
    runs it), with or without a `tty`: the same `PATH`, aliases, colours, prompt and history as the
    web terminal (`§Terminal`). With `tty` it is exactly `§Terminal`'s PTY (own session, controlling
    terminal, `O_NOCTTY` on the supervisor's side, `TIOCSWINSZ` from `tty.cols/rows`) and the
    fallback notice (zsh missing) is written to it; without `tty` (`ssh -T host`) it reads commands
    from stdin over pipes and writes no notice, so stdout stays clean. `SHELL` is the binary run.
  - `exec` — `<shell> -c <command>` with the same choice, **without** `-l` (OpenSSH's `$SHELL -c`:
    non-interactive, not a login shell, so no aliases); the command is one argv element, never
    re-quoted (`command` ≤ `SSH_EXEC_COMMAND_MAX` 8 KiB, no
    NUL; never logged, never stored, never audited), with or without `tty` (`ssh -t host cmd`);
    no fallback notice.
    Legacy `scp -O` arrives here as `scp -t|-f …` and works because the image carries `scp`.
  - `sftp` — the image's `/usr/libexec/openssh/sftp-server` (fixed path; never a path the client
    names) over pipes, no `tty`; `exec.refused { unsupported }` when the image lacks it (a custom
    image). Modern `scp` uses SFTP and lands here.
- **Identity and environment.** As `§Terminal`: agent uid/gid, cwd `/workspace`, the container
  environment **minus every `AURORA_*` variable** and minus the boot document. Added: `TERM` from
  `tty.term` (≤ 64 printable ASCII characters, else `xterm-256color`) and the client's `env`
  requests that the terminator accepted — only `LANG` and `LC_*`, at most `SSH_ENV_VARS_MAX` (16) of
  ≤ 256 bytes each; any other name (and every `AURORA_*` name) is dropped by the terminator and
  again by the supervisor. (P5-SKILLDIR, D-454) `SKILLS_DIR` is added after the sweep with the
  adapter's skills folder; it is not an accepted name, so a client cannot override it.
- **Pipes and status.** Without `tty`, stdout travels as `0x02` data and stderr as `0x03` data
  (supervisor → runner only); with `tty` both are the PTY's output (`0x02`). `exec.eof` from the
  runner closes the child's stdin (the client's `EOF`). When the child exits the supervisor drains
  its output, then sends `exec.exit { pty_id, status }` (or `{ signal }`, the signal's name without
  `SIG`, e.g. `KILL`), then `pty.close { reason: "eof" }`. Closing an exec early hangs up and then
  kills its process group exactly as a terminal (FIX-091 posture); the PTY master and pipe ends are
  `CLOEXEC` and never reach any other child (FIX-059 posture).
- **Refused at this layer** (defence in depth behind the terminator, `16 §W20`): any mode other than
  the three above, a `command` for `shell`/`sftp`, a `tty` for `sftp`, an `env` name outside the list
  — `exec.refused { reason: "unsupported" }`. There is **no** port forwarding, agent forwarding, X11
  or Unix-socket forwarding in the supervisor at all: it never dials or listens for an SSH channel.
- **Limits.** At most `EXECS_PER_SANDBOX` (12 = `SSH_CHANNELS_PER_SANDBOX` 8 + `FILE_SESSIONS_PER_SANDBOX`
  4; P5-FILES-1, D-316 — the per-kind caps are the runner's) concurrent execs, separate from
  `TERMINALS_PER_SANDBOX`, more → `exec.refused { limit }`; idle and lifetime as the runner
  instructs; everything closes on `shutdown`, channel loss, a stop, a restart or a freeze; nothing is
  resumed. An exec is not a process row, never a turn, never a transcript event and never stamps
  `last_engaged_at`; its bytes are never logged, counted beyond byte totals or visible to the
  harness, the gateway or the loopback proxies. An older supervisor ignores `exec.*` (the runner's
  `exec.open` times out after 5 s → leg 1 closes 4502): harmless.

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
