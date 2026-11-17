# P5-IA-4: People, Data, Platform, Chat platforms and Settings as coherent pages (owner, 2026-11-15)

## Goal

P5-IA-1 moved the existing pieces under these topics. This task makes each a coherent page:
- **People.** Sign-in was spread over an Identity page that said "set in Helm, never here", Sign-up &
  placement and a settings section. The workspace's people were a Settings tab.
- **Data.** An organization's conversation retention was on Settings, attachment retention and quota on
  Storage, and Administration's maximums on a third page.
- **Platform** and **Chat platforms**: named differently at each level.
- **Settings**: what remains once everything else has a topic.

## Linked requirements

- **Design** (D-354): the canvas “Aurora — agents, accounts and access”, https://claude.ai/artifact/3UwmUgrhh6MLCsfqdhTFmQ, boards Administration › People › Sign-in, Data › Retention, Platform › Email, Chat platforms; Organization › People, Data › Retention, Chat platforms, Settings; Workspace › Settings; Agent › Agents (list) (the BYO row). Read each board's note; where a board and the spec disagree, the spec wins.
- D-338, D-339, D-340, D-351; contracts **C-968, C-969, C-970, C-967 (the workspace part)**; uses C-963
  (`ScopeLadder`) and C-971 (`SettingsSectionFields`).
- Normative: `14 §Information architecture` (People, Data, Platform, Chat platforms and Settings rows;
  the settings map), `15 §Instance settings` (sections; unchanged).

## Wave and order

- **Wave 2.** Starts after P5-IA-1 (the pages, `SettingsSectionFields`) and P5-IA-2 (`ScopeLadder`).
- Disjoint from P5-IA-3 (other pages; this task alone edits `components/settings/settings-view.tsx`
  and the workspace's agents list header) and P5-IA-5 (the settings form's rendering).
- **SET-4/5** (lane `p5-apps`) register the `sign_in`, `email`, `retention`, `scim` and `limits` sections
  that render on these pages. They add no navigation (D-340). The Orchestrator orders this task after
  SET-4.
- Consumers: P5-UX-5 (Add people on the workspace's People page), P5-UX-6 (Administration › Overview,
  New users).

## Context

- **Administration:** Users, Organizations (and detail), Identity (`/admin/identity`), placement
  (`components/admin/placement-view.tsx`), secret stores (`components/admin/secret-stores-view.tsx`),
  runners, storage, audit, connectors.
- **Organization:** members (`components/org/members-view.tsx`), groups, directory sync; settings
  (`components/org/settings-view.tsx`: general, conversation retention, freeze, delete); storage (usage
  and attachment limits); audit; API keys; channels.
- **Workspace:** `components/settings/settings-view.tsx` (General, Access, API keys, Audit, Danger zone);
  `components/settings/access-tab.tsx`.

## Scope

1. **People (C-968).**
   - **Administration** `/admin/people`: four tabs.
     - **Users**.
     - **Organizations** (detail at `/admin/people/organizations/<id>`).
     - **Sign-in**: the `sign_in` section, the role-mapping view that was Identity, and the `scim`
       section (directory sync) as one page with three headed parts.
     - **New users**: placement. The setup checklist embeds the same component.
   - **Organization** `/o/<org>/people`: **Members**, **Groups** (detail at
     `/o/<org>/people/groups/<id>`), **Directory sync**.
   - **Workspace** `…/people`: what was Settings › Access, as a page of its own.
   - Delete the Identity page (its route redirects).
2. **Data (C-969).**
   - **Administration** `/admin/data`: **Retention** (the `retention` section), **Storage** (usage),
     **Audit log**.
   - **Organization** `/o/<org>/data`:
     - **Retention**: one form with conversation days (from Settings) and attachment days and quota
       (from Storage), under Administration's maximums as ladder rows;
     - **Storage**: usage;
     - **Audit log**.
3. **Platform and Chat platforms (C-970).**
   - **Administration** `/admin/platform`: **Runners**, **Secret stores**, **Email** (the `email`
     section), **Rate limits** (the `limits` section).
   - **Chat platforms**: `/admin/chat-platforms` and `/o/<org>/chat-platforms`. Content unchanged; titles
     and every sentence that names the page use the new name.
