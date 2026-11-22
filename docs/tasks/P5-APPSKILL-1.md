# P5-APPSKILL-1: Apps ship skills — the package side (owner, 2026-11-22)

## Goal

Owner, 2026-11-22 (D-452, D-453): "Change of D-295: recipes may include scripts. there's no difference
between adding the script as text to the recipe and shipping an executable version already." and
"'guides' and 'skills' are basically the same thing … eliminate the concept of guides and replace it
with skills, which is more common. So, now apps can ship with skills after all."

Today (P5-GUIDE) a package's agent content is `agent.guide[]`: reference files and text-only recipes
under `agent/<app_id>/`, projected into `.aurora/apps/<app_id>/`. After this brief a package ships
**skills**: `skills/<app_id>/<name>/SKILL.md` plus files, the same format as a skill-library skill,
scripts allowed. This brief covers what a package can contain and how it is checked, signed, stored and
compiled. P5-APPSKILL-2 delivers the skills to agents.

**P5-APPSKILL-1 and -2 are one Rust lane and merge together**, like P5-SKLIB-1/-2. Until -2 lands, a
build of this branch projects neither guides nor app skills.

## Decisions that bind this brief

- **D-452** (scripts allowed, not a widening) and **D-453** (a), (b), (h), (i).
- **SEC-150** (registry and digest rows); SEC-117 now covers only the summary, essentials and operations file.
- Contracts **C-1130** (archive, linter, limits), **C-1131** (digest with the executable bit),
  **C-1132** (snapshot, storage, migration `20261201000272`), **C-1138** (CLI).

## Order and lanes

- One Rust lane: this brief, then P5-APPSKILL-2 on the same branch. Size L.
- It edits `crates/aurora-registry/**`, `crates/aurora-core/src/limits.rs`, `services/aurora-cli/**`,
  the API's `plugins/ingest.rs` and `skill_libraries/content.rs`, one migration, and **every
  `plugins/builtin/*` package that ships a guide**. Serialize it with any other registry or built-in
  package work (P5-CATALOG is on hold; if it restarts first, the later lane rebases).
- **Security review (SEC-150) before merge**, together with -2. Tell the Orchestrator when both commits
  are in.

## Read first

- `docs/24-app-guides.md` §3, §4, §10, §12 (what is being replaced), `docs/09-app-plugins.md` §2 (archive,
  canonical digest), §6, §9, §10 (:1408), §12; `docs/10-agents-conversations.md` §Skills (:865) and the
  library rules (:960–975); `docs/22-addon-repos.md` :82–105.
- `docs/API_CONTRACTS.md` **C-1130..C-1132, C-1138** (and C-640..C-642, C-649, C-650 they amend);
  `docs/17-security.md` **SEC-150**; `docs/90-decisions.md` **D-452, D-453**.
- Code:
  - Manifest model `crates/aurora-registry/src/manifest/model.rs` (`GuideEntry` :1116, `Agent` :1137).
  - Archive paths `crates/aurora-registry/src/package.rs` (`allowed_path` :78–84, `AGENT_DIR` :96, :157).
  - Linter `crates/aurora-registry/src/validate/agent_guide_rules.rs` (whole file),
    `validate/mod.rs` :838–847, `validate/package_rules.rs` :52.
  - Snapshot `crates/aurora-registry/src/snapshot.rs` (`GuideKind` :107 … `compile_with_guides` :428).
  - Skill validation today: `services/aurora-api/src/domains/skill_libraries/content.rs`
    (`assemble` :255, `parse_skill_md` :456, `LoadedFile` :521, `finish` :645) and
    `services/aurora-api/src/domains/skills/service.rs` (`validate_name` :58 … `check_total` :137).
  - Limits `crates/aurora-core/src/limits.rs` :210–229, :438–465.
  - Ingest `services/aurora-api/src/domains/plugins/ingest.rs` (blobs :525–535).
  - Digest and signing: find the one function (`rg -n "canonical" crates/aurora-registry services/aurora-cli`).
  - CLI `services/aurora-cli/src/plugin/{mod,guide,openapi}.rs`, `templates/plugin.yaml` :31.
  - Built-ins with guides: atlassian, aws-keys, github, gitlab, google-workspace, grafana, indico, linear,
    microsoft-365, notion, postgresql, sentry (`plugins/builtin/*/plugin.yaml` `guide:` lines and
    `agent/**`); fixtures `plugins/fixtures/fixture-guide/`, `fixture-invalid-agent-guide-*`,
    `expected-invalid.txt`; `plugins/builtin/README.md`; `scripts/plugins/check_builtin.py`.

