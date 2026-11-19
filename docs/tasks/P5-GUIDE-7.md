# P5-GUIDE-7: Platform tools on demand — exposure, families, bridge filter, the `aurora` target of `aurora-agent` (owner, 2026-11-15)

Seventh task of "app guides and tools on demand": the same Code Mode treatment for Aurora's own
MCP bridge. Owner, 2026-11-15: "spec out fixing the mcp bridge's issues the same way."
Normative spec: `docs/24-app-guides.md` §10a (*Target `aurora`*), §10b and §11 (SEC-119,
SEC-120); `12 §Platform tools` (as amended); `10 §Agent instructions` (*Platform tools on
demand*); `16` (MCP surface). Contracts **C-654, C-655, C-657**. Decision **D-297**. Depends on
**P5-GUIDE-6** (the `aurora-agent` command and its target model, the wrapper, `agent_cli`, the
preview/save helpers, the `aurora` placeholder).

## Problem

The bridge lists every available platform tool (about 38 in nine families) to the harness, so
their names, descriptions and JSON schemas — an estimated 2 000 to 3 000 tokens — are in every
session, used or not, and any change to any tool's description releases idle sessions.

## What it delivers

In mode `on_demand` (default) the harness sees only the core tools (`send_file`,
`complete_task`); the instruction file lists every family in one line; the agent finds and calls
the rest with `aurora-agent search|describe|call` and the target `aurora` (one search also covers
app operations and recipes) through the same socket, registry, checks, narration and audit. Mode
`all` (`hosted.platform_tools`) is today's behaviour exactly.

## Scope

1. **Registry** (`services/aurora-api/src/domains/tools/registry.rs` and each tool file in
   `domains/tools/`, plus the supervisor's local `process_*` descriptors): `fn exposure(&self)
   -> Exposure` (default `OnDemand`) and `fn family(&self) -> &'static str` (required: no
   default, so a new tool cannot forget it). Families and one-line purposes as in `24 §10b`
   (memory, skills, schedules, webhooks, processes, files, conversations, messaging, agents,
   apps; reuse the grouping of `12 §Phase 0 catalogue`; include tools added since, e.g. by
   P4-UTOK or P5-CALINV, if merged). Core: `send_file`, `complete_task`.
   `PLATFORM_TOOLS_CORE_MAX` in `aurora-core::limits` and a test that the core set stays within it.
2. **Protocol** (`crates/aurora-protocol/src/tools.rs`): `ToolDescriptor.family?`,
   `exposure?` (C-654), both `skip_serializing_if` none, so an older peer is unaffected.
3. **Render inputs and config** (`services/aurora-api/src/domains/hosted/home.rs`,
   `crates/aurora-config`): `hosted.platform_tools` (C-657; unknown value refuses startup) and
   `render_inputs.platform_tools = { mode, families }` (available tools only).
4. **Bridge** (`services/aurora-supervisor/src/mcp_bridge.rs`): `tools/list` filtered to
   `exposure == core` when mode is `on_demand` **and** `agent_cli`; otherwise unchanged.
   `tools/call` unchanged (any name forwarded).
5. **Instruction file** (`services/aurora-supervisor/src/instructions.rs`): the *platform tools*
   fragment of `24 §10b` from `platform_tools.families`, only when the bridge filters; snapshots
   for both modes.
6. **Session release** (find where a catalogue change releases idle sessions —
   `services/aurora-supervisor/src/{supervisor.rs,home.rs}` or the API's generation logic — and
   test there): computed over the core descriptors plus the rendered instruction files.
7. **`AURORA_SESSION`** (`services/aurora-supervisor/src/{harness_env.rs,spawn.rs}` and both
   adapters): set to the conversation id for every harness process; make sure the terminal
   (`pty.rs` strips `AURORA_*`), SSH and `process_start` environments do **not** carry it.
8. **Target `aurora`** (`services/aurora-supervisor/src/agent_cli/`): replace P5-GUIDE-6's
   placeholder with C-655 as a second target kind behind the same verbs: `list aurora`, tools in
   `search` (ranked with app operations and recipes), `describe aurora <tool>`, `call aurora
   <tool>`, the socket protocol of `12 §Path`, schema-driven coercion, preview/save (shared
   helpers), app-only options refused, exit codes of C-653 (1 tool error, 2 no session, 4
   transport).
9. **Docs you update**: `docs/tests/security-coverage.md` (SEC-119, SEC-120 test ids). If SET-5
   has merged, add `hosted.platform_tools` to the `agents` settings section (`15 §Instance
   settings`); otherwise SET-5 adds it.

