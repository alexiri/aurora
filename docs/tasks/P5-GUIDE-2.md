# P5-GUIDE-2: Agent guides — home sync, supervisor projection, instruction file, admin API (owner, 2026-11-13)

Second task of "app guides on demand". Normative spec: `docs/24-app-guides.md` (§5.3, §6, §7,
§9, §11); contracts C-643, C-644, C-645, C-648; decisions D-171, **D-295**; SEC-117, SEC-91.
Also read `12 §Home projection` and `10 §Agent instructions`. Depends on **P5-GUIDE-1** (renderer,
guide blobs, snapshot keys, limits).

**Amendment of 2026-11-15 (D-295), applies to everything below:** the instruction file is
**always the index** (C-645 as amended); there is no inline mode and no `AGENT_NOTES_INLINE_MAX`.
Today's inline rendering remains only as the fallback when an app entry has no `guide_path`
(older API). The index paragraph and the preamble are rendered in their "no `agent_cli`" form
(the `aurora-agent` sentences come with P5-GUIDE-6, C-653); write the renderer so P5-GUIDE-6 adds
them behind one boolean. Recipes are projected like reference files (with the *Procedure*
header from P5-GUIDE-1's `reference_header`), and `PluginPackage.agent_guides[].files[]` carries
`kind` (C-648 as amended); `operations` in C-648 is `null` until P5-GUIDE-6.

## What it delivers

Every hosted agent gets `.aurora/apps/<app_id>/{guide.md,tools.md,<recipes and reference
files>}` for each connected app, kept current by the ordinary home sync, and an instruction file
that lists apps by one line each and carries no plugin notes. Instance admins can read a
package's agent guide through the API. **No gateway, policy, runner or hook-host change.**

## Scope

1. **Protocol** (`crates/aurora-protocol/src/runner.rs`): `HomeSyncKind::Apps = "apps"`;
   regenerate the JSON schemas the repo checks in. `HomeSyncPart.entries` stays untyped.
2. **API home sync** (`services/aurora-api/src/domains/hosted/home.rs`):
   - `render_inputs.apps[]` gains `app_id`, `summary`, `guide_path` (C-644); the four existing
     fields are unchanged, including the "too large → drop snippets" fallback.
   - New `apps` part, packed with the existing `pack()` after `tools`: per app of the render
     inputs, `guide.md` and `tools.md` from `aurora_registry::agent_guide::render`, then the
     package's guide files (C-643; `.md` / `.txt` get the header of `24 §5.3`). Every path goes
     into `manifest`.
   - Accounts for the renderer: extend `home_app_connections`
     (`crates/aurora-db/src/repo/hosted.rs`) with the grant's `mode`, `allow_tools`, `ask_tools`
     and the connection's `tenant_host` and instance host (join `app_instances`); the host of a
     connection of an instance-bound app without an instance is the app's `default_host`.
   - `AGENT_GUIDE_SYNC_MAX`: whole apps in `app_id` order; over budget → generated files only,
     `reference_available = false`, a warning and `aurora_home_guide_budget_exceeded_total`.
     Make the limit overridable in the test configuration only.
   - Guide bytes through an in-process cache keyed by digest (bounded 64 MiB, least recently
     used evicted; content-addressed, so no invalidation).
3. **Supervisor** (`services/aurora-supervisor/src/home.rs`, `instructions.rs`):
   - `.aurora/apps/` becomes a managed root with the acceptance rule of C-643 (exact depth, both
     grammars, `guide.md` / `tools.md` allowed); 0444 files in 0555 directories; pruned at commit.
     It must not collide with the other `.aurora/` content (`tools.json`, `sync/`, `pg/`, inbox).
   - `RenderInputs.apps[]` gains `app_id`, `summary`, `guide_path` (all optional on the wire).
   - The index rendering (always; the inline text only as the older-API fallback) and the
     preamble paragraph exactly as C-645 and `24 §6`.
4. **Admin API** (`services/aurora-api/src/domains/plugins/{dto.rs,routes.rs,service.rs,lifecycle.rs}`):
   `PluginPackage.agent_guides`, the file route, `agent_guide_changed` in the audit metadata
   (C-648); OpenAPI and `web/src/lib/api/schema.d.ts` regenerated; permission-matrix row for the
   new route (instance admin).
5. **Guard** `scripts/guards/agent-guide-advisory.sh` + its line in `run-all.sh` (SEC-91): fails
   if `services/aurora-gateway`, `crates/aurora-policy` or `services/aurora-hookhost` reference
   `.aurora/apps`. It must fail on a planted reference (prove it in the PR description) and must
   not pass vacuously (see FIX-068 for the failure modes to avoid).
6. **Docs you update**: `docs/tests/security-coverage.md` (test ids for SEC-117, SEC-91 once they
   exist), `docs/tests/hosted-coverage.md` if it lists home-sync parts.

## Out of scope

Format, linter, renderer text (P5-GUIDE-1; a wrong rendering is fixed there). Dashboard
(P5-GUIDE-3). Built-in package content (P5-GUIDE-4). Skipping unchanged files by digest, the
effective view, a route for members or agents, BYO agents (`24 §12`).

## Ownership

`crates/aurora-protocol/src/runner.rs` (one variant) and generated schemas;
`services/aurora-api/src/domains/hosted/home.rs`; `crates/aurora-db/src/repo/hosted.rs`
(`home_app_connections` and its row type); `services/aurora-api/src/domains/plugins/{dto.rs,routes.rs,service.rs,lifecycle.rs}`;
`services/aurora-supervisor/src/{home.rs,instructions.rs}` and its snapshots;
`services/aurora-api/tests/it/{home_agent_guides.rs,home_render_inputs.rs}`; the guard; the
e2e addition. Not `ingest.rs` / `registry.rs` (P5-GUIDE-1).

## Acceptance criteria

- [ ] **Written first, on the unmodified code**: a supervisor test that a sync carrying an
      unknown part kind and unknown render-input fields commits and renders today's instruction
      snapshot. It passes before and after.
- [ ] Supervisor snapshots: index with one app, with two, and with every built-in fixture app
      (all with small notes: no notes appear in the file); render inputs without `guide_path` →
      the pre-feature file byte for byte; preamble paragraph only with at least one app.
- [ ] Supervisor home tests: accepted and refused paths (deeper path, bad id, bad name,
      traversal), modes, prune when the app leaves the manifest.
- [ ] `home_agent_guides.rs`: everything listed in `18 §App guides on demand` (*API*): folders
      match the render inputs, manifest complete, every entry fits a part, custom-grant edit /
      revoke / disconnect / unavailable / new package version each give the expected next
      generation, the budget, the digest-only read, no credential field of the fixture
      connection anywhere in the generated files, an agent without a grant on app A gets nothing
      of A, the admin routes.
- [ ] A sync that changes only `.aurora/apps/**` leaves the instruction files identical and
      releases no idle session (test at the supervisor; if the release decision is taken
      elsewhere, find it and test there).
- [ ] End to end (`fake` harness, kind): the folder of `fixture-guide` exists with the expected
      files and modes under a custom grant; revoking the grant removes it at the next sync.
- [ ] Live, env-gated, reported and not a gate: the behavioural check of `18` (the agent reads
      `api.md` before calling) on `claude-code`; on `codex` if an endpoint is configured.
- [ ] fmt, clippy `-D warnings`, the full `aurora-api` and `aurora-supervisor` suites, guards,
      OpenAPI drift check. Report exact commands and results. Do **not** run performance budgets
      while other lanes build; record "not yet measured" for the sync-size numbers and measure
      on an idle machine: bytes and time of one home sync for an agent with every fixture app.

## Notes for the implementer

- Change files only with the Edit/Write tools. No `Co-Authored-By` lines in commits.
- Sandboxes keep their supervisor until their next boot, so new API + old supervisor is the
  normal state after a deploy; that is why the old render-input fields must not change.
- Suggested agent: `rust-developer` (Sonnet); review by `sonnet-integration-reviewer` against
  C-643..C-645 and C-648 before merge. Size L.

## Completion report (lane `p5-apps`, worker 35, 2026-11-18)

**Adapted to master** (brief of 2026-11-15):
- `PluginPackage.agent_guides` is filled on the **detail** route only (it reads the stored archive for the
  digests, as the snapshot compile does); lists and the lifecycle responses carry `[]`, like `impact` is `null`
  there. `changed` compares with the active version of the package; `operations` is `null` until P5-GUIDE-6.
- `agent_guide_changed` also rides the automatic activation in `ingest.rs` (the `plugin.activate` row with
  `auto: true`), not only `lifecycle.rs`, because a package with no widening never goes through approve.
- The test-only budget is `AppState::guide_sync_max` + `with_guide_sync_max` (like `stream_recheck`).
- The supervisor renders the index with `render_with(inputs, agent_cli)`; `render` passes `false` (P5-GUIDE-6 plumbs
  `agent_cli`). The preamble paragraph sits after *Credentials*, only in index mode (C-645).
- App id grammar in the supervisor is `[a-z][a-z0-9_-]{1,47}` (`manifest/mod.rs`); a file name is the C-640 grammar
  (`guide.md`, `tools.md`, `api.json` match it). A symlink in place of `.aurora/apps` or an app folder is removed
  before install (the agent's uid owns `.aurora`).
- No code anywhere sends `SessionRelease` for a home change, so "a folder-only sync releases no session" is true
  by construction; the supervisor test pins that the instruction files and render inputs stay byte-identical.
- `home_app_connections` gained `grant_mode`, `allow_tools`, `ask_tools`, `tenant_host`. The account host is the
  instance host, else the stored tenant host, else the app's `default_host`.
- Guard `agent-guide-advisory.sh` has `--self-test` (planted reference fails, clean passes, missing directory
  fails) and `limits-gaps.allow` lost `AGENT_GUIDE_SYNC_MAX`. Planted-reference run: exit 1 with the file and line.
- Two existing `home_render_inputs` tests assumed the inline form on the new wire; they now render the index from
  the wire and the inline form from the wire minus the folder fields (the older-API fallback).

**Follow-up (lane `p5-apps`, worker 36, 2026-11-18):** the end to end ran on the hosted stack (podman, `fake`
harness; the brief says kind, the folder is a sandbox matter and hosted.sh runs the same supervisor): `s14_agent_guide_folder_is_projected_into_the_sandbox_and_removed_on_revoke`
in `tests/e2e-hosted/tests/hosted.rs` uploads the checked-in `fixture-guide` signed with a key trusted for the run,
approves it, connects it, grants Daneel `custom` (`list_rooms` allowed, `book_room` ask), and in the real sandbox checks
modes (dirs 0555, files 0444), the `[allowed]` / `[approval]` / `[blocked]` marks of `tools.md`, the *Procedure* header
of the recipe, no credential under `/workspace/.aurora`, the instruction file listing the app with none of its notes,
the guide file refusing a write, and after the grant is deleted and a second turn, the folder and the index line gone.
`aurora_home_guide_budget_exceeded_total` is asserted in `home_agent_guides::the_budget_keeps_generated_files_and_drops_reference_files`
(`counter_value` delta of 1 with the tight budget, 0 with the default). Commands: `AURORA_TEST_PODMAN=1
AURORA_E2EH_PREFIX=p5apps-e2eh … bash scripts/e2e/hosted.sh s14_` 1 passed (first run failed on my own test's `sort` hiding
`stat`'s exit status, fixed); `cargo test -p aurora-api --test it -- home_agent_guides` 6 passed. The sync-size
measurement is **not yet taken**: the machine was at load 12 and P5-GUIDE-4 changes the content that matters, so it is
taken once with the built-in apps connected (see P5-GUIDE-4's report). Still not done: the live `claude-code` check.

**Not done here (worker 35, superseded by the follow-up above):** the kind end-to-end (`fixture-guide` folder under a custom grant, removed on revoke) and the live
`claude-code` behavioural check: both need the stack; see the lane's batch-gate report. The
`aurora_home_guide_budget_exceeded_total` increment is not asserted (no metrics test harness for it): the budget
behaviour is.

**Gate level: per task.** fmt; `cargo clippy -p aurora-protocol -p aurora-db -p aurora-supervisor -p aurora-api
--all-targets --all-features -- -D warnings` clean; supervisor lib 113 + it 136 green; `aurora-api --test it --
home plugins permission_matrix openapi skills hosted::` 84 tests (home_agent_guides 6 new, home_render_inputs 4);
guards rc 0 (schema check included); `UPDATE_OPENAPI=1` + `pnpm -C web gen:api` regenerated. Sync-size numbers: not
yet measured (machine busy). Deviation: the two coverage-doc rows were edited with a python one-liner.
