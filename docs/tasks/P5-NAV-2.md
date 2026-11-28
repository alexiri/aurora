# P5-NAV-2: Places: the rail shows one place, with the place card on top; Manage <Org> is a place with its own grouped menu (owner, 2026-11-27)

## Goal

On every Manage <Org> page the rail keeps showing an unrelated workspace's menu (none of it active) beside a 192 px
second menu, and six names appear in both with different meanings (N-4). Every place has its own way back, and Your
account has none (N-5, N-7); on a phone Manage's menu is a native select (N-8). Administration and Your account already
replace the rail; Manage and the place header must work the same way. The owner chose **nav-3** with nav-1 and nav-4
and accepted every proposed decision (D-508):

- the rail always shows **one place**; its top is the **place card** (a back line outside a workspace, then the card
  saying where you are);
- **Manage <Org> replaces the rail** with the organization's grouped menu: Overview · Workspaces · People; **What agents
  can use**: AI models · Apps & accounts · Chat platforms · Internet access · Skills; **Oversight**: Agents · Data;
  Settings;
- **Workspaces** is a new entry and page; Manage's Overview shows summary cards and what needs the owner;
- **members don't see Manage**; Manage addresses send them to their workspace;
- the **back line** returns to the workspace and page you came from;
- Administration and Your account use the same place card.

The place picker itself, the rail-foot Manage entry, search and the phone sheet are **P5-NAV-3** (next, same lane).

Decision: **D-508** (a)–(e), (h)–(j), (l). Contract: **C-1178** (the Workspaces route, the member redirect, the back
line's storage).

## Linked requirements (look at every image before coding)

- `docs/design/navigation/README.md` (binding), `review.md` N-4, N-5, N-7, N-8, nav-1, nav-4, "nav-3 in depth" (nav-3b,
  nav-3c) with the table "Entry · Today · Tabs inside", and the owner's decisions.
- **After:** `after/nav-3b.png` / `-light.png` / `.html` (Manage Acme › Overview, the grouped rail — the final version;
  it wins over `after/nav-1.*`, which shows Manage › Agents with an earlier, ungrouped menu), `after/nav-4.*` (the place
  card in every place), `after/nav-3c.*` (how places connect).
- **Before:** `before/N12-manage-overview.png`, `N13-manage-agents.png`, `N14-manage-skills.png`, `N15-manage-people.png`,
  `N16-manage-settings.png`, `N17-admin-overview.png`, `N18-admin-agents.png`, `N20-account.png`,
  `P03-phone-manage.png`, `P05-phone-drawer-manage.png`.
- `docs/14-web-dashboard.md`: the Shell (~332, stale: rewrite it), Manage "an area, not a second rail" (~1710–1717,
  rewrite), the Administration rail (~2377), Account rail (~1639), phone drawer (~3575).

## Wave and order

First of the places pair; **P5-NAV-3 follows in the same lane** (both change `app-shell.tsx` and the switcher).
Independent of P5-NAV-1. Reviewed by **P5-NAV-5**.

## Context (what exists today)

- `web/src/components/app-shell.tsx`: `SidebarContent` (~456–527: logo, switcher or `AdminRailHeader`, the context nav,
  footer with Administration for instance admins, Search, theme, user menu); the context switch (~467–505): in an
  organization context it renders `WorkspaceNav` of a **remembered workspace** (`lib/nav/manage-return.ts:65-72`, else the
  personal or first workspace), none active; `AdminRailHeader` (~432–454: "← Back to <Org>" from session storage
  `aurora.admin-return`, "You're in Administration"); `AdminNavList` (~346–385, groups `ADMIN_RAIL_GROUPS`,
  `lib/routes.ts:249-253`); `AccountNavList` (~333–344, under the workspace switcher, no back link).
- `web/src/components/org/manage-frame.tsx` (~31–100): "← Back to <workspace>", "Manage <Org>" (a `<p>`), the 192 px
  column (desktop) or a native `Select` (phone), count badges; `ORG_TOPICS` / `ORG_MEMBER_TOPICS`
  (`lib/api/roles.ts:47-65`); tabs inside pages: `org/org-topics.tsx` (`TopicPage`, underline).
- Manage's Overview lists the workspaces (`org/…`, find the component; `org/org-agents-list.tsx` is Agents).
- Who is a member: owners, admins and instance admins have the Set up floor and see all of Manage; members see Overview
  (and AI models / Internet access with "Show more controls") (`roles.ts:28-65`).

## Scope

1. **Place card** (`after/nav-4.png`): one component at the top of the rail in every place. Workspace: no back line; the
   card = workspace name · organization (it is the picker's trigger; P5-NAV-3 redesigns the picker). Manage: back line
   "‹ <workspace>" + card "<Org initials> · <Org> · Organization". Administration: back line + "Administration · This
   Aurora" (replaces `AdminRailHeader`'s two lines). Your account: back line + "<Person> · Your account" (replaces the
   workspace switcher there). Same size, padding and position everywhere.
2. **Back line** (C-1178, D-508(h)): entering Manage, Administration or Your account records the workspace and the full
   path you came from in the tab's session storage (extend `manage-return.ts` / `aurora.admin-return` into one helper);
   the back line returns there; opened from a link with no record → your last workspace in that organization; none →
   hidden.
3. **Manage replaces the rail** (`after/nav-3b.png`): in an organization context the rail shows the organization's menu,
   grouped with labels as Administration's is (Overview · Workspaces · People; WHAT AGENTS CAN USE: AI models · Apps &
   accounts · Chat platforms · Internet access · Skills; OVERSIGHT: Agents · Data; Settings), with today's count badges;
   delete the remembered-workspace rail, `ManageFrame`'s column and its phone `Select`. The drawer on a phone shows the
   same menu with the back line (the drawer reuses `SidebarContent`).
