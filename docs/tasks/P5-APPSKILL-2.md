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

- `docs/24-app-guides.md` §2, §5, §6, §7, §9, §10a, §11 (as P5-APPSKILL-1 left it);
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
7. **Words**: rename `docs/24-app-guides.md` → `docs/24-apps-for-agents.md` ("24 — Apps for agents:
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
- Hiding an app's skills per organization or agent (Q-APPSKILL-1), skills for BYO agents (no projected
  home), digest-based skipping of unchanged files.

## Completion report

Files changed, commands and results, every test renamed or removed, the remaining `rg` hits of
Acceptance 7 with their reason, and how the one merge function is shared by `effective()` and the
projection.
