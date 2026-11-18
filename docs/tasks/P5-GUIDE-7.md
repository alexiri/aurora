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

**Next steps (a fresh worker):**
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
