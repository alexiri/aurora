# P5-GUIDE-7: Platform tools on demand — exposure, families, bridge filter, `aurora-tool` (owner, 2026-11-15)

Seventh task of "app guides and tools on demand": the same Code Mode treatment for Aurora's own
MCP bridge. Owner, 2026-11-15: "spec out fixing the mcp bridge's issues the same way."
Normative spec: `docs/24-app-guides.md` §10b and §11 (SEC-119, SEC-120); `12 §Platform tools`
(as amended); `10 §Agent instructions` (*Platform tools on demand*); `16` (MCP surface).
Contracts **C-654, C-655, C-657**. Decision **D-297**. Depends on **P5-GUIDE-6** (the `tool`
subcommand placeholder, the wrappers, `agent_cli`, the preview/save helpers).

## Problem

The bridge lists every available platform tool (about 38 in nine families) to the harness, so
their names, descriptions and JSON schemas — an estimated 2 000 to 3 000 tokens — are in every
session, used or not, and any change to any tool's description releases idle sessions.

## What it delivers

In mode `on_demand` (default) the harness sees only the core tools (`send_file`,
`complete_task`); the instruction file lists every family in one line; the agent finds and calls
the rest with `aurora-tool` through the same socket, registry, checks, narration and audit. Mode
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
8. **`aurora-tool`** (`services/aurora-supervisor/src/tool_cli/`, `main.rs`): replace
   P5-GUIDE-6's placeholder with C-655: `list`, `search`, `describe`, `call`, the socket protocol
   of `12 §Path`, schema-driven coercion, preview/save (shared helpers from P5-GUIDE-6), exit
   codes 0/1/2/4.
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
`services/aurora-supervisor/src/{mcp_bridge.rs,tools.rs,instructions.rs,harness_env.rs,spawn.rs,tool_cli/**,main.rs}`
and the adapters' environment code. **Not concurrent** with any lane that edits
`domains/tools/` (e.g. FIX-150, P4-UTOK) or `mcp_bridge.rs`.

## Acceptance criteria

- [ ] Every test of `18 §App guides on demand` marked P5-GUIDE-7, each named in the report.
- [ ] SEC-119: every tool reachable by name in both modes with identical results, events and
      audit rows; e2e `memory_save` through `aurora-tool` and through MCP produce the same
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