4. **Settings.**
   - **Organization** `/o/<org>/settings`: **General** (name, URL, whether members can create workspaces,
     the member and agent limits), **API keys**, **Danger zone** (freeze, delete). Retention has moved to
     Data.
   - **Workspace** `…/settings`: **General** (name, time zone, memory language), **API keys**, **Audit**,
     **Danger zone**. People has moved (scope 1).
   - **The workspace's BYO setting (C-967)** moves, in this task, from Settings › General to the header
     of the workspace's agents list, as a ladder row: the organization's value above, the workspace's
     choice behind **Tighten…**.
5. **Empty states and read-only views.** Members who can open a page here see values as text and no
   controls (`14` "What a person sees of something they cannot do").
6. **Tests.**
   - Vitest: each tab renders its content; the organization's Retention form saves both values and shows
     the maximums above; the Sign-in page's three parts.
   - Playwright: change the organization's attachment retention on Data › Retention; open Sign-in from
     Administration; a workspace owner opens People from the sidebar; a workspace's agents list shows
     the organization's BYO value; old URLs land on the right tabs.
   - Axe and narrow-route suites, both themes.

## Constraints / file ownership

- Owns: the People, Data, Platform, Chat platforms and Settings pages at each level; the moved components
  (composition); the workspace BYO field's move to the agents list header; the deletion of the Identity
  page.
- Administration › Overview is P5-UX-6's.
- No API change.

## Acceptance criteria

1. People, Data, Platform, Chat platforms and Settings are one page each per level, with at most four
   tabs.
2. Sign-in is one page; an organization's retention is one form with Administration's maximums above it.
3. A workspace's people are a page in the sidebar, not a Settings tab.
4. Web gates; axe and narrow-route suites, both themes.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`): page composition, with one merged form.

## Completion report (lane p5-apps, 2026-11-17)

Commits: b3962aba (Data), 6b4b1246 (Sign-in, Chat platforms), 5c675fcc (BYO move, Playwright), d8cf9592 (gate fix).
Web only; no Rust, API, OpenAPI or schema change.

- **Already in place from P5-IA-1** (verified, not rebuilt): the People/Data/Platform/Chat platforms/Settings routes and tabs at each level, the redirects (incl. `/admin/identity`), the workspace People page, org Settings General with the member/agent limits, Platform Email and Rate limits.
- **Data (C-969):** `OrgRetentionForm` (`components/org/retention-form.tsx`): conversation days, file days and storage quota in one `PUT`, Administration's maximums as `ScopeLadder` rows, Discard changes. Storage keeps usage only; the old `RetentionForm`, `LimitsForm`, `OrgSettingsView` and `usePutRetention` are gone; tabs read "Audit log".
- **People › Sign-in (C-968):** one page of three headed parts: How people sign in (`sign_in`), Roles from the provider (new `ProviderRolesView`: the Identity page's mapping, read from the providers' `role_claim`/`role_mappings`, plus what the viewer's last sign-in made of them), Directory sync (`scim`).
- **Chat platforms (C-970):** titles and the sentences that named Connectors/Channels now say Chat platforms ("Open Administration › Chat platforms"); one header per page.
- **Workspace BYO (C-967):** `WorkspaceByoRow` in the agents list header (organization/instance value above, workspace choice behind Tighten…, text for members); removed from Settings › General (now name, Time zone, memory language).
- **Left for P5-IA-3** (Agents pages, not this brief's pages): Administration › Agents still stacks `agents`+`approvals` and `terminal`+`previews`.
- **Differences from boards:** AdminData's "organizations below" table is not built (D-351: the below table exists only for Internet access); quota is edited in MiB (the API's unit), the board shows GB.
- **Gates:** vitest 119 files / 2027 tests pass; typecheck, lint (0 errors; 3 existing warnings), `pnpm build`, `gen:api` (no diff), guards, `extract-schema --check`; Playwright mocked project 952 passed / 5 failed under load ~19: agent-tabs ×4 (page.goto timeouts) pass on rerun alone (8 passed), fix-127-mattermost was a real regression of mine (wrapping the page in an embedded header hid the connector title), fixed in d8cf9592 and green with stage4-routes/stage4-axe chat-platforms (93 passed); production project 8 passed; new `e2e/ia-4.spec.ts` 16 passed. Not run: cargo clippy/nextest, OpenAPI drift test (no Rust touched since the master merge).
- **Rule note:** the first commit's block removals used one throwaway python script (stated in its message).
