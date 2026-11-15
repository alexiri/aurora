# P5-UX-9: One setup page per app, dashboard (owner app-setup walkthrough, 2026-11-15)

## Goal

Give every app one page, at the workspace and at the org level. The page says whether people can use the
app, what is missing and who can fix it, and lets the viewer fix what their role allows, right there.

In the walkthrough:
- the team lead read Linear's Details, then its setup guide, which sent her to a non-existent "Admin,
  Apps, Linear";
- she never learned that she could register a Linear OAuth client for her workspace herself;
- the org owner had five separate pages and no per-app view.

## Linked requirements

- D-330; contracts **C-932, C-933**; reads C-931 (P5-UX-8), C-909 `Unlock`, C-940 (guide with values,
  P5-UX-13).
- Normative: `14 §App setup` §7, `14 §Visual design`.

## Context

- **Apps tab.** `web/src/components/apps/apps-tab.tsx` has **Details**, which opens a dialog with tools,
  hosts and the agents' instructions.
- **OAuth clients.** `web/src/components/apps/oauth-clients-panel.tsx` is the form. It has the setup note,
  the redirect URI with Copy (~63–98), and the app picker (~174). The same panel is used on the workspace
  and org Credentials tabs and in Admin.
- **Org Apps.** Org › Apps is `org-apps-list.tsx` plus `availability-view.tsx` (posture notice ~57–65).
  Governance › Apps is `components/governance/apps-posture-panel.tsx`.
- **Self-hosted apps.** The pages live at `/o/[org]/instances` and `/admin/instances`.
- **Routes.** The route `/o/[org]/w/[ws]/apps/[app]/connect` exists and redirects (C-842). Add `page.tsx`
  at `/o/[org]/w/[ws]/apps/[app]` and `/o/[org]/apps/[app]`.

## Scope

1. **`AppSetupPage`** (`web/src/components/apps/app-setup/*.tsx`), one component for both views, from
   `GET /v1/apps/{app}/setup`.
   - **Header:** icon, name (family-aware, `appDisplayName`), description, availability chip.
   - **Status:** one sentence per method, with a state chip (Ready / Needs setup / Admin only). Each
     sentence is built from `SetupMethod` (label, `needs`, `unlock`). Put the copy table in
     `lib/apps/setup-copy.ts` with unit tests.
   - **What is set up:** sign-in clients by level, self-hosted servers, availability, connections.
     Connection rows link to the Connections tab.
   - **Values to copy:** the callback URL; the external id when present. Use the shared copy-value
     primitive; add one to the shared set if it is missing.
   - **Actions per `can`:**
     - register a workspace or org OAuth client (the existing panel's form in a `Dialog`, the app and
       client key preselected, the callback URL shown);
     - register a self-hosted server (link to the Self-hosted apps page with the app preselected, or that
       page's form in a `Dialog` if it is a single form);
     - set availability (org view, the C-20 toggle);
     - **Connect** (workspace: the P5-UX-3 journey; org: P5-UX-11's org mode, or hidden until it
       merges);
     - for anything above the viewer's level, the `unlock` sentence with **Copy request**.
   - **Tools:** the former Details dialog content (tools, hosts, instructions to agents) as a section.
   - **Setup guide:** the guide rendered with `?workspace_id=|org_id=` (C-940), expanded, last. Before
     P5-UX-13 merges, render the raw guide as today.
2. **Routes and links (C-932, C-933).**
   - Add the pages.
   - The Apps tab's **Details** goes to the workspace page.
   - Every app name in the workspace, org and Admin OAuth client lists, Admin › Self-hosted apps, Admin ›
     Plugins (app rows) and Connections links to the setup page at the viewer's level.
   - Org › Apps gets the posture control at its top (reuse `apps-posture-panel` content, same `PUT`);
     Governance › Apps keeps it and links to Org › Apps.
3. **Tests.**
   - Vitest: the copy table over the P5-UX-8 test fixtures (GitHub no clients; Linear with a workspace
     client; AWS with the external id; GitLab without an instance); the `can` → action mapping; links
     from each index.
   - Playwright:
     - a workspace owner opens Linear's page, registers a workspace OAuth client from it, and the status
       turns Ready;
     - an org member who is not an org admin sees the org-client action replaced by the unlock sentence
       naming the org admins.
   - Axe and narrow-route entries `ws-app-setup` and `org-app-setup`, both themes, 390 px.

## Constraints / file ownership

- Owns: `components/apps/app-setup/*` (new), the two `page.tsx` files, `apps-tab.tsx` (Details link),
  `oauth-clients-panel.tsx` (reused in a dialog; preselection props only), `org-apps-list.tsx` /
  `availability-view.tsx` (posture at the top), the link edits in the listed index components.
- Not beside P5-UX-15 (OAuth client picker labels in the same panel) or P5-UX-7 (Credentials).
- Shared primitives only. Any missing one (copy-value, status sentence row) goes into the shared set.

## Acceptance criteria

1. From the workspace Apps tab, **Details** for Linear opens its setup page. With no client, the page says
   sign-in needs a client and offers **Register an OAuth client for this workspace**. After registering
   one, the page says Ready and **Connect** works.
2. For AWS, the page shows the organization's external id with Copy.
3. An org admin's org page for GitHub shows clients at the org and instance levels, and the posture
   control.
4. Every index listed in C-933 links to the setup page.
5. Web gates (typecheck, lint, vitest, build), axe and narrow-route suites, both themes.

## Dependencies

P5-UX-8 merged. P5-UX-3 for the workspace Connect action; until it merges, link to the existing connect
dialog. P5-UX-13 for the guide with values; until it merges, render the raw guide.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). A new page assembled from existing pieces, many cross-links, and two views of
one component.
