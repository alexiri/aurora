# P5-GUIDE-1: Agent guides — package format, linter, storage, snapshot, renderer, CLI (owner, 2026-11-13)

First task of "app guides and tools on demand". (Written as the first addon-related task of
phase 5; P5-ASRC merged and P5-CERN-ADDONS ran first, D-298.) Normative spec:
`docs/24-apps-for-agents.md` (§3, §4, §5, §8, §10); contracts C-640, C-641, C-642, C-646, C-647,
C-649; decisions D-170, D-171, **D-295**; SEC-117, SEC-91. Read `24` fully and `09 §2`, `§3.4
agent`, `§6`, `§9`; nothing else is needed.

**Amendment of 2026-11-15 (D-295, D-298), applies to everything below:**
- **Recipes**: a guide entry is `{ file, title, kind? }`, `kind` = `reference` (default) |
  `recipe`; the linter adds `agent_recipe_not_markdown` and `agent_guide_kind`; the snapshot
  always carries `kind`; `guide.md` gains the **Recipes** section (C-646) and a recipe's projected
  header says *Procedure* (`24 §5.3`); `api.json` joins the reserved names (C-640). Fixtures:
  `fixture-guide` gains one recipe; add `fixture-invalid-agent-guide-{recipe-not-markdown,kind}`.
- **No `AGENT_NOTES_INLINE_MAX`**: do not define it (removed by D-295). Limits from `24 §8` that
  belong to P5-GUIDE-6/7 are defined by those tasks.
- **`docs/22-addon-repos.md`** (moved here from P5-ASRC, which merged first): list the agent
  guide among the `plugin.yaml` fields that matter for addons (`agent.summary`,
  `agent.instructions` as the essentials, `agent.guide[]` with reference files and recipes),
  point to `24 §3` without restating the rules, and show `aurora plugin guide` in the worked
  example. The fixture addon repository's package (if P5-ASRC left one under `plugins/` or
  `tests/`) ships a guide file.
