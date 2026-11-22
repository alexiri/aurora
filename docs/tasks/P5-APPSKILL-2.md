# P5-APPSKILL-2: Apps ship skills — what agents and admins get (owner, 2026-11-22)

## Goal

After P5-APPSKILL-1 a package carries skills (`skills/<app_id>/<name>/…`, C-1130) and the snapshot lists
them (C-1132). This brief puts them in front of agents and admins:

- every agent that can use an app gets the app's skills in its harness skills folder, as the **lowest
  tier** (agent > workspace > organization > app);
- the app folder `.aurora/apps/<app_id>/` keeps only what Aurora writes: `app.md` (was `guide.md`),
  `tools.md`, `api.json`, and `app.md` and `aurora-agent` point at the app's skills;
- the effective skills route and the agent's skill tools show them as tier `app`;
- instance admins see each app's skills and files (scripts marked) with change marks, and the audit
  says when they changed;
- the words "agent guide" and "recipe" leave the code, docs and API.

Same Rust lane as P5-APPSKILL-1, on its branch; **both merge together** after the SEC-150 review.
**P5-SKILLDIR (D-454, C-1140, `alexiri/skills-dir`) goes first**: it sets `SKILLS_DIR` in every agent
environment and edits `services/aurora-supervisor/src/instructions.rs`, which this brief also edits.
Rebase on it; `app.md` names skills as `$SKILLS_DIR/<name>/SKILL.md`, the same path the instruction
file's Skills section uses.

## Decisions that bind this brief

- **D-453** (c) to (h); **D-452** (b), (c).
- **SEC-150** (projection, tier, review, audit rows); SEC-91 and SEC-117..SEC-120 hold for the generated
  files and `aurora-agent`.
- Contracts **C-1133** (effective skills), **C-1134** (projection), **C-1135** (`app.md`, `api.json`,
  instruction text, `aurora-agent`), **C-1136** (admin API), **C-1137** (audit).

## Read first

- `docs/24-apps-for-agents.md` §2, §5, §6, §7, §9, §10a, §11 (as P5-APPSKILL-1 left it);
  `docs/10-agents-conversations.md` §Skills; `docs/12-supervisor-harness.md` §Home projection and §Sync
  protocol (:818–853); `docs/16-wire-protocols.md` :113–115.
- `docs/API_CONTRACTS.md` **C-1133..C-1137** (and C-643..C-648, C-652, C-653 they amend);
  `docs/17-security.md` **SEC-150**, SEC-91, SEC-117.
- Code:
  - Home sync `services/aurora-api/src/domains/hosted/home.rs`: `skills_dir` :21–31, `GuideCache` :193,
    `app_folder` :292–402, render inputs :540–549, `render` :625 (skills loop :635–655), budget loop
    :669–705, `APPS_ROOT` :609; `crates/aurora-db/src/repo/hosted.rs` `skills` :397, `skill_files` :424.
  - Skills `services/aurora-api/src/domains/skills/service.rs` (`tier_of` :156, `rank` :167,
    `EffectiveItem` :185 … `effective` :202), `routes.rs` :256, DTOs; agent tools
    `services/aurora-api/src/domains/tools/skills.rs` (`skill_list` :117, `skill_get` :189).
  - Renderer `crates/aurora-registry/src/agent_guide.rs` (`guide_md` :466, `render` :602,
    `project_file` :146) and `agent_guide/api_json.rs` (recipes :508–520).
  - Supervisor `services/aurora-supervisor/src/instructions.rs` (`apps_index` :277–317, tests :613–790,
    snapshots `…index_with_one_app.snap`, `…two_apps.snap`); `src/home.rs` (`app_file_ok` :144, the
    `Apps` arm :298); `src/agent_cli/{mod,api,show,call}.rs` (`Recipe` :51, search :206–220,
    describe :410, fallback `call.rs` :472).
  - Admin `services/aurora-api/src/domains/plugins/{service,dto,routes,lifecycle,ingest}.rs`
    (`agent_guides` :223, `guide_file` :287, route :138–148, `agent_guide_changed` lifecycle :83, :161,
    :252, :269 and ingest :457, :586).

