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

Built on `alexiri/p5-skm-web`, 2026-11-27. Web only: no API, route or DTO change on the server.

### What was built

- **Manage <org> › Skills** (`/o/<org>/skills`, tabs Skills · Libraries): its own sub-nav entry after Agents; the
  Skills tab is gone from Manage › Agents; today's `SkillsEditor` moved in, with the "How skills combine" fold and the
  "Add a library" notice (shown with no library, now opening the Libraries tab).
- **Libraries as cards** (`libraries/library-cards.tsx`, replacing `libraries-table.tsx`): state chip, facts line,
  Check now, Edit and "…" (Browse skills · Pause/Resume · Delete…). `CardGrid` with its columns moved up one step, so a
  card has the whole row below a content width of 896 px (as drawn at 1256 px) and wider screens add columns.
  `LIBRARY_STATUS` words: Up to date, Paused, Needs attention, Over budget.
- **Administration › Skills** (`/admin/skills`, rail entry after Apps): the instance's libraries as the same cards, then
  **Library settings** with the Save bar. The block is gone from Defaults; `SkillLibrariesRow` and the now unused
  `SettingsGroup.lead` are deleted.
- **Who sees Skills:** `visibleTopics` lists the workspace's Skills to its owners (team and personal) and the
  organization's to owners and admins at Use. `ALTITUDE_REGISTRY["org.skills"]` is removed (nothing is wrapped with it).
- **Workspace Skills:** the lead "Skills every agent in <Workspace> can use" and **Add skill** in the header.
- `TopicPage.actions` may be a function of the selected tab (Add skill belongs to the Skills tab only).

### A rule of this brief that was relaxed

"Do not touch `skills-view.tsx`" could not hold together with scope 1 and 5 ("Add skill" in the header, using the
existing create flow): the create state lives inside `SkillsEditor`. The coordinator approved (2026-11-27) **one
optional, additive prop**, `create={{ open, onOpenChange }}`: the page owns whether the form is open and the editor
then draws no "New skill" button of its own (header or empty state). Without the prop nothing changes, so the agent's
Skills tab is untouched. **P5-SKILLS-3 can remove the prop** when it replaces the list.

### Redirects added or changed

| From | To | Answered by |
|---|---|---|
| `/o/<org>/agents?tab=skills` | `/o/<org>/skills` | `LegacyGate` on the Agents page |
| `/o/<org>/agents/skills/libraries` | `/o/<org>/skills?tab=libraries` | `next.config.ts` |
| `/o/<org>/skills/libraries` | `/o/<org>/skills?tab=libraries` | `next.config.ts` |
| `/admin/skill-libraries` | `/admin/skills` | `next.config.ts` |
| `/admin/agents?tab=skills`, `/admin/settings/skills` | `/admin/skills` | `LegacyGate` |

The old redirect `/o/<org>/skills` → `…/agents?tab=skills` is removed: that address is the page again.

### Comparison with `after/skill-6.png` and `-light.png` (1256 px dark and light, 390 px both themes)

Captured with the mocked API (organization "Acme Labs", two libraries as drawn) before the handover; the header,
tabs, sub-nav entry, lead, cards, chips, facts and actions match. Differences kept:

1. **Add skill has no chevron or menu**: scope 1 asks for the existing create flow until P5-SKILLS-3, so
   **Add from library…** stays above the list (coordinator, 2026-11-27).
2. **The list inside the Skills tab** (and its "Add from library…" row, update notice and "Select a skill" line) is
   today's: out of scope, P5-SKILLS-3. The **How skills combine** fold stays below it with that list (existing content
   this brief moves unchanged, REFINE-20 org-12).
3. **"n skills ·" is missing until P5-SKILLS-1 merges**: `skills_offered` is not in the schema yet, so a card reads
   "1 used by <Org>" or "None used yet". `countWords` already reads the field; `TODO(P5-SKILLS-1)` in
   `library-cards.tsx` marks the cast to drop. (The screenshots feed the field through the mock.)
4. **"Default branch"** instead of "Branch <name>" when the library has no `ref`: the API does not say the default
   branch's name.
5. **Pinned** counts the organization-level skills only (brief scope 2), the list the page can read in one request: a
   library used only by agents or workspaces shows no "Pinned".
6. **Folder and Copy address are no longer on the card** (neither the brief nor the mockup has them); Edit shows both
   the folder and the address.
7. **Header and tab spacing, tile icon size, button weights** are those of the shared `PageHeader`, `TopicPage`,
   `Tabs` and `Button` primitives. The left rail, the workspace name and the sample data are illustrative.
8. **390 px** (no phone frame is drawn): the chip and "…" wrap under the card's title, the facts wrap, one card per row.

No mockup exists for Administration › Skills: it uses the same cards (two columns at 1256 px there, the content area
being wider without the Manage sub-nav; no **Browse skills**, because browsing an instance library needs an
organization to browse as, C-1128), then the settings card.

### Tests

Changed because the UI moved: `skill-libraries-ui` (table → cards, both pages, Manage › Skills), `visible-topics`,
`roles`, `context-switcher`, `routes`, `legacy-routes`, `labels`, `admin-settings-operational`, `refine-20-batch3`,
`workspace-skills`; e2e `skill-libraries` (+ `support/skill-libraries.ts`; widths 1256/768/390, the new screens),
`altitude` (owners see Skills at Use), `ia-navigation`, `ia-3`, `support/mock-api.ts` (route lists, so `stage4-routes`
and `stage4-axe` visit the new addresses). Production specs and `scripts/e2e`: no hit for the old addresses or words.

### Commands and results

- **Fail first:** the changed unit tests against the base code (`d96d979f`, throwaway checkout): **54 failed**.
- One bug was found by Playwright and fixed: **Add skill** stayed on the Libraries tab (a tab click does not update
  `useSearchParams`). The new unit assertion failed first, then passed with `TopicPage.actions(tab)`.
- Whole vitest once, before the coordinator set the minimal gate: 3828 passed, 5 failed: 3 old navigation
  expectations (updated) and 2 that fail on the base commit in files this brief does not touch
  (`ui-text.test.ts` on `templates/review-version.tsx`; `audit-actions.test.ts` on `workspace_access.self_grant`),
  being fixed in the `ci-fix-1127b` lane.
- `test.sh … web vitest` on the files of the changed surfaces: 27 files, 342 passed; after the last edits, 12 files,
  132 passed.
- `test.sh … web playwright … --project=mocked`: `skill-libraries` all screens at 1256, 768 and 390 px in both themes
  with axe and no horizontal scroll (83 passed; the one failure was the new 390 px assertion, which had not scrolled
  to the button), then its journeys at 1280 and 390 px 12/12 passed (with the 4 temporary screenshot tests).
  `altitude`, `ia-navigation`, `ia-3`, `stage4-routes`, `stage4-axe` filtered to the tests of the changed pages and
  routes: 96 passed. The rest of those files is left to CI.
- `pnpm typecheck` and eslint on the touched files: ok. `scripts/guards/run-all.sh`, `extract-schema.py --check`,
  `cargo fmt --all --check`: ok. No production build locally (CI).

No SEC row: no authorization, API or data change. Navigation lists more to owners; every route keeps its own gate
(`OrgGate` owners and admins, `AdminGuard`, the workspace page read-only for members), and repository text stays
plain text.
