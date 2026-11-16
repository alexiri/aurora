# P5-IA-1: One structure for every level: navigation, routes, names (owner, 2026-11-15)

## Goal

The owner's administration walkthrough found the dashboard organized by when each feature was built:
- 18 Admin sidebar items plus 13 Settings sections;
- 14 organization items;
- the same topic under different names at each level;
- Administration reachable only through "All organizations" in the org switcher.

The owner approved one structure with the same topics and names at every level (D-338, D-351). This task
builds it: the navigation, the routes and tabs, a redirect from every old route, role visibility, the
Administration entry, the level badge, the new labels, settings sections rendered inside their topics,
the tour and a one-time notice that pages have new names.

Existing page **content moves unchanged** under the new routes. The briefs of waves 1 to 3 then rebuild
content inside those pages.

## Linked requirements

- **Design** (D-354): the canvas “Aurora — agents, accounts and access”, https://claude.ai/artifact/3UwmUgrhh6MLCsfqdhTFmQ, boards Read me › Navigation rail, Agent header and Patterns; Workspace › “Pages have new names” and A member's view; States › Tour and Navigation at 390 px; the page frame (rail, level, header sentence, tabs) of every board. Read each board's note; where a board and the spec disagree, the spec wins.
- D-338, D-340, D-351, D-352; contracts **C-926, C-960, C-961, C-962, C-971, C-975, C-987**.
- Normative: `14 §Information architecture` › "One structure for every level" (the five sentences, the
  vocabulary, topics per level, who sees what, settings in their topics), `14 §Onboarding` §3 (tour),
  `15 §Instance settings` (sections; unchanged), `14 §Visual design`.

## Wave and order

- **Wave 0**, the first web task. Every other web brief starts after it.
- **Release gate (D-352):** master is not deployed between this merge and the end of wave 1.
- **SET-4/5** (lane `p5-apps`) edit the settings form this task refactors. The Orchestrator orders them;
  they add no sidebar entries (D-340).

## Context

- **Navigation.** `web/src/components/app-shell.tsx`: `ADMIN_LINKS` (18 items), `ORG_LINKS` and `OrgNav`
  (~283–323), `WorkspaceNav` (~337–368), `NavItem` `data-tour` (~68), the account menu (~265–274).
- **Pages** live under `web/src/app/admin/*`, `web/src/app/o/[org]/*` and
  `web/src/app/o/[org]/w/[ws]/*`.
- **Settings.** `web/src/components/admin/settings/`: `settings-index-view.tsx`,
  `settings-section-view.tsx`, `settings-section-form.tsx` (`labelOf` ~133),
  `governance-settings-view.tsx`, `llm-settings-view.tsx`.
- **Old pages to host:** credentials (`components/credentials/credentials-view.tsx`), org governance
  (`components/governance/governance-view.tsx`, `byo-panel.tsx`, `apps-posture-panel.tsx`), policy
  (`components/policy/*`), workspace settings (`components/settings/settings-view.tsx`), org settings
  (`components/org/settings-view.tsx`).
- **Tour.** `web/src/components/onboarding/tour.tsx` `TOUR_STOPS` (~19–28).
- **Header.** `web/src/components/ui/page-header.tsx`.
- **Role data.** `useOrg().my_role`, `useWorkspace().my_role`, `useCanApprove`.
- **Onboarding state.** `user_onboarding` (`services/aurora-api/src/domains/onboarding/`), read in the
  web through `useMeOnboarding` (`web/src/lib/api/onboarding.ts`).

## Scope

1. **Route helper and role hook.**
   - `web/src/lib/routes.ts`: one function per topic and tab at each level, the per-app setup page and
     the query parameters of C-960. Every internal `href` uses it; a test fails on a hard-coded old path
     anywhere in `web/src`.
   - `web/src/lib/api/roles.ts`: `useViewerRoles(org, ws?)` and `visibleTopics(level, roles)` (C-961).
2. **Topic pages.** For each row of `14`'s table, create the route with `PageHeader`, the level badge
   and at most four tabs. Each tab renders the **existing** component of the page it replaces. The
   mapping is C-960's redirect list read backwards. Where a later brief rebuilds a tab, this task hosts
   the existing lists under the new names:
   - **Apps & accounts › Accounts** (workspace and organization): the existing connection, secret and
     LLM-key lists under the headings Apps, Websites and AI keys. P5-UX-4 replaces them.
   - **Internet access** (workspace and organization): the existing policy editor; at organization level
     the Governance egress panel above it. P5-IA-2 rebuilds the page.
   - **Apps & accounts › Apps**: the existing app lists. In a workspace, OAuth clients are reached from
     this tab until P5-UX-9's setup pages hold them.