## Work

1. **App skills per agent (C-1133).** One function returns an agent's app skills from the active snapshot
   for exactly the apps `home_inputs` lists, with app id, app name and version. `effective()` and the home
   projection both merge them **after** the database tiers, by the rules of C-1133 (other tiers win;
   longest-prefix app between apps). The home projection's own SQL stays for the database tiers; do not
   let the two merges drift: one merge function, called by both.
2. **Effective route and agent tools**: tier `"app"`, the `app` object, nullable `id` for app items; the
   OpenAPI document and `web/openapi.json` / `schema.d.ts` regenerated. Writes never reach an app skill.
3. **Projection (C-1134)**: app skills in the `skills` part with the provenance line and the `exec` flag,
   content from the blob cache (rename `GuideCache`); the `APP_SKILLS_SYNC_MAX` budget and its metric; the
   `apps` part with `app.md`, `tools.md`, `api.json` only; `guide_path` → `.aurora/apps/<id>/app.md`.
   The supervisor's `app_file_ok` accepts `app.md` (and no longer the reference-file grammar); its skills
   path handling is unchanged.
4. **Renderer (C-1135)**: rename the module to `aurora-registry::app_folder`; `guide_md` → `app_md` with
   the **Skills** section (needs the harness's skills dir and the shadowing result as inputs, so the line
   shows the real path and says when another tier's skill is used); remove the recipe and reference
   sections and `project_file`; `api.json` `skills`, `recipes: []`.
5. **Instruction file and `aurora-agent`**: the paragraph text of C-1135; `list`, `search`, `describe`
   for skills; drop recipe handling; the fallback names `app.md`. Update the snapshots.
6. **Admin API (C-1136)** and **audit (C-1137)**: `PluginPackage.agent`, the two file routes, the old
   route removed, `agent_changed` computed in one place for approve, activate and auto-approve (an
   executable-bit flip alone counts).
7. **Words**: rename `docs/24-apps-for-agents.md` → `docs/24-apps-for-agents.md` ("24 — Apps for agents:
   skills, the app folder and tools on demand") and update every reference outside `docs/history/`;
   rename code identifiers that say guide or recipe for agent content (`agent_guide*`, `GuideFile`,
   `Recipe`, …). Leave alone: the **setup guide** (`docs/setup.md`, `check_guide`, `guide_too_large`,
   `crates/aurora-registry/src/guide.rs`), the wire names `apps` and `guide_path`, the legacy-parse path
   and `agent_guide_ignored`, `recipes: []`, and "recipe" in unrelated senses (secret injection recipes,
   `propose_custom_connection {recipe}`, operator recipes).
8. **Guard**: extend `scripts/guards/agent-guide-advisory.sh` (rename it `agent-content-advisory.sh`) so
   no gateway, policy, hook-host or runner code reads `skills/` from a package or `.aurora/apps`.

## Tests move with the code (owner, 2026-11-20 — binding)

Every commit updates every test that sees what it changes; never skip, delete or loosen to get green; a
test goes only with the feature it covers (list them). Start from
`rg -n "guide\.md|agent_guide|AgentGuide|agent-guides|recipe|guide_budget" services crates tests web/src web/tests web/e2e scripts`.
Known callers: `services/aurora-api/tests/it/{home_agent_guides,plugins_agent_guide,main}.rs` (rename to
`home_app_skills.rs`, `plugins_agent_content.rs`), the skills ITs, `permission_matrix.rs` (the new admin
routes), `services/aurora-supervisor/tests/it/{home,agent_cli}.rs`, the `agent_cli` and `instructions`
snapshots, `tests/e2e-hosted/tests/hosted.rs` :2941–3003 (modes: now checks an app skill script 0555 and
`app.md` 0444), the registry render snapshots, `docs/tests/hosted-coverage.md`. Web: keep it compiling
with the new `PluginPackage.agent` and nullable effective `id` (the section itself is P5-APPSKILL-3):
`web/src/components/admin/agent-guide-section.tsx`, `plugin-detail-view.tsx`, `web/src/lib/api/plugins.ts`,
`skills-view.tsx`, `web/tests/agent-guide-section.test.tsx`, `web/e2e/agent-guide.spec.ts` (mocked fixture
shape).

## Acceptance

1. **Tier and shadowing** (API ITs): an agent with a grant on Indico gets `indico-*` skills in
   `.claude/skills` (claude-code), `.codex/skills/aurora` (codex) and `.agents/skills` (pi); an agent
   without a grant gets none; revoking the grant prunes them at the next sync; an agent-, a workspace- and
   an organization-tier skill of the same name each win (the app skill is in `shadowed` with
   `shadowed_by`); a disabled organization skill does not veto; two fixture apps `demo` and `demo-x`
   with the name `demo-x-thing` → `demo-x` wins.
2. **Projection**: the projected `SKILL.md` has the provenance line after the frontmatter; a script is
   0555, other files 0444 (supervisor IT and the hosted e2e check); over `APP_SKILLS_SYNC_MAX` the later
   apps project no skills, `app.md` says so, the metric increments.
3. **`app.md` / `api.json`**: registry snapshot tests for one app with two skills, one shadowed, for each
   harness's path; `api.json` has `skills` and `recipes: []`; an `aurora-agent` built from master parses
   the new `api.json` (test with the old struct or a recorded file).
4. **`aurora-agent`**: `search room` finds `indico skill indico-…`; `describe indico <skill>` prints the
   path; `list` shows counts.
5. **Effective route and tools**: the route returns `tier: "app"` items with `app` and `id: null`;
   `skill_list`/`skill_get` include them; `skill_update`/`skill_delete` of that name answer as for an
   unknown agent-tier skill; the effective `PATCH`/`DELETE` routes never reach it.
6. **Admin**: `PluginPackage.agent` with `changed` for a version that adds a skill, changes a file and
   flips a bit (`mode`); both file routes instance-admin only, `text/plain` + `nosniff`, 404s and the 400;
   the old route 404s; `agent_changed` on approve, activate and auto-approve, including a bit-only change.
7. **Words**: `rg -n -i "agent guide|agent_guide|guide\.md|recipe" services crates web/src docs --glob '!docs/history/**' --glob '!docs/tasks/**'`
   finds only the exceptions of Work 7 (list each remaining hit in the report).
8. **Docs in the same commits**: `24` (renamed; §2, §5, §6, §7, §9, §10a, §11, §12, §13), `09 §10`,
   `10 §Skills` (the app tier) and §Agent instructions, `12 §Home projection` / §Sync protocol, `16`
   :113–115, `05` (effective skills, admin routes), `14` :2272 (pointer to C-1139), `17` (SEC-91/117
   wording), `18` §App guides → §App skills, `00` glossary ("Agent guide" → "App skill", "App folder"),
   `15` (metric rename), `21`, `19`, `FEATURES` rows, `docs/tests/{security,hosted}-coverage.md`.
   `python3 scripts/extract-schema.py --check` and `bash scripts/guards/run-all.sh` pass.
9. **Gates** (targeted, through `/home/alex/orca/workspaces/aurora/heavy.sh <lane> …`; CI is the full gate):
   `cargo fmt --check`; `cargo clippy -p aurora-api -p aurora-registry -p aurora-supervisor -p aurora-protocol -p aurora-db --all-targets -- -D warnings`;
   `cargo test -p aurora-registry`; `cargo test -p aurora-supervisor`;
   `cargo test -p aurora-api --test it -- home skills plugins permission_matrix`;
   web: `tsc`, lint, vitest on the touched files.

## Out of scope

- The dashboard sections (P5-APPSKILL-3) beyond compiling; Indico content (P5-APPSKILL-4).
- Hiding an app's skills per organization or agent (never: owner, D-455), skills for BYO agents (no projected
  home), digest-based skipping of unchanged files.

## Completion report

**Done 2026-11-22 (lane B, branch `alexiri/p5-polish-b`, after the P5-SKILLDIR merge `98deb5e9`).** D-455 (app skills
cannot be switched off selectively) is what Q-APPSKILL-1 became; nothing here depends on a switch.

**How the one merge is shared.** `services/aurora-api/src/domains/skills/app_skills.rs`: `candidates(snapshot, app_ids)`
(the skills of the listed apps, with app id, name and version), **`merge(candidates, taken)`** (the only place that decides
tier shadowing and the longest-prefix rule between apps), `granted_app_ids` / `home::granted_connections` (the one definition of
"the apps the home lists": active connection with a grant, app available), `effective_of` (merge under an
`Effective`). The effective route and the agent tools call `effective_of`; the home projection calls `merge` with the names its
own SQL returns (`hosted::skills` now also returns the winning `tier`, so `app.md` can say which tier replaces a skill). The SQL
of the database tiers is unchanged; only the merge is shared.

**Words (Acceptance 7).** Remaining hits of `rg -n -i "agent guide|agent_guide|guide\.md|recipe"` outside history/tasks, with the reason:
`agent_guide_ignored` and the legacy parse path (kept by Work 7); the supervisor's accepted legacy name `guide.md` (rolling
upgrade); `recipes: []` in `api.json` (C-1135); the migrations `…000252_agent_guide_blobs.sql` and `…000272` (history and the rename);
registry tests `app_skills_lint.rs`/`agent_files.rs` that assert the removed codes are gone or build a legacy package; `docs/24`
(where it describes the removed format and the historical P5-GUIDE task rows), `docs/09`, `docs/18` (the superseded P5-GUIDE bullets are
kept under an amendment that says so), `docs/FEATURES.md`, `docs/PROGRESS.md`, `docs/API_CONTRACTS.md` (frozen P5-GUIDE contracts),
`docs/90-decisions.md`; unrelated senses of "recipe" in `aurora-llm` and the gateway (injection recipes). The setup guide
(`docs/setup.md`, `check_guide`, `setup_guide_md`, `guide_too_large`, `crates/aurora-registry/src/guide.rs`) is untouched.
**Incident, fixed before commit:** my first mass rename of `guide_md` → `app_md` also hit `setup_guide_md` in the connectors
code (a different field); I reverted every such file (`git diff` against master shows no change there) and regenerated
`web/openapi.json` and `schema.d.ts` afterwards.

