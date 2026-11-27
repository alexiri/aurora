# P5-SKILLS-1: Skills backend: display names, turning an inherited skill off for one agent, usage, and what a library can offer (owner, 2026-11-27)

## Goal

The Skills & Memory review (D-496) asks for skill pages the API cannot feed today:

- skills are shown under two spellings because only the folder slug exists ("Summarize pr" / `summarize-pr`, S-1, S-2);
- an organization or workspace skill cannot be turned off for one agent (S-6);
- nothing says whether a skill is ever used (S-12; "Used 3 times this week · last in …", "Used by 14 agents");
- the library picker cannot say how many skills a library offers, or which ones cannot be added to *this* agent and
  why (S-10, S-11), and the dashboard hard-codes the size budget.

This brief adds that: **server, API, supervisor report and projection only**; the pages are P5-SKILLS-2..4.

Decisions: **D-496**, **D-498** (display name; description as a sentence), **D-499** (off for one agent; off only; app
skills excluded), **D-505** (what a use is; the lines shown; 90 days). Contracts: **C-1173, C-1174, C-1175, C-1176**.
Security: **SEC-158** (add its row to `docs/17-security.md`: only people who may edit an agent can turn a skill off for
it, only for skills that reach it — 404 otherwise, never revealing other tenants' skills; usage lines name a
conversation only to its readers; a supervisor-reported use counts only for the agent and conversation of that turn).

## Linked requirements

