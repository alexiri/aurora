# P5-SKLIB-3: Skill libraries in the dashboard: add, edit, browse and approve at both levels (owner, 2026-11-21)

## Goal

Owner, 2026-11-21 (D-450): "setting up all the UI/UX to add/update/approve skills repos", after dropping
the GitHub requirement (P5-SKLIB-1) and adding instance libraries (P5-SKLIB-2).

Today the organization's Agents › Skills › Libraries page has a one-step **Connect a repository** dialog
that asks for an organization connection and `owner/name` (P5-SKLIB-1 swapped those two fields for an
address and a token, nothing more), and the instance has no page at all. After this brief:

- An organization owner/admin **adds a library by pasting the repository's address**, sees what Aurora
  found (branch, commit, how many skills) **before** saving, edits branch, folder and token later, and
  sees the libraries the instance shares with every organization.
- An instance admin does the same for the whole installation at **Administration › What agents may do ›
  Defaults › Skill libraries**.
- Everyone who adds skills sees both kinds in **Add from library**, and the update review (Approve /
  Reject with the full diff) works the same for a skill from either kind.

## Decisions that bind this brief

- **D-450** (all of it): where each page lives, the two-step add dialog, the edit rules, the status words,
  the grouped picker and the badge.
- D-448, D-449 for what the API does; **C-1122, C-1124, C-1128** for the shapes; **C-1129** for the UI
  contract. SEC-149: an organization's page never shows another organization's data (the API already
  filters; don't merge lists client-side across organizations).
- `docs/14-web-dashboard.md` §Visual design (binding for every screen): tokens only, the shared
  primitives and page patterns, both themes, no horizontal scroll at 390 px, no one-off styling where a
  primitive exists (add a missing one to the shared set). The P5-UX design canvas (D-354,
  <https://claude.ai/artifact/3UwmUgrhh6MLCsfqdhTFmQ>) is the visual reference for tables, dialogs and
  status chips. D-439 (full-width pages; forms keep their measure).
- D-407 words (Pause/Resume), D-375 (`SecretInput`), D-441 (`Fold`, neutral).

## Order and lanes

- One web lane, **after P5-SKLIB-1 and P5-SKLIB-2 have merged** (it needs both schemas). Size M.
- Files: `web/src/components/skills/{libraries-page,library,org-skills,skills-view}.tsx`, new
  `web/src/components/skills/libraries/` (shared table and dialogs), `web/src/app/admin/skill-libraries/page.tsx`,
  `web/src/lib/routes.ts` (`adminRoutes.skillLibraries`), `web/src/lib/api/skill-libraries.ts`,
  `web/src/components/admin/settings/operational-settings-views.tsx` (`SKILLS_PAGE`), `web/src/lib/labels.ts`,
  `web/src/lib/api/errors.ts`, mocks and tests. If another running lane holds `routes.ts` or
  `operational-settings-views.tsx`, rebase on it; the Orchestrator orders the merge.

## Read first

- `docs/API_CONTRACTS.md` **C-1122, C-1124, C-1128, C-1129**; C-571..C-574 (browse, add, read-only, review).
- `docs/14-web-dashboard.md` §Visual design (:90), Organization › Skill libraries (:1660), the agent Skills
  tab and Add from library (:1759), the workspace and org Skills rows (:2048), §Instance administration
  (:2133) and the settings-in-topics table (:1434, `skills` → What agents may do › Defaults).
- `docs/tasks/P5-SKLIB-1.md`, `P5-SKLIB-2.md` and their completion reports.
- Code: `web/src/components/skills/libraries-page.tsx` (whole file), `library.tsx` (`LibraryBadge` :38,
  `UpdateReviewDialog` :140, `LibrarySkillActions` :241, `AddFromLibrary` :334), `org-skills.tsx`
  (`LibrariesLink` :42, `AddLibraryCallToAction` :54), `skills-view.tsx` (:55), `ui/data-table.tsx`,
  `ui/confirm-dialog.tsx` (`FormDialog`), `ui/secret-input.tsx`, `ui/fold.tsx`, `ui/field.tsx`,
  `ui/notice.tsx`, `ui/status-dot.tsx`, `ui/copy-value.tsx`, `ui/page-header.tsx`, `ui/breadcrumb.tsx`;
  `components/admin/settings/operational-settings-views.tsx` (`SKILLS_PAGE` :96) and the page that renders
  What agents may do › Defaults; an existing admin sub-page with a breadcrumb back to its topic (copy it).

## Work

1. **Shared pieces** (`components/skills/libraries/`): `LibrariesTable` (`scope: "org" | "instance"`,
   `rows`, `canManage`), `AddLibraryDialog` (`scope`, `orgId?`), `EditLibraryDialog`, `LibraryStatus`
   (the D-450(e) words on `StatusDot`), used by both pages. Columns: Label; Repository (`repo` in mono
   with `CopyValue` of `url`; plain text, not a link); Branch or tag ("Default branch" when null); Folder
   ("Repository root" when empty); Status; Last checked (relative time + `sha7`); Skills in use
   (agent / workspace / organization counts); Pending updates (a link to the review list when > 0);
   actions (Check now, Edit, Pause/Resume, Delete) for managers only. The instance page adds
   "Organizations using it". At 390 px the table uses `DataTable`'s card layout, no sideways scroll.
2. **Add a library** (C-1129): step 1 *Where is it?* — Repository address (required; hint "The address
   you clone, or the repository's page. For example https://gitlab.example.org/team/skills or
   https://github.com/acme/skills/tree/main/skills"), Branch or tag (hint "Default: the repository's
   default branch"), Folder (hint "The folder that holds the skill folders. Default: the repository
   root"), and a closed `Fold` "This repository is private" with Access token (`SecretInput`) and User
   name (hint "Most hosts accept any name. Bitbucket: x-token-auth") and the note "Use a token that can
   only read this repository. Aurora only reads." → **Check**. Step 2 *What Aurora found* — the repository,
   the branch ("main (default branch)", or the tag, or the commit), `sha7`, "12 skills can be added" and,
   when any, "1 can't be added (open it after adding to see why)", the names as plain text (repository
   text is data: no markdown, no links), the truncated notice, Label prefilled → **Add library**, **Back**
   keeps every field. When the check filled branch or folder from a page URL, step 2 says so ("Branch and
   folder were taken from the address"). Errors: `library_url_invalid` on the address field with one
   sentence per `details.reason` (`ssh_url`: "Use the https address; Aurora doesn't use SSH."); others in
   an `ErrorNotice` above the fields; 429 says when to try again.
3. **Edit** (`EditLibraryDialog`): Label, Branch or tag, Folder; Access token shows "A token is saved
   (since <date>)" with **Replace** (reveals `SecretInput`) and **Remove** (confirm), or "No token" with
   **Add a token**; the address is shown read-only with "To use another repository, add a new library".
   A migrated row without `url` shows **Set address** instead (C-1122's one PATCH of `url`). Saving runs
   the server's re-read; its failure keeps the dialog open with the reason.
4. **Organization page** (`/o/<org>/agents/skills/libraries`): header "Skill libraries" with **Add a
   library** (owners/admins); section "Your organization's libraries" (`LibrariesTable scope="org"`), empty
   state "No libraries yet. Add a git repository of skills; people then add the skills they want to their
   agents, workspaces or the organization." with the button; section "Shared with every organization"
   (instance rows, read-only, status and counts for this org, an action **Browse** that opens the Add from
   library browser in read mode for this org), shown only when there is at least one. Members who are not
   owners/admins keep being sent where the page sends them today (`OrgGate`).
5. **Instance page** (`/admin/skill-libraries`, `adminRoutes.skillLibraries()`): breadcrumb "What agents
   may do › Skill libraries", description "Libraries here are offered to every organization. People add
   skills from them with the same permissions as any skill, and each organization approves its own
   updates.", **Add a library**, `LibrariesTable scope="instance"`, empty state. In `SKILLS_PAGE` the
   group is renamed "Skill libraries" with the description "Git repositories of skills that organizations
   and this instance offer to agents." and gains a first row "Skill libraries" — "<n> shared with every
   organization" — **Manage** (link). Add the page to the admin search / All settings index if those list
   sub-pages.
6. **Add from library** (`library.tsx` `AddFromLibrary`): the library picker lists "Your organization"
   then "Shared with every organization" (group headings, labels as data), passes `?org=` for instance
   libraries, and is shown when **either** kind has an active library (today: only org libraries). The
   rest of the flow (search, preview of exactly what is stored, commit sentence, budget) is unchanged.
7. **Badges and review**: `LibraryBadge` reads "Library · <repo> · <sha7>" (repo is now `host/path`);
   `UpdateReviewDialog` and `LibrarySkillActions` unchanged in behaviour; the read-only banner stays
   "Managed by a skill library; edit it in the repository or detach it". `org-skills.tsx`'s
   `AddLibraryCallToAction` and `LibrariesLink` say "Add a library".
8. **Words**: no "connection", "account" or "GitHub" left on these screens except in host examples and the
   token hint; add the new strings to `labels.ts` where its rules say; the labels and words tests pass.

## Tests move with the UI (owner, 2026-11-20 — binding)

In **every commit**:

1. **Find every test that sees what you change**: search `web/tests/**`, `web/e2e/**` (mocked **and**
   `production` projects), `scripts/e2e/*.sh` and fixtures for the old strings, selectors and components
   (`rg -n "Connect a repository|Skill libraries|Add from library|Library ·|lib-conn|lib-repo|skill-libraries" web/tests web/e2e scripts/e2e`).
2. **Update them in the same commit.** Never loosen an assertion so old and new both pass; never
   `.skip`/`test.fixme`/delete a test to get green.
3. **Run what you touched and what references it** through `heavy.sh`; `routes.ts` and the admin settings
   views are shared: run the **whole** web vitest suite once before handing over
   (`cd web && /home/alex/orca/workspaces/aurora/heavy.sh <lane> pnpm exec vitest run`). Production specs
   that can't run in a lane are updated by reading and listed in the report.
4. New behaviour gets new tests; old tests are kept and updated.

Known: `web/tests/skill-library.test.tsx`, `web/e2e/skill-libraries.spec.ts`, `web/e2e/support/skill-libraries.ts`,
`web/e2e/support/mock-api.ts`, `web/e2e/ia-navigation.spec.ts` and `ia-3.spec.ts` (admin routes and
redirect lists), `web/tests/labels.test.ts`, `web/tests/settings-in-words.test.tsx`.

## Acceptance

1. **Vitest**: the add dialog's two steps (a page URL fills branch and folder; Back keeps fields; the
   token fold closed by default and its values sent only when filled; each `library_url_invalid` reason
   shown on the field; `library_unreachable` above); the edit dialog (Replace, Remove, Add a token, Set
   address on a migrated row, the address read-only otherwise); status words for every status and the
   "Needs an access token" case; the org page's two sections (shared section hidden when empty, no actions
   on instance rows); the instance page; the picker's two groups and `?org=`; the badge text.
2. **Playwright, mocked**: organization journey — add a GitHub page URL → check → add → the row appears;
   edit → add a token → saved; Add from library on an agent lists both groups and adds from an instance
   library; an update on that skill → Approve with the full diff → applied. Instance journey — Defaults ›
   Skill libraries › Manage → add → the row and "Organizations using it". Both at 390 px and 1280 px with
   no horizontal scroll, light and dark.
3. **Visual**: every new screen uses tokens and the shared primitives; checked against the design canvas
   patterns for table, dialog and chips; any primitive you had to add is in `components/ui/` and named in
   the report.
4. **Docs in the same commits**: `14` (Organization › Skill libraries rewritten; the instance sub-page
   under §Instance administration; Add from library's two groups; the badge), `docs/operator/skill-libraries.md`
   (where the pages are, with the dialog's steps), `15` only if a setting's text changed. FEATURES and
   PROGRESS rows are the Orchestrator's at merge.
5. **Gates** (targeted, through `heavy.sh`): `pnpm exec tsc --noEmit`, lint, the whole vitest suite once,
   the mocked Playwright specs touched; `scripts/guards/run-all.sh`.

## Out of scope

- An organization hiding an instance library (Q-SKLIB-1); notifications when an instance library changes;
  any backend change (send it back to P5-SKLIB-1/-2's lane if the API is missing something).

## Completion report

Files changed, test commands and results, screenshots kept out of git, and anything left undecided.
Always name: the primitives added or changed, the admin search/index entry (added or not needed), and
every test file changed.

## Completion report (lane B, Sonnet, 2026-11-21)

Merged `master` and `b99fedec` (lane A's P5-SKLIB-1 + -2; schemas used as generated, nothing regenerated or hand-edited). No API change was needed.

**Built (all of Work 1–8, D-450 / C-1129):**
- `components/skills/libraries/`: `libraries-table.tsx` (`LibrariesTable`, `scope`, `rows`, `canManage`, DataTable with `stackBelow={900}` cards), `add-library-dialog.tsx` (two steps, **Back** keeps every field, token fold closed unless a token was typed, values sent only when filled, found names as plain text), `edit-library-dialog.tsx` (Replace / Remove with confirm / Add a token / Set address / read-only address), `library-status.tsx` (`LibraryStatus`, "Needs an access token", "Can't be read" + reason, migrated row without an address), `words.ts` (one sentence per `library_url_invalid` reason, rate-limit sentence), `instance-libraries-page.tsx`.
- Organization page rewritten (two sections, Browse in read mode; shared section only when there is one; no Hide, https only). Instance page `/admin/skill-libraries` (`adminRoutes.skillLibraries()`, `app/admin/skill-libraries/page.tsx`, breadcrumb back to Defaults, rail keeps What agents may do selected). `SKILLS_PAGE` group renamed "Skill libraries" with the lead row (`<n> shared with every organization`, **Manage**).
- `library.tsx`: `AddFromLibrary` now offers both kinds (grouped picker "Your organization" / "Shared with every organization", `?org=` always passed), new `LibraryBrowserDialog` (read mode for Browse), badge unchanged (`Library · host/path · sha7`); review/approve unchanged and tested for an instance-library skill. `org-skills.tsx` link says "Add a library". `labels.ts`: `LIBRARY_STATUS`. `errors.ts`: "Connect a narrower folder" → "Choose a narrower folder".
- Hooks (`lib/api/skill-libraries.ts`): `useInstanceSkillLibraries`, `useCheckLibrary`, create/patch/delete/check-now take an owner (`orgId` or `null` = instance); an instance change also invalidates every organization's list.
- **Primitives added or changed:** `ui/fold.tsx` gained `defaultOpen`; `SettingsGroup` gained an optional `lead` row (`settings-section-form.tsx`). No other new primitive (the table's "Copy address" is a small button on `useCopy`, not `CopyValue`, whose label+box block does not fit a cell).
- **Found while testing at 768 px (not in the brief):** `skills-view.tsx` laid its list and editor side by side from a viewport breakpoint (`md:`), so inside Agents › Skills (sub-nav rail) the editor got ~224 px and the page scrolled sideways. Now `@container` + `@3xl:`. Fixes Agents › Skills, Organization skills and workspace Skills at 768 px.
- **Admin search / All settings index:** not needed (it lists settings sections, not sub-pages).
- Docs: `14` (Organization › Skill libraries rewritten; Add from library's two groups and badge; instance sub-page under §Instance administration; settings table row), `docs/operator/skill-libraries.md` (where the pages are, the dialog's steps). `15` untouched (no setting text changed).

**Not done / for the Orchestrator:** no screenshots taken (none in git); I could not compare against the design canvas visually from here, only by the shared primitives and the axe/overflow specs. The Edit dialog has a User name field next to a new token (not in the brief; the API accepts `token_username`, Bitbucket needs it).

**Tests changed:** new `web/tests/skill-libraries-ui.test.tsx` (36 tests: both sections, columns and words, every status, actions, Browse, empty/off, add dialog steps/Back/token/each `library_url_invalid` reason/unreachable/429/failed add, edit dialog, instance page, picker groups + `?org=`, approve for an instance-library skill). Edited: `web/tests/skill-library.test.tsx` (the old libraries-page and Connect-dialog tests removed because the screens they drove no longer exist; their behaviour is re-tested in the new file), `admin-settings-operational.test.tsx` (lead row), `labels.test.ts`, `routes.test.ts`, `e2e/skill-libraries.spec.ts` (screens at 1280/768/390, light+dark, axe; journeys at 1280 and 390), `e2e/support/skill-libraries.ts` (stateful mocks, both levels), `e2e/support/mock-api.ts` (`/v1/admin/skill-libraries`, `/v1/admin/settings/skills`, route `admin-skill-libraries` in the sweep). No test deleted to get green, none skipped. Production (`*.prod.spec.ts`) specs: none reference these screens (searched `web/e2e`, `scripts/e2e`).

**Commands (all through `heavy.sh p5-polish-b`):**
- `pnpm typecheck`: passes. eslint on touched src and tests: clean.
- Whole `vitest run`: 3462 passed, 2 failed on the first run: `arbitrary-values` (my `@md:grid-cols-[8rem_…]`, fixed, passes) and `connect-dialog.test.tsx` "a failed result shows the error…" (an 11 s timeout under load; passes alone, untouched by this change). Re-ran the touched files after the fix: 50 passed.
- Playwright mocked (`AURORA_E2E_WEB_PORT=4123`): `e2e/skill-libraries.spec.ts` 66 passed; `stage4-routes` + `stage4-axe` filtered to the skills, Defaults and new admin routes: 20 passed.
- `bash scripts/guards/run-all.sh`: all guards passed.

I edited three one-line import/test lines with `sed -i` by mistake (labels.test import, an e2e helper call) and deleted the old test block with `sed -i '72,215d'`; everything else with Edit/Write.
