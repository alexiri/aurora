# P5-IA-4: People, Data, Platform, Chat platforms and Settings; sections inside topics (owner, 2026-11-15)

## Goal

These topics are scattered too.
- **Sign-in:**
  - Admin › Identity, read-only, saying "set in Helm, never here";
  - Sign-up & placement;
  - an empty Settings › Sign-in section;
  - Setup steps 1–2.
- **Retention:**
  - org conversation days on Org › Settings;
  - attachment days and quota on Org › Storage;
  - the instance maximums in an empty Settings section;
  - usage on Admin › Storage.
- **Secret stores** is one page reachable from two places.
- **Chat platforms** are "Connectors" at instance level and "Channels" at org level.
- **Settings sections** are separate pages beside all of this.

This task builds the People, Data, Platform and Chat platforms topics and the org and workspace Settings
pages. It also makes every settings section render **inside** its topic, with `/admin/settings` as the All
settings index (D-338, D-340).

## Linked requirements

- D-338, D-339, D-340; contracts **C-968, C-969, C-970, C-971**; uses C-963 (ladder) and C-973 (read-only
  values, P5-IA-5).
- Normative: `14 §Information architecture` (People, Data, Platform, Chat platforms, Settings rows;
  "Settings live in their topics"), `15 §Instance settings` (sections, drafts, locks: unchanged).

## Context

- **Settings:** `components/admin/settings/settings-index-view.tsx`, `settings-section-view.tsx`,
  `settings-section-form.tsx`. The section registry is `crates/aurora-config/src/settings.rs`.
- **Admin pages:**
  - Users (`/admin/users`), Organizations (`/admin/orgs`, detail);
  - Identity (`/admin/identity`);
  - placement (`components/admin/placement-view.tsx`);
  - secret stores (`components/admin/secret-stores-view.tsx`);
  - runners, storage, audit, connectors.
- **Org pages:**
  - members (`components/org/members-view.tsx`), groups, directory sync;
  - settings (`components/org/settings-view.tsx`: general, retention, freeze, delete);
  - storage (usage and attachment limits);
  - audit, API keys, channels.
- **Workspace pages:** `components/settings/settings-view.tsx` (General, Access, API keys, Audit, Danger
  zone).
- **SET-4 (in progress)** moves sign-in, e-mail and identity providers into the `sign_in` and `email`
  sections, and removes the Identity entry with a redirect to the sign-in section. **SET-5** (queued)
  registers agents, approvals, retention, skills, scim, previews, terminal and limits. Both build sections
  and section pages but add no sidebar entries (D-340). This task hosts their sections.

## Scope

1. **`SettingsSectionFields` (C-971).**
   - Factor the section form into a component that renders a whole section, or a `keys` subset, inside
     any page. It keeps the section's draft, preview, Helm locks, errors and save, sending the full
     document with untouched keys unchanged.
   - `settings-section-view.tsx` becomes a thin wrapper used only by All settings' search results, if at
     all.
   - `/admin/settings/<section>` redirects to the topic per `14`'s table (P5-IA-1 holds the table;
     confirm it here).
2. **All settings** (`/admin/settings`).
   - The index lists sections grouped by topic, each linking to its topic tab.
   - A search box over field labels and help (C-972 once P5-IA-5 merges; section names and descriptions
     until then).
   - Export and Import stay.
   - The Helm notice stays.
3. **People (C-968).**
   - **Administration** `/admin/people`:
     - **Users**;
     - **Organizations** (and `/admin/people/organizations/<id>`);
     - **Sign-in**: the `sign_in` section, plus the role-mapping view from Identity. Until SET-4 merges,
       show Identity's content there;
     - **New users**: placement. The setup wizard's step 2 keeps embedding the same component;
     - **Directory sync**: the `scim` section.
   - **Organization** `/o/<org>/people`: Members, Groups (detail at `/o/<org>/people/groups/<id>`),
     Directory sync.
   - **Workspace** `…/people`: was Settings › Access. P5-UX-5 adds Add people there; if it merged first,
     move its dialog with the tab.
4. **Data (C-969).**
   - **Administration** `/admin/data`: **Retention** (`retention` section), **Storage** (usage),
     **Audit log**.
   - **Organization** `/o/<org>/data`:
     - **Retention**: one form with conversation days (from Settings) and attachment days and quota
       (from Storage), and the instance maximums as ladder rows above (from `retention` when registered;
       until then, as `GET /v1/instance` or the org page reports them today);
     - **Storage** (usage);
     - **Audit log**.
5. **Platform and Chat platforms (C-970).**
   - **Administration** `/admin/platform`: **Runners**, **Secret stores** (one page; drop the Settings
     duplicate), **Email** (`email`), **Limits** (`limits`).
   - **Chat platforms**: `/admin/chat-platforms` (was Connectors) and `/o/<org>/chat-platforms` (was
     Channels). Content is unchanged; titles and the sentences that name the page are updated.
6. **Settings pages.**
   - **Organization** `/o/<org>/settings`:
     - **General**: name, URL, members can create workspaces, member and agent limits;
     - **API keys**;
     - **Danger zone**: freeze, delete.

     Retention has moved to Data.
   - **Workspace** `…/settings`: **General** (name, time zone, memory language; BYO moved to Agents in
     P5-IA-3), **API keys**, **Audit**, **Danger zone**. Access has moved to People.
7. **Overview** (`/admin`). The setup wizard is the checklist until setup is complete. Then the page shows
   health from the setup status (C-533):
   - runners, gateway and endpoints;
   - apps nobody can sign in to (C-924, P5-UX-6, if merged);
   - storage.

   The wizard stays reachable as "Run setup again".
8. **Tests.**
   - Vitest: `SettingsSectionFields` with a subset (saving keeps the other keys); the All settings index
     grouping and search; each moved tab; the org Retention form saving both values.
   - Playwright: change the org's attachment retention on Data › Retention; open Sign-in from the footer
     Administration link; old URLs land on the right tabs.
   - Axe and narrow-route suites, both themes.

## Constraints / file ownership

- **Owns:**
  - the settings form components;
  - the People, Data, Platform, Chat platforms and Settings pages at all levels;
  - the moved components (composition);
  - the Admin Overview.
- **Coordinate with SET-4/5 on lane `p5-apps`.** Their section components are hosted here; they add no
  nav. If SET-4 has changed `settings-section-form.tsx`, rebase onto it. Do not run beside SET-4/5 on
  the same files; the Orchestrator orders them.
- **Not beside P5-IA-3** (shared settings components).
- **No API change.** P5-IA-5 owns the settings metadata.

## Acceptance criteria

1. People, Data, Platform, Chat platforms and Settings are one page each per level. No setting is
   reachable from two places, and Secret stores exists once.
2. Every settings section renders inside its topic page. `/admin/settings` is the All settings index, and
   `/admin/settings/<section>` redirects.
3. Org retention is one form with the instance maximums shown above it.
4. Web gates; axe and narrow-route suites, both themes.

## Dependencies

P5-IA-1 merged. P5-IA-2's `ScopeLadder` for the retention ladder.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). It is broad: the settings form refactor needs care with drafts and Helm
locks; the rest is composition.
