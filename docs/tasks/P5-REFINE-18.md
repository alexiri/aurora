# P5-REFINE-18: Administration and policy presets (owner, 2026-11-20)

## Goal

UX review 5 walked a fresh install as the instance admin: the checklist, e-mail, Health, requests, then every Administration
page with a setting changed and saved on each. What is wrong now:

- **Broken.** "Try it" never completes. Turning on "Check where AI keys come from" fails on Helm-set endpoints but the switch
  stays on and a change is counted. Step 3 says "every new agent gets this key" but Try it asks for the same key again.
- **No sign of success or failure.** "Saved." or the error sits at the top of long pages, out of view. The confirm dialog says
  "`governance.egress.baseline_allowlist` becomes less strict." Errors name config keys and env vars (`gateway.allowed_private`,
  `LIBRARY_REFRESH_INTERVAL` in seconds for a minutes field).
- **Machinery first.** Chat platforms leads with connector hosts, bootstrap tokens and replicas. The app review page shows
  "Hooks · connect.exchange · token.mint", "Host claims", a digest. Storage lists tables. Rate limits has a permanent red banner.
  Add endpoint has 14 fields. Users shows two "alice" rows and a faint "···". Health says "did not answer from the API" while chat
  works; endpoints are called "AIGW shape" and "VidaiMock".
- **No policy in words.** Policy topics are a form of a dozen values whose interaction only the scope ladder explains.
- **Requests from people never reach Administration**, and declining Carol's request is "Not now" with no message for her.
- **Two kinds of page in one rail.** What an administrator decides and what an operator runs are mixed; "Set in Helm" repeats
  on every page.

The owner's decisions that apply (D-405 precedence: this brief, then D-405..D-419 and the artifact comments, then the mockup,
then the finding's fix text):

- **D-410.** Policy presets **Open / Balanced / Locked down** sit above the admin and organization policy forms.
  **Open is the default** on a fresh install. Editing a value by hand turns the card into "Custom · based on Open".