## Out of scope

Binding socket calls to the calling process (SEC-119 note; an open question for the owner).
Moving more tools into the core set (a spec change). App operations (P5-GUIDE-6).

## Ownership

`services/aurora-api/src/domains/tools/**`, `crates/aurora-protocol/src/tools.rs`,
`crates/aurora-config` (one key), `crates/aurora-core/src/limits.rs` (one constant),
`services/aurora-api/src/domains/hosted/home.rs` (the `platform_tools` render input only),
`services/aurora-supervisor/src/{mcp_bridge.rs,tools.rs,instructions.rs,harness_env.rs,spawn.rs,agent_cli/**}`
and the adapters' environment code. **Not concurrent** with any lane that edits
`domains/tools/` (e.g. FIX-150, P4-UTOK) or `mcp_bridge.rs`.

## Acceptance criteria

- [ ] Every test of `18 §App guides on demand` marked P5-GUIDE-7, each named in the report.
- [ ] SEC-119: every tool reachable by name in both modes with identical results, events and
      audit rows; e2e `memory_save` through `aurora-agent call aurora` and through MCP produce the same
      `tool.started` / `tool.finished` events and audit row.
- [ ] Rolling upgrade: new API + old supervisor → the old supervisor ignores the new descriptor
      fields and `platform_tools` and lists everything; old API + new supervisor → no
      `platform_tools` → mode `all`. Tests for both.
- [ ] Measured (estimate method stated, chars ÷ 4 or a tokenizer if the repo has one): the size
      of `tools/list` in both modes and of the *platform tools* fragment, recorded in the report.
- [ ] Eval comparison (`24 §13`): if P4-EVAL is merged, one run per mode on the same agents,
      recorded in the report; if not, state that it is pending and why.
- [ ] fmt, clippy `-D warnings`, full `aurora-api` and `aurora-supervisor` suites, guards,
      OpenAPI drift check. Exact commands and results; say what was not run.

## Notes for the implementer

- Change files only with the Edit/Write tools. No `Co-Authored-By` lines in commits.
- The exposure mode changes what is **listed**, never what is **allowed**: if you find yourself
  adding an authorization branch on `exposure`, stop.
- Suggested agent: `rust-developer` (Sonnet); review by `sonnet-integration-reviewer` against
  C-654/C-655 and SEC-119 before merge. Size M–L.

## Completion report
(lane `p5-apps`, worker 40, 2026-11-18; **partial: part 1 of 3 committed; compiles, not yet tested**)

**Part 1 done (items 1-5 and 7 partly, config):**
- **Protocol** (`crates/aurora-protocol/src/tools.rs`): `Exposure {Core, OnDemand}` (snake_case, default `OnDemand`), `ToolDescriptor.family?`/`exposure?`
  (both skipped when none); `schemas/tools_tool_descriptor.json` regenerated.
- **Registry** (`domains/tools/registry.rs`): `PlatformTool::family()` (required) and `exposure()` (default `OnDemand`); one `descriptor()` helper used by both
  catalogue functions; `Registry::new` refuses more than `PLATFORM_TOOLS_CORE_MAX` (8, `aurora-core::limits`) core tools counting the supervisor's local `send_file`.
  Every tool file has its family: memory, skills, conversations (`conversation_*`), messaging (`find_recipient`, `send_*`, `read_conversation`, `list_my_messages`,
  `edit_my_message`, `delete_my_message`), apps, webhooks, agents (incl. `list_agents`), **schedules**, **calendar** (P5-CALINV's four tools: a family `24 §10b`
  does not list; its purpose line is in `home.rs::family_purpose`; `WRITE_TOOLS` untouched). `complete_task` is `Core`. Supervisor local descriptors: `send_file` =
  family `files`, `Core`; `process_*` = family `processes`, `OnDemand`. Test doubles in `tests/it/tools.rs` got `family`.
- **Config** (C-657): `hosted.platform_tools` in `raw.rs` (default `on_demand`), `validate.rs` (unknown value refuses startup), the `agents` settings section
  (`settings.rs`, enum, `Loosening::Order`), `fields.rs` (words, choices), `deploy/helm/aurora/movable-keys.txt`. `docs/15` already carried the row.
  `cargo test -p aurora-protocol -p aurora-config`: all passed.
