# P5-POLISH-8: Navigation: one switcher, personal workspaces named from your side, the Administration rail, the account menu (owner, 2026-11-19)

## Goal

Make the rail say where you are, in one control that grows with the number of organizations and
workspaces. UX review 4 found:

- A new person sees two stacked pickers, "ORGANIZATION · Users" and "WORKSPACE · Bob Builder
  (personal)", plus a "Users › Bob Builder" pill above every page title.
- The personal workspace carries the person's own name: "This is Bob Builder, in Users".
- In Administration, the rail still shows "ORGANIZATION · Select organization" above the admin topics,
  and "Administration" appears both as a page badge and as the footer link.
- "All workspaces…" puts "All workspaces" in the workspace picker as if it were a workspace.
- The account menu:
  - is headed by an e-mail in caps;
  - has a thick teal frame (P5-POLISH-1 removes the frame);
  - has no icons or separators;
  - shows the admin's login "alice" instead of "Alice Admin".

The owner commented on the switcher proposal twice. First: "I'm not sure how well this single dropdown
is going to scale with many orgs and many Workspaces per org." Then: "Make the find look for Workspaces
or orgs."

## Linked requirements

- **Design reference (D-380).** Proposals **D4** (rail and switcher, with "After · switcher open") and
  **E8** (Administration rail), and the account-menu part of **F3**, in “Aurora UX fixes”,
  <https://claude.ai/artifact/WXbcpcrkbCLuxMSurpkuQW> (`#d4`, `#e8`, `#f3`).
  - Local copy `/home/alex/orca/workspaces/aurora/design-ux-fixes/`:
    - `after/d4.png`: the Welcome page with the new rail, then the switcher open; the multi-org
      example uses placeholder names (Acme Labs, Support team);
    - `after/e8.png`, `after/f3.png`;
    - `before/d4.png`, `before/e8.png`, `before/f3b.png`; `after/src/d4.html` for the exact rail.
    - The redrawn Aurora logo (two-tone teal ribbon with the small star, letter-spaced "AURORA") is in
      `after/src/d4.html`. Use the app's existing logo component, which is the source of that drawing.
  - The review: <https://claude.ai/artifact/33oHaXQpCU3oSVRVmgtY5Z> (screens 010, 060, 100, 300–302,
    400, 607).
  - Yellow dashed labels are commentary.
- **Decision:** **D-389** (the switcher and the names; the full rule is there). `14 §Information
  architecture` › the rail and level badge (C-960, C-961), amended by D-389.

## Wave and order

- **First, in parallel with P5-POLISH-1 and P5-POLISH-3.** It needs nothing new from the API.
- Lane C: **P5-POLISH-8** → P5-POLISH-10.
- If P5-POLISH-1 has merged first, rebase onto it. The account menu's frame comes from P5-POLISH-1;
  this brief changes the menu's content.

## Context

All paths are under `web/src/`.

- **Rail:** `components/app-shell.tsx`:
  - `NavItem` :55, `SidebarSelect` :96;
  - `OrgSelector` :165 (the "Select organization" placeholder :174), `WorkspaceSelector` :194;
  - org and workspace nav :312/:342, `PendingPill` :331;
  - `AdminNavList` :398 (chosen in `SidebarContent` :412 at :442);
  - the Administration footer link :448–457 (instance admins);
  - `UserMenu` (admin link :278);
  - the level value :485–490 (passed through `PageLevelProvider` :555);
  - the phone drawer :~512–550 (Radix Dialog, tour handling :474).
- **Level badge:** `components/ui/page-header.tsx:36` (`LevelBadge`), rendered at :104 and :112.
- **Routes:** `lib/routes.ts`, `lib/legacy-routes.ts`. The organization's workspace list page is
  `/o/<org>` (it lists workspaces).
- **Session data:**
  - `useSession` memberships;
  - workspaces carry `personal: boolean` and `personal_of?: PersonalOf | null` (`lib/api/schema.d.ts`
    around :10056; check the shape);
  - organizations and their workspaces are in the existing session or membership queries.
  - There is no server-side "recently used". Keep the last-used order per user in `localStorage` (try
    or catch; the list still works without it).