**Renamed.** `aurora-registry::agent_guide` → `app_folder` (`GuideAccount` → `AppAccount`, `GuideWebhook` → `AppWebhook`,
`Rendered.guide_md` → `app_md`, `render*` take an `AppSkills` instead of `reference_available`), tests `agent_guide.rs` →
`agent_files.rs`, `agent_guide_render.rs` → `app_folder_render.rs` (snapshots renamed with them), `agent_guide_hashes.txt` →
`agent_files_hashes.txt`, API ITs `home_agent_guides.rs` → `home_app_skills.rs`, `plugins_agent_guide.rs` →
`plugins_agent_content.rs`, new `skills_app.rs` (module `skills::app_tier`), `docs/24-app-guides.md` → `24-apps-for-agents.md` (every
reference updated), `scripts/guards/agent-guide-advisory.sh` → `agent-content-advisory.sh`, the metric
`aurora_home_guide_budget_exceeded_total` → `aurora_home_app_skills_budget_exceeded_total`, `AppState::guide_sync_max` →
`app_skills_sync_max`, `GuideCache` → `AgentFileCache`, `plugins::get_guide_blob` → `get_agent_blob`.

**Removed with the feature.** API `home_agent_guides::the_budget_keeps_generated_files_and_drops_reference_files` (replaced by
`skills::app_tier::over_the_budget…`), the guide-file assertions of `the_folder_matches_the_render_inputs_and_the_manifest`
and `grant_changes_move_the_generation_and_the_folder` (they now check the generated files only), supervisor `home.rs` accepted
`rest-api_v4.md` (now a refused path, by design), the `recipe` kind in `show.rs` and its tests (now skills).

