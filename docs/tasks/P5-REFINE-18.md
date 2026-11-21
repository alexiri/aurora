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

## Tests move with the UI (owner, 2026-11-20 — binding)

The owner: "Make sure the briefs not only change the website, but also the tests that test it! I don't want a ton of
broken tests because a button was renamed and the tests look for the old label." (P5-REFINE-1 merged with old labels
in tests, and master's CI broke until the CI Watcher fixed it in `ee7bfa0d`.) So, in **every commit** of this brief:

1. **Find every test that sees what you change.** Before committing a change to visible text, a label, an
   `aria-label`, a role, a heading, a route or `?tab=` id, a page's structure, an API field, an error code or message,
   search all test trees for the **old** strings, selectors and paths, and for the components you touched:
   - `web/tests/**` (vitest + Testing Library);
   - `web/e2e/**` (Playwright: the mocked default project **and** the `production` project that `scripts/e2e/*.sh`
     run against a real stack — `first-run.sh`, `kind.sh`, `files.sh`, `ssh.sh`, `channels.sh`, `approvals-park.sh`…);
   - `scripts/e2e/*.sh` themselves (they grep pages and call routes);
   - Rust tests that assert a message, an error code or a response shape (`services/*/tests/**`, `crates/*/src/**`
     `#[cfg(test)]`, `services/aurora-api/tests/it/**`);
   - fixtures and mocks (`web/e2e/fixtures/**`, `web/tests/**/mocks*`, MSW handlers, `web/openapi.json` snapshots).
   A plain `rg -n "<old text>|<old selector>" web/tests web/e2e scripts/e2e services crates` per change is the minimum.
2. **Update them in the same commit as the change.** New expectations use the new words exactly as the mockup shows
   them. Prefer `getByRole(…, { name })` or a `data-testid` over raw text where the text is not the point of the test;
   where the words are the point (they are the fix), assert the new words. Never loosen an assertion so that both the
   old and the new label pass, never `.skip`/`test.fixme`/delete a test to get green; a test is removed only when the
   feature it covers is removed, and the completion report names it and why.
3. **Run what you touched, plus everything that references it.** Through `heavy.sh`: every vitest file you changed
   **and** every vitest file that renders a component you changed; every Playwright spec (mocked project) that visits a
   page you changed. **Exception to the targeted-tests rule for this set (owner, 2026-11-20):** a commit that renames
   or restructures shared UI (a primitive, the app shell, the rail, the agent page, a label used on several pages) runs
   the **whole** web vitest suite once (`cd web && /home/alex/orca/workspaces/aurora/heavy.sh <lane> pnpm exec vitest run` (not `pnpm --dir web …`, which fails here)) before handing over. Production
   specs (`--project production`) can't run in a lane without a real stack: update them by reading, and list them in
   the completion report so the nightly / P5-REFINE-20 runs them.
4. **New behaviour gets new tests** (the brief's Tests section); old tests are **kept and updated**, not replaced.
5. **The completion report lists** every test file changed (and why), every test removed (and the removed feature),
   the commands run with their results, and the production specs that only the nightly will run.
6. A merge that turns master's CI red because a test still expects old UI is a send-back to this lane, not the CI
   Watcher's job.

## Docs to update

- `docs/14-web-dashboard.md` §Instance administration (rail, Set in Helm, presets, every changed page), §Topics per level, §9 First run;
  `docs/15` §Instance settings (the preset mapping table); `docs/PROGRESS.md` and `docs/FEATURES.md` rows, current at every commit.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). Many pages but each is bounded and has a mockup; the preset mapping and the rail regroup carry the
risk: review them closely.

## Completion report

Fill in when done; P5-REFINE-20 reads it.

## P5-REFINE-20 send-back, batch 1 (lane A, on master `4fad882a`)

