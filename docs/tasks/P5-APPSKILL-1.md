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

- `docs/24-apps-for-agents.md` §3, §4, §10, §12 (what is being replaced), `docs/09-app-plugins.md` §2 (archive,
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
- An organization switch to hide an app's skills: never (owner, D-455).

## Completion report

**Done 2026-11-22 (lane B, branch `alexiri/p5-polish-b`).** Q-APPSKILL-1 is settled as D-455 (app skills
cannot be switched off selectively): nothing in the brief changed.

**Where things landed.** `aurora-registry::skill_format` (registry, because the linter, the CLI and the API
all depend on it and it needs no I/O; it carries `check_name`, `path_ok`, `is_text`, `content_size`,
`clean_text`, `parse_skill_md[_typed]`, `check_content` and a typed `SkillError{kind,message}`). The API's
`skill_libraries/content.rs` (`finish`, `parse_skill_md`, `clean_text`) and `skills/service.rs`
(`validate_name`, `validate_path`, `content_size`) are thin wrappers: `rg "fn parse_skill_md|fn validate_name" services`
finds only the wrapper `validate_name` (it maps the shared result to `ApiError`; the grammar is not repeated).
Registry: `package.rs` (`skills/` paths, `Package.executable`), `digest.rs` (`\0x`), `builder.rs`
(`set_file_exec`), `validate/agent_rules.rs` (was `agent_guide_rules.rs`), `snapshot.rs` (`AgentFiles`,
`agent_files`, `compile_with_agent_files`, `SkillEntry`), `agent_guide.rs` (`project_skill_md`).

**Done beyond -1's strict text, because the types changed under it** (so the commit compiles and its tests
pass): the admin API of C-1136 (`agent` on `PluginPackage`, the two file routes, `agent_changed` audit key,
C-1137) and the `APP_SKILLS_SYNC_MAX` rename in `AppState`; `home.rs` no longer projects guide files (the
generated files stay; -2 adds the skills). The `guide.md` renderer lost its Recipes and reference lists;
`api.json` `recipes` is always `[]` (as C-1135 says). `reference_available` is still a parameter, unused until
-2 (marked in the code).

**Linter.** New codes as C-1130. Operations-file codes renamed because the `agent_guide_*` family is gone:
`agent_operations_file_name`, `agent_operations_reserved_name`, `agent_operations_missing_file`,
`agent_operations_not_text`, and `agent_unknown_app` (a file under `agent/<app>/` of an app the package does not
have). `agent_operations_listed` went with `agent.guide`. A missing `SKILL.md` in a skill folder is reported as
`app_skill_frontmatter`. `agent_guide_ignored` comes from `validate::warnings` (manifest) and
`validate::package_warnings` (also stray files under `agent/`); `aurora plugin validate` prints the latter.
An app id with an underscore cannot ship skills (a skill name has none): `aurora plugin new` says so and
scaffolds none for it; recorded in docs 22/24 and the README.

**Digest test.** `a_package_without_skills_keeps_its_digest_byte_for_byte`: `plugins/builtin/caldav` digest
`ccaa1428bb2ec78be4d1d5f693ab93f4cf845e0b03a13d27687cfda53d014b36`, computed on master's blobs by an
independent Python implementation of `09 §2` (not by the Rust code under test).

**Per-package version bumps** (minor): atlassian 1.4.1→1.5.0, aws-keys 2.3.0→2.4.0, github 2.4.2→2.5.0,
gitlab 1.4.3→1.5.0, google-workspace 1.4.0→1.5.0, grafana 1.0.0→1.1.0, indico 1.4.1→1.5.0, linear 1.3.0→1.4.0,
microsoft-365 1.3.0→1.4.0, notion 1.2.0→1.3.0, postgresql 1.4.0→1.5.0, sentry 2.3.1→2.4.0. 20 skills created from
the 20 guide entries (`git mv`, history kept), `guide.md` → `app.md` in their text; descriptions of reference
skills hand-written (recipes use their title). `scripts/plugins/check_builtin.py target/debug/aurora plugins/builtin/*/`: every package ok.

**Security notes (SEC-150, registry and ingest part).**
- *Bounds on third-party skill text and scripts:* the shared validator: name grammar and `aurora` prefix
  (`app_skill_name`), the name must equal the app id or continue it with `-` and the longest matching app id
  of the package must be the folder's (`the_longest_matching_app_id_owns_the_skill`); path grammar `[A-Za-z0-9._/-]`,
  ≤4 segments, ≤200 chars, no `.`/`..`/empty segment, no `skill.md` segment in any case (`app_skill_path`); ≤50 files per
  skill, each ≤64 KiB, body ≤24000 and body+files ≤32000 characters, ≤16 skills and ≤256 KiB per app; archive entries are regular
  files only (symlink and hard link → `archive_link`, `skills/` shape is checked in `allowed_path`, other
  shapes `path_unknown`); files must be valid UTF-8 without NUL or BOM (`app_skill_not_text`); only `SKILL.md`'s
  frontmatter is parsed (anchors/aliases refused, YAML bounded); the projected frontmatter is rebuilt from `name` and `description`
  only (`project_skill_md`), never copied.
