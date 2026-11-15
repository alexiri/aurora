# P5-IA-1: One structure for every level, navigation shell and routes (owner, 2026-11-15)

## Goal

The owner's administration walkthrough found the dashboard organized by when each feature was built:
- 18 Admin sidebar items plus 13 Settings sections;
- 14 org items;
- the same topic under different names at each level (Governance, Policy and Settings › Governance for
  internet access);
- Administration reachable only through "All organizations" in the org switcher.

The owner approved one structure with the same topics at every level and asked for a coherent experience
(D-338, D-342).

This task builds the **shell**: the navigation per level, the routes and tabs, the redirects from every
old route, role visibility, the Administration entry, the level badge and the tour. Existing page
content moves under the new routes **unchanged**; the content work is P5-IA-2..5 and the P5-UX briefs.

## Linked requirements

- D-338, D-342; contracts **C-960, C-961, C-962, C-975**; C-915 (refusals, P5-UX-1) and C-891 (gateway
  link builder) are touched for paths only.
- Normative: `14 §Information architecture` › "One structure for every level" (vocabulary, topics per
  level, who sees what, Administration entry), `14 §Onboarding` §3 (tour), `14 §Visual design`.

## Context

- **Navigation.** `web/src/components/app-shell.tsx`:
  - `ADMIN_LINKS` (18 items);
  - `ORG_LINKS` and `OrgNav` (~283–323);
  - `WorkspaceNav` (~337–368);
  - `NavItem` `data-tour` (~68).
- **Org switcher.** It lists "All organizations" as the way into Admin.
- **Pages.** They live under `web/src/app/admin/*`, `web/src/app/o/[org]/*` and
  `web/src/app/o/[org]/w/[ws]/*`. Credentials, governance and policy components are listed in
  P5-IA-2..4.
- **Tour.** `web/src/components/onboarding/tour.tsx` `TOUR_STOPS` (~19–28) drops stops whose nav link is
  missing (~87).
- **Header.** `PageHeader`: `web/src/components/ui/page-header.tsx`.
- **Role data.** `useOrg().my_role`, `useWorkspace().my_role`, `useCanApprove`. This task adds
  `useViewerRoles` (`lib/api/roles.ts`); P5-UX-1, which runs after it, uses it.
- **Gateway links.** The C-891 link builder lives in the gateway.

## Scope

1. **Route helper** `web/src/lib/routes.ts`: one function per topic and tab at each level, plus the setup
   page and the journey deep links (C-959). Replace every hard-coded internal `href` with it, and add a
   test that fails on a hard-coded old path anywhere in `web/src`.
2. **Topic pages as tab hosts.** For each row of `14`'s table, create the route (`page.tsx`) with
   `PageHeader` and a tab bar (`?tab=`, the existing tab primitive). Each tab renders the **existing**
   component of the page it replaces (for example `admin/plugins` content under
   `/admin/apps?tab=packages`). Tabs whose content another brief builds render the existing page until it
   lands:
   - Internet access (P5-IA-2) renders org Governance › Egress above the org Policy component, and the
     workspace Policy component.
   - AI models, Agents, People, Data and Platform (P5-IA-3/4) render the old components side by side.
3. **Redirects (C-960).** Implement every redirect of C-960: Next.js `redirects` for static paths, plus a
   small client redirect for `?tab=` mappings on pages that stay (`/o/<org>/governance`,
   `…/credentials`). Preserve the query and the anchor. The C-842 `connect=` and `reconnect=` links
   survive.
4. **Navigation (C-961).** Each level's sidebar lists its topics in `14`'s order, from
   `visibleTopics(level, roles)`:
   - org members: Workspaces, AI models, Internet access, Agents (read-only);
   - workspace members: no People or Settings;
   - Approvals only for approvers;
   - Administration only for instance admins.

   Read-only pages hide write controls and show one line naming who can change them. This task owns all
   navigation gating; P5-UX-1 (after this task) owns refusals and empty states.
5. **Administration entry and level badge (C-962).**
   - **Administration** goes in the sidebar footer and the account menu for instance admins.
   - The org switcher lists organizations only.
   - `PageHeader` gains `level`, and every page passes it:
     - "Administration";
     - "<org>";
     - "<org> › <workspace>".
6. **Labels.** Apply the vocabulary table to nav labels, page titles and tab names (C-960 / `14`). Inside
   page bodies, change only text that names a page or a navigation place ("see Governance" becomes "see
   Internet access"). Content wording belongs to the topic briefs.
7. **Gateway and docs links.**
   - Update the gateway's C-891 builder and its tests to the new paths (the old ones redirect, so this is
     cleanliness, not breakage).
   - Grep `docs/operator/`, `docs/14` page references and the builtin `docs/setup.md` guides for page
     names ("Admin ›", "Governance", "Policy page", "Credentials ›"), and update the operator docs.
   - Builtin guides are P5-UX-13's (placeholders); list what you found in the report for it.
8. **Tour (C-975).** One stop per visible workspace topic with its new name, plus the Agents › Access
   sentence of P5-UX-3. It is driven by `visibleTopics`.
9. **Tests.**
   - Vitest: the route helper; `visibleTopics` per role and level; each C-960 redirect (a table test);
     the level badge.
   - Playwright:
     - an instance admin reaches Administration from the footer link;
     - a member's org sidebar shows exactly Workspaces, AI models, Internet access and Agents;
     - every old route in C-960 lands on its new tab (one parametrised test);
     - `…/credentials?tab=apps&connect=github` opens the connect dialog (or journey) at the new route.
   - Axe and narrow-route suites gain the new topic pages (both themes, 390 px; at 390 px the tab bars
     scroll inside their own container).

## Constraints / file ownership

- **Owns:**
  - `app-shell.tsx`;
  - the new route folders and redirect config;
  - `lib/routes.ts`;
  - `page-header.tsx` (the `level` prop);
  - `tour.tsx`;
  - the org switcher;
  - link edits across `web/src`, done mechanically through the helper;
  - the gateway link builder (paths);
  - operator doc page references.
- **Does not change** page contents beyond labels and the "see …" texts; it moves them.
- **Runs first** among P5-IA and the P5-UX screen briefs (D-342). Not in parallel with any brief that adds
  pages under `web/src/app` (P5-UX-3, -5, -7, -9, -10, -11, P5-IA-2..4): those start after this merges,
  or build at the new routes and leave the redirects to this task.
- **SET-4/5:** their section pages must not add sidebar entries (D-340); this task does not wait for them.

## Acceptance criteria

1. Each level's sidebar shows the `14` topics for each role, and no item leads to a refusal.
2. Every C-960 old route redirects to the right tab, and no source file hard-codes an old path.
3. Administration has a labelled entry; the org switcher lists organizations only; every page shows its
   level badge.
4. Existing page contents work unchanged at their new routes (the existing page tests pass with updated
   paths only).
5. Web gates (typecheck, lint, vitest, build); axe and narrow-route suites, both themes; gateway tests for
   the link builder.

## Dependencies

None. Merge before the content briefs (D-342 order).

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). It is wide but mechanical: routing, navigation and a large link sweep through
a helper. The redirect table and visibility function need tests more than design.