3. **Settings sections inside topics (C-971).**
   - Factor the section form into `SettingsSectionFields { section, keys? }`: a whole section or a subset
     of its keys, inside any page, with the section's draft, preview, Helm locks, errors and save (the
     full document is sent, untouched keys unchanged).
   - Render each section in its topic per `14`'s map. `governance` splits three ways: AI models › Rules,
     Internet access, Agents › BYO.
   - `/admin/settings` becomes **All settings**: sections grouped by topic, each linking to its topic
     tab, a search box over section names and descriptions, Export and Import, the Helm notice.
     `/admin/settings/<section>` redirects.
   - Secret stores exists once, under Platform.
4. **Redirects (C-960).** Every old route, with query and anchor preserved. `connect=` and `reconnect=`
   deep links survive.
5. **Navigation and visibility (C-961).** Each level's sidebar lists its topics in `14`'s order, from
   `visibleTopics`. This task owns all navigation gating. On a page a role sees read-only, controls the
   viewer cannot use are not rendered and values read as text (`14` "What a person sees of something
   they cannot do"). P5-UX-1 then puts the `WhoCan` sentence where a page's main action is missing.
6. **Administration entry, level badge, your account (C-962).**
   - **Administration** in the sidebar footer and the account menu, for instance admins.
   - The org switcher lists organizations only.
   - `PageHeader` gains `level`; every page passes it.
   - The account menu item and `/account` title read **Your account**.
7. **Labels (C-926) and names.** Apply the vocabulary to navigation labels, page titles, tab names and
   every sentence that names a page ("see Governance" becomes "see Internet access"). Apply the C-926
   renames on the pages this task moves: **Allow once** and **Block** on the Approvals page and card,
   **Turn off** and **Turn on** for Disable and Enable. The labels inside the Internet access editor
   and `components/policy/simulate-panel.tsx` are **not** this task's: FIX-159 edits that panel beside
   this task, and P5-IA-2 renames those labels when it rebuilds the pages (inside the release gate).
8. **"Pages have new names" (C-987).**
   - API: `user_onboarding.notices_seen text[]`, returned by `GET /v1/me/onboarding` and appended by
     `PATCH { notice_seen }`. The migration marks `renames-2026-11` as seen for accounts created after
     it. Take the next free migration number from PROGRESS when you start, and edit that line. Move the
     planned row of `03` into its table and regenerate `spec_schema.json`. Add `ONBOARDING_NOTICES_MAX`
     to `crates/aurora-core/src/limits.rs`.
   - Web: a dismissible notice, from one table next to `routes.ts`; **What moved** in the account menu
     replays it.
     - Title: "Pages have new names". First line: "Every level now has the same pages, with the same
       names. Nothing was removed."
     - Rows, only those the viewer can open, each "<old name> → <new name>" with a link:
       Credentials → Apps & accounts; Policy → Internet access; Governance › Model access → AI models;
       Governance › Egress → Internet access; Governance › Apps → Apps & accounts › Apps; Governance ›
       BYO agents → Agents › BYO; Members → People; Settings › Access → People; Channels → Chat
       platforms; Storage and Audit → Data; All organizations → Administration; Plugins → Administration ›
       Apps & accounts; Connectors → Chat platforms; Identity → People › Sign-in; Placement → People ›
       New users.
9. **Tour (C-975).** One stop per visible workspace topic, with the new names and one sentence each:
   - Overview: "What your agents are doing, and what needs you."
   - Agents: "Chat with an agent here. Open an agent, then **Access** › **Give <Agent> an app** to let
     it use GitHub, Linear and more."
   - Apps & accounts: "The accounts your agents can use, and the apps you can connect."
   - Internet access: "Which websites agents can reach, and the rules that ask or block."
   - Approvals: "When an agent asks before a change, you answer here."
   - Activity: "Everything agents did, with what was allowed, asked and blocked."
   - People: "Who can use this workspace."
   - Settings: "The workspace's name, API keys and audit log."
10. **Operator docs.** Update page names in `docs/operator/` ("Admin ›", "Governance", "Policy",
    "Credentials"). Builtin package guides are P5-UX-13's.
11. **Tests.**
    - Vitest: the route helper; `visibleTopics` per role and level; every C-960 redirect (a table test);
      the level badge; `SettingsSectionFields` with a subset (saving keeps the other keys); the notice.
    - API test: `notices_seen` append, the limit, the default for new accounts.
    - Playwright:
      - an instance admin reaches Administration from the footer link;
      - a member's organization sidebar shows exactly Workspaces, AI models and Internet access;
      - every old route lands on its new tab (one parametrised test);
      - `…/credentials?tab=apps&connect=github` opens the connect dialog at the new route;
      - the notice appears once for an existing account and never for a new one.
    - Axe and narrow-route suites for the new topic pages, both themes, 390 px (tab bars scroll inside
      their own container).

## Constraints / file ownership

- **Owns:** `app-shell.tsx`; the new route folders and redirect config; `lib/routes.ts`;
  `lib/api/roles.ts`; `page-header.tsx` (the `level` prop); `tour.tsx`; the org switcher; the settings
  form components and the All settings index; the onboarding API fields and migration; link edits across
  `web/src` through the helper; operator doc page names.
- **Does not change** what a page does, only where it is and what it is called.
- This is a large task. The Orchestrator may run it as two back-to-back parts in one lane: the shell
  (scope 1, 2, 4, 5, 6) and the content (scope 3, 7, 8, 9, 10).

## Acceptance criteria

1. Each level's sidebar shows the `14` topics for each role; no topic has more than four tabs; no item
   leads to a refusal.
2. Every old route redirects to the right tab, and no source file hard-codes an old path.
3. Administration has a labelled entry; every page shows its level badge; the person's own account is
   "Your account".
4. Every settings section renders inside its topic page; All settings indexes them; Secret stores exists
   once.
5. No page title, tab or navigation label uses a retired word of the vocabulary table.
6. Existing page contents work unchanged at their new routes.
7. Rust and web gates (fmt, clippy, targeted tests, migration check, limits guard; typecheck, lint,
   vitest, build); axe and narrow-route suites, both themes.

## Suggested agent/model and reason

Sonnet. Wide but mechanical: routing, navigation, a large link sweep through a helper, one form refactor
and one small migration. The redirect table and the visibility function need tests more than design.

## Completion report

**Status: part 0 (API) done on `alexiri/p5-apps`; the web work (scope 1–7, 9, 10 and the web tests of 11) is not started.** Handoff for the next worker:

- **Done (commits `a01a9dad`, `3b52db98`).** Scope 8 API: migration `20261201000244_onboarding_notices.sql` (adds `user_onboarding.notices_seen text[]`, check ≤ 50; gives every existing person a row with `{}`; then sets the column default to `{renames-2026-11}` so a new row has it seen); `GET /v1/me/onboarding` returns `notices_seen` (a person without a row is answered `["renames-2026-11"]`: created after the release); `PATCH { notice_seen }` appends once (key 1–64 of `[a-z0-9-]`, else 400; a new key at 50 entries → 400; re-sending a known key at the limit is fine). `ONBOARDING_NOTICES_MAX` in `crates/aurora-core/src/limits.rs` (its `limits-gaps.allow` line removed); `docs/03` row moved into the `user_onboarding` table, `spec_schema.json` regenerated; openapi + `pnpm -C web gen:api` regenerated (`MeOnboarding.notices_seen`, `PatchOnboarding.notice_seen`). IT `onboarding::notices_default_for_new_accounts_append_once_and_are_limited`. Gates run: fmt, clippy (`aurora-api`, `aurora-db`, `aurora-core` all-targets), `onboarding`+`openapi` ITs (7 pass), `extract-schema.py --check`, guards (exit 0). Not yet run: the migration under `scripts/e2e/upgrade.sh`, full suites (batch gate).
- **Not done (web).** Everything in scope 1–7, 9, 10 and the vitest/Playwright/axe tests. Nothing in `web/src` was touched beyond the regenerated `schema.d.ts`. Suggested split (the brief allows it): part 1 = shell (routes helper `lib/routes.ts`, `lib/api/roles.ts`, topic pages, redirects through `next.config` + a client redirect for `?tab=` maps, nav in `app-shell.tsx`, `PageHeader` `level`, Administration entry, org switcher, "Your account"); part 2 = content (`SettingsSectionFields`, settings in topics + All settings, labels C-926, notice + `What moved` using `notices_seen`, tour, operator docs).
- **Pointers found while reading.** Web has 50+ pages under `web/src/app` (`/admin/*` 17 routes, `/o/[org]/*` 16, workspace 10); `useMeOnboarding` is `web/src/lib/api/onboarding.ts` (52 lines, add `notices_seen`/`notice_seen` there); the settings components are in `web/src/components/admin/settings/` (`settings-section-form.tsx` is the one to factor). Design canvas boards were not opened (artifact reading needs the Artifact tool; do it first).