## Work

### Part A: one skill validator

1. Move the pure skill rules into **`aurora-registry::skill_format`**: name grammar and reserved prefix,
   description, `SKILL.md` frontmatter parse (`parse_skill_md`), path grammar, depth, `skill.md` rule,
   body/total/file-count/file-size checks, and `finish` (assemble a skill from `(path, bytes, executable)`
   entries). It takes plain inputs and returns typed errors; no I/O.
2. The API's library reader (`skill_libraries/content.rs`) and the skills service call it instead of
   their own copies. Their behaviour, error codes and messages stay **exactly** as today (the library
   ITs are the proof); only the code moves.

### Part B: format, digest, linter (C-1130, C-1131)

1. `package.rs`: allow `skills/<app_id>/<name>/<path>` (regular files; the existing symlink/hard-link
   refusal covers it). Keep `agent/<app_id>/<file>` allowed (the operations file, and old guide files
   that are now ignored).
2. Canonical digest: the `\0x` marker for executable entries under `skills/`, one function for build,
   sign, verify and ingest. `build` sets modes from the source file's executable bit.
3. Linter: the `app_skill_*` / `app_skills_*` rules through `skill_format`, the UTF-8 text rule, the
   prefix rule, `agent_summary_required` with skills. `agent.guide` and non-operations files under
   `agent/` → the **warning** `agent_guide_ignored` (parsed, never an error; precedent `library_ignored`,
   P5-SKLIB-1). Remove the `agent_guide_*` errors and `agent_recipe_not_markdown`, and the
   `GuideEntry.kind` handling beyond parsing.
4. Limits: add `APP_SKILLS_MAX`, `APP_SKILLS_APP_MAX`, `APP_SKILLS_SYNC_MAX`; remove the four
   `AGENT_GUIDE_*` limits (and `guide_sync_max` wiring in `services/aurora-api/src/app.rs` :121, :262,
   :565, renamed for the new budget, which -2 uses).

### Part C: snapshot and storage (C-1132)

1. Snapshot `apps[].agent.skills` as C-1132; drop `guide` from what is emitted (an old stored version
   still compiles: its guide entries are ignored). Rename `compile_with_guides` and the guide digest
   types to what they now carry (skills and the operations file).
2. Migration `20261201000272_plugin_blobs_agent_file.sql` (`agent_guide` → `agent_file`, check and rows,
   one transaction). Ingest stores every `SKILL.md`, skill file and operations file as `agent_file`
   blobs. Update `docs/03` `plugin_blobs`.

### Part D: CLI (C-1138)

`aurora plugin preview` (replaces `guide`; until -2 changes the renderer it renders the generated files
it can, and the skills as projected), `new` scaffold, `build`, `validate`, `docs`, `import-openapi`
without reference files or `guide:` entries (update `services/aurora-cli/tests/golden/openapi/**`).

### Part E: built-in and fixture packages