- The renderer signature gains what the Recipes section needs (the entries' `kind`); keep it
  pure. Shape it so P5-GUIDE-6 can add `api_json: Option<String>` to `Rendered` without changing
  callers.

## What it delivers

A package can ship an **agent guide**: `agent.summary`, `agent.guide[]` and the files of
`agent/<app_id>/`. The core validates them, stores the files, puts their digests in the
snapshot, and can render the two files an agent will read (`guide.md`, `tools.md`). Nothing
reaches a sandbox yet: that is P5-GUIDE-2. After this task every existing package behaves and
hashes exactly as before.

## Scope

1. **Limits** (`crates/aurora-core/src/limits.rs`): `AGENT_SUMMARY_MAX` 160,
   `AGENT_GUIDE_FILES_MAX` 24, `AGENT_GUIDE_FILE_MAX` 64 KiB, `AGENT_GUIDE_APP_MAX` 256 KiB,
   `AGENT_GUIDE_SYNC_MAX` 2 MiB (used by P5-GUIDE-2; define it here so that task does not touch
   this file).
2. **Archive** (`crates/aurora-registry/src/package.rs`): accept `agent/<app_id>/<file>` (exactly
   that depth; `<file>` grammar of C-640); the existing path-safety, duplicate and
   decompressed-size rules apply.
3. **Manifest** (`crates/aurora-registry/src/manifest/model.rs`, `sanitize.rs`): `Agent.summary`,
   `Agent.guide: Vec<{ file, title }>`; strict keys.
4. **Linter** (new `crates/aurora-registry/src/validate/agent_guide_rules.rs`, wired from
   `validate/mod.rs`): every rule and violation code of C-640, including listed ⇔ present,
   UTF-8 without control characters other than LF and TAB and without BOM, the three size
   limits, `agent_summary_required`, `agent_guide_computed_field`.
5. **Snapshot** (`crates/aurora-registry/src/snapshot.rs`, and the compile path
   `services/aurora-api/src/domains/plugins/registry.rs`): `apps[].agent.summary` and
   `apps[].agent.guide[] = { file, title, sha256, size }`, both omitted when absent. Follow the
   `hooks_digest` precedent: digests come from the stored archive at compile. The submitted
   manifest type and the snapshot type differ in those two fields; do not loosen
   `deny_unknown_fields` on either.
6. **Storage**: migration `crates/aurora-db/migrations/<next>_agent_guide_blobs.sql` (the
   `plugin_blobs.kind` check gains `agent_guide`; forward-only, no data change);
   `services/aurora-api/src/domains/plugins/ingest.rs` writes one blob per guide file in the
   ingest transaction for a package that is not `invalid`; a repo function in
   `crates/aurora-db/src/repo/plugins.rs` that reads a guide blob **by digest only** (C-642
   explains why the kind must not be in the predicate).
7. **Renderer** (new `crates/aurora-registry/src/agent_guide.rs`): a pure function, no I/O,

   ```rust
   pub struct GuideAccount { pub id: String, pub label: String, pub host: Option<String>,
                             pub service: Option<String>, pub access: GrantAccess }
   pub enum GrantAccess { Full, Custom { allow: BTreeSet<String>, ask: BTreeSet<String> } }
   pub struct Rendered { pub guide_md: String, pub tools_md: String }
   pub fn render(app: &AppEntry, lost_hosts: &[&HostEntry], accounts: &[GuideAccount],
                 package_version: &str, reference_available: bool) -> Rendered;
   pub fn reference_header(app_name: &str, package_version: &str) -> String;   // 24 §5.3
   ```

   (names may change; the shape may not gain I/O or a database type). Output is exactly C-646
   and C-647. Reuse the one-line sanitizer and the fence helper semantics of
   `services/aurora-supervisor/src/instructions.rs` (`inline`, `fenced`): move them to a shared
   place (`aurora-core::sanitize` already exists) rather than copying; the supervisor keeps
   compiling and its snapshot keeps passing.
8. **CLI** (`services/aurora-cli/src/plugin/{cmds.rs,pack.rs,mod.rs,templates/}`): `guide`
   (C-649), `new` scaffold, `build` packs `agent/**`, `docs` includes the agent guide.
9. **Fixtures** (`plugins/fixtures/`): `fixture-guide` (`09 §13`), and
   `fixture-invalid-agent-guide-{unlisted,missing,reserved-name,name,not-text,too-large,summary-required}`
   with their lines in `expected-invalid.txt`, signed with the test key like the others.
10. **Docs**: `plugins/builtin/README.md` gains the authoring rules of `24 §10` and the fixture
    notes. Do not edit `docs/*.md` other than to fix an error you prove; report spec problems.

## Out of scope

Home sync, supervisor, instruction file, admin routes, dashboard (P5-GUIDE-2/3); built-in
packages (P5-GUIDE-4); `import-openapi` (P5-GUIDE-5); any gateway, policy or hook-host change.

## Ownership (disjoint from every other open lane)

`crates/aurora-registry/**`, `crates/aurora-core/src/limits.rs` (the five constants) and
`crates/aurora-core/src/sanitize.rs` (moved helpers), one migration,
`crates/aurora-db/src/repo/plugins.rs` (one function),
`services/aurora-api/src/domains/plugins/{ingest.rs,registry.rs}`, `services/aurora-cli/src/plugin/**`,
`plugins/fixtures/**`, `plugins/builtin/README.md`, `docs/22-addon-repos.md` (the agent-guide
paragraph), `services/aurora-supervisor/src/instructions.rs` (only the helper move). P5-ASRC's
code in `domains/plugins` is merged: rebase on master before starting.

## Acceptance criteria

- [ ] Tests first for compatibility: the compiled snapshot of every built-in and fixture package
      as it is today has the **same hash** before and after this task; the digest of each
      existing package is unchanged.
- [ ] `crates/aurora-registry/tests/it/agent_guide.rs`: the linter matrix (one case per code),
      snapshot shape (present / omitted), and the renderer under `insta` for every case listed in
      `18 §App guides on demand` (*Registry*), including the hostile-text corpus and the SEC-91
      property test (no input prints `allowed` for a tool the grant blocks).
- [ ] `fixture-guide` validates, builds, signs and tests; each `fixture-invalid-agent-guide-*`
      is rejected with exactly its code.
- [ ] API it-test: ingest of `fixture-guide` stores one `agent_guide` blob per file; an `invalid`
      package stores none; a guide file byte-identical to an existing blob of another kind is
      readable through the new repo function.
- [ ] `aurora plugin guide --app <id> --access custom --out <dir>` on `fixture-guide` equals a
      checked-in golden directory; `aurora plugin new x && aurora plugin validate` passes.
- [ ] `cargo fmt --check`, clippy `-D warnings` on the touched crates and their reverse
      dependencies, the targeted tests above, the quick gate, guards. Report exact commands and
      results; say what was not run.

## Notes for the implementer

- Change files only with the Edit/Write tools. No `Co-Authored-By` lines in commits.
- A custom grant's unknown tool ids (a tool removed by an upgrade) are ignored by the renderer;
  new tools are `blocked` under a custom grant (`09 §10`), which the "else blocked" rule gives
  for free.
- Suggested agent: `rust-developer` (Sonnet). Size L. Escalate to the main session if the
  snapshot type split forces a change in what the gateway deserializes.

## Completion report (lane `p5-apps`, worker 34, 2026-11-18)

**Adapted to master** (brief of 2026-11-15; master has FIX-160, P5-UX-2/13, P4-UTOK/NARR/EVAL):
- `plugin.yaml` source tree: `agent/**` is picked up by `pack.rs::source_files` (the builder only
  knew the fixed files); `Package::read` accepts `agent/<app>/<file>` and the directories
  `agent`, `agent/<app>`.
- The snapshot type split: `snapshot::AgentEntry`/`GuideFile`/`GuideKind` (strict), the manifest
  keeps `Agent` + `GuideEntry` (accepts `sha256`/`size` only so the linter can name them). The
  gateway deserializes `Snapshot` from this crate, so it needed no change. `compile` keeps its
  signature (16 call sites in tests and the gateway fixtures); `compile_with_guides` takes the
  per-package `GuideDigests` computed by `snapshot::guide_digests` from the stored archive (API
  compile path reads the archive when the package declares hooks **or** a guide; the CLI `compile`
  and `test` use it too).
- `GrantAccess`/`GuideAccount`/`Rendered`/`render`/`reference_header` as the brief; `reference_header`
  and a new `project_file` take the entry `kind` (Reference/Procedure). `render` ignores
  `package_version` for now (the projected files carry it); `api_json` joins `Rendered` in GUIDE-6.
- Helpers moved: `inline` and `fenced` are now `aurora_core::sanitize::{inline, fenced}` (the
  supervisor's exact functions, so its snapshot is unchanged); `aurora_core::sanitize::one_line` is
  a different sanitizer (strips format characters, adds an ellipsis) and was not substituted.
- `aurora plugin guide` takes a positional package (archive or source dir, default `.`) and writes
  `<out>/<app_id>/<file>`; stdout form prints `==> <app_id>/<file> <==`. A source directory needs
  its icon (fixtures get theirs from `mkfixture.py`).
- Added the P5-UX-13 tool `description`s to `fixture-guide` (warning-free); `tool_description_missing`
  stays a warning on the invalid fixtures, as on the existing ones.
- Limits guard: `AGENT_SUMMARY_MAX`, `_GUIDE_FILES_MAX`, `_FILE_MAX`, `_APP_MAX` left
  `limits-gaps.allow`; `AGENT_GUIDE_SYNC_MAX unread` stays until P5-GUIDE-2 reads it.
- Not built here (later tasks): `app_id_reserved`, `agent.operations`, `api.json`.

**Evidence.** Compatibility test first: `crates/aurora-registry/tests/it/agent_guide.rs`
(`snapshots_of_packages_without_a_guide_keep_their_hash`, golden `agent_guide_hashes.txt`, 65
snapshot hashes of every built-in and fixture package without guide keys recorded **before** the
snapshot change; unchanged after). Linter matrix: `agent_guide_lint.rs` (one case per code, incl.
`agent_recipe_not_markdown`, `agent_guide_kind`). Renderer: `agent_guide_render.rs`, 7 `insta`
snapshots (full, custom, two accounts, lost claim, account hosts, no tools, classifiers, budget),
the hostile-text corpus and the SEC-91 property (all 27 allow/ask combinations × full/custom ×
lost claim × `block_untooled`, plus a removed tool id in the row). API: `plugins_agent_guide.rs`
(two blobs, none for `invalid`, by-digest read of a blob that exists under another kind,
snapshot digests after approve). CLI: golden `services/aurora-cli/tests/golden/guide/` and
`aurora plugin new` validates. Fixtures: `fixture-guide` + 9 `fixture-invalid-agent-guide-*`
(`scripts/fixtures/build.sh`: passed=67 failed=0; each invalid one rejected with its own code, the
only other finding is the `tool_description_missing` warning). The fail-before check is by
construction for the linter and renderer tests (the code did not exist); the digest-by-kind test
was written against `get_blob(.., "agent_guide")`, which returns `None`, to show why the new
function exists.

**Gate level: per task.** `cargo fmt --all`; `cargo clippy -p aurora-core -p aurora-registry
-p aurora-db -p aurora-cli -p aurora-supervisor -p aurora-api --all-targets --all-features -- -D
warnings` clean; `cargo test -p aurora-registry -p aurora-cli -p aurora-supervisor --lib --test it
--test plugin` all green (registry it 120, cli plugin 5, supervisor lib 107 / it 131);
`cargo test -p aurora-api --test it -- plugins connect::` 64 passed; guards rc 0;
`extract-schema.py --check` ok. Not run (batch gate): full api suite, gateway tests, e2e scripts.
Deviations: `Cargo.lock` gained `insta` as a dev-dependency of `aurora-registry`; three one-line
doc/test edits used python/sed instead of Edit (main.rs mod line, a README path); golden
directories and `insta` snapshots were written by their tools.