4. **Workspaces page** (C-1178): `/o/<org>/workspaces` lists every workspace of the organization (name, kind
   personal/team, owners, agents count, created) with **New workspace**; move the list out of Overview (keep a link).
5. **Manage Overview** (`after/nav-3b.png`): title "<Org>" with lead "Everything <Org>'s workspaces share", summary cards
   (Workspaces · People · Agents · AI keys, each a link), and **Needs you**: the items behind Manage's count (only what
   an owner or admin can act on, D-508(j)), each linking to its page.
6. **Titles name the scope** (D-508(i)): Manage page headers read "Agents in <Org>", "People in <Org>", "Skills in <Org>"…
   (leads unchanged).
7. **Members** (D-508(e), C-1178): remove `ORG_MEMBER_TOPICS`; a member opening a Manage address is redirected (last
   workspace in that org, else first, else `/`); find and fix every link that sends members to a Manage page (e.g.
   "All n in <Org>" in the switcher, help links, empty states) so none dead-ends.
8. **Docs**: rewrite `14`'s Shell paragraph and the Manage paragraph to the places model; update the account and admin
   rail paragraphs to the place card; record the Workspaces route.

## Out of scope

The picker's content, the rail-foot Manage entry, Ctrl K places, the phone top bar and sheet (P5-NAV-3); agent pages
(P5-NAV-1); any API change (counts and lists come from existing routes; if one is missing, say so in the report rather
than adding a route).

## Constraints / file ownership

Owns `app-shell.tsx` (rail content per place, place card, footer untouched until P5-NAV-3), `org/manage-frame.tsx`,
`lib/api/roles.ts` (topics, groups), `lib/routes.ts` (org routes, groups), `lib/nav/manage-return.ts`, the new
Workspaces page and the Overview content, `legacy-routes.ts` if a redirect is needed, `14`. Tokens and shared primitives
only; both themes; 390 px without horizontal scroll.

## Acceptance criteria

- As Alice (owner of Acme): Manage Acme › Overview matches `after/nav-3b.png` / `-light.png` at 1256 px (one column,
  grouped menu, place card with back line, summary cards, Needs you); every Manage page has no second menu; the back line
  returns to the exact page she came from.
- Administration and Your account show the place card of `after/nav-4.png`; Your account has a way back.
- As Bob (member of Acme): no Manage anywhere; `/o/acme/people` sends him to his workspace.
- `/o/<org>/workspaces` exists and lists every workspace; Overview no longer carries the list.

## Tests move with the UI (owner, 2026-11-20 — binding) and the lane gate (owner, 2026-11-27)

1. Before each commit that changes visible text, a nav entry, a route, a heading or structure: `rg -n "Manage |Back to
   your workspace|You're in Administration|ORG_MEMBER_TOPICS|ManageFrame|<old selector>" web/tests web/e2e scripts/e2e`
   and update **every** hit in the same commit: e.g. `web/tests/visible-topics.test.ts`, `refine-20-batch3.test.tsx`,
   the org and admin page tests; `web/e2e/ia-navigation.spec.ts`, `ia-3.spec.ts`, `altitude.spec.ts`,
   `skill-libraries.spec.ts`, `stage4-routes.spec.ts`, `stage4-axe.spec.ts`, `support/mock-api.ts` route lists;
   production specs. Never loosen, skip, `fixme` or delete a test to get green.