1. Convert every built-in guide entry into a skill under `skills/<app_id>/`:
   - a recipe `<stem>.md` → `<app_id>-<stem>` with `description` = its title (already a task), body = the
     file without the generated provenance line;
   - a reference file → `<app_id>-<topic>` (for example `indico-api-families`, `gitlab-api-conventions`),
     `description` starting with what it is for ("How the GitLab REST API is addressed, paginated and
     rate-limited; read before the first request").
   Drop the `guide:` lists; keep `summary`, `instructions`, `operations`. Bump each package's minor
   version. Indico's three files are converted the same mechanical way here; P5-APPSKILL-4 replaces them.
2. Fixtures: `fixture-guide` becomes `fixture-app-skills` (two skills, one with an executable script and a
   nested `reference/` file); replace the `fixture-invalid-agent-guide-*` set with
   `fixture-invalid-app-skill-*` cases for each new error code, and add `fixture-legacy-guide` (an old
   `agent.guide` package: valid, one `agent_guide_ignored` warning). Update `expected-invalid.txt`.
3. `plugins/builtin/README.md`: the authoring rules for app skills (what goes in the summary, the
   essentials, a skill per task, scripts in `scripts/` committed executable, no secrets, no hard-coded
   hosts: the host comes from the account in `app.md`).

## Tests move with the code (owner, 2026-11-20 — binding)

In **every commit**, find every test that sees what you change and update it in the same commit; never
`.skip`, delete or loosen a test to get green. A test is removed only with the feature it covers (list
them in the report). Start from `rg -n "agent_guide|AgentGuide|GuideEntry|GuideKind|guide_files|kind: recipe|agent_recipe|aurora plugin guide|compile_with_guides" crates services scripts plugins`.
Known callers: `crates/aurora-registry/tests/it/{agent_guide,agent_guide_lint,agent_guide_render,agent_api_json,agent_operations,main}.rs` and their snapshots, `services/aurora-api/tests/it/{plugins_agent_guide,home_agent_guides}.rs`, `services/aurora-cli/tests/openapi.rs`, the API library ITs (`tests/it/skill_library*.rs`, which must pass **unchanged** after Part A).

## Acceptance

1. **Shared validator**: every existing skill-library and skills-service test passes without edits to
   its expectations; `rg -n "fn parse_skill_md|fn validate_name" services` finds no second copy.
2. **Linter matrix** (registry tests): each `app_skill_*`/`app_skills_*` code from a minimal fixture; a
   foreign app id; `indico-tools` skill named `indico-x` refused; traversal, depth 5, `Skill.md` as a file
   name, NUL byte, BOM, 64 KiB + 1, 51 files, 17 skills; a legacy guide package valid with one warning.
3. **Digest** (SEC-150): a package without `skills/` has the same digest as on master (test against a
   built-in's recorded digest); flipping a skill file's executable bit changes the digest and
   `verify` fails; `build` of a git checkout keeps `100755` scripts executable.
4. **Snapshot**: `apps[].agent.skills` sorted, with `executable`; no `guide` key for any built-in or for
   the legacy fixture; a stored version with `agent.guide` still compiles.
5. **Migration**: rows and check moved to `agent_file`; `python3 scripts/extract-schema.py --check`.
6. **Built-ins**: every converted package validates with no warning; `python3 scripts/plugins/check_builtin.py`
   passes; no `guide:` key and no file other than `operations.json` left under any `plugins/builtin/*/agent/`.
7. **CLI**: `aurora plugin preview` on `fixture-app-skills` writes the skills with modes; the openapi
   goldens regenerate without reference files.
8. **Docs in the same commits**: `09 §2` (archive, digest), `§3.4 agent`, `§6` codes, `§9`, `§10`, `§12`;
   `22` :95–105; `02 §Limits`; `03 plugin_blobs`; `24` §3, §4, §8, §10, §12 rewritten for skills (the doc
   is renamed in -2); `docs/tests/security-coverage.md` SEC-150 row (registry part).
9. **Gates** (targeted, through `/home/alex/orca/workspaces/aurora/heavy.sh <lane> …`; CI is the full gate):
   `cargo fmt --check`; `cargo clippy -p aurora-registry -p aurora-core -p aurora-cli -p aurora-api --all-targets -- -D warnings`;
   `cargo test -p aurora-registry`; `cargo test -p aurora-cli`;
   `cargo test -p aurora-api --test it -- skill_library skills plugins`; `bash scripts/guards/run-all.sh`.

## Out of scope

- Delivering skills to agents, the tier, `app.md`, `api.json`, `aurora-agent`, the admin API and audit
  (P5-APPSKILL-2); the dashboard (P5-APPSKILL-3); new Indico content (P5-APPSKILL-4).
- Binary skill files (the projection sends text only).
- An organization switch to hide an app's skills (open question Q-APPSKILL-1).

## Completion report

Files changed, commands and results, tests removed with the guide format (each named), the digest test's
recorded value, which crate `skill_format` landed in and why, and the per-package version bumps.