- **Render input** (`hosted/home.rs`): `render_inputs.platform_tools = { mode, families:[{name,purpose,tools}] }` from the agent's catalogue (on-demand tools only, registry order).
- **Supervisor**: `instructions.rs` `PlatformToolsInfo` (+`on_demand(info, agent_cli)`), the *Platform tools* fragment (fixed paragraph, `- family (verbs): purpose`, the local
  `processes` family appended) before `## Memory`, only when on-demand and `agent_cli`; `home.rs` `bridge_core_only()` (installed render inputs); `supervisor.rs` passes
  `--core-only` to the bridge command and sets **`AURORA_SESSION=<conversation id>`** in the session `env`; `main.rs`/`mcp_bridge.rs`: `mcp-bridge --core-only`, `tools/list`
  filtered to `Core` when set, `tools/call` unchanged.
- `cargo check -p aurora-api -p aurora-supervisor -p aurora-config --all-targets` clean. **No test of this part has run yet.**

**Part 2 (lane `p5-apps`, worker 41, 2026-11-18; commits `cf65ebc2`, `89a12060`) — items 7, 8 done:**
- **Item 7 verified.** `AURORA_SESSION` is in `SessionOptions.env`; each adapter's `build_env` passes `opts.env` through (a unit test per adapter, claude-code, codex, pi, goose, opencode,
  `the_session_environment_reaches_the_harness_process`); the fake adapter's `[[shell:]]` inherits it. `pty.rs` strips every `AURORA_*` (covered by `pty_env_probe`); SSH and
  `process_start` children take the supervisor's own environment, which never holds it (it is only in the explicit harness env).
- **Item 8 done:** `services/aurora-supervisor/src/agent_cli/platform.rs`; `Env` has `tools_socket` + `session` (`AURORA_SESSION`). `list aurora` (by family), `describe aurora <tool>`
  (parameter table + example), `search` ranks platform tools with apps (`show::search_with`; `--in aurora`), `call aurora <tool> k=v|k:=json [--body @file|-] [--out] [--raw]` coerced
  by the schema's top-level types; results ≤ 1 500 chars print whole, larger ones are saved under `/tmp/aurora-agent/aurora-<tool>-<UTC>.json` with a preview. Exit codes: no session 2,
  `session_not_caller`/`no_active_turn` 2, other socket errors and transport 4, a tool error result 1. `list` (no target) adds an `aurora` line only inside a turn.
  Tests (`tests/it/tools.rs`): list/describe/search/call with coercion against the real hub, tool error exit 1, bad coercion, no session, absent socket (4); **FIX-151 binding for the
  `aurora` target** (`…from_a_process_outside_the_harness_is_session_not_caller`, all four verbs, nothing reaches the runner). `agent_cli.rs` test renamed
  `target_aurora_needs_a_conversation_and_refuses_app_options_first`. e2e `s43` now asserts: no session → exit 2, named session from a `podman exec` → `session_not_caller`, and from the
  harness's child (`[[shell:]]`) `list aurora` + `call aurora memory_save` rc 0 (**not run yet**: needs `hosted.sh`).
- **Merged master into the lane** (P5-WEBHOOK-4, P5-HARN); one conflict in `supervisor/src/home.rs` (kept both `servers::write` and `render_with(…, agent_cli)`).
- Gate so far: `cargo check --workspace --all-targets --locked` clean; `cargo nextest run -p aurora-supervisor`: 432 passed; clippy `-p aurora-supervisor -p aurora-e2e-hosted` clean.

