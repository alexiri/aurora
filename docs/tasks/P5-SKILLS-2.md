# P5-SKILLS-2: Skills have a home at each level: Manage <org> › Skills with Libraries as cards, Administration › Skills, and visible to their owners (owner, 2026-11-27)

## Goal

The review found skills hard to find and inconsistent (S-8, S-9, S-11, S-13): the organization's skills are a tab under
Manage <org> › Agents shown only with "Show more controls" (even to the owner), the workspace Skills page is missing
from its own owner's rail, libraries are a table that scrolls sideways at 1256 px with its actions off-screen, and the
instance's skill settings sit at the bottom of a 3,100 px "Defaults" page. Implement the review's **skill-6** structure
and "Also" items 1 and 2:

- **Manage <org> › Skills**, its own sub-nav entry, with tabs **Skills · Libraries**;
- **Libraries as cards** with their state and actions visible;
- **Administration › Skills** for the instance's libraries and the library settings;
- the workspace's **Skills** entry and Manage <org> › Skills **visible to their owners** without "Show more controls".

The **list** inside the Skills tab (and the workspace page) becomes the shared list of P5-SKILLS-3; this brief moves
today's `SkillsEditor` into its new home unchanged, so the two can run in parallel.

Decisions: **D-496**, **D-506** (a)–(c). Contract: **C-1177** (routes and redirects), C-1176 (`skills_offered`, used
for the cards when P5-SKILLS-1 has merged; until then show the counts the API has and leave a `TODO(P5-SKILLS-1)` only if
the field is missing — the verifier checks it is gone).

## Linked requirements (look at every image before coding)

- `docs/design/skills-memory/README.md` (binding), `review.md` S-8, S-9, S-11, S-13, proposal skill-6 and "Also" 1–2.
- **After:** `after/skill-6.png` / `-light.png` / `.html` — two frames: the **Skills** tab ("Skills · What every agent
  in Acme can do, and where it comes from." · **Add skill** · tabs Skills · Libraries · the shared list) and the
  **Libraries** tab ("Libraries are git repositories of skills. Add one, then pick the skills Acme should use." · **Add a
  library** · cards: label, repository, state chip "Update available"/"Up to date", "Branch main", "Checked 2 min ago",
  "17 skills · 1 used by Acme", "Pinned e4bb5d9", **Check now**, **Edit**, "…"). The Manage Acme sub-nav in the mockup
  shows the new **Skills** entry between Agents and People; the left rail is illustrative (README rule 2).
- **Before:** `before/S10-org-skills.png`, `S09-org-agents-default.png`, `L01-libraries-empty.png`,
  `L03-library-checked.png`, `L04-library-added.png`, `L05-library-row-scrolled.png`, `L09-org-skills-with-library.png`,
  `S07-ws-skills.png`, `A01-admin-skill-libraries.png`, `A02-admin-agents-skills.png`.
- `docs/14-web-dashboard.md` §Visual design, `CardGrid` (~372, ~413, D-439), org Skills and Libraries (~1766–1812),
  workspace Skills (~2199), Administration skills group (~1512), altitude rows (~2526–2529).

## Wave and order

No dependency: can start at once, in parallel with P5-SKILLS-1 and P5-MEMORY-1. P5-SKILLS-3 later replaces the list
inside these pages; P5-SKILLS-4 replaces the add-from-library dialog.

## Context (what exists today)

- Org Skills: `web/src/components/skills/org-skills.tsx` (`OrgSkills`), mounted as a tab in
  `web/src/components/org/org-topics.tsx:~118` (`AdminOrgAgents`), shown only at Expert altitude
  (`ALTITUDE_REGISTRY["org.skills"]`, `web/src/lib/altitude-registry.ts:~78`); tab ids `TABS.orgAgents` in
  `web/src/lib/routes.ts:32`. Manage sub-nav: `ORG_TOPICS`/`ORG_MEMBER_TOPICS` (`web/src/lib/api/roles.ts:47-49`),
  `ManageFrame` (`web/src/components/org/manage-frame.tsx`). Legacy redirects: `web/src/lib/legacy-routes.ts:142-145,
  191-192`.
- Libraries page: route `web/src/app/o/[org]/agents/skills/libraries/page.tsx`, `web/src/components/skills/libraries-page.tsx`,
  `libraries/libraries-table.tsx` (Check now, Edit, Pause/Resume, Delete, Browse), `add-library-dialog.tsx`,
  `edit-library-dialog.tsx`, `library-status.tsx`, `words.ts`, `instance-libraries-page.tsx` (instance, routed from
  `web/src/app/admin/skill-libraries/page.tsx`).
- Workspace Skills: `web/src/app/o/[org]/w/[ws]/skills/page.tsx`, `workspace-skills.tsx`; rail entry from
  `visibleTopics` (`roles.ts:55-79`: Use altitude shows only Overview, Agents, Approvals), `ALTITUDE_REGISTRY["rail.skills"]`
  (setup, `altitude-registry.ts:22`), `WorkspaceNav` (`app-shell.tsx:274-300`).
- Administration: `/admin/internet-access?tab=defaults` renders `AgentsDefaultsView` then `SkillsSettingsView bare`
  (`web/src/components/admin/admin-topics.tsx:~108-135`; `admin/settings/operational-settings-views.tsx`:
  `SkillLibrariesRow` ~100, `SKILLS_PAGE` ~122, `SkillsSettingsView` ~390).

## Scope

1. **Manage <org> › Skills.** New sub-nav entry **Skills** (after Agents) for owners and admins, route
   `/o/<org>/skills` with tabs **Skills** (default) and **Libraries** (`?tab=libraries`); remove the Skills tab from
   Manage › Agents; redirects from `/o/<org>/agents?tab=skills`, `/o/<org>/agents/skills/libraries`, and the legacy
   `/o/<org>/skills/libraries`. Header and lead text as drawn; **Add skill** in the header (the existing create flow
   until P5-SKILLS-3).
2. **Libraries tab as cards** (`CardGrid`, `Card`): per library the label, repository (`host/path`), one state chip
   (Up to date · Update available · Paused · Needs attention with the reason · Over budget), Branch, "Checked <when>"
   (`When`), "**n skills · m used by <Org>**" (offered from C-1176 when present; used = skills added from it in the
   organization), "Pinned <short sha>" when the organization's skills from it share one commit (else "Pinned at n
   commits"), actions **Check now**, **Edit**, and "…" (**Browse skills**, **Pause**/**Resume**, **Delete…** with today's
   confirm text). Empty state with **Add a library**. No table, no sideways scroll at any width; at 390 px one card per
   row. The instance's shared libraries keep their own section ("Shared with every organization") as cards too.
