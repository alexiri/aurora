# P5-UX-9: One setup page per app (owner, 2026-11-15)

## Goal

Give every app one page, for a workspace and for an organization, that says whether people can use the
app, what is missing and who can fix it, and lets the viewer fix what their role allows, right there.

In the app-setup walkthrough:
- a team lead read Linear's Details, then its guide, which sent her to a page that does not exist;
- she never learned that she could register a Linear OAuth client for her own workspace;
- the organization's owner had five separate pages and no view per app;
- the OAuth client form offered four GitHub entries told apart only by internal keys;
- whether an organization allows all apps or only some was set in Governance, while the list of apps was
  on another page.

## Linked requirements

- D-330, D-334, D-351; contracts **C-932, C-933, C-946 (forms)**; reads C-931, C-940, C-917, C-942.
- Normative: `14 §Agents, accounts and access` §4, `14 §Information architecture` (Apps & accounts row,
  "Who can fix this"), `14 §Visual design`.

## Wave and order

- **Wave 2**, after P5-UX-3 in the same web lane (both edit `apps-tab.tsx`).
- Starts after P5-UX-2 (setup status), P5-UX-13 (guides with values, `CopyValue`), P5-UX-4 (accounts
  rows) and P5-IA-1 (the Apps & accounts pages).
- Consumers: P5-UX-6 (Administration › Overview links here), P5-UX-11 (the organization's Connect).

## Context

- **Apps tab.** `web/src/components/apps/apps-tab.tsx`: **Details** opens a dialog with the app's tools,
  hosts and the instructions shown to agents.
- **OAuth clients.** `web/src/components/apps/oauth-clients-panel.tsx`: the form with the setup note and
  the redirect URI with Copy (~63–98), and the app picker labelled `${app_name} (${client_key})` (~174).
  Used on the organization's and Administration's OAuth clients tabs; in a workspace it is reached from
  the Apps tab (P5-IA-1).
- **Organization's Apps tab.** `org-apps-list.tsx` and `availability-view.tsx` (the notice that points
  to Governance ~57–65). The availability switch is `components/governance/apps-posture-panel.tsx`, shown
  as the Apps tab of `governance-view.tsx`.
- **Self-hosted apps tabs.** At `/o/<org>/apps?tab=self-hosted` and `/admin/apps?tab=self-hosted`.
- **Routes.** `/o/[org]/w/[ws]/apps/[app]/connect` exists and redirects (C-842). This task adds the
  pages at `/o/[org]/w/[ws]/apps/[app]` and `/o/[org]/apps/[app]`.

## Scope

1. **`AppSetupPage`** (`web/src/components/apps/app-setup/*`), one component for both views, from
   `GET /v1/apps/{app}/setup`.
   - **Header:** icon, name (family-aware), description, the level badge, an availability chip.
   - **Status:** one sentence per way to connect, with a chip (Ready / Needs setup / Admin only), built
     from the method's label, `needs` and `who_can`. Put the copy table in `lib/apps/setup-copy.ts`, with
     unit tests.
   - **What is set up:**
     - OAuth clients by level. In the workspace view, the workspace's clients for this app are listed
       here with Register, Edit and Remove (the panel's form in a `Dialog`, app and client preselected,
       the callback URL shown).
     - Self-hosted apps registered for it.
     - Availability in the organization.
     - The app's accounts: `AccountsList` filtered by `app_id`.
   - **Values to copy:** `CopyValue` for the callback URL and, when present, the external id.
   - **Actions,** per `can`:
     - **Connect** (the journey);
     - in the organization view: register an organization client, register a self-hosted app (a link to
       the Self-hosted apps tab with `?package=<package>`, which opens its form with the app chosen),
       change availability;
     - for anything above the viewer's level, the `WhoCan` sentence.
   - **What it can do:** the former Details content (tools with their descriptions, hosts, instructions
     shown to agents).
   - **Setup guide:** the guide with the viewer's values (`?workspace_id=|org_id=`), expanded, last.
2. **Links (C-933).**
   - The Apps tab's **Details** goes to the setup page; the dialog is removed.
   - Every app name in the Apps, OAuth clients, Self-hosted apps and Accounts tabs of a workspace or an
     organization links to the setup page at the viewer's level.
   - Administration has no setup page. An app name there opens its catalogue entry
     (`/admin/apps/packages/<id>`); add to that entry the link **OAuth clients for this app** (the OAuth
     clients tab with `?package=<package>`).
   - The workspace Apps tab no longer links to a separate OAuth clients list.
3. **OAuth clients named by method (C-946).** The OAuth client form's app picker and the client lists
   show "<app> — for <method label>" from `used_by_methods`; the `client_key` is a tooltip.
4. **Availability on the organization's Apps tab.** Put the availability switch (all apps, or only the
   ones listed) above the list, with one Save, in words ("People can connect every app" / "People can
   connect only the apps switched on below"). Remove the Apps tab and panel from `governance-view.tsx`.
5. **Tests.**
   - Vitest: the copy table over fixtures (GitHub with no clients; Linear with a workspace client; AWS
     with the external id; GitLab without a registered server); the `can` → action mapping; the links
     from each tab; the picker labels.
   - Playwright:
     - a workspace owner opens Linear's page, registers a client for the workspace there, the status
       turns Ready, and **Connect** opens the journey;
     - a workspace owner who is not an org admin sees who can register an organization client, by name;
     - the AWS page shows the organization's external id with Copy;
     - an org admin switches the organization to "only the apps switched on below" on the Apps tab.
   - Axe and narrow-route entries `ws-app-setup` and `org-app-setup`, both themes, 390 px.

## Constraints / file ownership

- Owns: `components/apps/app-setup/*` (new); the two `page.tsx` files; `apps-tab.tsx` (Details link);
  `oauth-clients-panel.tsx` (dialog use, preselection, labels); the Self-hosted apps tab's `package=`
  preselection; `org-apps-list.tsx` and `availability-view.tsx` (the switch); `governance-view.tsx`
  (removing the Apps tab); the link edits in the listed tabs and on the Administration catalogue entry.
- Uses, and does not change, `AccountsList`, `WhoCan`, `CopyValue` and the journey.

## Acceptance criteria

1. From the workspace's Apps tab, **Details** for Linear opens its setup page. With no client, the page
   says sign-in needs one and offers **Register an OAuth client for this workspace**. After registering
   one, the page says Ready and **Connect** works.
2. For AWS, the page shows the organization's external id with Copy.
3. An org admin's page for GitHub shows clients at the organization and Aurora levels, and each client
   is named by the way of connecting it serves.
4. Which apps people may connect is set on the organization's Apps tab, with the list.
5. Web gates (typecheck, lint, vitest, build); axe and narrow-route suites, both themes.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). A new page assembled from existing pieces, many cross-links, and two views of
one component.
