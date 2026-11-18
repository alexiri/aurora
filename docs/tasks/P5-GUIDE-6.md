# P5-GUIDE-6: `aurora-agent` — operations file, `api.json`, one in-sandbox command (app targets) (owner, 2026-11-15)

Sixth task of "app guides and tools on demand": the *search* and *execute* half of Code Mode.
Normative spec: `docs/24-app-guides.md` §3.1 (`operations`), §3.5, §4, §5.4, §6 (the `agent_cli`
sentences), §8, §10 (item 4 of the import), §10a (the command, *App targets*), §11 (SEC-118,
SEC-120). Contracts **C-651, C-652, C-653, C-656**, and the amended C-640 (incl. the reserved app
id `aurora`), C-641, C-643, C-645, C-648, C-649. Decision **D-296**. Read `24` fully, `09 §3.4
tools` and `§3.4 hosts` (the catalogue and inject stages), `12 §Loopback proxies`, `11 §Sandbox
image`. Depends on **P5-GUIDE-2** (home sync, `apps` part, renderer wiring) and **P5-GUIDE-5**
(the import's intermediate model).

**One command (owner, 2026-11-15).** There is a single in-sandbox command, `aurora-agent`, whose
*targets* are connected app ids and the reserved id `aurora` (platform tools). This task builds
the command, its shared verbs and output machinery, and the app targets; the `aurora` target is
P5-GUIDE-7 and here only answers "not available in this version" (exit 2) and stays out of
`list` and `search`. Structure the code so P5-GUIDE-7 adds a second target kind behind the same
verbs (a small trait or enum over "app" and "platform"), not a second command.

## What it delivers

A package can ship an operations file; the core validates it and renders, per agent and app, an
`api.json` into the app's folder; the sandbox gets `aurora-agent`, which lists targets, searches
operations and recipes, describes an operation and calls it with the right account, pagination,
the response saved to a file and a bounded preview. The instruction file and `guide.md` mention
the command when the sandbox has it. **No gateway, policy, runner or hook-host change.**

## Scope

1. **Limits** (`crates/aurora-core/src/limits.rs`): `AGENT_OPERATIONS_MAX` 2 000,
   `AGENT_OPERATIONS_FILE_MAX` 512 KiB, `AGENT_API_JSON_MAX` 512 KiB, `AGENT_CLI_PREVIEW_MAX`
   2 000, `AGENT_CLI_PAGES_DEFAULT` 20, `AGENT_CLI_PAGES_MAX` 200, `AGENT_CLI_SAVE_MAX` 64 MiB.
2. **Format and linter** (`crates/aurora-registry`): `agent.operations` in the manifest model;
   the operations-file model with `deny_unknown_fields` everywhere; the rules and codes of C-651
   in a new `validate/agent_operations_rules.rs`. The forbidden-header rule needs the app's inject
   stage header names: read them from the parsed `hosts[]`, never from a hard-coded list alone.
   JSON paths reuse the restricted parser of the `library` block (`09 §3.4 library`). **Reserved
   app id**: `aurora` is rejected for any app (`app_id_reserved`, C-640); check that no built-in
   or fixture package uses it.
3. **Snapshot and storage**: `apps[].agent.operations = { file, sha256, size, count }` (C-641);
   the file is stored as an `agent_guide` blob at ingest (same code path as guide files).
4. **Renderer** (`crates/aurora-registry/src/agent_guide.rs`): `api.json` exactly as C-652,
   deterministic, with the drop order of `24 §5.4`. **Tool mapping uses the gateway's matcher**:
   find the function the gateway uses to classify a request into a catalogue tool (in
   `aurora-registry` or `aurora-policy`) and call it; do not write a second matcher. If it lives
   in a crate the registry cannot depend on, stop and report (that is a design question for the
   main session). `guide.md` gains the `api.json` line (C-646 as amended).
5. **Home sync** (`services/aurora-api/src/domains/hosted/home.rs`): add `api.json` to each
   app's entries in the `apps` part (not for database apps); the operations file is not
   projected. `base_url` per account from the same host resolution `guide.md` uses.
6. **`aurora-agent`** (`services/aurora-supervisor`): a new `agent` subcommand in `main.rs` and a
   module `src/agent_cli/` implementing `24 §10a` and C-653: `list`, `search`, `describe`,
   `call` with the target model above; the app target in full. HTTP through the inherited
   `HTTPS_PROXY` with the crate's `reqwest` (proxy from the environment, the system trust store
   the image already configures for `curl`); no other network path. Path building with
   per-segment percent-encoding; `--path` validation; argument coercion; pagination styles with
   the same-origin rule; save and preview; options that do not apply to a target refused (exit
   2); exit codes of C-653 (recognise a gateway refusal by the gateway's error body shape, `06
   §Errors`). Output files under `/tmp/aurora-agent/` (created 0700) unless `--out`; refuse an
   `--out` under `/workspace/.aurora/` or `/workspace/memory/` (SEC-120).
7. **`agent_cli`** (C-653): the supervisor computes it at start (`/opt/aurora/bin/aurora-agent`
   exists and is executable) and passes it to the instruction renderer: the `aurora-agent`
   sentences of the index paragraph and the preamble (C-645).
8. **Image** (`images/sandbox/bin/aurora-agent`, and the Containerfile line that installs
   `images/sandbox/bin/*` if it is not a glob): a two-line wrapper `exec
   /usr/local/bin/aurora-supervisor agent "$@"`.
9. **Import** (`services/aurora-cli/src/plugin/import_openapi*`): C-656, the fourth output, from
   P5-GUIDE-5's model.
10. **Built-in packages**: an operations file for **indico** (the room search and booking
    operations at least, so the recipe of P5-GUIDE-4 can use `aurora-agent call indico …`) and
    **gitlab** (generated with the import from the vendor's OpenAPI document if one is usable,
    trimmed to the catalogue's areas); update those recipes to show the `aurora-agent` calls.
    Fixtures: `fixture-guide` gains a small operations file (one paginated list, one path
    parameter, one write), the `fixture-invalid-agent-operations-*` packages of `18`, and one
    `fixture-invalid-app-id-reserved`.
11. **CLI preview**: `aurora plugin guide` also writes `api.json` (C-649 as amended).
12. **Admin API**: `PluginPackage.agent_guides[].operations` and `changed.operations` (C-648 as
    amended); the file route serves the operations file. OpenAPI and `schema.d.ts` regenerated.

## Out of scope

The `aurora` target's real behaviour, exposure and the bridge (P5-GUIDE-7). Generated
per-language clients, executable recipes (`24 §12`). Any gateway change: if a call needs one,
stop and report.

## Ownership

`crates/aurora-registry/src/{manifest/,validate/agent_operations_rules.rs,validate/mod.rs,agent_guide.rs,snapshot.rs}`,
`crates/aurora-core/src/limits.rs` (the seven constants),
`services/aurora-api/src/domains/hosted/home.rs` (the `api.json` entries only),
`services/aurora-api/src/domains/plugins/{ingest.rs,registry.rs,dto.rs,service.rs}` (operations
only), `services/aurora-supervisor/src/{main.rs,agent_cli/**,instructions.rs}` (the `agent_cli`
flag and sentences), `images/sandbox/bin/aurora-agent`,
`services/aurora-cli/src/plugin/**` (import and guide), `plugins/builtin/{indico,gitlab}/**`,
`plugins/fixtures/**`. Not `mcp_bridge.rs`, not `services/aurora-api/src/domains/tools/**`
(P5-GUIDE-7).

## Acceptance criteria

- [ ] Every test of `18 §App guides on demand` marked P5-GUIDE-6 (operations linter corpus,
      reserved app id, `api.json` snapshots, `aurora-agent` unit tests for app targets, import
      output), each named in the report.
- [ ] SEC-118 and SEC-120 tests exist and pass; `docs/tests/security-coverage.md` rows updated
      with the test ids.
- [ ] Compatibility: a package without `operations` has the same digest and snapshot fragment
      as before; an old supervisor ignores `api.json` (it is just another file in the `apps`
      part) and renders no `aurora-agent` sentence.
- [ ] End to end (kind, `fake` harness scripted to run shell commands): `aurora-agent call
      fixture-guide <list op> --all` reaches the mock upstream through the gateway with
      `x-aurora-connection-id` and the injected credential, joins two pages, saves the file and
      prints a preview under the bound; a blocked operation exits 3 with the gateway's reason;
      `aurora-agent call aurora memory_save …` exits 2 with "not available in this version".
- [ ] `aurora plugin import-openapi` on the three checked-in documents writes a byte-stable,
      valid `operations.json`.
- [ ] fmt, clippy `-D warnings` on touched crates and reverse dependencies, the full
      `aurora-registry`, `aurora-supervisor` and the targeted `aurora-api` suites, guards
      (including SEC-91's guard with the `agent` exception), OpenAPI drift check. Exact commands
      and results in the report; say what was not run.

## Notes for the implementer

- Change files only with the Edit/Write tools. No `Co-Authored-By` lines in commits.
- `aurora-agent` is help for the agent, not a security boundary: the gateway decides. But it must
  never *add* reach (SEC-118): no host other than the account's or the operation's validated
  host, no header other than declared ones and `x-aurora-connection-id`.
- Keep stdout small and predictable: models read it. Snapshot-test the preview format.
- Suggested agent: `rust-developer` (Sonnet). Size L (registry, API, supervisor, image, CLI,
  packages). The package content (step 10) can be split to a `haiku-worker` once the format is
  merged.

## Completion report
(lane `p5-apps`, worker 38, 2026-11-18; **partial: part 1 of 6 done, commit 7135a8f4**)

**Done (scope items 1, 2, 3, 12 partly):**
- Limits: the seven constants in `crates/aurora-core/src/limits.rs`.
- Format and linter: `agent.operations` in `manifest/model.rs`; `crates/aurora-registry/src/operations.rs` (closed model,
  `parse`, `count`); `validate/agent_operations_rules.rs` (all `agent_operations_*` codes of C-651; forbidden-header rule reads
  the app's `hosts[]` inject stages (`header`, `remove_header`) plus the fixed list; hosts = exact `hosts[]` and `{instance}` for an
  instance-bound app; JSON paths through the `library` `JsonPath`); `agent_guide_rules.rs` (operations name, summary required,
  `agent_operations_listed`, the file counts as listed, own size limit and linter, `agent_guide_missing_file`);
  `RESERVED_APP_ID` → `app_id_reserved` in `validate::manifest`. No built-in or fixture package uses the id `aurora` (grep).
- Snapshot: `AgentEntry.operations: Option<OperationsRef{file,sha256,size,count}>` (absent from the serialized fragment without an
  operations file: a package without one is byte-identical). `GuideDigests` stays a map; the operations file is one more
  `(app, file)` entry and the count lives under the non-file key `(app, "operations.count")` (`OPERATIONS_COUNT_KEY`).
- Storage: the existing ingest loop already stores every `agent/**` file as an `agent_guide` blob; the file route
  (`service::guide_file`) now serves the operations file; `PluginAgentGuide.operations` and `changed.operations` are filled
  (`service.rs`, DTO fields already existed). `has_guide` checks in `registry.rs`/`service.rs` include operations.
- Tests (`crates/aurora-registry/tests/it/agent_operations.rs`, 7): valid shapes, one case per violation code, header rule from
  `hosts[]`, too many/too large, listed/named/present, reserved app id, snapshot fragment and compatibility. Gates run:
  `cargo fmt --all --check`, `cargo clippy -p aurora-registry -p aurora-core -p aurora-api --all-targets --all-features -- -D warnings`
  clean, `cargo test -p aurora-registry` all targets (22 lib + 135 it, 0 failed). Not yet run for this part: the api it suite,
  OpenAPI drift (no DTO shape changed; the `operations` fields pre-existed), guards.

**Not done (in this order):**
4. Renderer `api.json` in `agent_guide.rs` (C-652; the gateway's matcher is `identify::identify_tool_in(app, instance, host, port, path, query, method, body_prefix, truncated, headers, connect_method)` in the same crate; `{param}` → a placeholder segment; `Rendered` gains `api_json`; `guide.md` line).
5. Home sync `api.json` entries (`hosted/home.rs`).
6./7. `aurora-agent` in the supervisor (`agent_cli/`, `main.rs` subcommand; the supervisor does **not** depend on `aurora-registry`, so it needs its own small `api.json` reader) and the `agent_cli` flag in `instructions.rs`.
8. Image wrapper — **ask the coordinator first** (`images/sandbox/` is being edited by P5-HARN Part E).
9. Import output C-656; 10. indico/gitlab operations files and `fixture-guide` etc. fixtures; 11. `aurora plugin guide` writes `api.json`.