2. **Run only what this lane changed**: new tests, the vitest files of components you changed
   (`scripts/dev/test.sh <lane> web vitest <files>`), and the drift guards `scripts/dev/test.sh <lane> web vitest
   tests/audit-actions.test.ts tests/arbitrary-values.test.ts tests/ui-text.test.ts tests/labels.test.ts`; `pnpm lint`
   and `pnpm typecheck` through `scripts/dev/heavy.sh <lane> …`. Playwright specs are updated by reading; GitHub CI runs
   everything.
3. The completion report lists every test file changed and why; a merge that turns CI red with a stale test comes back
   to this lane.

## Docs to update

`14` (Scope 8), `API_CONTRACTS.md` C-1178 (as built), PROGRESS and FEATURES rows at every commit.

## Suggested agent/model and reason

Sonnet `nextjs-developer` (app shell, routing and permissions in the UI, several pages).

## Completion report

Commits; screenshots (1256 dark/light, 390) beside `after/nav-3b*`, `after/nav-4*`; remaining differences with reasons;
the links fixed for members; tests changed; commands and results.

### Report (2026-11-28, branch `alexiri/p5-nav-2`)

**Built.** The rail shows one place (`app-shell.tsx`): `components/nav/place-card.tsx` (card and back line),
`OrgNavList` (the grouped menu, `ORG_RAIL_GROUPS`, today's counts), Administration and Your account cards.
`manage-frame.tsx` is deleted (column and phone select); `org/manage-gate.tsx` redirects members. New
`/o/<org>/workspaces` (`org-workspaces.tsx`); the Overview is `org-overview.tsx` (summary `StatLink`s, Needs you).
`lib/nav/manage-return.ts` is the one back-line helper (key `aurora.place-return`; C-1178 as built). Manage titles
name the scope (`TopicPage` got `tabsName`, so tab bars keep their names).

**Not in the API (no route added).** `GET /v1/orgs/{org}/workspaces` carries no owners: a team workspace's owners are
read per workspace from `GET /v1/workspaces/{ws}` (`audience.owners`; N requests for N team workspaces). A list field
would remove them.

**Links fixed for members** (none dead-ends; the gate catches typed and mailed addresses):
- switcher "All n in <Org>": owners and admins → `/o/<org>/workspaces`; a member's button opens the whole list in the menu;
- Ctrl K: no organization entry where the person is only a member;
- an approval asked by an organization rule, and the app-access "Internet access" link → the workspace's Internet access;
- Internet access "Default rule" hint ("AI models") and an agent's model-readiness fix (organization key, AI model
  rules): words without a link;