- **Welcome:** `components/onboarding/welcome-view.tsx` (cards :86–108; card 1 "This is <workspace>, in
  <org>"); the title "Welcome, <name>" in the person's own personal workspace (P5-UX-7).

## Scope

1. **One switcher (D-389).** Replace `OrgSelector` and `WorkspaceSelector` with one `ScopeSwitcher` at
   the top of the rail.
   - **The button** shows the workspace name (bold) on the first line and the organization name (muted)
     on the second, with an avatar of the workspace's initial. A person with exactly one organization
     and one workspace sees the same button without the chevron, and it is not interactive.
   - **The menu** (Radix popover or dropdown, max height and scroll inside), in this order:
     1. A search field, "Find a workspace or organization". It filters by name and groups the results
        under **Workspaces** and **Organizations**. Choosing an organization result opens its
        workspaces in the menu (step 4).
     2. **The current organization's workspaces:** the organization name as a section heading, then at
        most 5 workspaces, most recently used first. Each row shows the workspace name and the
        viewer's role there ("Owner", "Member"), with a check on the current one. Then "All <n> in
        <Org>" when there are more than 5.
     3. **Other organizations:** one row each, "<Org> · <n> workspaces".
     4. Choosing another organization replaces the list with that organization's workspaces, under a
        back arrow and the organization name.
     5. "All <n> organizations" (when there are more than 3), and the "All …" links, open the existing
        full list page (`/o/<org>` for one organization's workspaces; the organizations list page for
        all; reuse what exists, or add the page under `routes.ts` if there is none).
     6. **New workspace** at the bottom, when the viewer may create one.
   - Keyboard: arrow keys move, Enter chooses, Escape closes, and typing goes to the search field.
     Axe-clean.
   - The organization-level pages (Workspaces, AI models, …) are reached from the organization row's
     own menu entry ("Organization settings" under the organization heading, where the viewer may see
     them) and from the rail when you are on them. "All workspaces" never appears as a workspace
     value.
2. **Personal workspaces named from your side (D-389).**
   - One helper `workspaceDisplayName(ws, viewer)` in `lib/`:
     - "Your workspace" for the viewer's own personal workspace;
     - "<First name>'s workspace" for someone else's (first name from `personal_of.display_name`;
       the whole display name when it has one word);
     - the stored name for shared workspaces.
   - Use it in the switcher, the Welcome cards ("This is your workspace in Users. Daneel works here, on
     its own computer."), page titles that print the workspace, the organization's workspace list, and
     agent labels outside their workspace (P5-POLISH-9 uses it).
   - The stored name is unchanged.
3. **Level pill (D-389).**
   - Drop `LevelBadge` from `PageHeader` on workspace and organization pages, where the switcher says
     where you are.
   - Keep it only where a page is reached without the switcher showing its level (for example a page
     opened from a notification while the rail shows another workspace). In that case show
     "<Org> › <Workspace>" as plain text above the title, not a pill.
4. **Administration rail (E8).** In Administration the rail shows, as in `after/e8.png`:
   - the logo;
   - **Back to <Org>** (the organization and workspace you came from, kept in session storage;
     otherwise your first workspace);
   - a level header: "You're in" (eyebrow) and **Administration**;
   - the admin topics;
   - no switcher and no "Select organization".
   - The page header of admin pages has no "Administration" badge.
   - The footer's **Administration** entry stays (it is how you get in) and is marked as current there.
5. **Account menu (F3).** The `UserMenu`, as in `after/f3.png`:
   - **Header:** avatar, display name ("Alice Admin", never the login) and e-mail on two lines.
   - A separator, then **Your account** (user icon), **Administration** (shield; instance admins),
     **Show me around**, **What moved**.
   - A separator, then **Sign out** (out icon).
   - The footer under the rail shows the display name, and "Instance admin" when it applies.
   - Anywhere the rail or menus print a person, use the display name. Grep `app-shell.tsx` for
     `username` and `login`.
6. **Phone.** The drawer uses the same `ScopeSwitcher` (full-width popover, or a full-screen list on a
   phone) and the same Administration rail.
7. **Tests:**
   - **Vitest:**
     - `ScopeSwitcher`:
       - one org and one workspace: not interactive;
       - many: at most 5 recent, then "All n in Org";
       - search matches workspaces and organizations, grouped;
       - an organization result opens its workspaces;
       - the back arrow;
       - New workspace only when allowed;
       - keyboard;
     - `workspaceDisplayName` cases;
     - `PageHeader` without the pill;
     - the admin rail's Back link;
     - the account menu's items per role.
   - **Playwright** (mocked), 1280 and 390 px, both themes, axe, no console errors:
     - switch workspaces;
     - search for an organization;
     - go to Administration and come back;
     - Welcome shows "This is your workspace in Users".
     - Add mock data with 2 organizations and 8 workspaces.
   - Update the specs that clicked the old selectors (`ia-navigation`, tour, `who-can` and others).

## Out of scope

- Topic order and visibility (P5-IA-1, unchanged).
- Page content.
- The general wording pass.

## Constraints / file ownership

- **Owns:** `components/app-shell.tsx`, `components/ui/page-header.tsx`, a new
  `components/nav/scope-switcher.tsx`, `lib/workspace-name.ts` (new), the Welcome card sentence in
  `components/onboarding/welcome-view.tsx` (card 1 only), the rail parts of `lib/routes.ts`.
- Do not change `getting-started.tsx` (P5-POLISH-7 owns the Next cards).
- Tokens and primitives only. The rail keeps its navy island in both themes (D-354).

## Acceptance criteria

1. Bob (one organization, one workspace) sees one non-interactive switcher reading "Your workspace /
   Users", and no level pill (`after/d4.png`).
2. With 2 organizations and 8 workspaces, the menu shows 5 recent workspaces of the current organization,
   "All 8 in Users" and the other organization as one row. The search finds both kinds (`after/d4.png`,
   the open-switcher part).
3. Administration's rail has Back to <Org> and the "You're in Administration" header, with no
   organization picker (`after/e8.png`).
4. The account menu is headed by the display name and e-mail, with icons and Sign out apart
   (`after/f3.png`).
5. Web gates: typecheck, lint, vitest, build, Playwright mocked (axe, narrow-route, both themes).
   Report the counts.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). Navigation used on every page; keyboard and accessibility matter.

## Completion report — review pass 1 (P5-POLISH-11)

- **#1 done.** The switcher menu is sized to `--radix-popover-content-available-height` (no fixed 24 rem), and the search
  box is sticky, so it stays in view after the drill-in.
- **#2 done.** A personal workspace's avatar is its owner's initial (viewer's display name for your own).
- Gates: typecheck, `context-switcher` vitest (22 passed), Playwright `p5-polish-8-switcher.spec.ts` (+ connect-journey)
  37 passed.