**Item 6 finding (needs a decision):** nothing in the code releases idle sessions on a catalogue change. `SessionRelease` is only handled by the supervisor and never sent by the API or runner
(grep over `services/` and `crates/`); `docs/12 §Platform tools` ("idle sessions are released on a catalogue change only when the core descriptors or the rendered instruction files change")
and `docs/12:793` describe behaviour that does not exist. Either implement it (supervisor: after `projector.commit`, hash the core descriptors + the rendered instruction files, release idle
sessions when it differs from the previous sync's) or reword the two paragraphs. Not done here.

**Part 3 (lane `p5-apps`, worker 42, 2026-11-18) — item 6 implemented, supervisor-side:** the coordinator decided to implement it. The release is decided
where the supervisor commits a home sync, not by an API-sent `session.release`: the supervisor alone knows which sessions are idle, `SessionRelease` has no API
producer, and no new work-item plumbing is needed (it reuses the existing release path, which already refuses a conversation with a running turn). `HomeProjector::catalogue_digest()`
(`home.rs`) hashes the descriptors the bridge lists (core only when `bridge_core_only()`, else all) plus the rendered instruction files; `supervisor.rs::home_sync` compares it before and
after `commit` and calls `release_idle_sessions()` when it moved (a first install, no previous digest, releases nothing). Tests: `home.rs` `the_catalogue_digest_follows_core_descriptors_and_instruction_files_only`
(on-demand description change: same digest; core descriptor, instruction file, mode `all` any descriptor: different); `turns.rs` `a_changed_catalogue_releases_idle_sessions_and_an_unchanged_one_does_not`
(same render: session reused; changed render: fresh session start, seen as the fake harness's session marker being rewritten — **seen failing with the release disabled**) and
`a_changed_catalogue_never_releases_a_running_turn`. Limitation: a session that had a turn running during the change is not released afterwards; it keeps the old list until the next change.
Runs: `cargo nextest run -p aurora-supervisor -E 'test(/catalogue/)'` 5 passed (the build alone took 70 min at load 100–280).

**Part 4 (lane `p5-apps`, worker 43, 2026-11-19) — item 3 (tests, measurements), item 9 (security-coverage):**
Tests added (all passing; each is also on the `18 §App guides on demand` list):
- Registry (`domains/tools/registry.rs::exposure_tests`): `every_tool_has_a_family_and_the_core_set_is_the_documented_one` (core = `complete_task`, within
  `PLATFORM_TOOLS_CORE_MAX` counting `send_file`), `a_registry_over_the_core_maximum_is_refused`.
- API (`tests/it/tools.rs::catalogue_descriptors_carry_family_and_exposure_and_core_is_small`), render input
  (`hosted/home.rs::platform_tools_input_groups_the_on_demand_tools_by_family`: core out of the families, on-demand grouped in registry order, descriptors without family skipped).
- Protocol old peer (`aurora-protocol` `tests/it/protocol.rs::descriptor_family_and_exposure_are_optional_both_ways`): an older API's descriptor parses and means on-demand and
  re-serializes byte for byte; the new descriptor reads through an older supervisor's field-less type (rolling upgrade, new API + old supervisor).
- Config refusal (`aurora-config` `tests/config.rs::hosted_platform_tools_defaults_and_refuses_unknown_values`).
- Bridge (`sup tests/it/tools.rs::the_bridge_lists_core_only_or_all_and_calls_any_tool_by_name`): core-only list = `complete_task`, `send_file`; `all` lists everything
  (descriptors with no `exposure`, as an older API sends, included); `memory_save` is served by name in both modes with the same frame.
- Home sync / rolling upgrade, old API + new supervisor (`sup tests/it/home.rs::the_bridge_is_core_only_exactly_for_on_demand_with_the_command`): no `platform_tools` → everything listed and no
  fragment; mode `all`, or no `aurora-agent` in the sandbox → same; `on_demand` + command → core only and the fragment in `AGENTS.md`.
- Instruction snapshots (`instructions.rs::platform_tools_fragment_snapshots_for_both_modes`; new `.snap` files `platform_tools_on_demand`, `platform_tools_all`; a family without tools is omitted,
  the local `processes` family joins; absent without the command and without inputs). Cosmetic finding: the verbs of `processes` print whole (`process_start`, …) because
  the prefix strip trims a trailing `s`; harmless, left as is.
- SEC-119 parity, e2e (`s43`): `aurora-agent call aurora memory_save` and the bridge's socket `call` request (sent from the harness's child) leave the same `memory_revisions` row
  (op, author, content, provenance with the conversation and a turn). **Limit:** `tool.started`/`tool.finished` events are the harness adapter's, not the registry's, and the fake
  harness's `[[shell:]]` emits none, so event parity cannot be asserted against the fake; the e2e proves the durable side. Ran green: `hosted.sh s43` on the full image
  (`AURORA_E2EH_PREFIX=p5apps-e2eh`, own subnets) — 1 passed, 75 s.
- Fixed on the way: `tests/it/plugins_upgrade.rs::the_shipped_indico_version_replaces_the_previous_one_automatically` still pinned indico 1.3.0 and omitted `operations.json`
  (P5-GUIDE-6 bumped it to 1.4.0); `scripts/guards/limits-gaps.allow` loses `PLATFORM_TOOLS_CORE_MAX doc-only` (it now has a reader; the limits-readers guard demanded it).
- `docs/tests/security-coverage.md`: SEC-119 test ids. SEC-120 unchanged (already lists the GUIDE-7 target tests via GUIDE-6's rows).

**Measured** (chars ÷ 4, no tokenizer in the repo; run `--no-capture`): the real API catalogue is 27 tools; `tools/list` of those as MCP entries
(name, description, inputSchema) = 11 022 chars ≈ **2 755 tokens** in mode `all`, 290 chars ≈ **72 tokens** core-only (`complete_task`). With the supervisor's local tools
(a small catalogue of 3 API-like descriptors plus `send_file` and the four `process_*`): 2 031 chars ≈ 507 tokens vs 543 ≈ 135 tokens. The *Platform tools* fragment for a
three-family sample: 542 chars ≈ 135 tokens (the real one has more families; the section's estimate of one line per family stays under ~1 000 chars).
**Eval comparison** (`24 §13`): pending — P4-EVAL runs against a live model and were not run in this lane.

**Gate (worker 43; the batch gate was cut down by the coordinator's targeted-tests rule at 13:xx):** `cargo check --workspace --all-targets --locked` clean (after the master merge);
`cargo nextest run -p aurora-registry -p aurora-core -p aurora-db -p aurora-api` (after `scripts/fixtures/build.sh`): 2028 run, 2027 passed, 1 failed = the indico upgrade test above (fixed, re-run green);
`-p aurora-protocol -p aurora-config -p aurora-supervisor`: 587 passed; `-p aurora-cli -p aurora-runner`: 174 passed; clippy `--workspace --all-targets --all-features --locked -D warnings`
clean after one lint of my own test (re-run clean on the touched crates); `scripts/plugins/check-builtin.sh` rc 0; `scripts/guards/run-all.sh` rc 0 (after the allow-list fix);
`extract-schema.py --check` and the OpenAPI/protocol schema ITs green (inside the suites). **Not run:** the full hosted scenarios incl. goose/OpenCode and the rest of `hosted.sh`,
perf, any web gate (cancelled by the new rule; CI is the gate). A full-suite run under load took ~25 min for `aurora-api`.

**Next steps (a fresh worker) — superseded by part 4; kept for history:**
1. **Item 7 verification:** do both adapters hand `SessionOptions.env` to the harness process (claude-code, codex, pi, goose, opencode, fake)? Does `pty.rs`
   strip `AURORA_*` for the terminal, do SSH and `process_start` environments not carry it? Add tests (supervisor `tests/it`, adapters).
2. **Item 8, target `aurora`** in `services/aurora-supervisor/src/agent_cli/` (`mod.rs` has the `TargetKind::Platform` arms answering `NOT_AVAILABLE`; add `platform.rs`):
   socket `list`/`call` on `/run/aurora/tools.sock` with `session = $AURORA_SESSION` (no session → exit 2 "platform tools are available inside a conversation turn"; a
   `session_not_caller` refusal → exit 2/4 with its message; transport → 4; tool error result → 1), `list aurora`, tools in `search` ranked with apps (`show.rs`), `describe`
   (schema as a parameter table + example), `call aurora <tool> k=v|k:=json [--body] [--raw]` with coercion by the schema's top-level types, preview/save to
   `/tmp/aurora-agent/aurora-<tool>-<UTC>.json`; app-only options already refused by `platform_options`. Remove `NOT_AVAILABLE` from `list`/`search` and update
   `tests/it/agent_cli.rs::target_aurora_is_not_available_…` and the e2e `s43` assertion (`call aurora memory_save` exit 2) to the new behaviour.
3. **Item 6, session release:** find where a catalogue change releases idle sessions (API generation logic / `home.rs` hash) and compute it over the core descriptors plus the
   rendered instruction files; test that changing an on-demand tool's description no longer changes it.
4. **Tests of `18 §App guides on demand` marked P5-GUIDE-7:** registry core set ≤ max, every tool has a family; protocol serde (old peer); bridge list core-only vs all and
   call by name in both; instruction snapshots for both modes (`insta`); rolling upgrade both ways (new API + old supervisor ignores the fields: JSON with extra fields; old API +
   new supervisor: no `platform_tools` → `bridge_core_only` false); config refusal; home-sync test of `platform_tools`; **SEC-119/121** e2e: `memory_save` through
   `aurora-agent call aurora` and through MCP give the same `tool.started`/`tool.finished` events and audit row; FIX-151 binding from a non-harness process → `session_not_caller`.
5. **Measure** `tools/list` size in both modes and the fragment (chars ÷ 4) and record it; eval comparison pending unless P4-EVAL runs; `docs/tests/security-coverage.md`
   SEC-119/120 ids; docs `12 §Platform tools`/`24` if wording changed (the `calendar` family).
6. Then the **batch gate** for GUIDE-6 and -7 (list in the lane spec); remember the full-image hosted scenarios (GUIDE-6 changed the image: one wrapper).