- **D-414.** The starter (organization) key applies to every new hosted agent and, when added, to agents already there ("Given to
  Daneel and 1 other agent"). With **no starter key**: Try it is not offered, step 3 stays "Add an AI key so agents can answer".
- **D-417.** One attention feed per level; requests have reasons, a decline message, history; the rail of whoever can act shows
  the count.
- **D-407.** Admins by role in text; display names from the sign-in's `name` claim.
- **D-383.** A health line says only what a real call proved ("Answered 2 min ago").
- **D-406.** Expert items (All settings search/export/import, raw ids, Technical details) sit behind "Show more controls".

## Linked requirements

- **Artifact (source of truth):** <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>. Finding anchors `…#f-<key>`; the layer above
  `…#above`.
- **Local reference (workers cannot open artifacts):** `/home/alex/orca/workspaces/aurora/design-ux-review-5/`, called `UXR5/` from
  here on: `UXR5/findings.md` (authoritative text), `UXR5/layer-above.md`, `UXR5/priorities-themes.md`,
  `UXR5/after/<key>.png` (dark) and `-lt.png` (light) = the visual spec, `UXR5/after/src/<key>.html` = markup (PNG wins; yellow
  dashed notes are commentary), `UXR5/before/<key>-b<n>.png`, `UXR5/screens/<id>.png`.
- **Findings:** admin-1..admin-14, setup-1..setup-8. **Layer-above:** rec 6 (policy presets, `UXR5/layer-above.md` rec 6,
  `https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#above`), rec 9 (Administration in two halves, rec 9), rec 7 (attention), rec 8 (the
  area with a Back link, shared with brief 17), rec 1 (altitudes).
- **Decisions:** D-405, D-406, D-407, D-410, D-414, D-417, D-383, D-341 (setting words in `lib/settings-words.ts`), D-384 (amber only
  when the viewer can act). **Contracts:** briefs 5 (C-1061..C-1066: starter key, Try it, key origin errors, endpoint health,
  display names), 6 (C-1067..C-1072: readiness, package errors, storage categories) and 8 (C-1080..C-1082: attention, requests,
  decline): read them in `docs/API_CONTRACTS.md` when merged. No new contract in this brief.
- **Docs:** `docs/14-web-dashboard.md` §Instance administration (`### Instance administration`), §Topics per level, §9 First run,
  §Settings in words; `docs/15` §Instance settings for key names and values.

## Wave and order

- **Lane D**, after **17** and after backend briefs **5, 6, 8** for their items: 17 → **18**. Needs 1, 2, 3 merged (vocabulary,
  primitives, altitudes).
- Waiting items (do the rest first; say in the report what was picked up when):

| Item | Waits for |
|---|---|
| 8 (admin-1, a failed save is not kept; which endpoints) | **5** |
| 12 (setup-1, setup-2, setup-4: Try it ticks, starter key, "Given to…", no-key path, real-call health) | **5** |
| 7 (admin-5 status from the host, Next line), 11 (admin-12 package errors as words), 16 (admin-14 storage categories) | **6** |
| 13 (setup-5 requests in Administration, setup-8 Decline with message) | **8** |

## Context

All paths under `web/src/` of master c0fe7d10.

- **Rail and topics:** `lib/api/roles.ts` (`ADMIN_TOPICS` :12, `visibleTopics("admin")` :20), `lib/routes.ts` (`adminRoutes`, `TOPIC_LABEL`,
  `topicHref`, `sectionPage`), `components/app-shell.tsx` (`AdminRailHeader` ~:340, `AdminNavList`, the Administration entry
  :410–420, the Back link `RETURN_KEY` :310–360), `components/admin/admin-topics.tsx` (`AdminOverviewPage` :36, `AdminAgentsPage` :46,
  `AdminAppsPage` :64, `AdminAiModelsPage` :106, `AdminInternetAccessPage` :122, `AdminPeoplePage` :130, `AdminChatPlatformsPage` :172,
  `AdminDataPage` :180, `AdminPlatformPage` :198, `AdminAllSettingsPage` :216), `components/topic-page.tsx`.
- **Settings pages:** `components/admin/settings/settings-section-form.tsx` (`SettingsSectionForm` :147, `SettingsStackForm` :192, the
  save notice :295, error handling :334–340, the confirm `ConfirmDialog` :404–423, `ConfirmDetails` :432–460 with "This allows
  more than before:" :440), `settings-field-controls.tsx`, `lib/settings-words.ts` (`durationWords`, `editUnit` :84, `valueWords`),
  `components/ui/setting-field.tsx` (the "Set in Helm" pill :134–139), `components/ui/lock-pill.tsx`,
  `components/admin/settings/settings-index-view.tsx` (All settings: locked notice :106–130, `SettingsIndexView` :71).
- **Policy forms:** `components/admin/settings/governance-settings-view.tsx`, `components/governance/{models-tab,byo-panel,egress-tab,
  org-governance,controls}.tsx`, `components/internet/{admin-internet,org-internet,internet-level}.tsx`, `components/ui/choice-cards.tsx`
  (`ChoiceCards`), `components/ui/scope-ladder.tsx`. Keys: `governance.egress.minimum_posture`, `governance.egress.open_lane_minimum`,
  `governance.egress.baseline_allowlist`, `governance.llm.minimum_posture`, `governance.llm.allowed_key_scopes`,
  `governance.llm.require_key_origin`, `governance.agent_tokens.max_age_days`, `byo.enabled`, `byo.minimum_posture` (docs/15).
- **Setup/Overview:** `components/admin/setup-view.tsx` (`SetupView` :484, `Checklist` :216, rows :230–262, `useTryIt` :88–105 where the
  Open chat link is built, `HealthView` :370, `ChecklistRow` :190), `components/admin/attention-section.tsx`, `attention-badge.tsx`,
  `components/overview/setup-request-row.tsx` ("Not now" :65–73), `lib/onboarding/guide.ts` (`HELLO_MESSAGE`).
- **Chat platforms:** `components/connectors/` (`ConnectorsAdminView`, `add-connector.tsx`, `connector-detail.tsx`, `host-tokens.tsx`,
  `available-callout.tsx`, `shared.tsx`).
- **App review:** `components/admin/plugins-view.tsx` (`PluginsView` :204, `UploadForm` :48, `ViolationList` :28, `reachesLine` :115),
  `components/admin/plugin-detail-view.tsx` (`PluginReview` :105, collapsibles Hooks :298, Egress per hook :309, Host claims :333,
  digest :195, Approve :377, `reasonLabel` :50, `hostText` :56).
- **Endpoints:** `components/admin/settings/llm-endpoint-dialog.tsx` (`EndpointDialog` :272, fields :338–441, Test :455 and result
  :216/462), `llm-controls.tsx`, `llm-model-fields.tsx`; add-key dialog `components/credentials/llm-form.tsx` (scopes hint :230).
- **E-mail:** `components/admin/settings/email-settings-view.tsx` (`keys` :16, reasons :25), `operational-settings-views.tsx` (limits
  :152, "Private destinations" danger notice :452, previews notice :376).
- **Data/People:** `components/admin/audit-view.tsx`, `components/audit/audit-panel.tsx`, `lib/api/admin-audit.ts`,
  `components/account/admin-users-view.tsx` (`UsersTable` :30, "More for" :96, "Show role values" :103),
  `components/account/admin-storage-view.tsx` (tables :75), `components/org/storage-view.tsx` (`UsageBar` :21).
- Pieces from briefs 1–3 and 17 to use: `ResponsiveTable`, `ConfirmDialog`, save bar, `lib/labels.ts`, `TechnicalDetails`,
  `EmptyState`, `Altitude`, `StatusLine`, `LockPill`, the 404-in-shell pattern; the preset slots of brief 17 (org AI models and Internet
  access pages).

## Scope

Every item: build what the After PNG shows, both themes, 1256 and 390 px, with the words quoted.

### 1. The rail in two halves and one "Set in Helm" page (rec 9)

- Reference `…#above` rec 9 (`UXR5/layer-above.md` rec 9). No single mockup: draw it with the visual kit; headings are small caps
  group labels like the other rails.
- New order (labels follow brief 1's vocabulary; **route ids stay**, old addresses redirect, C-960 style):

```
Administration
  Overview
  POLICY FOR EVERYONE
    What agents may do   (/admin/internet-access: tabs Internet access · AI keys and rules · Bring your own · Defaults)
    Apps                 (/admin/apps: Apps · Sign-in apps · Self-hosted · Advanced)
    Chat platforms       (/admin/chat-platforms)
    People & sign-in     (/admin/people)
    Data                 (/admin/data: Retention · Storage · Audit log · Usage)
  THIS INSTALLATION
    Health & runners     (/admin/platform: Health · Runners · Computers)
    AI endpoints         (/admin/ai-models: the endpoints)
    E-mail, Secret stores, Rate limits  (tabs of /admin/platform or their own entries; keep today's tab ids)
    Set in Helm          (/admin/set-in-helm, new)
    All settings         (/admin/settings; search, Export and Import are Expert)
```

- **Picks made here (the sources differ):** (a) Chat platforms stays one page in the policy half, **platforms first with the
  connector machinery in an Advanced fold on the same page** (admin-6's mockup; rec 9 put hosts under the installation half;
  Health & runners also lists each host's status with a link). (b) The old "Agents" topic splits: Defaults go under What agents
  may do › Defaults (model, idle time, skills library refresh), Computers under Health & runners.
- **Set in Helm page.** Lists every Helm-locked value from the settings index (`locked` in `useSettingsIndex`: key, helm key,
  entry id): the setting in words (`settings-words`), its **current value in words**, "Set in your Helm values" and the Helm key
  in `<code>`, and a link to the doc on changing it. Intro: "These values are set when Aurora is installed. To change one, change
  the Helm values and upgrade." Every `LockPill owner="Set in Helm"` (`setting-field.tsx`, `llm-controls.tsx`,
  `secret-stores-view.tsx`, `operational-settings-views.tsx`, `sign-in-*`) links to `/admin/set-in-helm#<key>` and the long
  "remove it from your values" text goes from each page. The All settings notice (`settings-index-view.tsx` :106) becomes one
  line linking here.
- **Area, not a replaced rail** (rec 8, shared with brief 17): Administration keeps its own header but renders as an area with
  **← Back to <workspace name>** (the remembered `RETURN_KEY` workspace). Never swap the rail to something unrelated; the org
  detail page in Administration keeps the Administration rail.
- Red is for failures only (theme "Contrast and colour meaning"): checked in the items below.

### 2. Policy presets (rec 6, D-410)

- Reference `…#above` rec 6 (`UXR5/layer-above.md` rec 6). No mockup file: build with `ChoiceCards`, kit tokens.
- New component `components/policy/policy-presets.tsx` ("How much may agents do here?") placed first on **Administration › What agents may
  do** and, through brief 17's slots, on **organization AI models** and **organization Internet access**. Three cards, each with a
  sentence of consequences:
  - **Open** (default): "Agents reach any website; people may add their own AI keys; owners may run agents elsewhere."
  - **Balanced**: "Agents reach any website but ask before changing data on sites they have no account for; only the
    organization's AI keys; agents run on Aurora."
  - **Locked down**: "Agents reach only the allow-list; organization keys only; no outside agents."
  - **Customise ▸** (Expert, `Altitude`) reveals today's fields. A "Custom · based on Open" pill replaces the choice when values
    were edited by hand; show "Open (2 settings set in Helm)" with a lock pill when a Helm-locked value keeps a preset from
    applying fully, never an error.
- **Mapping** (a table `PRESETS` in `lib/policy-presets.ts`, ~8 keys, unit-tested both directions: apply, and detect from effective
  values): intent per key, values exactly from the registry (`docs/15 §Instance settings`; read each key's allowed values, do not
  guess): `governance.egress.minimum_posture` / `open_lane_minimum` (what an agent may reach without an account),
  `governance.egress.baseline_allowlist` (kept), `governance.llm.minimum_posture` and `allowed_key_scopes` (who may bring keys),
  `byo.enabled` and `byo.minimum_posture` (outside agents), `governance.agent_tokens.max_age_days` (unchanged unless the registry
  ties it). Choosing a preset writes the same `instance_settings` sections through the existing PUT (one save bar, one confirm in
  words: item 4). At organization level a preset can only tighten what the level above allows: cards the level above forbids are
  disabled with the reason ("Set by the instance admin").
- **Open must equal a fresh install's defaults** (D-410). Read the registry defaults; if any key's default differs from Open's value,
  list the difference in the report and the Orchestrator decides (do not change defaults silently).
- **Checklist (the owner's pick, differs from rec 6's text):** no sixth checklist step (the mockups and D-391 keep five). Under the
  checklist show one line "What agents may do · Open · Change" (not counted), linking to the preset card.
- Test: card selection writes the mapped values; hand edit shows "Custom · based on Open"; Helm lock shows "(n settings set in Helm)".

### 3. Confirm in sentences (admin-3)

- Anchor `https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#f-admin-3`. After `UXR5/after/admin-3.png`, `UXR5/after/admin-3-lt.png`; markup
  `UXR5/after/src/admin-3.html`; Before `UXR5/before/admin-3-b1.png`, `UXR5/before/admin-3-b2.png`; screens `UXR5/screens/A059-internet-saved.png`,
  `UXR5/screens/A079-retention-saved.png`.
- `ConfirmDetails` (`settings-section-form.tsx` :432) no longer prints `<code>key</code>: reason`. Dialog "Allow more than before?",
  one block per section with a heading ("Internet access", "Data retention") and **a sentence per change**: "Agents everywhere may
  now reach docs.example.org." / "Request activity is kept 45 days instead of 30." Buttons **Go back** and **Yes, save**.
- Build `describeChange(key, before, after)` in `lib/settings-words.ts` from the D-341 labels and `valueWords`; templates for the
  loosening keys (allow-list entries added/removed, retention days, posture words, key scopes, BYO) and a generic fallback "<Label> changes
  from <old> to <new>." Config keys, ids and `reason` strings live only under **Technical details** in the dialog. A key with no template
  fails a vitest that walks the registry's loosening keys.
- The sentences are the ones presets (item 2) and the preview use.

### 4. Field-level errors in the field's unit (admin-4)

- Anchor `…#f-admin-4`. After `UXR5/after/admin-4.png`, `UXR5/after/admin-4-lt.png`; markup `UXR5/after/src/admin-4.html`; Before
  `UXR5/before/admin-4-b1.png`, `UXR5/before/admin-4-b2.png`; screens `UXR5/screens/A037-skills-invalid.png`,
  `UXR5/screens/A054-endpoint-test-conn.png`, `UXR5/screens/A058-internet-invalid.png`.
- On `settings_invalid` (`settings-section-form.tsx` :337) map each error (pointer such as
  `governance.egress.baseline_allowlist[0].host`) to its field and print a message **under the field in its own unit**: the library
  refresh field (minutes) "Enter 1 minute or more. Aurora checks at most once a minute."; private ranges "That isn't a valid address
  range. Use a form like 10.0.0.0/24."; allow-list row "Row 1: that isn't a website name. Try docs.example.org." Convert server seconds
  to the field's unit with `editUnit`/`durationWords`. Config keys and env names appear only in a **Technical details** line under the
  form ("LIBRARY_REFRESH_INTERVAL, gateway.allowed_private").
- On error, scroll to the first bad field; the save bar says "2 fields need fixing" (brief 2's bar).
- Sign-in and the e-mail section's reasons that mention `gateway.allowed_private` (`sign-in-provider-dialog.tsx` :171,
  `email-settings-view.tsx`) say "Aurora can't reach private addresses. Ask whoever installed Aurora to allow it." (the key under
  Technical details).

### 5. Save feedback in view (admin-2)

- Anchor `…#f-admin-2`. After `UXR5/after/admin-2.png`, `UXR5/after/admin-2-lt.png`; markup `UXR5/after/src/admin-2.html`; Before
  `UXR5/before/admin-2-b1.png`, `UXR5/before/admin-2-b2.png`; screens `UXR5/screens/A032-agents-defaults-invalid.png`,
  `UXR5/screens/A033-agents-defaults-saved.png`, `UXR5/screens/A068-signin-invalid.png`, `UXR5/screens/A069-signin-saved.png`.
- Brief 2 provides the save bar and adopts it on every settings form. **Check every Administration form uses it** (no
  `SavedNotice` at the top of `SettingsStackForm`, no page-top "Saved."); here add only what is admin-specific: the saved sentence
  names what changed ("Saved. New agents use these defaults.", "Saved. Applied to every replica within seconds." stays on forms that
  say it today) and, where the primitive supports it, **Undo** for 10 s. If brief 2's bar has no Undo, skip it and say so in the report.

### 6. A failed save is not kept (admin-1, waits for brief 5)

- Anchor `…#f-admin-1`. After `UXR5/after/admin-1.png`, `UXR5/after/admin-1-lt.png`; markup `UXR5/after/src/admin-1.html`; Before
  `UXR5/before/admin-1-b1.png`; screens `UXR5/screens/A055-ai-rules.png`, `UXR5/screens/A056-ai-rules-saved.png`,
  `UXR5/screens/A093-admin-settings.png`.
- "Check where AI keys come from" / "Agents may only use keys whose origin Aurora can verify." with the switch. When the save fails
  the switch **springs back to Off** and the page shows, under it, in place and kept until dismissed: "Not turned on. 2 AI endpoints
  have no key origin, so their keys couldn't be checked: <endpoint display names>. These endpoints are set in Helm, so add the origin
  there, then try again." with **Technical details →** and, on the right, "Nothing changed" and **Dismiss**. The names come from the
  error body (brief 5's contract: which endpoints), by **display name**; no raw keys. The endpoint names link to Set in Helm.
- The All settings "changes" count and the Dashboard badge show nothing for a save that failed (the server no longer persists it).

### 7. Chat platforms: platforms first, next step (admin-5, admin-6)

- **admin-6.** Anchor `…#f-admin-6`. After `UXR5/after/admin-6.png`, `UXR5/after/admin-6-lt.png`; markup `UXR5/after/src/admin-6.html`; Before
  `UXR5/before/admin-6-b1.png`; screens `UXR5/screens/A071-admin-chat.png`.
  - Header "Chat platforms" / "Where people can talk to agents." A list of platforms: **Mattermost** · "Available" · button **Added** /
    **Add**; **Slack** · "Not available yet"; **Microsoft Teams** · "Not available yet" (the "not available yet" set is a web
    constant in `lib/chat-platforms.ts` of platforms the product names but has no connector type for; types the API lists are
    "Available"). Below, a closed fold **Advanced · connector hosts, tokens and replicas** holding today's hosts, bootstrap/host
    tokens (`host-tokens.tsx`) and replicas. Expert items inside use `Altitude`.
- **admin-5.** Anchor `…#f-admin-5`. After `UXR5/after/admin-5.png`, `UXR5/after/admin-5-lt.png`; markup `UXR5/after/src/admin-5.html`; Before
  `UXR5/before/admin-5-b1.png`, `UXR5/before/admin-5-b2.png`; screens `UXR5/screens/A072-add-connector.png`,
  `UXR5/screens/A073-add-mattermost.png`, `UXR5/screens/A076-chat-after-mm.png`. **Waits for brief 6** (readiness).
  - Status from the host (one readiness source): "Mattermost · Connector host running · 1 replica · **Ready**" (never "Waiting for the
    connector host" while a host serves it). Under it: "Next: an organization owner connects Mattermost in their Chat platforms page."
    with a button **Open Acme's Chat platforms** (the organization the person is in; with several, a menu).

### 8. App review in a plain summary (admin-7)

- Anchor `…#f-admin-7`. After `UXR5/after/admin-7.png`, `UXR5/after/admin-7-lt.png`; markup `UXR5/after/src/admin-7.html`; Before
  `UXR5/before/admin-7-b1.png`; screens `UXR5/screens/A041-admin-app-github.png`.
- `PluginReview` opens with the app tile, "GitHub · App package, version 1.4" and **Approve**, then **What this app does**: three
  sentences built from the manifest: "Reaches api.github.com." (`reachesLine`) "Signs people in with GitHub." "Can read and change
  repositories." (from the app's levels/tools). Then **Technical details** (collapsed) holding the existing sections unchanged:
  Hooks (`connect.exchange`, `token.mint`), Egress per hook, Host claims, the digest and `{instance}` placeholders. The "Approve
  does not wait for the admin to open them" rule of P5-POLISH-10 E5 stays: the summary is on screen, the sections are in the fold
  (the fold is closed, the sentences carry the decision).
- A missing sentence falls back to "Review the details below" and fails a vitest on a sample of manifests.

### 9. Add AI endpoint in two steps (admin-8)

- Anchor `…#f-admin-8`. After `UXR5/after/admin-8.png`, `UXR5/after/admin-8-lt.png`; markup `UXR5/after/src/admin-8.html`; Before
  `UXR5/before/admin-8-b1.png`, `UXR5/before/admin-8-b2.png`; screens `UXR5/screens/A053-add-endpoint.png`,
  `UXR5/screens/A054-endpoint-test-conn.png`.
- `EndpointDialog` (`llm-endpoint-dialog.tsx` :272) becomes a two-step dialog "Add AI endpoint" with the step strip **Connect and
  test · Models and roles**. Step 1: **Name** (display name), **Address**, **Key** (masked), **Test connection** with the result
  **next to the button** ("Connected. 12 models found." or the failure sentence, no scrolling), **Advanced options** (fold: provider,
  endpoint id, allowed paths, key origin and the other extras). Footer **Cancel**, **Next: models and roles** (enabled once the test
  passed or the admin skips with "Skip the test"). Step 2: default model, models, allowed models, roles. The endpoint id is
  generated from the name (kept editable under Advanced); it cannot change later (existing hint).
- The dialog keeps a fixed top anchor (no jump when fields appear).

### 10. Terminal off, said where it matters (admin-9)

- Anchor `…#f-admin-9`. After `UXR5/after/admin-9.png`, `UXR5/after/admin-9-lt.png`; markup `UXR5/after/src/admin-9.html`; Before
  `UXR5/before/admin-9-b1.png`, `UXR5/before/admin-9-b2.png`; screens `UXR5/screens/A200-settings-terminal-after-terminal.png`,
  `UXR5/screens/A201-agents-computers-after-terminal.png`.
- The terminal switch row: "Web terminal" / "Lets people open a terminal on their agent's computer." and, when Off, the neutral text
  "Off: people see that the terminal isn't available". **Health** (Health & runners › Health) lists it: "Web terminal · Off. People
  see "not available" on their agent's Computer tab. **Turn on**". Same for Files/SSH/Previews rows where the setting exists
  (`terminal`, `ssh-line.tsx`, `previews-line.tsx`). Off is neutral, never amber or red.

### 11. Audit as stacked sentences, system events hidden (admin-10)

- Anchor `…#f-admin-10`. After `UXR5/after/admin-10.png`, `UXR5/after/admin-10-lt.png`; markup `UXR5/after/src/admin-10.html`; Before
  `UXR5/before/admin-10-b1.png`; screens `UXR5/screens/A081-data-audit.png`.
- `AuditPanel` rows (shared with workspace Audit; brief 16 does the workspace one; reuse its sentence table and stacked
  `ResponsiveTable`): avatar, "Alice turned on the web terminal" / "20 Nov, 14:20 · Instance settings" / **Details**. No column is
  clipped; the raw event under Details.
- "40 system install events are hidden." with a **Show system events** switch **with a count**, default off (designer's note: hiding
  must be visible). Classify system rows client-side from the loaded pages (the install/sync actions of `plugin.*`/addon sync run by
  the system actor; read the action list in `docs/10`/`lib/audit-url.ts` and `AuditEntry` actor fields); the count is of loaded
  rows. Once P5-REFINE-6 has merged, use its server filter instead (**C-1088**: `include_system=false` and the
  `hidden_system` count), so the count covers every row, not only the loaded pages.
- Header "Audit log" / "What administrators did." with the scope control ("Everything").

### 12. Apps: order, search, plate (admin-11)

- Anchor `…#f-admin-11`. After `UXR5/after/admin-11.png`, `UXR5/after/admin-11-lt.png`; markup `UXR5/after/src/admin-11.html`; Before
  `UXR5/before/admin-11-b1.png`; screens `UXR5/screens/A040-admin-apps-expanded.png`.
- `PluginsView` lists apps **A to Z** with a **Search** field and the count ("14 apps"); each row logo, name, status chip
  (**Approved** / **Not approved**). Logos sit on a light plate in dark mode (brief 2 provides the plate primitive; use it on every
  app logo here: AWS, GitHub, Notion, Sentry).

### 13. Requests reach Administration (setup-5, setup-8, waits for brief 8)

- **setup-5.** Anchor `…#f-setup-5`. After `UXR5/after/setup-5.png`, `UXR5/after/setup-5-lt.png`; markup `UXR5/after/src/setup-5.html`; Before
  `UXR5/before/setup-5-b1.png`, `UXR5/before/setup-5-b2.png`; screens `UXR5/screens/A190-admin-overview-requests.png`,
  `UXR5/screens/A191-admin-health-requests.png`, `UXR5/screens/A192-ws-overview-asks.png`.
  - The Administration rail shows a count next to **Overview** (and **Chat platforms** for chat platform asks): "Overview 2",
    "Chat platforms 1". The count is **requests only an instance admin can resolve** (an ask for a chat platform when none is connected,
    an app that needs an instance-level sign-in app) plus the existing attention items; the same request is counted once, on the
    entry where it can be resolved (designer's note: Overview carries the total, the topic carries its own part and the Overview
    does not add it twice).
  - Overview: "Aurora is running. 2 requests are waiting for you." and a **Needs you 2** list in brief 8's row shape: "Bob Builder wants
    Jira" / "Needs an Atlassian sign-in app that only an instance admin can register." / **Set up** / **Decline**; "Carol Member wants a
    chat platform" / "Your organization has no chat platform connected." / **Connect** / **Decline**. Replace today's
    `AttentionSection` rows with the shared `NeedsYou` renderer when brief 8's feed exists.
- **setup-8.** Anchor `…#f-setup-8`. After `UXR5/after/setup-8.png`, `UXR5/after/setup-8-lt.png`; markup `UXR5/after/src/setup-8.html`; Before
  `UXR5/before/setup-8-b1.png`; screens `UXR5/screens/A194-ask-not-now.png`, `UXR5/screens/A195-ask-declined.png`.
  - The button is **Decline** on the card and in the dialog. Dialog "Decline Carol's request?": "Carol Member asked for a chat platform. She
    will be told you declined." and an optional **Message for Carol (optional)** text area (hint "Carol sees this with the notice.");
    **Cancel**, **Decline**. Same in `setup-request-row.tsx` (:65–73) and org Apps (`org-apps-table.tsx` :250) and the chat
    access card (`access-request-card.tsx` :82) via one shared dialog. Done and declined items stay 30 days under **History**.

### 14. Setup checklist: Try it, key, no-key path (setup-1, setup-2, setup-3, waits for brief 5)

- **setup-1.** Anchor `…#f-setup-1`. After `UXR5/after/setup-1.png`, `UXR5/after/setup-1-lt.png`; markup `UXR5/after/src/setup-1.html`; Before
  `UXR5/before/setup-1-b1.png`; screens `UXR5/screens/A025-chat-answer.png`, `UXR5/screens/A026-checklist-after-hello.png`,
  `UXR5/screens/A190-admin-overview-requests.png`.
  - Header "Set up Aurora" / "Five steps from a fresh install to your first answer." and "4 of 5". Rows: **Sign-in** "Development
    sign-in" **Review**; **Where new people land** "The Acme organization, each in their own workspace" **Change**; **AI key** "Company
    key · every new agent gets it"; **Try it** "Daneel answered Alice Admin 2 min ago" **Open chat**; **5 E-mail · optional** "For
    invitations and approval notices" **Set up**. Try it ticks itself on the first finished turn of any agent the admin owns (brief 5
    makes `useTryIt` true from the server; remove the web-only `replied` probe when it does).
- **setup-2.** Anchor `…#f-setup-2`. After `UXR5/after/setup-2.png`, `UXR5/after/setup-2-lt.png`; markup `UXR5/after/src/setup-2.html`; Before
  `UXR5/before/setup-2-b1.png`, `UXR5/before/setup-2-b2.png`; screens `UXR5/screens/A015-add-key-filled.png`, `UXR5/screens/A021-try-it-chat.png`,
  `UXR5/screens/A022-welcome-access-given.png`.
  - After adding the starter key: toast "Company key added. Daneel can use it now." and the AI key row "Company key · every new agent in Acme gets
    it" with the line "Given to Daneel and 1 other agent already in Acme." (count from brief 5). Step 4 "Try it · Say hello to your own
    agent · **Open chat**" and the agent card "Daneel @daneel · Uses Company key · no setup left · **Ready**".
  - **No starter key (D-414):** Try it is **not offered** (the row shows "Comes after the AI key"); step 3 stays "Add an AI key so agents can
    answer". Remove the "Welcome asks to give Daneel the key" path (`BackfillOffer` stays only for old agents brief 5 could not serve).
- **setup-3.** Anchor `…#f-setup-3`. After `UXR5/after/setup-3.png`, `UXR5/after/setup-3-lt.png`; markup `UXR5/after/src/setup-3.html`; Before
  `UXR5/before/setup-3-b1.png`; screens `UXR5/screens/A021-try-it-chat.png`.
  - **Open chat** goes straight to the agent's chat (`useTryIt` href, `setup-view.tsx` :104 builds `…/agents/<handle>?tab=chat`) with the
    hello prefilled and focused, **not sent**: "Hello Daneel! What can you help me with?" (`HELLO_MESSAGE` with the agent's name) through
    the `?draft=` hand-off of C-1035 (P5-POLISH-5; brief 13 fixes its Done-step bug, read its fix). Chat shows "Nothing yet. Say hello,
    or press Send as it is." The four-card Welcome guide is for regular people, not this path.

### 15. Health from real calls (setup-4, waits for brief 5)

- Anchor `…#f-setup-4`. After `UXR5/after/setup-4.png`, `UXR5/after/setup-4-lt.png`; markup `UXR5/after/src/setup-4.html`; Before
  `UXR5/before/setup-4-b1.png`; screens `UXR5/screens/A030-health.png`.
- `HealthView` lines (D-383): "Sign-in · No role mappings: only the admin e-mail list makes admins. **Open sign-in**"; "Runners · 1 runner
  online."; "AI models · Company AI gateway answered 2 min ago. Users AI gateway has not been asked yet. **Open AI models**"; "Storage · 0 B in 0
  stored files." Never "did not answer from the API" unless a real call failed ("Did not answer at 14:02: <reason>"). Endpoint names are
  the **display names** (no "AIGW shape", "VidaiMock"; `lib/labels.ts` for any provider/shape word). Brief 5 supplies the last real
  call per endpoint.

### 16. E-mail field and the add-key dialog (setup-6, setup-7)

- **setup-6.** Anchor `…#f-setup-6`. After `UXR5/after/setup-6.png`, `UXR5/after/setup-6-lt.png`; markup `UXR5/after/src/setup-6.html`; Before
  `UXR5/before/setup-6-b1.png`; screens `UXR5/screens/A027-email-setup.png`, `UXR5/screens/A029-email-unreachable.png`.
  - Header "Mail server" / "Where sign-in, invitation and notification e-mail is sent from." **Mail server address** is a one-line
    input (not a 140 px textarea; fix the generic control for single-line string settings in `settings-field-controls.tsx`) with the
    example "For example smtp://mail.example.com:587" and **Send as**. Error: "Aurora can't reach private addresses. Ask whoever installed
    Aurora to allow it." (item 4).
- **setup-7.** Anchor `…#f-setup-7`. After `UXR5/after/setup-7.png`, `UXR5/after/setup-7-lt.png`; markup `UXR5/after/src/setup-7.html`; Before
  `UXR5/before/setup-7-b1.png`, `UXR5/before/setup-7-b2.png`; screens `UXR5/screens/A014-add-key-dialog.png`, `UXR5/screens/A016-after-add-key.png`.
  - Dialog "Add an organization AI key" / "Every new agent in Acme gets this key." with **Name** ("Company key"), **Provider** (display
    names only: "Company AI gateway · Works with Claude Code.", no "(litellm)"), **API key** with **Show**, **Cancel**, **Add key**. The
    "Allowed key scopes: …" line is dropped for admins adding the org key (`credentials/llm-form.tsx` :230). Fixed top anchor, reserved
    space for the field that appears (brief 2's dialog rule).

### 17. People, storage, limits, package errors (admin-12, admin-13, admin-14)

- **admin-13.** Anchor `…#f-admin-13`. After `UXR5/after/admin-13.png`, `UXR5/after/admin-13-lt.png`; markup `UXR5/after/src/admin-13.html`; Before
  `UXR5/before/admin-13-b1.png`, `UXR5/before/admin-13-b2.png`; screens `UXR5/screens/A061-admin-people.png`, `UXR5/screens/A062-user-menu.png`.
  - Users shows **display names** ("Alice Admin" and the e-mail-name as secondary "alice · Instance admin"; brief 5 fixes the name), a
    regular-weight **Manage** button (not "···"), and at the bottom "Roles from sign-in: none" instead of "Show role values (0)".
    Stacked rows on a phone.
- **admin-12 (waits for brief 6).** Anchor `…#f-admin-12`. After `UXR5/after/admin-12.png`, `UXR5/after/admin-12-lt.png`; markup
  `UXR5/after/src/admin-12.html`; Before `UXR5/before/admin-12-b1.png`; screens `UXR5/screens/A228-upload-bad-package.png`.
  - Upload dialog "Upload an app": the chosen file "notes.txt" **Choose another**; the error "That isn't an app package. Use a .tar.gz
    made with `aurora plugin pack`." with **Technical details** (the code `archive_corrupt`, "invalid gzip header"). Map every package
    error code to a sentence in `lib/labels.ts` (`reasonLabel`, `plugin-detail-view.tsx` :50 is the start); **Cancel**, **Upload**.
- **admin-14 (waits for brief 6).** Anchor `…#f-admin-14`. After `UXR5/after/admin-14.png`, `UXR5/after/admin-14-lt.png`; markup
  `UXR5/after/src/admin-14.html`; Before `UXR5/before/admin-14-b1.png`, `UXR5/before/admin-14-b2.png`; screens `UXR5/screens/A080-data-storage.png`,
  `UXR5/screens/A092-platform-limits.png`.
  - Storage "What people have stored in Aurora.": **Conversations** "Messages and what agents did · 4.1 GB", **Files** "Uploads and agent
    files · 1.6 GB", **Activity** "Request history, kept 30 days · 0.8 GB" (brief 6's category mapping; tables only under Technical
    details). Org Storage uses the same categories.
  - Rate limits: "Limits are healthy. Nothing has been slowed down in the last 24 hours." in neutral/green text. The permanent red
    "Private destinations" banner (`operational-settings-views.tsx` :452) becomes neutral text on the Rate limits page ("Entries in
    the allowed-private list open an address range to agents, apps and sign-in. Add only ranges you trust."); red appears only when
    a limit is actually being hit.

### 18. The Administration count badge, everywhere it is entered (owner, 2026-11-20; P5-REFINE-20 row G4)

The owner reported the navigation's count badges don't work. FIX-167 Part B (D-370, C-1022) promised "the
Administration count", but `AttentionBadge` (`components/admin/attention-badge.tsx`) is mounted **only in the account
menu** (`components/app-shell.tsx` ~:155). Show it on **every** Administration entry: the sidebar-foot Administration
link, the account-menu entry, and Overview in the Administration rail. It counts the instance attention items plus the
requests only an instance admin can resolve (C-1080/C-1082, P5-REFINE-8, D-417), for instance admins only, live (the
existing refetch interval is fine; also on window focus). Same rules as P5-REFINE-16 item 19, with vitest for each
mount point and a mocked Playwright spec where the count drops after an item is resolved, without a reload.

## Out of scope

- Policy preset UI on pages outside Administration and the two organization pages; workspace Internet access.
- The workspace Audit (brief 16), Manage Acme (17), vocabulary (1), primitives (2), altitude registry (3).
- Backend work (5, 6, 8). A server-side system-event filter for audit (note only).

## Acceptance criteria

1. Every item matches its After PNG (dark) and `-lt.png` (light) at 1256 px and 390 px; no horizontal scroll at 390.
2. The rail shows Overview, POLICY FOR EVERYONE and THIS INSTALLATION; every old Administration address redirects; Set in Helm lists
   every locked value and every lock pill links to it.
3. The preset card shows on Administration › What agents may do and on the two organization pages; applying a preset writes the
   mapped keys; hand edits show "Custom · based on Open"; Open equals the fresh-install defaults (or the difference is reported).
4. The confirm dialog never shows a config key outside Technical details; each change is a sentence.
5. Field errors are under the field in its own unit; config keys only in Technical details.
6. Save feedback is in the bar in view; no page-top "Saved.".
7. A failed "Check where AI keys come from" save leaves the switch Off with the explanation; the changes count does not move.
8. Chat platforms lists platforms first; status comes from the host; the Next line shows.
9. App review opens on three plain sentences; hooks, claims, digest in Technical details.
10. Add endpoint is two steps with the Test result beside the button.
11. The terminal-off notice is by the switch and on Health.
12. Audit entries are sentences, stacked; "Show system events" has a count.
13. Apps A to Z with search and a light plate behind logos.
14. Requests appear as a rail count and Needs you rows; Decline takes a message.
15. Try it ticks itself; the no-key path matches setup-2; Open chat prefills the hello and does not send.
16. Health says what real calls returned, by display name.
17. The e-mail field is one line; the add-key dialog has no "(litellm)" and no scopes line.
18. Users show display names, a Manage button and "Roles from sign-in: none"; Storage is by category; Rate limits are red only for
    problems; a bad package says what to do.
19. No snake_case, UUID or dotted key outside `TechnicalDetails`/`<code>` on these pages (label guard).
20. Web gates (targeted): typecheck, lint on touched files, the vitest and mocked Playwright specs below with axe.

## Tests

Targeted only; GitHub CI is the full gate. Heavy commands through `/home/alex/orca/workspaces/aurora/heavy.sh <lane> <command>`;
never `localhost:4000` or the aurora-dev cluster.

- **Vitest:** `PRESETS` apply and detect (every preset, custom, Helm-locked); `describeChange` for every loosening key; error-to-field
  mapping with unit conversion; Set in Helm list; rail topics and redirects; failed save springs the switch back; chat platform list
  and Next line; app-review sentences; endpoint two-step state machine; terminal-off line; audit system-event filter and count; apps
  sort and search; decline dialog with message; checklist rows per state (key, no key, tried); health lines; users rows; storage
  categories; package error sentences.
- **Playwright (mocked)** at 1280 and 390 px in both themes, axe, no console errors: each preset applied; a loosening confirm; a
  failing key-origin save; chat platforms; app review; add endpoint; audit toggle; decline with message; checklist with and without
  a key and Open chat; e-mail field; users; storage.
- Update specs that read the old rail labels, "Not now", "Saved." at the top, "Verdict", "Show role values".

## Docs to update

- `docs/14-web-dashboard.md` §Instance administration (rail, Set in Helm, presets, every changed page), §Topics per level, §9 First run;
  `docs/15` §Instance settings (the preset mapping table); `docs/PROGRESS.md` and `docs/FEATURES.md` rows, current at every commit.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). Many pages but each is bounded and has a mockup; the preset mapping and the rail regroup carry the
risk: review them closely.

## Completion report

Fill in when done; P5-REFINE-20 reads it.

- **Items done** (number, finding keys, commit):
- **Items waiting for a backend brief** (which, what is missing, when picked up):
- **Deviations from the mockup or this brief and why:**
- **Screenshots compared** (key, theme, width, path of the new screenshot):
- **Tests run** (commands and counts):
- **Open questions:**