- *Executable bit:* recorded by the archive reader for entries under `skills/` only, hashed as `\0x` in the one
  `digest` function; flipping it either way gives `digest_mismatch`.
- *Provenance and tiering:* `project_skill_md` prepends the third-party provenance line (one line whatever the app
  name/version hold); the tier rule (an app skill never overrides a higher tier) and the instruction file are -2.
  Nothing app-supplied reaches the instruction file in -1 (guide files are no longer projected at all).
- *Storage/serving:* ingest stores only `SKILL.md`, skill files and the declared operations file as `agent_file`
  blobs, none for an `invalid` package, none for old guide files; the admin file route serves only paths the
  stored archive listed for that skill, by digest, `text/plain`, `nosniff`, attachment, `no-store`, instance admins only,
  path grammar 400.
- *Tests that fail without each rule:* name/prefix/longest-prefix/frontmatter/path/text/size/count/per-app limits:
  `app_skills_lint.rs` (each case asserts the exact code, so removing the rule fails it); digest: `app_skills_package.rs`
  (`an_executable_skill_file_adds_x_to_its_digest_line`, `flipping_the_executable_bit…`, `the_reader_records_the_bit_of_skill_files_only`,
  `a_symlink_or_hard_link_under_skills_is_refused`); ingest: `plugins_agent_guide.rs`
  (`a_package_that_breaks_a_skill_rule_is_invalid_with_the_code` stores no blob, `an_old_package_with_a_guide…` stores none,
  `the_admin_routes_serve_the_skills` incl. the 404 for `plugin.yaml` and the 400s, `a_new_version_reports_what_changed…`
  with a mode-only change). I did not run the "seen failing first" step by temporarily removing each rule; the assertions are exact-code
  matches on inputs that only that rule catches (stated honestly).

**Tests changed.** Removed with the guide format: registry `agent_guide_lint.rs` (replaced by `app_skills_lint.rs`),
`agent_guide_render::reference_files_left_out_by_the_budget_are_not_listed` and `projected_reference_files_carry_the_third_party_header`
(the latter replaced by `a_projected_app_skill_carries_the_provenance_line`), `agent_operations::…_not_also_a_guide_file…` (now
`the_operations_file_is_named_and_present`, the `agent_operations_listed` assertion went with `agent.guide`), snapshot `it__agent_guide_render__budget.snap`;
API `home_agent_guides::the_budget_keeps_generated_files_and_drops_reference_files`, `the_admin_routes_serve_the_guide`,
`a_new_version_reports_what_changed_and_audits_it` (the last two moved into `plugins_agent_guide.rs` on skills; the budget test returns
for skills in -2); CLI `guide_renders_the_fixture_package_to_the_golden_directory` (→ `preview_renders…`), `tests/golden/guide/`, the
`operations.md`/`ops-*.md` openapi goldens. Changed with review: 8 insta snapshots (only the Recipes and reference-file lines and `api.json`
`recipes` went), `builtin_versions.rs`, `plugins_supersede.rs`, `plugins_upgrade.rs` (new versions, skills in the archive),
`openapi.rs`, `agent_guide.rs`, `agent_api_json.rs`, `webhook.rs`.

**Commands.** `cargo fmt`; `cargo clippy -p aurora-registry -p aurora-core -p aurora-cli -p aurora-api -p aurora-db --all-targets -- -D warnings`;
`cargo test -p aurora-registry` (216 ok); `cargo test -p aurora-cli` (all ok, `aurora-gateway` rebuilt first: `check-snapshot` reads the new snapshot);
`cargo test -p aurora-api --test it -- plugins_agent_guide home_agent_guides skill plugins schema` against the throwaway `pg-p5-polish-b`;
`python3 scripts/extract-schema.py --check`; `bash scripts/guards/run-all.sh`; `scripts/plugins/check_builtin.py`. Results are in the worker report.
Not run (CI is the gate): `scripts/fixtures/build.sh` (the new fixtures and `mkfixture.py` modes are unexercised locally), e2e, web.

**Open / left for -2 and -3.** `tests/e2e-hosted` (`fixture-guide`) and the web admin (`agent_guides` in `web/src/lib/api/plugins.ts`,
`agent-guide-section.tsx`, `web/tests/agent-guide-section.test.tsx`, `web/e2e/agent-guide.spec.ts`, `web/openapi.json`, `schema.d.ts`) still
reference the old shapes; the openapi/schema regeneration waits for -2 where the API contract is complete.
