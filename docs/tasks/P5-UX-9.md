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

- **Design** (D-354): the canvas “Aurora — agents, accounts and access”, https://claude.ai/artifact/3UwmUgrhh6MLCsfqdhTFmQ, boards Workspace › Apps & accounts › Linear (setup page) and › Apps; Organization › Apps & accounts › Apps; Administration › Apps & accounts › Apps. Read each board's note; where a board and the spec disagree, the spec wins.
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

## Completion report (worker 27, 2026-11-17)

Commits on `alexiri/p5-apps`: `3a23d352` (OpenAPI/hooks/copy table), `214c7661` (page, links, clients, availability),
`39447394` (the obsolete Details-dialog e2e test), `830d938b` (merge of master, which carries the regenerated
`spec_schema.json`).

**Found and fixed (outside the brief's Owns, needed by it).** `GET /v1/apps/{app}/setup` answered a struct named
`SetupStatus`, the same name as the admin setup wizard's response. utoipa keeps one schema per name, so `openapi.json`
and `schema.d.ts` typed the app route as the wizard (`placement_saved`, `steps`) and none of C-931's fields existed in
the web types. The struct is now `AppSetupStatus` (`services/aurora-api/src/domains/apps/{setup,routes}.rs`); OpenAPI
and `schema.d.ts` regenerated.

**Built.**
- `lib/apps/setup-copy.ts` (+ `tests/setup-copy.test.ts`): chip per method (Ready / Needs setup / Admin only, by `needs`
  and the viewer's `can`), one sentence per method, the headline, `clientLabel` ("GitHub — for Sign in with GitHub"), the
  `can` → actions mapping.
- `components/apps/app-setup/{app-setup-page,sections}.tsx`, the two `page.tsx` files, `components/apps/app-link.tsx`
  (`useAppHref`/`AppLink`: workspace page, organization page, or the Administration catalogue entry by the page's level).
  The workspace view gates on workspace owner (a member reads `WhoCan`); the organization view on `OrgGate` owner/admin.
  Organization **Connect…** opens the existing choose-a-workspace dialog: the journey's organization mode is P5-UX-11.
- `AccountsList` gained an optional `appId` (filter, no Add, one-line empty text); the brief says it is used unchanged,
  this is the minimal additive prop the page needs.
- OAuth clients: forms and lists say "<app> — for <method>" (key as tooltip), `OAuthClientDialog` for Register on the
  page (Edit/Remove stay inline in the row), `?package=[&client_key=]` opens the form with the client chosen; Self-hosted
  tab `?package=` opens its form with the app chosen; app names in the Apps, OAuth clients, Self-hosted and Accounts tabs
  link (members' account rows stay plain text); the catalogue entry links **OAuth clients for this app**.
- Organization Apps tab: one switch (radio, in words) + the list with switches + one Save; `apps-posture-panel.tsx`
  and the Governance Apps tab are deleted.
- The workspace Apps tab no longer lists OAuth clients; the Details dialog and `AppDetailBody` are gone (the content is
  "What it can do" on the page).

**Left / not done.** "Self-hosted apps registered for it" is shown for the viewer's organization only (the setup
status lists those); the guide sits last at full width, not in the aside as on the board (the spec says last).

**Gates (batch, after P5-UX-9).** `cargo fmt --all --check` ok; `bash scripts/fixtures/build.sh` 57/57; `cargo test -p
aurora-api -- --test-threads=8` (all targets) exit 0; `cargo clippy --workspace --all-targets --all-features -- -D
warnings` exit 0; `pnpm -C web typecheck`, `lint` (4 pre-existing warnings), full `vitest` exit 0, `build` exit 0, `gen:api`
no diff; Playwright mocked full run (`--project mocked`) 1049 passed, 4 failed — all four the obsolete "Details shows the
close icon" test, replaced and re-run (`connect-dialog.spec.ts` 17/17); `e2e/app-setup.spec.ts` 20/20 (both themes, 1280
and 390 px; the 390 px Save uses `dispatchEvent` because the Next dev indicator covers it); production project 8/8.
`guards` and `extract-schema.py --check` were red on the branch before merging master (stale `spec_schema.json`, fixed on
master by `f24c0abf`); both green after the merge. Axe and narrow-route entries `ws-app-setup`, `org-app-setup` are in
the suites (passed in the full run).
