# P5-HARN: More harnesses: Pi, goose and OpenCode, treated the same as Claude Code (owner, 2026-11-17)

## Goal

Add three harnesses next to `claude-code` and `codex`: **Pi** (`pi`, https://github.com/earendil-works/pi),
**goose** (`goose`, https://github.com/aaif-goose/goose) and **OpenCode** (`opencode`,
https://github.com/anomalyco/opencode). Owner, 2026-11-17: "they should all get the same treatment as Claude,
but Claude remains the default" (D-361). Each one gets everything `claude-code` has today. That covers
selection per agent, its dialects, instructions and skills, platform tools and `aurora-agent`, the default
model with roles and per-model usage (when the harness has a slot for them), session resume and purge,
switching, provider-error classification, the conformance suites, the default sandbox image, the web
terminal command, the dashboard and the operator docs. `hosted.default_harness` stays `claude-code`.

## Linked requirements

- D-361 (owner), D-362 (design); open questions Q-HARN-1 and Q-HARN-2 (PROGRESS §Open questions). The
  recommended answers are the defaults below until the owner says otherwise.
- Contracts **C-1010..C-1016** (`docs/API_CONTRACTS.md` §Phase 5 — P5-HARN).
- **SEC-133**, which extends SEC-66 (`17`).
- Normative: `12 §Adapters pi, goose and opencode` (the per-adapter contract; read it first),
  `12 §Harness selection and adapter contract`, `12 §Adapter codex` (the closest model), `12 §Terminal`,
  `11 §Sandbox image`, `08 §Wire formats and harness reach`, `08 §Model roles`, `14` (Instructions harness
  picker, model roles group), `15` (`hosted.*`, `runner.image_harnesses`), `16 §W5` (`session.purge`).
- Precedents to copy, not redesign: P4-HAR (`docs/tasks/P4-HAR.md`, D-122, D-153), FIX-138 item 1b (D-247,
  C-750), P5-MODELS-1/2 (D-303, D-304, C-877, C-879, C-881), FIX-103 (the npm lockfile pin).

## Context

- Adapters: `services/aurora-supervisor/src/adapters/{claude_code,codex}/` (`mod.rs` with `PINNED_VERSION`,
  `config.rs`, `launcher.rs`, `stream.rs`, `tests.rs`, `fixtures/`). Shared refusal mapping:
  `adapters/provider_error.rs`. Terminal environment: `src/harness_env.rs`. Wrappers:
  `images/sandbox/bin/{claude,codex}`.
- Known ids: `crates/aurora-config/src/validate.rs` (`KNOWN_HARNESSES`); dialects and display names:
  `services/aurora-api/src/domains/hosted/model.rs`; terminal models: `services/aurora-api/src/domains/terminal/service.rs`.
- Image: `images/sandbox/Containerfile` (`SANDBOX_HARNESSES`, `org.aurora.harnesses`, the Claude Code npm
  pin under `images/sandbox/harness/claude-code/`, the Codex checksum pattern). The `os` stage already
  installs `nodejs`. Check its version: Pi needs Node.js 22.19 or newer.
- Web: `web/src/components/agents/instructions-tab.tsx` (picker, model roles group),
  `web/src/components/agents/model-readiness.tsx`, `web/src/components/admin/settings/llm-controls.tsx`
  (harness badges and the "no enabled harness" notice).
- Operator doc: `docs/operator/harnesses.md` (today "enabling Codex").
- What the research found (2026-11-17, from the projects' docs, **not yet checked against binaries**; Part A
  verifies every item and corrects `12`):
  - **Pi**: MIT, TypeScript on Node.js, npm `@earendil-works/pi-coding-agent` (v1.5.2). `pi --mode rpc`:
    JSONL commands on stdin and events on stdout. A `prompt` sent during a run with
    `streamingBehavior: "steer"` is delivered before the next LLM call, and `abort` is supported. Sessions
    take `--session-dir` and `--session-id`. The config directory is `PI_CODING_AGENT_DIR`, with
    `models.json` (`api`: `anthropic-messages`, `openai-responses`, `openai-completions`) and `auth.json`.
    MCP is native (`mcp.json`, stdio) and exposed through `codemode` by default. Pi reads `AGENTS.md` and
    `CLAUDE.md`, and skills in `.agents/skills`. It has no permission system and no subagents. Per-message
    usage has `input`, `output`, `cacheRead` and `cacheWrite`. Network switches: `PI_OFFLINE=1`,
    `PI_TELEMETRY=0`, `PI_SKIP_VERSION_CHECK`.
  - **goose**: Apache-2.0, Rust, release binaries (v1.59.2). `goose run -i - --output-format stream-json`
    with `--name` and `--resume`, and `goose acp` (ACP over stdio). The state root is `GOOSE_PATH_ROOT`, and
    sessions are in SQLite `sessions.db`. Providers are `anthropic` (`ANTHROPIC_HOST`), `openai`
    (`OPENAI_HOST`, `OPENAI_BASE_PATH`) and `litellm`. Bedrock and Vertex need cloud credentials. MCP
    servers are stdio extensions. `CONTEXT_FILE_NAMES` sets the instruction files, `GOOSE_MODE=auto` turns
    off approvals, and `GOOSE_SUBAGENT_PROVIDER`/`_MODEL` set the subagent model.
    `GOOSE_CONTEXT_LIMIT` is also available. Other switches: `GOOSE_TELEMETRY_ENABLED=false`,
    `GOOSE_DISABLE_KEYRING=1`, `GOOSE_DISABLE_SESSION_NAMING=true`. The stream-json event schema is
    undocumented.
  - **OpenCode**: MIT, release binaries. GitHub marks v1.19.x as Latest, and v2.3.0 is newer. Their config
    schemas differ, and the docs site now documents v2. Commands: `opencode run --format json --session <id>
    --standalone --auto`. Config is `opencode.json` with `{env:VAR}` substitution, and providers come in
    Anthropic, OpenAI Responses, OpenAI Chat and OpenAI-compatible packages. MCP servers are `type: local`
    (Code Mode by default), and instructions are `AGENTS.md` only. Skills live in `.opencode/skills`,
    `.agents/skills` and `.claude/skills`. Sessions are stored in SQLite (`OPENCODE_DB`), and
    `opencode session delete <id>` removes one. Also `update: "disable"` and per-model `limit.context`.
    Without `--standalone`, the CLI talks to a shared background service.

## Scope

Five parts in one lane, in order, with a batch gate after each part. Part A is the base for the
others. B, C and D are independent of each other but share the image and registry files, so they run one
after another. E comes last.

**A — Pin, verify, and generalize the registry.**
1. Pin one version of each CLI (Q-HARN-2: OpenCode on the line GitHub marks **Latest**). Run each pinned
   binary in the dev sandbox image against `aurora-testkit`'s mock upstreams (Messages, Responses and Chat).
   Record in `12 §Adapters pi, goose and opencode` the exact flags, config keys, event names and
   variables you find, as was done for Claude Code 2.1.319 in P5-MODELS-1. Fix the section where the docs
   were wrong. A dialect, role or switch that the pinned version does not have is removed from the section,
   not emulated. Where the section leaves a choice to the pinned version (goose `run` vs `acp`, a skills
   directory, a usage key), record the choice there and as a D-362 amendment line.
2. Generalize anything that is still written for two harnesses: `KNOWN_HARNESSES`, the dialect and
   display-name tables, `HarnessView` (C-1010), the registry, the conformance runner's adapter list, the
   `session.purge.harness` values (C-1014), and the per-harness `harness.env` sections (C-1015). No
   `if codex` branches outside an adapter.
3. Image (C-1012): add the three runtimes to the default image with pinned versions. Pi gets an npm lockfile
   under `images/sandbox/harness/pi/` and `npm ci --ignore-scripts`. goose and OpenCode are release archives
   with a SHA-256 per architecture. Extend `SANDBOX_HARNESSES` and the label default. Set a `--version`
   check equal to each adapter's `PINNED_VERSION`. Measure the image size before and after, and record it in
   the completion report.

**B — `pi` adapter.** **C — `goose` adapter.** **D — `opencode` adapter.** Each one implements
`12 §Adapters pi, goose and opencode` and C-1013:
- capabilities, rendered configuration (placeholders only, re-rendered and read back before every start),
  environment, state directory and purge, events → `HarnessEvent`, provider errors through
  `provider_error.rs`, the effort table, models (`model_roles`, `subagent_role`, `model_meta`) only where
  the section maps them, and per-model usage → `Done { usage.by_model }`;
- the conformance suite (`tests/conformance/harness`) in CI against the mock upstream of each declared
  dialect, plus env-gated live runs through a real gateway and LiteLLM;
- golden tests of the rendered files and environment, showing no secret and no vendor-network switch left on;
- the API dialect table and display name.

**E — Terminal, dashboard and docs.**
1. Terminal (C-1015): the `pi`, `goose` and `opencode` wrappers in `/opt/aurora/bin/`, built the same way as
   `claude`/`codex`. Each refuses with exit 127 when its harness is not enabled. Their state lives in
   `/workspace/.aurora/terminal/<id>`, they read their `harness.env` section, and the terminal open passes
   their models (C-750 (4), C-881).
2. Dashboard (C-1016): the harness picker lists the new harnesses with their display names. The
   "Not used by Codex" note becomes "Not used by <display name>", shown for whichever roles a harness does
   not map. The LLM endpoint badges and the "no enabled harness" notice are driven by the declared
   dialects, so an `openai_chat`-only endpoint now shows the harnesses that serve it. Use the shared
   primitives only. Check both themes and 390 px.
3. Docs: rewrite `docs/operator/harnesses.md` as "Harnesses" (one table: harness, dialects, reachable
   providers, roles, steer, image size). Update the `08 §Wire formats and harness reach` table, the
   `12` dialect table, and `15`. Add rows for each new adapter in `18 §Harness adapters`.

## Constraints / file ownership

- The lane owns `services/aurora-supervisor/src/adapters/**`, `src/harness_env.rs`,
  `images/sandbox/**`, `tests/conformance/harness/**`, the hosted dialect/display tables, the terminal
  models code, the Instructions picker, `docs/operator/harnesses.md`, and its section of
  `docs/API_CONTRACTS.md`. Do not run it beside a lane that edits the supervisor adapters, the sandbox
  image, or `services/aurora-api/src/domains/terminal/`.
- **No credential anywhere in the sandbox** (SEC-01, SEC-66, SEC-133). Configs refer to the placeholder
  through an environment variable, never a literal. Remove `auth.json` (Pi, OpenCode), `secrets.yaml`
  (goose) and any keyring before every start. All model traffic goes through the per-session proxy.
- **No vendor network.** Turn off telemetry, update checks, model catalogues (pi.dev, models.dev), share
  links, session naming calls and the OpenCode background service. The live suite proves that a turn
  touches only the endpoint host, and only the inference path set on it.
- Use the harness's own file and shell tools. Turn off built-ins that duplicate a platform feature with
  their own storage or a vendor service (goose `memory` and `computercontroller`; OpenCode's hosted web
  search, if the pinned version has one). Expose MCP tools directly, not through the harness's own code
  mode, so on-demand platform tools (D-297) behave as they do for `claude-code`.
- Platform-tool calls must come from descendants of the harness process the supervisor started (D-301).
  For OpenCode that means `--standalone` and never the shared service. The MCP round-trip conformance test
  proves this for each adapter.
- Add no dependency without a pin and a checksum. Keep `Cargo.lock` changes to what the adapters need.
- Disk: `df -h /home` before every image build, and stop below 50 GB. Change files only with the Edit/Write
  tools: never python, sed, awk, perl, `cat >>` or heredocs. Do not touch `scripts/perf/*`.

## Acceptance criteria

1. Part A: `12 §Adapters pi, goose and opencode` matches the pinned binaries, and the completion report
   lists every correction. The image builds with all five runtimes, and `SANDBOX_HARNESSES=claude-code`
   still builds a slim image. The label and `runner.image_harnesses` list the new ids.
2. For each new adapter, the conformance suite passes in CI against its mock upstreams. That covers the
   terminal-event guarantee, abort, resume, steer (refused between turns; for `pi`, accepted and joined
   during a run only if it passes, otherwise `steer = false`), the missed steer, the answer rule, the MCP
   round trip including the D-301 ancestry check, and correction handling. The completion report has an
   env-gated live run through the gateway and LiteLLM on every declared dialect.
3. Golden tests of the rendered config and environment show no secret, a placeholder only, and every
   vendor-network switch off. A credential file planted between turns is gone at the next start.
4. A per-model usage test covers each adapter: the four counters, the totals equal to the sum, and an
   unverified model folded into the default (D-304).
5. Switching an agent between any two of the five harnesses bumps the epoch, queues purges that the
   **old** adapter carries out (its files are gone), and drops incompatible model overrides with a notice.
   During a turn the switch gets 409.
6. The hosted e2e SEC-01 probe passes on a sandbox for each new harness. Ask the coordinator before running
   `hosted.sh`.
7. Each new command works in the web terminal: it uses the agent's model through the machine proxy, and is
   refused when its harness is not enabled.
8. Dashboard: the picker, the generalized roles note and the endpoint badges render in both themes with no
   horizontal scroll at 390 px. Lint, type check, unit tests and the production build pass.
9. fmt and clippy `-D warnings` pass, the supervisor, runner and API tests pass (full `aurora-api` with
   `--test-threads=8`), and the OpenAPI drift test passes. Add a SEC-133 row as `proven` in
   `docs/tests/security-coverage.md`.

## Dependencies

- These are merged: P4-HAR, P4-LLM-1..5, FIX-138, P5-MODELS-1 and P5-MODELS-2.
- Do not run Part E beside a web lane that edits `instructions-tab.tsx` or the admin LLM settings.

## Suggested agent/model and reason

Sonnet (`rust-developer`) for Parts A–D, and Sonnet (`nextjs-developer`) for Part E. Each adapter is
substantial and parses an external process's event stream. Part A's findings may change the spec, so the
coordinator reviews `12` after Part A, before Part B starts.