- `docs/design/skills-memory/README.md`, `review.md` §Skills: S-1, S-2, S-6, S-10, S-11, S-12 and proposals skill-1,
  skill-2, skill-3, skill-5, skill-6. Look at `after/skill-1.png` (switch column "For Daneel", chips "Off 1", "Daneel's
  version replaces the workspace's · Compare"), `after/skill-2.png` (the usage line), `after/skill-3.png` ("Managed by
  Acme's admins. You can turn it off for Daneel."), `after/skill-5.png` (libraries with skill counts; "8 can't be added ·
  Why?"), `after/skill-6.png` ("Used by 14 agents").
- Contracts C-1173..C-1176; `docs/10-agents-conversations.md` §Skills (~968) and §Git skill library (~1009);
  `docs/03-data-model.md` §7 (`skills` ~1223, `skill_libraries` ~1265); `docs/05` skill rows (~895) and §Skill
  libraries (~916); `docs/16-wire-protocols.md` (the supervisor's `tool.started` report); C-330..C-335 (skills), C-570..,
  C-1122..C-1128 (libraries), C-1133 (app tier), D-455.

## Wave and order

First of the skills set, in parallel with P5-MEMORY-1 and P5-SKILLS-2. Migrations `20261201000289` (title, off table)
and `20261201000290` (`skill_uses`); if a guard demands ascending order at merge time, the second to merge renumbers.
**Security review before merge** (SEC-158), by the coordinator. The conversation-reference helper ("name a
conversation only to its readers") is written by P5-MEMORY-1: if it has merged, use it; if not, write it in the shared
place agreed in that brief and tell the Orchestrator, so the second lane reuses it.

## Context (what exists today)

- Domain `services/aurora-api/src/domains/skills/` (`routes.rs`, `service.rs`: `tier_of`/`rank` ~152–169,
  `effective()` ~198–249, `overridden_in` ~252–269, files ~529–575; `library.rs`; `app_skills.rs`), tools
  `domains/tools/skills.rs` (`skill_list`, `skill_get`, `skill_create/update/delete`), repo
  `crates/aurora-db/src/repo/skills.rs` (`list_reaching` ~129, `bump_agent` ~341), projection
  `crates/aurora-db/src/repo/hosted.rs:~416-438` (`DISTINCT ON (name) … WHERE enabled` — **its own rule, not
  `service::effective`**) consumed by `domains/hosted/home.rs:~750-777`, eval fingerprint `domains/evals/service.rs:~596`.
  Libraries: `domains/skill_libraries/` (`dto.rs`, `service.rs`, `routes.rs:~310-386` browse, `content.rs` reasons,
  `refresh.rs`), `skills/library.rs:66` (budget).
- `skills.name` is the slug (≤64); there is no title. `skills.enabled` is per row. A disabled skill never vetoes a lower
  tier; the first enabled row of a name wins, others are `shadowed_by`.
- Usage: nothing records it. Agents load skills natively from `<skills dir>/<name>/SKILL.md` (`home.rs:24-35`,
  `SKILLS_DIR`) or through `skill_get`. The Claude Code adapter's `summarize()` (`services/aurora-supervisor/src/adapters/claude_code/stream.rs:85-93`)
  drops the skill name; `tool.started` is persisted in `domains/hosted/reports.rs:~478-486`.
- Limits `crates/aurora-core/src/limits.rs:210-217`; path rules `crates/aurora-registry/src/skill_format.rs:17-64`.

## Scope

1. **Display name (C-1173, D-498).** `skills.title` (nullable, ≤80, plain text, trimmed; no newline) in
   `20261201000289`; accepted on create and update (agent, workspace, organization tiers; refused 409
   `library_skill_readonly` on library skills as today); returned everywhere `Skill`/`EffectiveSkill`/library items
   appear; carried by export, import and templates when set (amend their contracts in place). The agent's
   `skill_create`/`skill_update` tools accept an optional `title`.
2. **Off for one agent (C-1174, D-499).** Table `agent_skills_off` (same migration), routes `PUT|DELETE
   /v1/agents/{agent}/skills/{skill_id}/off`, permission = may edit the agent, target = a workspace or organization
   skill that reaches the agent (incl. library-added ones), 409 `skill_not_inherited` for the agent's own or an app
   skill, 404 for anything that doesn't reach the agent. **One rule for the effective set:** apply the override in
   `service::effective` **and** in the projection (best: make the projection call the same function, or share one SQL
   rule; prove both agree with a test over agent/workspace/org/app rows with the override set). Off means the skill is
   treated as not enabled **for that agent only**: a lower tier's skill of the same name may then apply, exactly as if
   the row were disabled. `EffectiveSkill.off_for_agent`, shadow reason `off_for_agent`; skill tools and fingerprint
   honour it; `bump_agent` so the home re-syncs; audit `skill.agent_off`/`skill.agent_on` with the skill and agent.
3. **Usage (C-1175, D-505).** `skill_uses` in `20261201000290`, pruned after 90 days by the existing retention sweep
   (or a new step in it). Writers: `skill_get` (body read; not a file read); the supervisor — add the optional `skill`
   field to `tool.started` (`16`) and set it in each adapter where the harness's own skill loading is visible: Claude
   Code's `Skill` tool (its argument), and a read of `<skills dir>/<name>/SKILL.md` by any read tool; do the adapters
   whose stream shows it (Codex, Pi, goose, OpenCode) and list in the report those that cannot. The API maps the
   folder name to the skill that reaches the agent **at that moment** (after overrides) and writes one row per turn and
   skill at most. Readers: `EffectiveSkill.usage { week, last { at, conversation } }` (conversation through the
   readers-only helper) and `Skill.used_by_agents` (distinct agents, 30 days) on workspace and organization lists.
4. **Libraries (C-1176).** Store and return `skills_offered` / `skills_unavailable` from the last check/refresh; browse
   items get `blocked` for the requested tier (`name_taken` when a skill of that folder name already exists at that
   tier, `budget` when adding it would exceed the tier's budget, `invalid` with the existing reason) and the response
   `budget { bytes, used }` from the server's real limit (`library.rs:66`, the `bytes_per_agent` setting).
5. **Regenerate client types:** `web/openapi.json` via the foundation test's `UPDATE_OPENAPI=1` mode
   (`services/aurora-api/tests/it/foundation.rs:~545`, through `scripts/dev/test.sh`), then `cd web && pnpm gen:api`;
   keep today's pages compiling and green.

## Out of scope

Every page (P5-SKILLS-2..4); app skills switching (D-455); per-agent **on** for a disabled skill (D-499(b)); memory.

## Constraints / file ownership

Owns `domains/skills/**`, `domains/tools/skills.rs`, `domains/skill_libraries/**` (data fields only), `repo/skills.rs`,
`repo/hosted.rs` (the skills query), the supervisor adapters' `tool.started` skill field, `reports.rs` (the usage write),
the two migrations. Agent-facing text (tool results and errors) states facts in plain words and never cites docs, D-,
C- or FIX- ids (D-483). Own worktree; `DATABASE_URL` never on port 55432; no co-author lines.

## Acceptance criteria

- With the review's data, `GET /v1/agents/{daneel}/skills/effective` shows titles where set, `off_for_agent` after
  `PUT …/off` on "Onboarding new hire", and Daneel's projected skills folder no longer contains it (IT test on the
  projection) while another agent of the workspace still has it.
- A `skill_get` and a Claude Code `Skill` tool call each produce one `skill_uses` row; `usage.week` and `last` show for
  the agent; `used_by_agents` counts distinct agents; a non-reader sees `conversation: null`.
- Browsing a library for Daneel returns `blocked` for a name Daneel already has and for an over-budget skill, and the
  real budget; libraries return `skills_offered`.
- `web/openapi.json` and `schema.d.ts` regenerated; CI's drift check passes.

## Tests

IT tests in `services/aurora-api/tests/it/skills*.rs` and `skill_library*.rs` (title round trip, export/import, the off
matrix incl. permissions and 404/409, effective = projection, usage writes and privacy, library fields), supervisor
adapter unit tests for the `skill` field, a migration test. Database tests use `aurora_testkit::TestDb`.

## Tests move with the UI (owner, 2026-11-20 — binding)

1. Before each commit that changes an API field, a route, an error code or message, a tool result or the wire report:
   `rg -n "<old>" web/tests web/e2e scripts/e2e services crates tests` and update every hit **in the same commit**
   (Rust IT tests asserting shapes, supervisor tests, `web/e2e/support/mock-api.ts` and `support/skill-libraries.ts`
   mocks, `web/openapi.json`, `tests/e2e-hosted/**`).
2. Never loosen an assertion to accept old or new; never skip, `fixme` or delete a test to get green.
3. Run tests **only** through `scripts/dev/test.sh` (D-491): `scripts/dev/test.sh <lane> rust -p <crate>` — the
   **whole** test set of every crate you touched or whose generated schema you changed (aurora-api, aurora-db,
   aurora-supervisor, …); web files you touched with `scripts/dev/test.sh <lane> web vitest <files>`. Never `cargo
   test` by hand, never your own Postgres, never port 55432; other heavy commands through `scripts/dev/heavy.sh`.
4. Production specs: update by reading; list them. 5. A merge that turns master red with a stale test comes back here.

## Docs to update

`API_CONTRACTS.md` C-1173..C-1176 (final names, in place) and the amended C-330..C-335/C-1122.. entries; `05`, `03` §7,
`10` §Skills, `16` (`tool.started.skill`), `17` (SEC-158), PROGRESS and FEATURES rows at every commit.

## Suggested agent/model and reason

Sonnet `rust-developer` (data model, two effective-set paths to unify, supervisor adapters); the coordinator reviews
SEC-158 on Opus before merge.

## Completion report

Commits; final field names; how the projection and the effective set now agree; which adapters report skill use and
which cannot; the privacy and permission matrices tested; tests changed; commands and results.