3. **Administration › Skills.** A new entry in Administration's navigation, route `/admin/skills`: the instance's
   libraries (the instance libraries page, as cards like scope 2) and the **library settings** (on/off, refresh interval,
   size cap — today's `SkillsSettingsView` with the SaveBar); remove that block from the Defaults page and turn
   `SkillLibrariesRow` into a link to `/admin/skills` if anything still needs it; redirect `/admin/skill-libraries`.
4. **Who sees Skills (D-506(b)).** The workspace's **Skills** rail entry shows for the workspace's owners (team and
   personal workspaces) at the Use altitude; Manage <org> › Skills shows for owners and admins without "Show more
   controls"; members keep today's view; Tests and other expert pages keep the switch. Update the altitude registry rows
   and `visibleTopics`.
5. **The workspace Skills page** uses the same page frame as the org page (header, lead "Skills every agent in <Workspace>
   can use", Add skill) around today's list; P5-SKILLS-3 replaces the list.

## Out of scope

The list, the side panel and the editor (P5-SKILLS-3); the add-from-library dialog (P5-SKILLS-4); any API change.

## Constraints / file ownership

Owns `org-skills.tsx`, `org-topics.tsx` (the tab removal), `manage-frame.tsx`/`roles.ts` (topics), `routes.ts` (tab
ids, builders), `legacy-routes.ts`, the libraries page and `libraries/**` components (table → cards), the new
`/o/[org]/skills` and `/admin/skills` routes, `admin-topics.tsx` and `operational-settings-views.tsx` (the move),
`altitude-registry.ts` (`rail.skills`, `org.skills` rows), `workspace-skills.tsx` (frame only). Do not touch
`skills-view.tsx` or `library.tsx` (P5-SKILLS-3/-4). Tokens and shared primitives only; both themes; 390 px.

## Acceptance criteria

- As Alice (owner of Acme), Manage Acme shows **Skills** with no switch; both tabs match `after/skill-6.png` /
  `-light.png` at 1256 px (list content aside); Libraries are cards with every action visible at 1256 px and 390 px.
- As Bob (owner of his workspace), the rail shows **Skills** with "Show more controls" off.
- `/admin/skills` holds the instance libraries and settings; the Defaults page no longer does.
- Every old URL redirects; no dead link remains (`rg` the old paths).

## Tests

Vitest: topics per role and altitude (owner/admin/member, team/personal), redirects, cards (states, counts, actions),
the admin move. Playwright (mocked): Manage › Skills both tabs, Administration › Skills, at 1256 and 390 px in both
themes, axe clean; update `stage4-routes.spec.ts`/`stage4-axe.spec.ts` route lists.

## Tests move with the UI (owner, 2026-11-20 — binding)

1. Before each commit that changes visible text, a label, a heading, a route or `?tab=`, a nav entry or structure:
   `rg -n "<old text>|agents/skills/libraries|tab=skills|skill-libraries|<old selector>" web/tests web/e2e scripts/e2e`
   and update every hit **in the same commit**: `web/tests/skill-libraries-ui.test.tsx`, `workspace-skills.test.tsx`,
   `refine-20-batch3.test.tsx`, `admin-settings-operational.test.tsx`, `visible-topics.test.ts`, `agent-sections.test.ts`;
   `web/e2e/skill-libraries.spec.ts` and `support/skill-libraries.ts`, `altitude.spec.ts`, `ia-navigation.spec.ts`,
   `ia-3.spec.ts`, `stage4-routes.spec.ts`, `stage4-axe.spec.ts`, `support/mock-api.ts` (route lists ~944–987, admin
   settings ~526–552); production specs.
2. New expectations use the mockup's words exactly; never loosen, skip, `fixme` or delete a test to get green.
3. Run through `scripts/dev/test.sh` only (D-491): vitest files you changed and those rendering what you changed; this
   brief changes the app shell's navigation, so the **whole** vitest suite once (`scripts/dev/test.sh <lane> web
   vitest`); every Playwright spec visiting a changed page (`--project=mocked`); lint, typecheck, build through
   `scripts/dev/heavy.sh <lane> …`.
4. Production specs: update by reading; list them. 5. A merge that turns master red with a stale test comes back here.

## Docs to update

`14` (Manage <org> › Skills, Libraries as cards, Administration › Skills, altitude rows, routes),
`API_CONTRACTS.md` C-1177 (as built), PROGRESS and FEATURES rows at every commit.

## Suggested agent/model and reason

Sonnet `nextjs-developer` (navigation, routes and a table-to-cards rebuild across several files).

## Completion report

Commits; screenshots (1256 dark/light, 390) beside `after/skill-6*.png`; differences with reasons; redirects added;
tests changed and why; commands and results.