- **1 (setup-2).** The Add-key flow no longer depends on per-call mutation callbacks: `AiKeyDialog` awaits the placement save (`mutateAsync`), so the toast "<key> added. Daneel can use it now." (no "n of 5 done") and the line "Given to Daneel and 2 other agents already in Acme." always show; `useCreateSecret` invalidates the setup status and the agents' answer-readiness, so the checklist is current when the dialog closes; the setup status is read again every 5 s while a key exists but its agent cannot answer yet (`useAdminSetup`), so Try it ("Say hello to your own agent", **Open chat**) appears without a reload; a new agent card (Daneel `@daneel` · Uses <key> · no setup left · **Ready**) under the checklist from `answer-readiness`. Backend (key stayed "checking" for over a minute): `verify` (the one function Test and the background re-check share) now drops the remembered listing of what the key serves (`hosted::model::forget_listing`): a read that failed at save was cached for 30 s, so Try it and the agent's `can_answer` kept saying "no model" after the re-check had passed. **Not reproduced:** the cluster is not available to this lane; whether the AIGW dev endpoint answers its model list at all (the key became "works" only by a real chat call) needs the verification lane's re-check. Unit test `forgetting_a_key_drops_its_failed_listing_and_only_its_own`; the existing IT `refine5_a_pending_key_is_rechecked…` passes.
- **2 (setup-1).** `SetupTryIt.done_at` (C-1063, docs/API_CONTRACTS, openapi and `schema.d.ts` regenerated, `try_it_done` returns the newest finished turn's time): the done row reads "Daneel answered Alice Admin 2 min ago" with **Open chat** on the row. IT `refine5_health_reads_real_calls_and_try_it_completes_by_itself` asserts `done_at`.
- **3 (setup no-key).** Step 3 reads "Add an AI key so agents can answer".
- **4 (setup-5).** One **Needs you** list: the sign-in-app gaps and "E-mail isn't set up" are rows of the shared `NeedsYouCard` (`extra`, `NeedsYouRow`, `SignInGapRows`), not a second card; the headline stays first on the Health view.
- **5.** The History fold is under the page (after the Health card, or after the checklist), never first.
- **6 (setup-4).** The AI models line says "has not been asked yet" unless a real call failed (then "did not answer at 14:02: the endpoint could not be reached", from the new `SetupCheck.last_error`, C-1063); a probe alone is not a problem tone. The Web Push row's `push.enabled` and the `aurora-api push status` command are under Technical details.
- **7 (setup-6).** Field label "Mail server address" with the line "For example smtp://mail.example.com:587" (`fields.rs`), no page-top banner when every error sits under its field, Send as: "Enter an email address when the mail server address is set."
- **8 (G4).** `ATTENTION_REFETCH_MS` is 30 s (Administration and organization counts); the sidebar dialog says "AI key" (`settings-section-form`).
- **Tests.** `setup-checklist` (toast, given line, Open chat, agent card, no-key words, one Needs you in order, real-call words incl. probe-only), `field-errors`, `admin-settings` (no banner), `parked-approvals` (Technical details), `admin-settings-llm` ("2 AI keys"), IT `onboarding` (`done_at`, `last_error`), e2e `p5-polish-10` (toast; progress line), `support/settings-words`. For the nightly (production specs, edited by reading): `onboarding-journeys` step 3/5 still match. Vitest: whole suite once, 3291 passed and one timeout (`connect-journey`, passes alone). `cargo clippy -D warnings` (aurora-api, aurora-db, aurora-config), `cargo fmt --check`, `aurora-config` tests, `scripts/guards/run-all.sh`, `pnpm typecheck`, eslint on touched files: clean.

## Fourth pass (after P5-REFINE-8 merged, master `df34a330`)

Done (the items that waited on brief 8): **18 per-entry counts (G4).** The Administration badge now reads `GET /v1/attention/counts` (C-1082, the shared `useAttentionCounts`) through `useAdminBadge`: the total on the sidebar-foot link, the account-menu entry and **Overview** in the Administration rail (the old `GET /v1/admin/attention` item count and `useAdminAttention` are gone), and each rail topic carries only its own part (`administration.topics`: Apps & accounts, Chat platforms, People), so a request is counted once per place by design. One `CountBadge` (amber `AttentionChip`, none at zero or on a failed read, "N need your attention" for a screen reader). Live: `useRefreshAttentionOnMutation` invalidates every attention query after any successful mutation. **Item 13 (setup-5, setup-8).** Overview's **Needs you** card (`AttentionSection`) is the Administration feed `GET /v1/admin/attention` in the shared row shape (`AttentionRow`/`NeedsYouCard`): asker avatar, "Bob Builder wants Jira" / "Needs … only an instance admin can register." / the reason in quotes / the action (**Set up**, **Connect a chat platform**, **Review**, **Open**) and **Decline** on requests, with the count chip; a **History** fold shows what closed in 30 days (decline message, who by role). The Health headline reads "Aurora is running. 2 requests are waiting for you." **Decline** is the one shared dialog `DeclineRequestDialog` ("Decline Carol’s request?" / "Carol Member asked for a chat platform. They will be told you declined." / optional **Message for Carol** up to 300 characters / Cancel, Decline) on the Administration card (`POST /v1/admin/requests/{id}/decline`), the workspace Needs-you row and Organization › Apps (`…/app-setup-requests/{id}/decline`); "Not now" is gone from those three. Docs: `14-web-dashboard.md`. **Not done / deviations:** the chat access card (an agent asks for an app in the conversation, a different route with no message) keeps its own "Not now"; the Health view still has its own "Needs you" section (sign-in gaps, e-mail), so Overview can show two cards with that heading; no screenshots (no browser run in this lane). **Tests:** `tests/admin-attention.test.tsx` rewritten for the counts and the feed card (badge total and topic, null for non-admins, zero, failure, refresh after a mutation, row shape, Decline with and without a message, History), `tests/context-switcher.test.tsx` (G4 topic badges), `tests/setup-request-row.test.tsx`, `tests/ask-admin.test.tsx`; mocked specs edited by reading, not run: `e2e/admin-attention.spec.ts`, `e2e/app-setup-requests.spec.ts`, `e2e/support/mock-api.ts`. Removed tests: none.

## Third pass (after P5-REFINE-6 merged, master `2a0278a8`)

Done (the items that waited on brief 6): **admin-5** (a connector whose host runs reads "Connector host running · 1 replica · Ready", never "waiting for the host"; the "Next: an organization owner connects <name> in their Chat platforms page." line with "Open <Org>'s Chat platforms" links, from `next_step = owner_connects`, C-1068), **admin-12** (the upload dialog is "Upload an app", shows the chosen file with **Choose another**, one sentence per problem from `violations[].sentence` and the codes under Technical details, C-1071), **admin-14** (Storage by category — Conversations, Files, Activity — with the tables under Technical details, C-1072; the Rate limits "Private destinations" red banner is neutral text; the green "Limits are healthy" line is **not** built: no API says whether a limit is being hit), **C-1088** (the audit asks `include_system=false` and shows the server's `hidden_system` count; the client-side classifier is gone). Still waiting on brief 8: item 13 and the per-entry counts. Org Storage keeps its own page (the API counts only files per organization). Tests: `admin-audit-view`, `plugins-admin`, `storage`, `connectors-admin` (updated or added); specs: `admin-settings.spec` (rate limits text).

## Second pass (worker session on `alexiri/p5-apps`, after master `2243097b`)

**Done in this pass, one commit per item** (items 1, 4+5, 7, 8, 9 each have their own commit; 10, 11, 14, 15, 17 admin-13, the org presets, the checklist line and the Provider rename are in the commits that follow them in `git log`; ids are in the worker_done message).

- **1. The rail in two halves** `ADMIN_RAIL_GROUPS` / `ADMIN_RAIL_LABEL` (`lib/routes.ts`), rendered by `AdminNavList`: Overview · POLICY FOR EVERYONE (What agents may do, Agent defaults, Apps, Chat platforms, People & sign-in, Data) · THIS INSTALLATION (Health & runners, AI endpoints, Set in Helm, All settings). Route ids and tabs are unchanged (so no redirects are needed); page titles follow the rail. **Set in Helm** page (`/admin/set-in-helm`): every locked value in words with its current value, the Helm key in `<code>`; every "Set by the installation" pill links to its row; the long "remove it from your values" text is gone; the All settings notice is one line. The Back link already existed (`AdminRailHeader`, brief 17).
  - **Page merge (built later, lane B, `alexiri/p5-polish-b`, 2026-11-21; the deviation above is gone).** What agents may do (`/admin/internet-access`) is one page with four tabs, `?tab=internet-access|ai-keys|byo|defaults` (`TABS.adminAgentPolicy`, `adminRoutes.agentPolicy`): Internet access (preset card, organizations), AI keys and rules (the old AI models › Rules), Bring your own (old Agents › BYO), Defaults (old Agents › Defaults plus the skills library refresh). The Agents topic is gone from the rail (`AdminTopic` has no `agents`, `ADMIN_TOPICS` 10 entries; "Agent defaults" entry removed). **Health & runners** (`/admin/platform`) tabs `health|runners|computers|secret-stores|email|limits`: Health is the Overview health view on its own page (`AdminHealthPanel`; Overview keeps its own Health tab), Computers is the old Agents › Computers; E-mail, Secret stores and Rate limits stay tabs here with their ids. **AI endpoints** (`/admin/ai-models`) is the endpoints alone (no tab bar). Old addresses redirect (`legacy-routes.ts` rules + `LegacyGate` on `/admin/agents`, `/admin/ai-models`, `/admin/internet-access`): `/admin/agents` (no tab, `defaults`, `skills`) → `/admin/internet-access?tab=defaults`; `?tab=byo` → `?tab=byo`; `?tab=computers` → `/admin/platform?tab=computers`; `/admin/ai-models?tab=rules` → `/admin/internet-access?tab=ai-keys`; `/admin/internet-access?tab=limits` → `?tab=internet-access`; `/admin/settings/<section>`, `governanceSetting` and All settings' groups follow (`SETTINGS_SECTION_PAGE`, group names from `ADMIN_RAIL_LABEL`). Docs: `14-web-dashboard.md` (topics table, section table, the rail paragraph). **Judgement calls:** (1) six tabs on Health & runners (the TopicPage's "at most four" is a comment, not a rule; the brief allows E-mail, Secret stores and Rate limits as tabs with today's ids); (2) Health first on `/admin/platform`, as the brief lists it, so the address without a tab now opens Health, not Runners. **Not done:** the command palette's page list (P5-REFINE-19, lane A) was not on master when this was built: add `/admin/internet-access?tab=ai-keys|byo|defaults` and `/admin/platform?tab=health|computers` to it, and drop "Agent defaults". No screenshots (no browser in this lane). **Tests:** changed `legacy-routes`, `routes`, `roles`, `visible-topics`, `context-switcher` (rail), `settings-section-fields` (All settings groups, links), `admin-settings-governance`, `harness-picker`, `setup-checklist`, `harness-controls`, `settings-in-words`, `admin-settings-stacked` (pathname/title); new `admin-page-merge.test.tsx`; mocked specs edited by reading, not run: `ia-3` (three tests), `ia-navigation` (rail), `admin-settings` (redirects and URLs), `support/mock-api.ts` (route list). Production specs / scripts/e2e: searched, none visit these addresses or labels. Removed tests: none. Commands (all through `heavy.sh p5-polish-b`): full web vitest first run 9 files / 17 failures, all from the moved addresses and labels, fixed; last full run 223 of 224 files and 3141 of 3142 tests passed, the one failure `connect-dialog.test.tsx` (a failed OAuth result) passed when run alone, 17 of 17 (a timing flake under load, not touched by this change); `pnpm typecheck` clean; eslint on touched files clean; `scripts/guards/run-all.sh` all passed. No Rust changed (no cargo, no `extract-schema --check`). Specs written by reading, after the 2026-11-21 note on SCHED-ONCE: the new `ia-3` assertions only use tab roles and headings that the mocked pages already render (`TopicPage` tabs are real `role=tab` buttons, no sr-only inputs); not run.
- **2. Organization presets + the checklist line.** `OrgPolicyPresets` (`components/policy/org-policy-presets.tsx`, mapping `ORG_PRESETS` in `lib/policy-presets.ts`, unit-tested: apply, detect, disabled-by-the-instance) on organization AI models and organization Internet access; writes the governance document with one PUT; a card looser than the instance floor is disabled "Set by the instance admin". **D-427 (owner, answering the first-pass question):** the instance Balanced card changes only key scopes and outside agents and says so; **organization Balanced** on Internet access adds an organization rule "Ask before changing data" (the existing Ask rule shape: allow + require approval, host `*`, POST/PUT/PATCH/DELETE; no new API), published at once when the policy draft has nothing else waiting, else it joins the draft for the page's bar. The checklist shows "What agents may do · Open · Change" under its five rows (not counted), linking to `/admin/internet-access#presets`.
- **4. Field errors in the field's unit** (`fieldErrorWords`, `technicalNames`, `settingKeyOfPath` in `lib/settings-words.ts`; `SettingsStackForm`): seconds → the field's unit, "Row 1: …" for list entries, address ranges, website names; names only under **Technical details**; the first bad field is scrolled into view; the bar says "n fields need fixing" (brief 2's). Private-address refusals (endpoint test, sign-in provider, secret stores, probe messages) say "Aurora can't reach private addresses. Ask whoever installed Aurora to allow it."
- **5. Save feedback.** Every Administration form already uses brief 2's save bar (no `SavedNotice` at the top of the stack form; `import-dialog` keeps its own). Added the sentence that names what changed ("Saved. New agents use these defaults."); Undo for 10 s was already in the bar.
- **7 (first half, admin-6).** Chat platforms: Mattermost Available/Added (+ Manage), Slack and Microsoft Teams "Not available yet" (`lib/chat-platforms.ts`), then a closed **Advanced · connector hosts, tokens and replicas** fold with the connector cards and host tokens. A plain "Add chat platform" button under the list keeps the choose step (and the self-hosted path) reachable. **The second half (admin-5, status from the host, the Next line) is not done:** it waits for brief 6 (C-1068), as the brief says.
- **8 (admin-7).** `PluginReview`: tile, "App package, version", Approve/Reject/Turn off/on, "What this app does" (`lib/app-summary.ts`, up to three sentences from the manifest; fallback "Review the details below." with a test on a sample); the status/package/digest card, hooks, egress, context, claims and conversions sit in a closed Technical details (sections open inside). The agent guide section stays outside the fold (agents read it). The third sentence is the app's own summary (the package carries no tool levels); "Can read and change repositories." in the mockup is that summary.
- **9 (admin-8).** `EndpointDialog`: add is two steps (step strip, Name, Address, **Test connection** with the result beside it, "Skip the test", Advanced options, "Next: models and roles", then Models and roles, Done); the id is made from the name until edited; editing shows both on one page. **Not built:** the masked **Key** field of the mockup: endpoints never hold keys here ("AI keys are never part of an endpoint", the test sends no credential), so a Key field would need an API change.
- **10 (admin-9).** The terminal switch says "Lets people open a terminal on their agent's computer." and, off, "Off: people see that the terminal isn't available" (neutral); Health lists "Web terminal · Off. People see “not available” on their agent's Computer tab." with **Turn on** (a link to the switch). Files/SSH/Previews have no off-state setting in this release; their lines are unchanged.
- **11 (admin-10), client side.** Administration Audit log: stacked sentences ("Alice changed the web terminal settings" / "20 Nov, 14:20 · Instance settings", new templates in `AUDIT_SENTENCE` for the instance-admin actions), "What administrators did.", and **Show system events** with the count of hidden system install rows (`isSystemAudit`, from the loaded pages). The server filter (C-1088) waits for brief 6. The scope control stays "Instance / All organizations" (not the mockup's "Everything" menu).
- **14 (setup-1/2/3).** Try it comes from `SetupStatus.try_it` (ticks itself; the web-only probe is gone); without a key: "Comes after the AI key" and no Open chat; Open chat goes to the agent's chat with "Hello Daneel! What can you help me with?" typed (`?draft=`), not sent; after adding the key: toast "Company key added. Daneel can use it now." and the row "… every new agent in Acme gets it" + "Given to Daneel and 1 other agent already in Acme." (from `starter_applied`, kept in the page until reload). `BackfillOffer` stays only for old agents (`personal_workspaces_without_starter`).
- **15 (setup-4).** Health's AI models line: each endpoint by display name, "answered 2 min ago" / "has not been asked yet" / "did not answer at 14:02" from `last_ok_at`/`last_error_at`/`evidence` (no reason text: the check carries none).
- **16.** The add-key select is **Provider** (label renamed in `llm-form.tsx`, with its ~15 tests and specs in the same commit). The dialog title, description, display names and no "(litellm)"/scopes line were done in the first pass.
- **17 (admin-13).** Users: avatar, display name, "alice · Instance admin", a regular **Manage** button (aria-label "Manage <name>"), "Roles from sign-in: none" when no one has any; the status column became part of the name line.
- **Still waiting** (as the brief's table says): 7 second half (admin-5), 11 server filter (C-1088), 12 (admin-12 package errors), 16/17 admin-14 (storage categories, Rate limits text) on **brief 6**; item 13 (requests in Administration, Decline with message) and the per-entry counts of item 18 on **brief 8**. Also not done: the Altitude wrapper on the Expert items inside the Chat platforms Advanced fold, and screenshots (no stack in the lane).
- **Tests** (all through `heavy.sh p5-apps`): see "Second pass: tests" below.

### Second pass: tests

- **Test files changed:** `context-switcher` (rail groups and labels), `visible-topics`, `roles` (11 admin topics), `admin-settings` (one-line Helm notice, pill link, field-error words), `admin-settings-llm` (two steps, "Address", "Connected." wording, private-address sentence), `admin-settings-stacked` (saved sentence, field error in words, terminal off note), `probe-failed-message`, `connectors-admin` (platform list, Advanced fold, Add buttons, icon count), `admin-audit-view` (sentences, "Show system events"), `account-settings` (Manage button), `setup-checklist` (Try it from the server, no-key path, hello draft, "Given to…", preset line, health lines), `credentials`, `credential-access`, `llm-key-form` (Provider label). **New:** `set-in-helm`, `field-errors`, `app-summary`, `org-policy-presets`. No test removed.
- **Playwright specs edited by reading (not run):** `admin-secret-stores`, `admin-settings`, `ia-3`, `ia-4`, `ia-navigation` (rail), `internet-access` (titles), `fix-127-mattermost` and `support/connectors` (platform list Add/Manage), `p5-polish-10` (app review fold), `admin-attention` (Advanced fold), `audit-detail` (sentence, Technical details), `llm-key-modes`, `accounts`, `credential-access`, `welcome-summary` (Provider label). **For the nightly / production project:** `onboarding-journeys`, `first-run` (label only; the journey's step 5 still expects Welcome's "Give Daneel access", which brief 5 (D-414) removes: check it), and the specs that visit Administration › Chat platforms, Overview, Health, Users and Audit.
- **Commands** (all through `heavy.sh p5-apps`): full web vitest on the branch (shared UI changed: rail, lock pill, page titles, labels): first run 17 failures, all mine (old words, closed folds, fetch counts, the preset matcher), fixed; last full run 207 of 208 files passed, 2876 of 2877 tests, the one failure a test of mine expecting the field error without its unit ("things"), fixed and re-run with the four other touched files: 5 files, 57 tests passed. `pnpm typecheck`: clean after fixing one type in a new test. eslint on the 45 touched files: clean. `scripts/guards/run-all.sh`: all passed. No Rust changed: no cargo, no `extract-schema --check`.
- **Send-backs fixed in this lane, one commit each** (found by the CI Watcher on master): P5-REFINE-17 models-tab definition list (axe `definition-list`/`dlitem`; vitest checks the structure), org Bring your own "Port previews" region (the wrapper Section reused the id `org-previews`), and the preset matcher (a key the section does not carry is skipped: a single Helm-locked key reads "Open (1 setting set in Helm)", a vitest pins it).

**First pass (kept for the record; its "not done" list is superseded by the second pass above where it says so).**

- **Items done** (commit: the one that carries this report, on `alexiri/p5-apps`):
  - **18 (badge, G4):** the Administration count was already on the sidebar-foot link and in the account menu (FIX-167); added it to **Overview in the Administration rail**, and made it live: `useRefreshAttentionOnMutation` (`lib/api/attention.ts`, mounted in the shell for instance admins) refetches after any successful mutation, on top of focus and 5 minutes. It still counts `GET /v1/admin/attention` only; the per-entry counts and request counts of C-1080/C-1082 wait for brief 8.
  - **2 (presets), instance level only:** `lib/policy-presets.ts` (`PRESETS`, `detectPreset`, `applyPreset`), `components/policy/policy-presets.tsx` (card, preview, confirm, Customise fold, "Custom · based on Open", "Open (n settings set in Helm)" with lock pill) mounted first on Administration › Internet access. Mapping table in `docs/15`. **Open equals the registry defaults for all six keys** (a test reads them from `settings.rs`): no difference to report.
  - **3 (admin-3):** `describeChange` in `lib/settings-words.ts`; the confirm is "Allow more than before?", a sentence per change, a heading per section, keys and reasons under Technical details, **Go back** / **Yes, save** (`ConfirmDialog` gained `cancelLabel`). A vitest walks the registry's loosening keys (parsed from `settings.rs`): every one reads as a sentence with no key and no snake_case.
  - **6 (admin-1):** a `key_origin_required` refusal springs the switch back to its saved value and shows "Not turned on. 2 AI endpoints have no key origin … Nothing changed" with **Dismiss**, endpoints by display name, server message under Technical details. The endpoint names do not link to Set in Helm (that page is not built).
  - **12 (admin-11), partly:** Apps are A to Z in every list and have a "Search N apps" field (placeholder carries the count). The Waiting / From Aurora / Other layout of P5-POLISH-10 is kept (not the mockup's tile grid); the light plate behind logos is brief 2's `AppIcon`, already in place.
  - **16, partly:** setup-6: a single-line string setting is an `<input>` (threshold for a text area is now 2000 characters, was 500: the mail server address is 512). setup-7: option text is the display name only (no "(litellm)") and the "Allowed key scopes" line is gone.
  - **D-426 (owner message):** `ROLE_LABEL` is now `ROLE_NAME` (Strongest / Everyday / Fastest model) on the endpoint dialog, the endpoint role line and the agent readiness notes; `droppedRoleWarning` no longer says "model model".
- **Items waiting for a backend brief:** 5's items (admin-1 endpoint names link, setup-1/2/4 Try it, starter key, "Given to…", no-key path, real-call health) have merged but are **not done** for lack of time (see below). Waiting on **6**: admin-5 status from the host and Next line (item 7 second half), admin-12 package errors (item 17), admin-14 storage categories and the Rate limits text (item 17), audit server filter (item 11, C-1088). Waiting on **8**: item 13 (requests in Administration, Decline with message), the per-entry counts of item 18.
- **Not done, no dependency (for a second pass):** 1 (rail in two halves, Set in Helm page, area header), 2's organization pages and the checklist line, 4 (field errors in the field's unit), 5 (saved sentences/Undo check), 7 first half (chat platforms first), 8 (app review summary), 9 (two-step endpoint dialog), 10 (terminal-off notice), 11 (audit stacked, system events), 14 (checklist Try it/no-key/Open chat), 15 (health lines), 16 (the "Provider" label, dialog title and description of the add-key dialog; the "Which service is this key for?" label is used by about 15 tests and specs, so it is a batch of its own), 17 admin-13 (users).
- **Deviations from the mockup or this brief and why:**
  - Balanced: "ask before changing data on sites they have no account for" has no instance key (it is a workspace policy default); Balanced therefore differs from Open only in key scopes and outside agents. **Answered by D-427 (second pass above): the instance card says so, the organization Balanced asks.**
  - Customise is a plain toggle in an `Altitude id="policy.presets"` wrapper (registry row existed, `later` removed).
  - The loosening sentence for a keyed list entry (sign-in provider) is generic ("Sign-in providers: corp now allows more than before."); the server's warning sentence still shows under it.
  - No screenshots were taken (no stack in the lane).
- **Screenshots compared:** none.
- **Tests** (all through `heavy.sh p5-apps`):
  - Full web vitest once (shared UI changed): 198 files, 2808 passed, 3 failed; the 3 failures were mine (an old "(bedrock)" option name in `llm-key-form.test.tsx`, the preset card's legend word in `ui-text.test.ts`, my own preset test's static mock) and were fixed; the re-run of those files plus the touched ones: 7 files, 48 passed.
  - `pnpm typecheck`: clean. eslint on touched files: clean after removing one unused variable. `scripts/guards/run-all.sh`: all passed. No Rust changed, so no fmt/clippy/extract-schema run.
  - **Test files changed:** `context-switcher` (badge on the rail), `admin-attention` (count follows a mutation), `admin-settings`, `-governance`, `-operational`, `-stacked`, `-sign-in`, `-llm` (new confirm wording and button, role names), `credential-access` (role names), `llm-key-form` (option text), `admin-apps-list` (A to Z, search). **New:** `describe-change.test.ts`, `policy-presets.test.tsx`. Specs: `e2e/admin-attention.spec.ts` (new count-drops-without-reload test), `e2e/admin-settings.spec.ts` (preset card, Customise before the locked egress value). **Not run** (Playwright rule): those two specs and the mocked specs that visit Administration › Internet access (`internet-access`, `stage4-axe`); **production specs** that look at the confirm button ("Save") or the add-key label: none found by search.
  - No test removed.
- **Open questions:** (Balanced is answered by D-427.) Should the preset card also set `governance.llm.require_key_origin`? Left untouched.
- **P5-REFINE-20 send-back, batch 3 (lane A), items 14–16 done (2026-11-21):** (14, 1f5c5c2e) one `ConnectorNextStep` (the
  mockup's accent box: "**Next:** an organization owner connects Mattermost in their Chat platforms page." with "Open <org>'s Chat
  platforms") on the platforms list **and** on the platform's own page, for `next_step` `owner_connects` and `connect` (an
  instance admin who also owns an organization gets `connect`, C-1068, which is why the line was missing); with a host running
  and no host report yet, "Chat platform hosts" says "A connector host is running and offers Mattermost. It is listed here after
  its first report." instead of contradicting "Connector host running". (15, b5f086fd) a bad package's one problem is said once
  (its sentence is the 422 message); several problems keep the message and one sentence each; codes under Technical details.
  (16, d2ec5967) the Helm role warning on an AI endpoint card says "Set the … roles in your Helm values." and keeps
  `llm.endpoints.<id>.roles` under Technical details. Tests: `connectors-admin.test.tsx` (1 new; the list test reads the bold
  "Next:" sentence across elements), `plugins-admin.test.tsx` (exactly once, 1 new), `admin-settings-llm.test.tsx`. None removed.

Send-back batch 4 #5 (lane A): "Administration" is said once in its rail: the sidebar-foot entry is not shown inside Administration (the rail header "You're in Administration" names it; Overview carries the count there). Outside Administration the foot entry stays. Tests: `context-switcher` (1 new, 1 edited); Playwright by reading: `e2e/admin-attention.spec.ts`.

Send-back batch 5 #3 (lane A, admin-9, `92ad4d42`): the terminal switch was drawn from `default` (the application's `false`) while
the API said `value: true, source: "default"`. Helm's top-level `terminal.enabled: true` renders `terminal.available`, which the
setting follows (D-237(d)). Now (C-782 note): with the followed key in the configuration, `GET /v1/admin/settings/terminal`
reports `default` as that value and, until the dashboard stores one, `source: "helm"` with `locked: false` (still switchable; a
stored value reads `dashboard`; Reset returns to the deployment's value). No wiring: unchanged (`default: false`,
`source: "default"`). The form draws an untouched, unstored control from the effective `value`, and after Reset from `default`.
The shared `Switch` carries `aria-checked`. The shared `SettingField`'s "Reset to default (…)" button wraps (`whitespace-normal`,
`h-auto`, `max-w-full`), so "(Follows what your deployment provides)" no longer widens the page at 390 px. Tests:
`it/settings_ops.rs::the_terminal_and_scim_switches_apply_live` (helm/unlocked/default true, then dashboard after a save) and
`::switches_need_the_deployments_wiring` (no wiring: default), `web/tests/admin-settings-stacked.test.tsx` (1 new: on from Helm
with `aria-checked`, off after a click, Reset back to on, the Reset class wraps). Run: those ITs (`-- settings_ops files:: settings`,
95 passed), the 4 touched vitest files (34 passed), then the whole web vitest suite once, because `Switch` and `SettingField` are
shared (243 files, 3341 tests passed); `pnpm typecheck`, eslint on the touched files, clippy on `aurora-api`. Playwright by
reading: `e2e/org-pages.spec.ts`, `e2e/app-setup.spec.ts` (switches, unaffected), mock `terminal.enabled` (`available: false`,
unchanged). None removed.
