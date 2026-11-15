# P5-GUIDE-1: Agent guides — package format, linter, storage, snapshot, renderer, CLI (owner, 2026-11-13)

First task of "app guides on demand" and the **first addon-related task of phase 5** (before
P5-ASRC and P5-CERN-ADDONS). Normative spec: `docs/24-app-guides.md` (§3, §4, §5, §8, §10);
contracts C-640, C-641, C-642, C-646, C-647, C-649; decisions D-170, D-171; SEC-117, SEC-91.
Read `24` fully and `09 §2`, `§3.4 agent`, `§6`, `§9`; nothing else is needed.

## What it delivers

A package can ship an **agent guide**: `agent.summary`, `agent.guide[]` and the files of
`agent/<app_id>/`. The core validates them, stores the files, puts their digests in the
snapshot, and can render the two files an agent will read (`guide.md`, `tools.md`). Nothing
reaches a sandbox yet: that is P5-GUIDE-2. After this task every existing package behaves and
hashes exactly as before.

## Scope

1. **Limits** (`crates/aurora-core/src/limits.rs`): `AGENT_SUMMARY_MAX` 160,
   `AGENT_GUIDE_FILES_MAX` 24, `AGENT_GUIDE_FILE_MAX` 64 KiB, `AGENT_GUIDE_APP_MAX` 256 KiB,
   `AGENT_GUIDE_SYNC_MAX` 2 MiB, `AGENT_NOTES_INLINE_MAX` 4 000 (the last two are used by
   P5-GUIDE-2; define them here so that task does not touch this file).
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

`crates/aurora-registry/**`, `crates/aurora-core/src/limits.rs` (the six constants) and
`crates/aurora-core/src/sanitize.rs` (moved helpers), one migration,
`crates/aurora-db/src/repo/plugins.rs` (one function),
`services/aurora-api/src/domains/plugins/{ingest.rs,registry.rs}`, `services/aurora-cli/src/plugin/**`,
`plugins/fixtures/**`, `plugins/builtin/README.md`,
`services/aurora-supervisor/src/instructions.rs` (only the helper move). The paused P5-ASRC lane
touches `domains/plugins`: it rebases on this task, not the reverse.

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
