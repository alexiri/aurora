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

- D-361 (owner), D-362 (design), D-363 (owner answers): the new harnesses are **not enabled by default**,
  and **OpenCode is pinned on v1.19.x**.
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
  - **OpenCode**: MIT, release binaries. **We pin v1.19.x** (D-363), which GitHub marks Latest. v2.3.0 is
    newer, but its config schema, permission format and server API differ, and **the research read mostly
    the v2 docs** that opencode.ai shows by default. Use `https://opencode.ai/v1/docs` and the v1 notes in
    `/docs/migrate-v1`. Part A must re-derive the v1 facts from those docs and the binary. What the v1
    material showed: `opencode run --format json`, `--session <id>`, `--continue`, `--model`. Config is
    `opencode.json` with `{env:VAR}` substitution, `provider.<id>` with `npm`, `options` and `models`
    (`limit.context`), `autoupdate: false`, a `permission` block keyed by tool, `mcp.<name>` with
    `type: "local"`, `command` and `enabled`, and `AGENTS.md`, with `CLAUDE.md` as a fallback. The v2-only
    items (`--standalone`, `--auto`, `update: "disable"`, `permissions` rules, Code Mode, the
    `/api/session` server) do **not** apply until Part A finds them in the pinned v1 binary.

## Scope

Five parts in one lane, in order, with a batch gate after each part. Part A is the base for the
others. B, C and D are independent of each other but share the image and registry files, so they run one
after another. E comes last.

**A — Pin, verify, and generalize the registry.**
1. Pin one version of each CLI (OpenCode: the latest **v1.19.x** release, D-363). Run each pinned
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
  For OpenCode that means its private in-process mode, never a shared or background server. The MCP round-trip conformance test
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

## Completion report

### Part A (2026-11-18, worker 33)

