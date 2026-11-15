# P5-GUIDE-2: Agent guides — home sync, supervisor projection, instruction file, admin API (owner, 2026-11-13)

Second task of "app guides on demand". Normative spec: `docs/24-app-guides.md` (§5.3, §6, §7,
§9, §11); contracts C-643, C-644, C-645, C-648; decisions D-171; SEC-117, SEC-91. Also read
`12 §Home projection` and `10 §Agent instructions`. Depends on **P5-GUIDE-1** (renderer, guide
blobs, snapshot keys, limits).

## What it delivers

Every hosted agent gets `.aurora/apps/<app_id>/{guide.md,tools.md,<reference files>}` for each
connected app, kept current by the ordinary home sync, and an instruction file that lists apps
by one line each once the plugin notes pass `AGENT_NOTES_INLINE_MAX`. Instance admins can read a
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
   - Inline / index rendering and the preamble paragraph exactly as C-645 and `24 §6`.
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
- [ ] Supervisor snapshots: inline at exactly `AGENT_NOTES_INLINE_MAX`; index at one character
      more; render inputs without `guide_path` → the pre-feature file byte for byte; preamble
      paragraph only with at least one app.
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