**Security notes (SEC-150, agent side).**
- *Never overrides a higher tier:* `merge` puts every app skill whose name an enabled skill of another tier holds into `shadowed` and
  never projects it (`another_tier_always_wins_and_a_disabled_skill_does_not_veto`: agent, workspace and organization each win;
  a disabled organization skill does not veto; unit test in `app_skills.rs`). A skill of another app of the same name loses to the
  longest-prefix app (`between_apps_the_longest_prefix_of_the_name_wins`).
- *Only agents that can use the app:* the candidates come only from the apps of the agent's own granted, available connections
  (`an_agent_with_a_grant_gets_the_app_skills…` also asserts an agent without a grant has no byte of the app in any part, and that a
  revoked grant prunes them at the next sync).
- *Provenance and rebuilt frontmatter:* the projected `SKILL.md` is built by `project_skill_md` (own `name`/`description`, the
  third-party provenance line, the validated body; the package's other frontmatter keys never reach the home — `a_projected_app_skill_carries_the_provenance_line`).
- *Modes:* a file with the bit is sent with `exec: true` and the supervisor installs `0555`, others `0444`
  (`…_in_every_harness_folder…` checks the part entries; the e2e checks the real modes; supervisor tests pin the modes).
- *Paths:* built only from validated segments (skill name from the snapshot grammar, file paths from the shared path grammar) under
  the harness skills dir, which is the API's own constant; the supervisor's managed-root rules re-validate and prune.
- *Instruction file:* nothing app-supplied beyond the one-line summary, as D-453 allows; `app.md` carries names and descriptions
  only through the one-line sanitizer (`hostile_skill_text_stays_on_its_line`), and skill bodies are never inlined anywhere.
- *Not executed:* no platform component runs a script; the new guard `agent-content-advisory.sh` fails if the gateway, policy engine,
  hook host or runner mention `.aurora/apps`, `skill_format`, `SKILLS_DIR`, `SkillEntry` or `agent_files`.
- *Budget:* the 2 MiB limit is enforced in `app_id` order with the metric (`over_the_budget…`).
- As in -1 I did not run each regression test red first by deleting its rule; each asserts an exact outcome of the rule.

**Tests changed (reviewed, not regenerated blindly):** supervisor insta snapshots `index_with_one_app`, `index_with_two_apps` (the
paragraph says `app.md` and "the app's skills"), `list_apps`, `list_app`, `search_book_room` (skill instead of recipe); registry
`api_json_*` snapshots (`"skills": []` added) and the `app_folder_render__*` snapshots (the `guide.md` header became `app.md`);
`tests/e2e-hosted` s14 (new package and checks; compiled only, it needs the kind stack), `permission_matrix.rs` (two routes), web
`agent-guide-section.test.tsx` / `e2e/agent-guide.spec.ts` (new `PluginPackage.agent` shape), `skills-view.tsx` (app items filtered until -3).

**Web.** `PluginPackage.agent`, the two file routes and the nullable effective `id` compile; the section keeps its form (skills and
the operations file as rows, scripts marked, `Script bit` mark) and P5-APPSKILL-3 gives it the final one; the effective view
lists database tiers only until -3 adds "From apps".

**Database server of the ITs.** Every IT ran against my own container `pg-p5-polish-b` (`127.0.0.1:55611`) via
`AURORA_TEST_DATABASE_URL`; the worktree's `.env` pointed `DATABASE_URL` at `:55432` (the dev DB) until you changed it, but
the testkit reads `AURORA_TEST_DATABASE_URL` first, so no test touched `:55432` (compile-time sqlx checks may have read `.env`; they are read-only).

**Commands and results.** See the worker report: `cargo fmt`, `cargo clippy -p aurora-api -p aurora-registry -p aurora-supervisor -p aurora-protocol -p aurora-db -p aurora-cli -p aurora-core --all-targets -- -D warnings`,
`cargo test -p aurora-registry`, `-p aurora-supervisor`, `-p aurora-cli`, `-p aurora-api --test it -- home skills plugins permission_matrix openapi schema`,
`cargo check -p aurora-e2e-hosted --tests`, `python3 scripts/extract-schema.py --check`, `bash scripts/guards/run-all.sh`, web `tsc`, eslint and the touched vitest files.
