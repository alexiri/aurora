# P5-NAV-3: The place picker, Manage at the rail's foot, places in search, and places on a phone (owner, 2026-11-27)

## Goal

The switcher gives Bob (who can only use his workspace) the same popover as Alice, "Manage Acme" is a plain row among
workspaces, and nothing outside the popover leads to Manage (N-6). Implement the review's **place picker** and its
entry points (D-508 (f), (g), (k)):

- the switcher becomes the **place picker**: groups **Workspaces in <Org>**, **Organization**, **Administration**,
  **Other organizations**, **New**, each shown only when there is something in it; search across organizations when
  there are many places;
- **Manage <Org>** also at the **rail's foot** for owners and admins, above Administration, with its count;
- **Ctrl K** finds places;
- on a **phone** the top bar shows the place card; tapping it opens the picker as a **sheet**.

Decision: **D-508**. Builds on **P5-NAV-2** (the place card, the places' menus, the back line).

## Linked requirements (look at every image before coding)

- `docs/design/navigation/README.md` (binding), `review.md` N-6, nav-3 and "nav-3 in depth" (nav-3a, nav-3d) with the
  table "Picker group · What it lists · Who sees it", and the owner's decisions.
- **After:** `after/nav-3a.png` / `-light.png` / `.html` (the picker for Bob, Alice and Carol — the final version, it
  wins over `after/nav-3.*`'s first frame), `after/nav-3.*` (second frame: the rail foot with Manage Acme above
  Administration), `after/nav-3d.*` (phone: place card in the top bar, the picker as a sheet, the drawer),
  `after/nav-5.*` (phone frames 1 and 3).
- **Before:** `before/N11-switcher-open.png`, `N22-switcher-bob.png`, `N10-alice-ws-overview.png`, `P01-phone-drawer.png`,
  `P05-phone-drawer-manage.png`.
- `docs/14-web-dashboard.md`: the Shell and phone top bar / drawer paragraphs (as rewritten by P5-NAV-2), Search.

## Wave and order

After **P5-NAV-2**, in the same lane. Reviewed by **P5-NAV-5**.

## Context (what exists today)

- `web/src/components/nav/scope-switcher.tsx` (~79–460): search, the org's 5 recent workspaces, "All n in <Org>" (>5,
  links to Manage's Overview for anyone), **Manage <Org>** for owners/admins/instance admins with a badge (~243–248),
  Other organizations (≤3, each drilling into its workspaces, ~220–231) and "All n organizations", New workspace / New
  organization; a static card without menu when there is one workspace, one org and no create rights (~372–379); the
  closed switcher carries Manage's badge (~388).
- Rail foot: Administration for instance admins with its count (`app-shell.tsx:~509-520`); Search ⌘K (`SearchButton`
  ~109–126); phone top bar and drawer (~593–637; drawer from the right).
- Search: find the Ctrl K command palette's sources (pages, agents, conversations…).

## Scope

1. **The picker** (`after/nav-3a.png`, the table in `review.md`): opened from the place card in every place. Groups, in
   order, each only when non-empty:
   - **Search** field when the person has more than 6 places; it searches workspaces and organizations across
     organizations, results grouped by organization, with "↑↓ to move · ⏎ to open · Esc to close".
   - **WORKSPACES IN <ORG>**: the 5 most recent with the person's role ("Owner · Acme", "Member · Acme", "Organization
     workspace"), ✓ on the current; **All n workspaces →** (owners/admins: Manage › Workspaces; members: the full list
     inside the picker).
   - **ORGANIZATION**: **Manage <Org>** with the org's initials, "People, apps, AI models, data", the role chip and the
     count — owners, admins, instance admins only.
   - **ADMINISTRATION**: **Administration** "Settings for everyone on this Aurora" with its count — instance admins only.
   - **Other organizations**: up to 3 with the role; choosing one opens your last workspace there (or its Manage when you
     have none and may manage it); "All n organizations".
   - **New workspace in <Org>**, **New organization** — when allowed.
   A person with nothing else to open gets the card without a menu (today's rule). Keyboard: arrows, Enter, Esc, type to
   search; focus returns to the card.
2. **Rail foot** (`after/nav-3.png`, second frame): **Manage <Org>** with its count, above **Administration**, for owners,
   admins and instance admins; members never see it.
3. **Search**: Ctrl K also lists places ("Manage Acme", "Administration", every workspace by name) for the people who may
   enter them.
4. **Phone** (`after/nav-3d.png`, `after/nav-5.png`): the top bar shows the place card in short ("AC Acme ·
   Organization ▾"); tapping it opens the picker as a bottom sheet (`Dialog` sheet variant) with the same groups; the
   drawer shows the current place's menu with its back line (from P5-NAV-2). No native select anywhere.
5. **Docs**: `14`'s shell/switcher paragraph describes the picker groups and who sees what; Search lists places.

## Out of scope

The places' menus, the place card's look, the back line, Manage's pages (P5-NAV-2); agent pages (P5-NAV-1); any API change
(the picker uses today's data: workspaces, organizations, roles, counts).

## Constraints / file ownership

Owns `nav/scope-switcher.tsx` (rewrite as the place picker), `app-shell.tsx` (rail foot and phone top bar only), the
search palette's sources, `14`. Tokens and shared primitives only; both themes; 390 px.

## Acceptance criteria

- Bob, Alice and a person with many workspaces in several organizations see the pickers of `after/nav-3a.png` (dark and
  light), with the groups and visibility of the table; keyboard works; search crosses organizations.
- Alice sees Manage Acme at the rail's foot above Administration; Bob does not.
- Ctrl K finds "Manage Acme" for Alice and not for Bob.
- At 390 px the top bar shows the place card and opens the picker as a sheet (`after/nav-3d.png`); no native select.

## Tests move with the UI (owner, 2026-11-20 — binding) and the lane gate (owner, 2026-11-27)

1. Before each commit that changes visible text, a label, a role or structure: `rg -n "Find a workspace|All .* in |Manage
   Acme|New workspace in|<old selector>" web/tests web/e2e scripts/e2e` and update **every** hit in the same commit (the
   switcher's vitest files, `web/e2e/ia-navigation.spec.ts`, `ia-3.spec.ts`, any spec opening the switcher or the phone
   drawer, `support/mock-api.ts`, production specs). Never loosen, skip, `fixme` or delete a test to get green.
2. **Run only what this lane changed**: new tests, the vitest files of components you changed, and the drift guards
   `scripts/dev/test.sh <lane> web vitest tests/audit-actions.test.ts tests/arbitrary-values.test.ts tests/ui-text.test.ts tests/labels.test.ts`;
   `pnpm lint` and `pnpm typecheck` through `scripts/dev/heavy.sh <lane> …`. Playwright specs are updated by reading;
   GitHub CI runs everything.
3. The completion report lists every test file changed and why; a merge that turns CI red with a stale test comes back
   to this lane.

## Docs to update

`14` (Scope 5), PROGRESS and FEATURES rows at every commit.

## Suggested agent/model and reason

Sonnet `nextjs-developer` (one complex component with role-dependent content, keyboard handling and a phone sheet).

## Completion report

Commits; screenshots of the three pickers (dark, light), the rail foot, the phone sheet, beside the mockups; remaining
differences with reasons; tests changed; commands and results.