- deleting a workspace → `/` for a member (was the organization's page);
- `/` never lands a member on `/o/<org>`: another of their workspaces, else "You are not in a workspace yet" (without
  this, Manage → `/` → Manage would loop).
- Unchanged because only owners, admins or instance admins see them: "Add a key" (`add_key`), chat-platform connect
  buttons, setup-request rows, Administration's "Open <org>'s Chat platforms".

**Bugs found in my own work, each with a test seen failing first.** (1) `ManageGate` mounted a Manage page for a
member when the organization answered before `/v1/me`, and threw `TypeError` when `/v1/me` failed. (2) The manage
page's route has a `ws` parameter, so the shell drew a workspace menu there; only `/w/` addresses are a workspace
place now.

**Emergency reader (FIX-277).** The workspace layout, the manage page and `lib/emergency.ts` are unchanged. A reader
is an organization owner, so the gate lets them through. A reader's workspace pages are recorded for the back line
(flag `reader`, because that workspace is not among their memberships); a workspace an administrator is only
redirected from is not recorded. `fix-235`, `fix-236`, `fix-277` and `fix-277-reader` pass.

**Security notes.** The redirect is navigation only; no API, authorization or route changed on the server. The back
line follows a stored path only for the same user id and only inside `/o/<org>/w/<ws>` (tests reject schemes, `//`,
backslashes, dot segments plain and encoded, sibling prefixes), and never reads it from a URL.

**Mockup comparison** (`e2e/places.spec.ts` screenshots at 1256 and 390 px, both themes, beside `after/nav-3b*` and
`after/nav-4*`; sample data differs). Fixed after the first comparison: the lead sat on the cards, People used the
two-person icon, a Needs you row's arrow wrapped at 390 px, the Workspaces table sat on its lead. Kept:
- no teal ring on the place card: it marks the new element in `nav-3b`/`nav-3`; `nav-4`, the card's own mockup, and
  `nav-1` have none;
- count badges on the menu entries as well as in Needs you: Scope 3 keeps today's badges;
- the rail's foot (rule above it, theme switch, "Instance admin" under the name, no Manage entry) is P5-NAV-3's. Because
  that foot is taller than the mockup's, at 783 px high the Manage and Administration menus scroll inside the rail
  (Agents, Data and Settings below the fold); the drawer does the same on a phone;
- Needs you shows the item's own action button ("Review", "Fix") where the mockup draws a status chip: it is the
  action the feed offers (and **Decline** on requests, which would otherwise be lost); the row and arrow open the page;
- waiting invitations (an uncounted row) and the History fold stay under Needs you: existing functions of this page;
- "Needs you" uses the app's section heading size, a step smaller than drawn;
- Workspaces has no mockup; it uses `ResponsiveTable`.

**Side effect of the width.** Without the 192 px column, Manage pages are wider. Library cards would have gone two per
row at 1256 px against D-506, so their second column now starts at 1024 px of content (`library-cards.tsx`, `14`).

**Left for later.** The member variants inside Manage pages (Agents, Chat platforms, AI models, Internet access) can no
longer be reached through the routes; their components and unit tests are unchanged.

**Tests changed** (why):
- new `tests/places.test.tsx`, `e2e/places.spec.ts`: this brief;
- `context-switcher`: back line and place cards replace "Back to <Org>"/"You're in"; "All n" per role; Account has no switcher;
- `visible-topics`, `roles`: new order with Workspaces; members get nothing;
- `refine-17-org`, `refine-20-batch3`: `pickRailWorkspace` → `lastWorkspace`; `peopleLine` went with the People card;
- `refine-17-18-feeds`, `refine-20-org-not-found`: `ManageFrame` → `OrgNavList`/`ManageGate`; no phone select;
- `org-manage`: the FIX-235 rows are on Workspaces;
- `palette-registry`, `command-palette`: no organization entry for a member;
- `internet-pages`, `skill-libraries-ui`: scope titles;
- e2e `ia-navigation`, `org-pages`, `who-can`: members are redirected (these replace the member-view journeys);
  `who-can`'s "member" was in fact an instance admin and is now a real member;
- e2e `p5-polish-8-switcher`, `fix-027-shots`, `fix-235`, `altitude`, `skill-libraries`: place cards, back line,
  Workspaces, scope titles; `support/mock-api.ts`: the new route in `ROUTES`.

**Commands and results** (all through `test.sh` / `heavy.sh`):
- vitest, every test file touching a changed module plus the four drift guards: 106 files, 1512 tests pass (before the
  two later fixes); after them, the 7 files of the gate fix (62) and the 7 shell files (93) pass;
- Playwright (mocked): `places`, `fix-235`, `fix-236`, `fix-277`, `fix-277-reader`, `org-pages`, `who-can` 91 pass on
  the final code; `ia-navigation`, `p5-polish-8-switcher`, `altitude`, `ia-3`, `ia-4`, `skill-libraries` pass, and the
  `org-` routes of `stage4-axe` (64) and `org-`/account routes of `stage4-routes` (52). The whole `stage4-axe` did not
  finish within one 10-minute command (242 passed, none failed, when stopped);
- `pnpm typecheck` and eslint on touched files: pass; `scripts/guards/run-all.sh`, `extract-schema.py --check`,
  `cargo fmt --all --check`: pass. No Rust, OpenAPI, schema or migration change.
- one flake to know about: in one run of those seven specs, `fix-236`'s owner journey timed out waiting for the
  address after **Open emergency read access** (`waitForURL **/o/acme/w/main`). That run still had the manage-page rail
  bug above, as had an earlier run in which it passed, so the cause is not established. After the rail fix it passed in
  the full rerun and then `--repeat-each=5` (50 of 50);
- fail-first, the new and updated tests over the old `web/src` (new modules left in place so they import): 40 of 105
  fail in `places` (23 of 29), `context-switcher` (12), `visible-topics` (3), `roles` (1) and `palette-registry` (1);
  `org-manage` passes there because the page it renders is the new one. The two later bugs were each seen failing
  before their fix.

**Screenshots** are not committed: `AURORA_SHOTS=<dir> scripts/dev/test.sh <lane> web playwright e2e/places.spec.ts
--project=mocked` writes them again (overview, rails and Workspaces; 1256 and 390 px; light and dark).