- **A.2 (commit 38b3e358)**: `KNOWN_HARNESSES` (6 ids, one shared message), `HarnessId`, the API dialect and display tables (+ a test that they equal `KNOWN_HARNESSES`), `TerminalModels` slots with `get`/`set`/`is_empty` (no `if codex` in the terminal service, `harness_env` apply paths, state directories); `schemas/` regenerated. Not yet generalized because it needs the adapters or the pins: the supervisor registry ids (added with each adapter), `harness_model_roles` (C-882) and `skills_dir` in `hosted/home.rs` (with each adapter), the `harness.env` render sections (with Part E's wrappers).
- **A.1 pins and checks** (binaries run against a throw-away mock Messages/Responses/Chat upstream plus a logging CONNECT proxy; Pi inside an AlmaLinux 10 container with its `nodejs` 22.23.4): Pi **1.5.2**, goose **1.59.2**, OpenCode **1.19.6**. SHA-256 in `12`. Corrections to the documented behaviour (all in `12 §Adapters pi, goose and opencode`; D-362(h)):
  1. OpenCode: `--auto` **does** exist in 1.19.6 (the brief listed it as v2-only); no `--standalone`. `opencode run` is in-process by default (no shared server unless `--attach`). Text arrives whole (no deltas). Events carry no model id: per-model usage comes from `opencode db "<SQL>"` over its `message` table (subagent sessions included).
  2. OpenCode: the vendor network is bigger than the brief listed: `models.opencode.ai` (off with `OPENCODE_DISABLE_MODELS_FETCH`), a background `npm install @opencode-ai/plugin` into every config directory (skipped only for a non-writable directory), and a title request to the small model per new session (`agent.title.disable`). `OPENCODE_DISABLE_PROJECT_CONFIG` also stops project `AGENTS.md` discovery, so the rendered config uses `instructions`.
  3. Pi: MCP tools are hidden behind `codemode` unless the server has `exposure: "direct"` (then `mcp__<server>__<tool>`); project trust is not a setting to turn off: `.pi/` and project `.agents/skills` simply need `--approve`, which is never passed; `$HOME/.agents/skills` loads without it. A `prompt` sent while idle **starts a run** even with `streamingBehavior: "steer"`, so the adapter must refuse between turns; during a run it is queued and joins before the next model call (`steer = true`). The terminal event is `agent_settled`; an error is the last assistant message's `stopReason: "error"`; 429s are retried 3× (about 14 s).
  4. goose: `stream-json` has tool events and usage, so `goose acp` is not needed. `input_tokens` includes cached tokens. A resumed session keeps its stored model unless `--model` is passed. It fetches `models.dev` once per start with no switch (refused by the proxy, tolerated). The stdio MCP server must answer `server/discover`. No thinking content, no per-model usage.
- **Not yet verified**: goose's `openai`/chat path and `GOOSE_SUBAGENT_*` against a live subagent (Part C), the OpenCode `--thinking` stream and stdin prompt (Part D), live runs through the gateway and LiteLLM.
- **A.3** (image, Containerfile, wrappers, size measurement): the Pi lockfile `images/sandbox/harness/pi/{package.json,package-lock.json}` is generated (`npm install --package-lock-only --ignore-scripts`, 123 packages, `npm ci --ignore-scripts` verified in a container). **Done (A.3)**: `images/sandbox/Containerfile` installs Pi (`npm ci --ignore-scripts`, Node ≥ 22.19 checked), goose and OpenCode (SHA-256 per architecture; the arm64 digests are GitHub's release asset digests, the x86_64 ones were also computed locally and are equal), each with a `--version` check equal to the pin; `SANDBOX_HARNESSES` defaults to all five and the label lists them. Built and run: full image 3.07 GB, before (`claude-code,codex`) 2.49 GB, slim (`claude-code`) 2.16 GB; the slim image has only claude-code in `/opt/aurora/harness` and the label `claude-code,fake`; all three new versions print the pins. The three test images were removed. Dependabot ignore for the Pi package and the pinned-on-purpose rows are in `docs/ops/dependencies.md`. `runner.image_harnesses` has no chart default (it is `[]` and the podman backend reads the label), so there is nothing to list there; the docs (Part E) say so. Wrappers `pi`/`goose`/`opencode` are Part E.
- **Deviations from the Edit-only rule**: none in the repository. In the scratchpad (throw-away mock and run scripts) I used `sed -i` once, a stray no-op `sed` on `/dev/null` (changed nothing), `cp`, and `node -e` to write scratch JSON configs. `npm install --package-lock-only` is the repo's documented lockfile generator.

### Part A batch gate (2026-11-18, worker 34; after `git merge master`)

`cargo check --workspace --all-targets --locked` ok; `nextest` `aurora-config` + `aurora-supervisor` 365/365; `aurora-protocol` 37/37 and `UPDATE_SCHEMAS=1 cargo test -p aurora-protocol` (no `schemas/` diff); `aurora-api` full 1659/1659 (after `scripts/fixtures/build.sh`, 67 ok; includes the OpenAPI drift test); `cargo clippy --workspace --all-targets --all-features -D warnings` exit 0; guards exit 0; `extract-schema.py --check` ok; `pnpm gen:api` no diff. Part A owes no SEC-133 row (the adapters are Parts B–D).

### Part B — `pi` adapter (2026-11-18, worker 34)

- **What**: `services/aurora-supervisor/src/adapters/pi/{mod,config,launcher,stream,tests}.rs` (one long-lived `pi --mode rpc` per session, `PINNED_VERSION = "1.5.2"`; config rendered and read back at every start and every turn; `auth.json` removed each time; state in `<home>/.aurora/harness/pi/sessions/<ref>/`; steer accepted only after Pi acknowledged the turn's prompt; abort settles or kills and restarts; per-model usage), the shared `adapters/render.rs` (layout, exclusive write with read-back, planted file/link/directory replaced; reused by Parts C and D), the registry (`KNOWN`, `adapter_with("pi")`), `aurora-testkit`'s new `harness_upstream.rs` (`HarnessUpstream`: mock Messages, Responses and Chat upstreams with scenarios and fixed usage), `tests/conformance.rs` (the common suite gained `steer_needs_ack`/`missed_steer` profile flags; Pi against **each** of its three dialects, per-model usage, MCP ancestry, vendor-network and credential checks, env-gated live run), `tests/fixtures/mcp-probe.js` (a stdio MCP server that records its process ancestry), the API's `skills_dir` (`.agents/skills` for `pi`/`goose`/`opencode`, unit test), `docs/12` (corrections below), `docs/18 §Harness adapters`.
- **Corrections to `docs/12` found by running the adapter against the real binary**: (1) `models.json` `apiKey` must be `"$ANTHROPIC_API_KEY"` (a `$NAME` reference); Part A's "the name of the variable" made Pi send the literal text as the key. The conformance credential assertion (`x-api-key` = `aurora-placeholder`) failed before the fix with `ANTHROPIC_API_KEY`, which is the fail-before check for that test. (2) Retry: `settings.json` `retry.maxRetries = 2` (as `codex`) makes a refusal arrive after about 6 s instead of 14 s; verified with `fail-429` (`auto_retry_start` 1/2, 2/2). (3) A steer sent before Pi has acknowledged the turn's prompt starts a run of its own (Pi takes a few hundred ms to start), so the adapter refuses until the acknowledgement; a steer that still races the run's end is aborted and waited for. (4) The session ref is the conversation id (or the resume ref), so a purge by ref removes one directory. (5) The first prompt waits up to 10 s for `direct` MCP servers (Pi's docs), so no readiness wait is needed.
- **Verification**: `cargo clippy -p aurora-supervisor -p aurora-testkit -p aurora-api --all-targets --all-features -D warnings` exit 0; `nextest -p aurora-supervisor -p aurora-testkit` 306/306 with `AURORA_PI_BIN` = a wrapper that runs the pinned Pi from a Node 22 container (`localhost/p5harn-b-pi:dev`, built from the lockfile; the host has Node 20); `nextest -p aurora-api --lib` 182/182; guards exit 0; `extract-schema --check` ok. Conformance against the real Pi: say, steer between turns, resume, abort, provider-error (`fail-quota` → `provider_quota`), tool, steer while running (join), correction, on Messages, Responses and Chat; every request carried `aurora-placeholder` on the dialect's inference path only and the refusing proxy saw no request; per-model usage (input/output/cache read/cache write, totals = sum) on all three dialects; an MCP call came from a descendant of the Pi process. The API's full it-suite was not rerun for this part (the only API change is the `skills_dir` match, covered by its unit test and the lib suite); it runs at the final batch gate.
- **Not run**: the env-gated live suite through a real gateway and LiteLLM (no endpoint here), `hosted.sh` (asked before running it, per the brief).

### Part C — `goose` adapter (2026-11-18, worker 34)

- **What**: `services/aurora-supervisor/src/adapters/goose/{mod,config,launcher,stream,tests}.rs` (one `goose run -i - --output-format stream-json` per turn, `PINNED_VERSION = "1.59.2"`; root = the conversation's directory with the rendered `config/config.yaml`, the seeded model catalogue and the session database; files rendered and read back before every turn, planted `secrets.yaml`, config, catalogue and `data` links replaced; `--resume` only once the session exists), `render.rs` (nested paths, each directory on the way checked), registry (`KNOWN`, `adapter_with("goose")`), `harness_model_roles("goose") = [large, medium, small]` (`aurora-protocol`), the testkit mock (`HarnessUpstream::start_with_tools`, a `delegate` scenario, a recording fallback for any other path), conformance (`tests/conformance.rs`: goose against Messages and Chat, resume with a model change, usage, MCP ancestry, subagent role model, live run), `docs/12` and `docs/18`.
- **Corrections to `docs/12` found with the binary** (all folded in): (1) **`--no-profile` must not be used**: it ignores `config.yaml`'s extensions, so the rendered `developer` and `aurora` never loaded (Part A's drive line had it). (2) **Providers**: `anthropic` for `anthropic_messages`, **`litellm` for `openai_chat`**; the `openai` provider routes `o*`, `gpt-5*` and `*codex*` models to `/v1/responses` whatever `OPENAI_BASE_PATH` says, so a dialect cannot promise a path through it, and **`openai_responses` is not declared** (the API's dialect table already lacks it). (3) **`--resume` of a name that does not exist is an error**; handled. (4) `apps` and `extensionmanager` are on by default and are switched off with `type: platform, enabled: false`; `memory` and `computercontroller` are not in 1.59.2's default set. (5) Effort: `GOOSE_THINKING_EFFORT` low/medium/high/max works (budgets 4000/10000/16000/32000). (6) goose retries 429 and 5xx three times: a refusal arrives after about 7 s, no setting; every 429 is called "Rate limit exceeded", so quota/billing text is read first. (7) State: the root holds the rendered config too (one root), so it is inside the session directory and purged with it.
- **The models.dev follow-up (your review)**: **there is a switch-free way**. goose reads `<root>/data/model_catalog/models_dev_api.json` and `models_dev_api.etag` and, when both exist and are fresh, makes **no connection** to models.dev. The adapter renders both before every turn (only the session's models, with their context window). The etag file must end with a newline, or goose fetches anyway (found by the "proxy saw no request" assertion, which is the fail-before check). What a person sees: **nothing** with the seed. Without it: one refused `CONNECT models.dev:443` per start, recorded by the gateway as a denied egress request in the workspace's request log (a row per turn), about 0.2 s of start delay here (a proxy that refuses at once; the gateway round trip is the real figure). **Nothing is suppressed in the proxy and no decision is needed for this one.**
- **A decision for you (new, found here)**: goose's `litellm` provider (`openai_chat`) sends **`GET /model/info`** (context-window discovery) to the endpoint host at every start. It is not an inference path, so the gateway refuses it `403 llm_path_not_permitted` before injecting any credential, goose falls back to the rendered `GOOSE_CONTEXT_LIMIT` and carries on, but it shows as **one refused request per turn** on `openai_chat` endpoints. No switch stops it (`GOOSE_CONTEXT_LIMIT` does not, for this provider). An operator can add `/model/info` to the endpoint's `allowed_paths`; I did not special-case anything. The alternative (the `openai` provider with `GOOSE_CONTEXT_LIMIT` makes no extra request) misroutes `gpt-5*`/`o*`/codex models to the Responses path, which is worse. Tell me if you want a default (e.g. the gateway treating `GET /model/info` as permitted-without-credential for LiteLLM endpoints) and I will not build it without your word.
- **Verification**: conformance against the real goose 1.59.2 (host binary): say, steer between turns, resume, abort, provider error (`fail-quota` → `provider_quota`, after goose's 7 s of retries), tool, correction on **Messages and Chat**; credential = `aurora-placeholder` on the inference path only (plus the `GET /model/info` above); the proxy saw no request on any dialect; resume in a new process with another model sends the new model with the history; per-turn usage with the input net of the cache; MCP call from a descendant of the goose process; the delegated subagent's request used the role's model. **Gate**: `cargo clippy -p aurora-supervisor -p aurora-testkit -p aurora-protocol -p aurora-api --all-targets --all-features -D warnings` exit 0; `nextest -p aurora-supervisor -p aurora-testkit -p aurora-protocol` 371 tests, 370 passed and one failed, `aurora-testkit` `a_leaked_database_older_than_the_ttl_is_reaped…` (a database-reaping test that passed in Part B's gate and **passed alone, 13 s**, when rerun; it failed under load while the API suite and other lanes ran: flake, unrelated to the change); `UPDATE_SCHEMAS=1 cargo test -p aurora-protocol` no `schemas/` diff; `nextest -p aurora-api --lib` all pass and the API it-tests matching `harness|roles` 50/50; guards exit 0; `extract-schema --check` ok. The full API it-suite runs at the final gate.
- **Not run**: the env-gated live suite (no endpoint here), `hosted.sh` (asked first, per the brief).

### Part D — `opencode` adapter (2026-11-18, worker 35)

- **What**: `services/aurora-supervisor/src/adapters/opencode/{mod,config,launcher,stream,tests}.rs` (one `opencode run --format json -m aurora/<model> [--session ses_…] [--variant <effort>]` child per turn, in-process (no `--attach`), `PINNED_VERSION = "1.19.6"`; **the prompt on stdin**; the whole root (`config/ data/ cache/ state/` as the four XDG directories) in the conversation's directory; `config/` wiped and rendered with read-back before every start, `auth.json`/`mcp-auth.json` removed; session ref = conversation id, OpenCode's `ses_…` id kept in the live session and re-read with `opencode db` after a restart; usage per model from `opencode db` (recursive over subagent sessions, since the child started), fallback one default-model entry), registry (`KNOWN`, `adapter_with("opencode")`), `harness_model_roles("opencode") = [large, medium, small]`, the testkit mock (`fail-402`; `start_with_all_tools` with the harness's subagent tool name), `tests/conformance.rs`, `docs/12`, `docs/18`.
- **Findings against the real 1.19.6 (all in `docs/12`)**: (1) **Prompt size rule met by stdin**: with no positional message `opencode run` reads all of stdin; a 400 KB prompt (3× MAX_ARG_STRLEN) arrives whole on Messages and Chat (test), so nothing is truncated or refused for size. (2) **npm install**: both options verified; a read-only directory is *not* used because the supervisor, OpenCode and the agent are all uid 1000 (the agent can `chmod` it back and a mode bit cannot be read back); instead `config/opencode/{package.json,package-lock.json,node_modules/@opencode-ai/plugin/package.json}` are rendered each start (fail-before: without them six `CONNECT registry.npmjs.org:443` per turn reach the proxy; with them none). The uid model and this choice are written into `12`. (3) **Retries**: no setting in 1.19.6 (no config key or variable); a 429 is reported after the sixth request, about 72 s (measured); so the delay is in the mapping: a retryable error's message ends "(OpenCode retried the request for about 70 seconds before giving up)". The suite's refusal case uses a new mock `fail-402` (OpenCode reports it at once → `provider_quota`); the 429 text is a unit test over a recorded event. (4) Session ref: the directory must be addressable by the reported ref, so the ref is the conversation id and the `ses_…` id is mapped by the adapter (`ses_` ids cannot be chosen). (5) The effort table is verified end to end (`--variant high` → `thinking.budget_tokens` 16000 on Messages, `reasoning_effort: "high"` on Chat); `--thinking` is not used (text arrives whole). (6) No title/small-model request (`agent.title.disable`): a tool turn makes exactly two upstream requests.
- **Verification**: conformance against the real OpenCode 1.19.6 (host binary from the downloaded release, SHA-256 as in `12`): the common suite (say, steer between turns, resume, abort, provider error `fail-402` → `provider_quota`, tool, correction) on **Messages, Responses and Chat**; credential = `aurora-placeholder` on the inference path only, the refusing proxy saw **no request** on any dialect; resume in a new process with another model and the earlier turns in context, purge by ref; four counters per model on all three dialects, totals = sum, and a subagent's tokens as a second entry under the role's model; MCP call from a descendant of the opencode process; 400 KB prompt whole; effort in the request. Not run: the env-gated live suite (no endpoint here). **Gate**: `cargo clippy -p aurora-supervisor -p aurora-testkit -p aurora-protocol -p aurora-api --all-targets --all-features -D warnings` exit 0; `cargo nextest run -p aurora-supervisor -p aurora-testkit -p aurora-protocol -p aurora-config` 519/519 (with `AURORA_OPENCODE_BIN` and `AURORA_GOOSE_BIN` set, so goose's conformance ran too; Pi's needs the container and ran in Part B); `UPDATE_SCHEMAS=1 cargo test -p aurora-protocol` no `schemas/` diff; `nextest -p aurora-api --lib` 182/182 and the API it-tests matching `harness|roles` 50/50; guards exit 0; `extract-schema.py --check` ok. The full API it-suite and the rest run at the final gate.
- **Not done / left**: the SEC-133 row stays `pending` until Part E's hosted SEC-01 probe per harness; `hosted.sh` not run (asked first).

## Suggested agent/model and reason

Sonnet (`rust-developer`) for Parts A–D, and Sonnet (`nextjs-developer`) for Part E. Each adapter is
substantial and parses an external process's event stream. Part A's findings may change the spec, so the
coordinator reviews `12` after Part A, before Part B starts.
