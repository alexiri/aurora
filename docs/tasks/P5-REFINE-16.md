# P5-REFINE-16: Workspace pages, first day and account (owner, 2026-11-20)

## Goal

UX review 5 walked Bob's workspace pages, a person's first day and the Account pages. What is wrong now:

- **Workspace pages speak in machinery.** "Check what would happen" answers "Verdict: block / Guidance: blocked_by_policy /
  Winning credential: none". Activity is a nine-column table of `POST litellm.…svc.cluster.local/v1/messages turn · allow ·
  200`. Audit shows `api_key.revoke` with a UUID and a green "Critical". Delete workspace asks for a slug nobody has seen.
- **People pages promise things they do not do.** Add people says "by e-mail" but sends nothing. A person shows as "joined"
  before they ever signed in. Promoting someone to Owner happens at once, with no question.
- **Pages with nothing to say.** API keys do not say what a key can do. Skills opens on precedence rules and "home sync".
  Approvals says "Nothing is waiting for you." twice and never shows what an approval looks like. Apps & accounts shows
  "Loading…" for 8–15 s.
- **First day.** The switcher card looks live while it loads, and on a phone jumps into a workspace. Welcome ticks "AI key"
  for a key that is still being checked. The tour covers the cards it talks about. "Daneel can reach any website" reads as a
  warning with no way out.
- **Account.** A red "Notifications are blocked" shows before anyone asked for notifications. API keys has no Create. Chat
  accounts does not name the platform or the bot. SSH keys and Notifications have no rail entry. A pasted RSA key enables
  Add with no hint.

The owner's decisions that apply (D-405 sets precedence: this brief, then D-405..D-419 and the artifact comments, then the
mockup, then the finding's fix text):

- **D-407.** Text that points to an administrator names the **role** ("Ask an admin"), not a login. Display names come from
  the sign-in's `name` claim. Pause/Resume replaces Freeze in the UI.
- **D-415.** One **live** timeline per agent ("what it can do, is doing, did", in sentences) feeds the workspace Activity page,
  the rail and a Technical view.
- **D-417.** Requests and attention items have one feed shape per level; the Needs you rows read it.
- **workspace-9 (owner comment).** Apps stays a **grid of square cards**, with search and two groups. It does not become rows.
- **D-406.** Expert items sit behind the per-person "Show more controls" switch (brief 3's `Altitude`).

## Linked requirements

- **Artifact (source of truth):** <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>. Finding anchors are
  `…#f-<key>`; the priorities are `#top`, the layer above `#above`.
- **Local reference (workers cannot open artifacts):** `/home/alex/orca/workspaces/aurora/design-ux-review-5/`. Called
  `UXR5/` from here on. In it:
  - `UXR5/findings.md`: the authoritative text of every finding (problem, fix, After caption, designer's note);
  - `UXR5/priorities-themes.md`, `UXR5/layer-above.md`;
  - `UXR5/after/<key>.png` (dark) and `UXR5/after/<key>-lt.png` (light): the visual spec;
  - `UXR5/after/src/<key>.html`: the mockup markup (the PNG wins where they differ; yellow dashed notes are commentary);
  - `UXR5/before/<key>-b<n>.png`: the Before crops; `UXR5/screens/<id>.png`: the full screenshots.
- **Findings in this brief** (details per item in Scope): workspace-1..12, firstday-2, firstday-3, firstday-4, firstday-5,
  account-1..5. Layer-above recs 4 (timeline, `UXR5/layer-above.md` rec 4, `…#above`), 7 (one Needs you, rec 7) and 10
  (approvals example card, rec 10).
- **Decisions:** D-405, D-406, D-407, D-415, D-417 (see the plan in the Orchestrator's notes). **Contracts:** the contracts
  of briefs 5, 8 and 9 (read `docs/API_CONTRACTS.md` once they have merged). One small contract of this brief: scope 6
  (see there).
- **Docs:** `docs/14-web-dashboard.md` §Workspace (`### Workspace`), §10 People, §5 Approvals, §Account, §Onboarding.

## Wave and order

- **Lane C**, after **P5-REFINE-12**: 3 → 12 → **16**. Needs merged first: **1** (vocabulary), **2** (primitives:
  `ResponsiveTable`, `ConfirmDialog`/`TypeToConfirm`, `SecretOnce`, save bar, skeleton rows, `lib/labels.ts`,
  `TechnicalDetails`, `formatWhen`), **3** (`Altitude`).
- Items that wait for a backend brief (do all the others first; do these when it has merged and say so in the report):

| Item | Waits for |
|---|---|
| 2 (Activity: live timeline and per-agent feed) | **9** (per-agent timeline). Part A ships before it |
| 6 (workspace-4: "Not signed in yet") | small additive field, scope 6 (this brief) |
| 11 (workspace-11 Needs you rows on Overview) and Overview/rail counts | **8** (attention feeds) |
| 13 (firstday-3: only a key that answered counts) | **5** (key verification state, access-8 / setup-1) |
| 17 (account-3: name the platform and the bot) | a member-readable "chat platforms I can link" read, not in 5, 6, 8 or 9 (gap, see Out of scope) |

## Context

All paths under `web/src/` of master c0fe7d10.

- **Check a request (workspace-1):** `components/policy/simulate-panel.tsx`: `SimulationResult` :34 ("Verdict:" :38, "Guidance:"
  :52, "Winning credential:" :56), `SimulatePanel` :89; the dialog is opened from `components/internet/internet-level.tsx`.
- **Activity (workspace-2):** `components/approvals/activity-view.tsx` (`ActivityView` :47, `DECISIONS` :19, `decisionTone`
  :35, table :102, decision badge :142), `useActivity` in `lib/api/approvals.ts`. Routes: `wsRoutes(...).activity()`.
  The agent rail is `components/agents/status-rail/` (not changed here).
- **Audit (workspace-3):** `components/audit/audit-panel.tsx` (`AuditPanel` :40, table :100, "Critical" :138),
  `components/audit/audit-detail.tsx` (`AuditDetail` :161, table :124), `components/settings/audit-tab.tsx`,
  `lib/api/account.ts` `useAudit` :196.
- **People (workspace-4, 5):** `components/settings/access-tab.tsx` (role select :214–226, `ROLE_LABEL` :38, `joinedText` :111,
  "What can each role do?" :394, `AccessTab` :302), `components/settings/add-people.tsx` (dialog :122, result sentence :46,
  the link block :129, `WaitingList` :198), `components/workspace/workspace-topics.tsx` (People page, `PeopleBody`).
  `AccessBinding` has `joined_at` but no "has signed in" field (`lib/api/schema.d.ts` :6250).
- **API keys (workspace-6):** `components/settings/keys-tab.tsx` (create form :54–80, list via `components/account/key-table.tsx`),
  `components/account/one-time-key-dialog.tsx` (replaced by brief 2's `SecretOnce`).
- **Settings (workspace-7, 8):** `components/settings/general-tab.tsx` (`SavedNotice` :63 "Settings saved."),
  `components/settings/danger-tab.tsx` (slug confirm :19–35), `components/ui/type-to-confirm-delete.tsx`,
  `lib/api/account.ts` `usePatchWorkspace`. The switcher is `components/nav/scope-switcher.tsx`.
- **Apps (workspace-9, 12):** `components/apps/apps-tab.tsx` (`AppsTab` :123, product grouping :141, grid `ul` :160, status
  lines :34–47, `needs_admin` line "Needs {admin} first" :45), `components/accounts/accounts-panel.tsx` and
  `components/states.tsx` (`Loading`).
- **Skills (workspace-10):** `components/skills/workspace-skills.tsx` (:28–33), `components/skills/skills-view.tsx`.
- **Approvals (workspace-11):** `components/approvals/approvals-view.tsx` (`approvalsSentence` :107, `EmptyState` :204),
  the real question `components/approvals/approval-question.tsx` (`ApprovalQuestion` :59, answers :128–136),
  `components/chat/approval-card.tsx`. Overview: `components/overview/needs-you.tsx`, `setup-request-row.tsx`
  ("Not now" :65–73).
- **First day:** switcher `components/nav/scope-switcher.tsx` (`ScopeSwitcher` :86, `single` :339, face :330, "Choose a workspace" :326);
  guide `lib/onboarding/guide.ts` (`computeGuide` :59, `providerText` :119), `components/onboarding/getting-started.tsx`,
  `components/onboarding/welcome-view.tsx`; tour `components/onboarding/tour.tsx` (`TOUR_STOPS` :23, the card :168, placement
  :125–140); the reach sentence `lib/summary/internet.ts` `reachSentence` :43, used by `lib/summary/agent.ts` :98.
- **Account:** rail `components/app-shell.tsx` `ACCOUNT_LINKS` :266–271 (SSH is linked from Profile only, `routes.ts`
  `accountRoutes` :167); `components/account/profile-view.tsx` (sections :108–167, SSH link :151), `approval-notifications.tsx`
  (blocked text :93, `denied` :30), `my-keys-view.tsx`, `ssh-keys-view.tsx` (hint :61, input :85, Add), `components/channels/account-channels-view.tsx`
  (:52–57). Pages: `app/account/{page,security,api-keys,channels,ssh,delete}`.
- **Shared pieces from briefs 1–3** (use, do not rebuild): `ResponsiveTable`, `ConfirmDialog` (consequence text),
  `TypeToConfirm` (visible name), `SecretOnce`, the save bar and inline "Saved", `lib/labels.ts`, `TechnicalDetails`,
  `Skeleton` rows, `EmptyState`, `formatWhen`, `Altitude`.

## Scope

Every item: build what the After PNG shows, in both themes, at 1256 and 390 px, with the words quoted here.

### 1. Check a request in sentences (workspace-1)

- Anchor `https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#f-workspace-1`. After `UXR5/after/workspace-1.png`,
  `UXR5/after/workspace-1-lt.png`; markup `UXR5/after/src/workspace-1.html`; Before `UXR5/before/workspace-1-b1.png`;
  screens `UXR5/screens/B161-check-dialog.png`, `UXR5/screens/B162-check-result.png`.
- `SimulationResult` starts with one sentence in the first line, then the explanation sentence, then **Technical details**
  (collapsed, `TechnicalDetails`): the old Verdict, Guidance code and Winning credential, unchanged inside.
- Sentences (table in `lib/labels.ts`, one per verdict and guidance code; a missing one fails the label test):
  - blocked by a rule: "**Blocked.** Your rule "Block evil.example.org" stops it." The rule name links to the rule
    (the existing rule editor deep link). Under it: "Dani can't reach this website, and nothing will ask you first."
  - allowed: "**Allowed.** <why in words>." (a rule allows it, the allow-list has it, or no rule applies).
  - asks first: "**Dani will ask you first.** <why>."
  - blocked with no rule (default): "**Blocked.** No rule allows it, and the default for sites without an account is to block."
  - Use the agent's name in the second line ("Dani"), as the dialog already knows the agent.
- Buttons: **Check another** and **Done**.

### 2. Activity: a person's view, then a live timeline (workspace-2, rec 4, D-415)

- Anchor `…#f-workspace-2`; also `https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#above` rec 4 (`UXR5/layer-above.md` rec 4).
  After `UXR5/after/workspace-2.png`, `UXR5/after/workspace-2-lt.png`; markup `UXR5/after/src/workspace-2.html`; Before
  `UXR5/before/workspace-2-b1.png`; screens `UXR5/screens/B154-activity.png`.
- Header: **Activity**, "What your agents did on the internet and through your apps." Filters in words: three selects
  **Everything** (Everything · Answers · Calls to apps · Approvals · Automations), **Any agent**, **Any outcome**
  (Any outcome · Answered · Blocked · Waited for you · Failed). No snake_case values anywhere.
- Rows are sentences with a meta line, a time (`formatWhen`, ticks every 30 s) and a **Details** link:
  - "Dani answered you" / "Used Workspace default · 1 call" / "11 min ago";
  - "Dani called GitHub" / "Blocked by your rule 'Block evil.example.org'" / "24 min ago";
  - "Dani asked before changing docs.example.org" / "Waiting for you in Approvals" (links to Approvals) / "1 h ago".
- **Details** opens the raw row under the sentence: "Details · the raw request" with method, host, path, decision and
  status in mono. The model-endpoint call ("POST litellm…/v1/messages turn · allow · 200") is one **answered** row, not a
  tool call.
- **Part A (no backend wait).** Build the sentence rows from today's request log (`useActivity`) with templates in
  `lib/labels.ts`. Keep today's table as the **Technical view**, at Expert (`Altitude level="expert"`, "n more controls
  hidden · Show"), on `ResponsiveTable` (stacked under 900 px), with the same filters as before.
- **Part B (after brief 9).** The sentence list reads the per-agent timeline feed (structured kinds plus names; sentences
  from `lib/labels.ts`). "Any agent" merges the agents' feeds (or uses a workspace-level parameter if brief 9's contract
  has one: read it). It is **live** over the chat's existing stream: a new row appears at the top, an in-progress row
  ("Running "Morning summary"…", "Waiting for you") turns into its outcome in place; when the person has scrolled, show
  "3 new · Show" instead of moving the list. The Technical view streams new requests the same way.
- Date and time formats: only `formatWhen`.

### 3. Audit as sentences (workspace-3)

- Anchor `…#f-workspace-3`. After `UXR5/after/workspace-3.png`, `UXR5/after/workspace-3-lt.png`; markup
  `UXR5/after/src/workspace-3.html`; Before `UXR5/before/workspace-3-b1.png`; screens `UXR5/screens/B178-audit.png`.
- Header **Audit**, "Who changed what in this workspace." One filter, "All actions".
- Entry = avatar + sentence + meta + severity: "Bob revoked the API key ci-script" / "Nov 20, 14:02 · Bob Builder" /
  **Critical**; "Alice changed Carol to Owner" / **Important**; "Bob added Carol Member" / **Routine**. The key's, person's or
  app's **name** comes from the entry's target, never a UUID; when the target is gone say "a deleted API key".
- **Critical is red** (danger tone), Important amber, Routine neutral. Never green.
- Stacked rows on `ResponsiveTable` (sentence line, meta line); no clipped columns. The raw event (`api_key.revoke`, ids,
  before/after) is under **Details** inside `TechnicalDetails`, in the existing `AuditDetail`.
- One sentence template per audit action in `lib/labels.ts`; an action with no template renders "<Person> did something
  ("<readable action>")" and fails the label test, so it gets noticed.

### 4. Add people says how, and who has not signed in (workspace-4)

- Anchor `…#f-workspace-4`. After `UXR5/after/workspace-4.png`, `UXR5/after/workspace-4-lt.png`; markup
  `UXR5/after/src/workspace-4.html`; Before `UXR5/before/workspace-4-b1.png`, `UXR5/before/workspace-4-b2.png`; screens
  `UXR5/screens/B164-add-people.png`, `UXR5/screens/B165-carol-added.png`.
- In the Add people dialog, **before** the person submits, under the e-mail field: "Aurora doesn't send an e-mail. <Name or
  address> can open this workspace at <address> once she signs in." with **Copy link** (the existing `CopyValue`, address
  `${origin}${workspacePath}`). Use the typed address until a name is known. The sentence in the done state stays.
  Dialog description: "Give one person access to this workspace." (no "by e-mail").
- A person who has never signed in shows "**Not signed in yet**" (neutral chip) and "added just now" instead of
  "Joined …", with **Copy link** beside it. Rule: show the chip while the person's `signed_in` is false (scope 6). Once
  true, "Joined <formatWhen>".
- The "Waiting for first sign-in" list keeps its job for people who are not in the organization yet; its rows use
  `ResponsiveTable` styling and `formatWhen`.

### 5. Confirm promotions (workspace-5)

- Anchor `…#f-workspace-5`. After `UXR5/after/workspace-5.png`, `UXR5/after/workspace-5-lt.png`; markup
  `UXR5/after/src/workspace-5.html`; Before `UXR5/before/workspace-5-b1.png`, `UXR5/before/workspace-5-b2.png`; screens
  `UXR5/screens/B166-carol-owner.png`, `UXR5/screens/B168-roles-explainer.png`.
- **Owner's choice (designer's note, picked here):** the question is asked **only for a change to Owner**. Changes to
  Approver or Member apply at once, with the inline "Saved" of brief 2. (Demotions also apply at once, as today.)
- Choosing Owner in the role select opens `ConfirmDialog` (the select shows the old value until Confirm): title "Make Carol
  an Owner?"; a person line with avatar and "Member now → Owner"; "Owners can, in addition to what members can do:" then
  the list "Add and remove people · Change other people's roles · Delete this workspace"; "You can change her back to
  Member at any time."; buttons **Cancel** and **Make Owner**. Pronoun: use the person's name when unknown ("them").
- The role lists come from one table `ROLE_CAN` (Owner, Approver, Member) shared with the "What can each role do?" fold and
  with scope in brief 17 (organization); put it in `lib/roles-explain.ts`.
- The "What can each role do?" fold is **open by default in the Add people dialog** (layer-above rec 16: the first owner
  never learns Approver exists).

### 6. Small field for "signed in" (workspace-4 data)

- **Built by P5-REFINE-6 as C-1086** (coordinator, 2026-11-20): `AccessBinding.signed_in`. This brief only reads it;
  until P5-REFINE-6 merges, the row shows no "Not signed in yet" line. (Do not build it here.) The same goes for
  account-3's platforms and bot: **C-1089** `GET /v1/me/chat-platforms`, built by P5-REFINE-6.
- Targeted cargo test: the two views return `signed_in: false` for a person who never signed in and `true` after
  `last_login_at` is set. Visible to workspace owners only, as the list already is (no new exposure: SEC check in the
  review comment).

### 7. API keys say what a key can do (workspace-6)

- Anchor `…#f-workspace-6`. After `UXR5/after/workspace-6.png`, `UXR5/after/workspace-6-lt.png`; markup
  `UXR5/after/src/workspace-6.html`; Before `UXR5/before/workspace-6-b1.png`, `UXR5/before/workspace-6-b2.png`; screens
  `UXR5/screens/B172-settings-apikeys.png`, `UXR5/screens/B174-apikey-created.png`.
- Header **API keys** with primary **New API key** (opens a small dialog: name, optional expiry; replaces the inline form).
- Under it: "A key acts as you in this workspace through the Aurora API. Anyone who has it can do what you can, so keep it
  secret." and a link **API documentation** (the docs address the instance already serves; if none exists, the repo's
  `docs/05-control-plane-api.md` rendered link is not usable: ask in the report, do not invent a URL).
- Each row: name, "Can do everything you can in <workspace name> · created 3 days ago · last used today" (or "never used"),
  **Revoke** (ladder rung "rotate/disruptive": `ConfirmDialog` "Revoke ci-script? Anything using it stops working now.").
  The API has no scopes, so there is no scope line.
- The created key opens `SecretOnce` (brief 2); the old `one-time-key-dialog.tsx` is deleted when nothing uses it.

### 8. Saving the workspace name (workspace-7)

- Anchor `…#f-workspace-7`. After `UXR5/after/workspace-7.png`, `UXR5/after/workspace-7-lt.png`; markup
  `UXR5/after/src/workspace-7.html`; Before `UXR5/before/workspace-7-b1.png`, `UXR5/before/workspace-7-b2.png`; screens
  `UXR5/screens/B169-settings-general.png`, `UXR5/screens/B171-settings-saved.png`.
- General uses brief 2's save bar (first change shows **Discard** / **Save**, then "Saved" for 2 s). The "Settings saved."
  notice goes. The success line next to the control reads "Saved. The workspace is now called Bob's Lab." for the name.
- After a successful save the scope switcher and the page chrome show the new name at once: invalidate the `useResolve`,
  `useWorkspaces` and `me` queries on success (the switcher builds its list from `me.memberships` and `useWorkspaces`,
  `scope-switcher.tsx` :89–122) and patch the cached `me` workspace entry so there is no stale "Your workspace".
- **Memory language** (rec 16): shown only at Expert as "Language Dani remembers in" (wrap with `Altitude`); default it
  from the creator's locale when a workspace is created (`MEMORY_LANGUAGES` in `lib/nav/landing.ts`).
- A personal workspace's name shows the same everywhere the page shows it ("Bob's workspace" once the sign-in name is
  taken from the claim, brief 5), via `workspaceDisplayName` only.

### 9. Delete workspace asks for the visible name (workspace-8)

- Anchor `…#f-workspace-8`. After `UXR5/after/workspace-8.png`, `UXR5/after/workspace-8-lt.png`; markup
  `UXR5/after/src/workspace-8.html`; Before `UXR5/before/workspace-8-b1.png`; screens `UXR5/screens/B179-danger-zone.png`.
- `DangerTab` uses brief 2's `TypeToConfirm` with the **visible name**: title "Delete Bob's Lab?", "This deletes the
  workspace, its agents, files and conversations. It can't be undone.", "Type the workspace name to confirm: Bob's Lab",
  buttons **Cancel** and **Delete workspace**. The API keeps its `confirm` string: send the slug the API expects, derived
  from the visible name match (the server check is unchanged; the typed text is only compared in the browser). Read
  `deleteWorkspace(wsId, confirm)` in `lib/api/account.ts` first; if the server compares the typed text to the slug, send
  the slug after the browser compares the visible name.

### 10. Apps: grid, search, two groups (workspace-9, owner comment) and Skills copy (workspace-10)

- **workspace-9.** Anchor `…#f-workspace-9`. After `UXR5/after/workspace-9.png`, `UXR5/after/workspace-9-lt.png`; markup
  `UXR5/after/src/workspace-9.html`; Before `UXR5/before/workspace-9-b1.png`, `UXR5/before/workspace-9-b2.png`; screens
  `UXR5/screens/B149-apps-tab.png`, `UXR5/screens/B092-give-app.png`.
  - `AppsTab` keeps the **grid of square cards** (`sm:grid-cols-2 lg:grid-cols-3`). Add a **Search apps** field at the right
    of the heading, and two groups under headings: **Ready to connect** and **Needs an admin first**. Cards inside a group
    sort A to Z. A card keeps its icon, name, one-line purpose and its button (**Connect**, or **Ask an admin**).
  - The journey's own grouping and search (`apps/connect-journey/`) is the model: reuse its filter function, do not copy it.
  - A product with a connected account stays in a third group **Connected**, above the two, as today's card status
    ("Connected · …") says; empty groups are not drawn. No matches: "No app matches "<text>"."
  - The `needs_admin` line reads "An admin sets up <what> once" (D-407: no person's name); **Ask an admin** opens the ask
    dialog whose request goes to all admins (brief 8). Until brief 8 has merged keep the existing request call.
- **workspace-10.** Anchor `…#f-workspace-10`. After `UXR5/after/workspace-10.png`, `UXR5/after/workspace-10-lt.png`; markup
  `UXR5/after/src/workspace-10.html`; Before `UXR5/before/workspace-10-b1.png`; screens `UXR5/screens/B150-workspace-skills.png`.
  - `workspace-skills.tsx` header **Skills** with description "A skill is a set of instructions that teaches an agent how to do
    one job your way, such as writing a release note or triaging an issue. Every agent in this workspace can use them.";
    **New skill** on the right. List rows: "Release notes · Written by Bob Builder · used by Dani · On".
  - The amber precedence notice goes. A collapsed disclosure **How skills combine** holds: "Skills come from your
    organization and from this workspace. If both have a skill with the same name, the workspace's version is used. New and
    changed skills reach your agents shortly." No "home sync" anywhere.

### 11. Approvals: one empty state with an example (workspace-11, rec 10)

- Anchor `…#f-workspace-11`; `…#above` rec 10 (`UXR5/layer-above.md` rec 10). After `UXR5/after/workspace-11.png`,
  `UXR5/after/workspace-11-lt.png`; markup `UXR5/after/src/workspace-11.html`; Before `UXR5/before/workspace-11-b1.png`;
  screens `UXR5/screens/B152-approvals.png`.
- Header **Approvals**, "Dani asks you here before it changes something." The sentence "Nothing is waiting for you." shows
  **once** (the empty state title; `approvalsSentence(0)` is not drawn above it too). Body: "When Dani needs a yes from you,
  it shows up here and in your notices." (Use the workspace's first agent's name; "your agents" with several.)
- Under it an **Example** card, built from `ApprovalQuestion` with sample data (no API call, no countdown, no Live pill,
  an "Example · this is what Dani asks before a change" label): "Dani wants to close issue #214 on GitHub Acme/web ·
  "Login page flickers" · it says the fix is merged". Pressing **Allow once**, **Always allow** or **Block** shows one
  explanation line under the card and changes nothing: Allow once "Dani does it this time, and asks again next time.";
  Always allow "Dani does this kind of thing without asking from now on. You can undo it in Apps & accounts."; Block "Dani
  is told no, and carries on without it." The sample is never counted or sent.
- The Example card shows only while nothing is waiting (with the empty state). It is hidden as soon as a real approval
  is waiting, so the two are never confused.
- Needs you rows on Overview (after brief 8): `NeedsYou` renders the workspace attention feed in brief 8's shape (one row
  per item: sentence, detail, since, one action), with the asker's own open requests shown as "Waiting for an admin since
  Tuesday". Until then it keeps today's rows.

### 12. Skeleton rows instead of "Loading…" (workspace-12)

- Anchor `…#f-workspace-12`. After `UXR5/after/workspace-12.png`, `UXR5/after/workspace-12-lt.png`; markup
  `UXR5/after/src/workspace-12.html`; Before `UXR5/before/workspace-12-b1.png`; screens `UXR5/screens/B144-apps-accounts.png`,
  `UXR5/screens/B194-phone-accounts.png`, `UXR5/screens/B205-accounts-LIGHT.png`.
- Apps & accounts (and Apps) show brief 2's skeleton rows (the grid shows skeleton cards) under the real header and tabs,
  with the line "Loading your accounts…" for screen readers and a quiet visible caption.
- **Investigate the slow `/apps` load** (the designer's note: a skeleton hides the wait, it does not remove it). Find the
  slow call (the readiness fan-out per app in `GET /v1/workspaces/{ws}/apps`, `services/aurora-api`) with a measured
  number from the throwaway cluster; fix it if it is a small change (batch the per-app lookup) or write the finding and a
  proposed brief in the report. Do not run perf budgets while other builds run (the machine rule).

### 13. First day (firstday-2, firstday-3, firstday-4, firstday-5)

- **firstday-2, the switcher always opens a list.** Anchor `…#f-firstday-2`. After `UXR5/after/firstday-2.png`,
  `UXR5/after/firstday-2-lt.png`; markup `UXR5/after/src/firstday-2.html`; Before `UXR5/before/firstday-2-b1.png`,
  `UXR5/before/firstday-2-b2.png`; screens `UXR5/screens/B015-switcher.png`, `UXR5/screens/B184-phone-switcher.png`.
  - While `me`/`useResolve` load, the card shows a **disabled** state ("Loading workspaces…", muted, no hover, no chevron,
    `aria-busy`), not a live-looking box. Loaded: it always opens its panel (the list: workspaces, "Create a workspace"),
    on desktop and on the phone, even with one workspace. Today `single` (`scope-switcher.tsx` :339) renders a non-button;
    keep that only for the true single-workspace single-organization member who cannot create anything, and then say so in
    the card ("Your workspace · Acme"), not by looking disabled.
  - On the phone the card must open the panel; find what navigates straight into the shared Default workspace (the drawer's
    handler in `app-shell.tsx`) and remove it.
- **firstday-3, count only keys that answered.** Anchor `…#f-firstday-3`. After `UXR5/after/firstday-3.png`,
  `UXR5/after/firstday-3-lt.png`; markup `UXR5/after/src/firstday-3.html`; Before `UXR5/before/firstday-3-b1.png`; screens
  `UXR5/screens/B182-phone-welcome.png`. **Waits for brief 5** (the key's verification state).
  - `computeGuide` (`lib/onboarding/guide.ts`) marks "AI key" done only for a key whose state is answered/verified, or an
    organization key that works. A key still being checked is a step in state `checking`: "**1 AI key** My Anthropic key is
    still being checked. It counts once it has answered." and "**2 Say hello** Comes after the step above.", "0 of 2 done".
    Add the state to the guide's step type, the vitest table and `getting-started.tsx`.
- **firstday-4, the tour dims the page.** Anchor `…#f-firstday-4`. After `UXR5/after/firstday-4.png`,
  `UXR5/after/firstday-4-lt.png`; markup `UXR5/after/src/firstday-4.html`; Before `UXR5/before/firstday-4-b1.png`; screens
  `UXR5/screens/B004-tour-1.png`, `UXR5/screens/B008-tour-5.png`.
  - Add a backdrop (the page dimmed, `inert`-free so Esc and arrows still work) with the highlighted link kept bright (a
    raised ring over a cut-out or a lifted copy; no layout shift). The card sits **beside** the highlighted link in free
    space, never over the cards it describes: on desktop to the right of the sidebar link (as today, check it does not cover
    page content at 1256 px), at 390 px at the bottom as today. Heading "Step 1 of 8" with Back, Next and Skip tour, as in the
    mockup (today's text "Stop 1 of 8" becomes "Step"). Tour stops follow brief 1's names (Automations etc.) and brief
    11's agent page (Settings button).
- **firstday-5, a fact with a way out.** Anchor `…#f-firstday-5`. After `UXR5/after/firstday-5.png`,
  `UXR5/after/firstday-5-lt.png`; markup `UXR5/after/src/firstday-5.html`; Before `UXR5/before/firstday-5-b1.png`; screens
  `UXR5/screens/B003-first-landing.png`.
  - On Welcome (the first landing) the sentence is "Daneel can't use any app yet. It can reach websites; you can limit
    that." where **you can limit that** links to Internet access (owners; members see "It can reach websites." with no
    link). Implement as a Welcome-only rendering of the reach clause; the Access page keeps the exact table of `docs/14 §8`
    (`reachSentence` unchanged). Update `docs/14 §8` with the Welcome variant and the vitest table of
    `lib/summary/agent.ts`.

### 14. Notifications are blocked only after the person asked (account-1)

- Anchor `…#f-account-1`. After `UXR5/after/account-1.png`, `UXR5/after/account-1-lt.png`; markup
  `UXR5/after/src/account-1.html`; Before `UXR5/before/account-1-b1.png`; screens `UXR5/screens/A170-account.png`,
  `UXR5/screens/B016-your-account.png`.
- `approval-notifications.tsx`: the red "Notifications are blocked for this site…" (:93) shows only after the person turned
  "Notify me in this browser when an approval needs me" on and the browser refused (permission `denied` **and** they just
  tried, or the switch is on). A browser that blocks by default shows nothing until then. Section: "Approval notifications"
  / "How you hear about approvals that need a decision."; "E-mail me approvals nobody is watching" / "Only the title and a
  link to the dashboard are sent."; "Notify me in this browser when an approval needs me" / "Works while a tab is open.
  Stored in this browser only."

### 15. Create an API key on Account (account-2)

- Anchor `…#f-account-2`. After `UXR5/after/account-2.png`, `UXR5/after/account-2-lt.png`; markup
  `UXR5/after/src/account-2.html`; Before `UXR5/before/account-2-b1.png`; screens `UXR5/screens/A172-account-apikeys.png`.
- `my-keys-view.tsx` header **API keys** / "Personal keys you created for workspaces (for example for the CLI)." with
  **Create key** on the right. The dialog "Create an API key": **Name** (placeholder "My laptop CLI"), **Workspace** select
  ("Default · Acme", every workspace the person is in), hint "The key can act only in this workspace.", **Create key** and
  **Cancel**. It calls the per-workspace create (`useCreateWorkspaceKey(wsId)`), then shows `SecretOnce`. Empty state:
  "You haven't created an API key" with the same button.

### 16. Chat platforms name the platform and the bot (account-3)

- Anchor `…#f-account-3`. After `UXR5/after/account-3.png`, `UXR5/after/account-3-lt.png`; markup
  `UXR5/after/src/account-3.html`; Before `UXR5/before/account-3-b1.png`; screens `UXR5/screens/A174-account-channels.png`.
- Header **Chat platforms** / "Link your chat account so Daneel recognises you there." One row per connected platform:
  "Mattermost · Message @aurora-bot in Mattermost and send `link`." with **Link account**. When none is connected:
  "No chat platform is connected yet. An admin can connect one." (D-407, the role, never a name.)
- **Waits for a read** that lists the chat platforms a member can link (type, bot handle, integration). Brief 6 has no such
  member-readable route. Until it exists, keep the current page and say so in the report (gap).

### 17. Account rail: SSH keys and Notifications (account-4)

- Anchor `…#f-account-4`. After `UXR5/after/account-4.png`, `UXR5/after/account-4-lt.png`; markup
  `UXR5/after/src/account-4.html`; Before `UXR5/before/account-4-b1.png`, `UXR5/before/account-4-b2.png`; screens
  `UXR5/screens/A170-account.png`, `UXR5/screens/A175-account-ssh.png`.
- `ACCOUNT_LINKS` (`app-shell.tsx` :266): **Profile · Security · Notifications · SSH keys · API keys · Chat platforms · Delete
  account** (SSH keys only when `features.ssh` is on). New page `app/account/notifications/page.tsx` holds
  `ApprovalNotifications` and push devices (`push-devices.tsx`); `accountRoutes.notifications`. Profile is only the profile:
  "Profile / How you appear to other people in your organizations." with **Display name** (editable), **Default
  organization**, and below "Roles from sign-in, tour replays and appearance stay below." (the existing sections, in that
  order). The profile's SSH link (:151) goes.
- Old addresses keep working (Profile's anchors). Account keeps the last workspace in the switcher and "Back to <Agent>"
  (brief 2's pattern; use it, do not rebuild).

### 18. Validate an SSH key on paste (account-5)

- Anchor `…#f-account-5`. After `UXR5/after/account-5.png`, `UXR5/after/account-5-lt.png`; markup
  `UXR5/after/src/account-5.html`; Before `UXR5/before/account-5-b1.png`; screens `UXR5/screens/A177-ssh-bad-key.png`.
- In `ssh-keys-view.tsx` parse the pasted text as it changes: a line starting `ssh-ed25519 ` (one key, base64 body) is
  valid; anything else is invalid. For `ssh-rsa`, `ecdsa-…` or others: the field shows at once "Only ed25519 keys are
  supported. Create one with `ssh-keygen -t ed25519`." (the command in a copy block) and **Add key** stays disabled. A
  private key pasted (`-----BEGIN`) says "That's a private key. Paste the public key (the file ending in .pub)." and never
  sends it anywhere. Server validation stays authoritative.

### 19. Count badges in the workspace rail and on the phone (owner, 2026-11-20; P5-REFINE-20 rows G1, G2, G6)

The owner reported the navigation's count badges don't work. This brief owns them for the workspace:
- **Approvals** (G1): the count of pending approvals the viewer can decide. Today `PendingPill`
  (`components/app-shell.tsx` ~:219) reads `usePendingApprovals` with no refetch and no stream outside the Approvals
  page, so it goes stale. Read the count from C-1082 (`GET /v1/attention/counts`, P5-REFINE-8) when that has merged,
  and keep it live (the approvals stream where it is open, else a refetch on focus and every 30 s); until then, at
  least add the refetch on focus and the interval.
- **Overview** (G2): the workspace's attention count (C-1082) next to Overview, and the same number in the Overview's
  Needs you heading.
- **Phone** (G6): the drawer shows the same badges; the hamburger button shows a dot when anything waits.
- Rules: no badge at zero; only for people who can act; screen-reader text ("2 need your attention"); inside the link;
  amber only because the viewer can act (D-384); fits at 390 px. Tests: vitest for zero/hidden, the count, and a
  member who can't act; a mocked Playwright spec where the count drops after deciding, without a reload.

## Out of scope

- Names and words of brief 1 (vocabulary), primitives of brief 2, altitude registry of brief 3. Use them.
- The agent page, status rail and Settings (brief 11), agent settings (12), Access (13), Automations (14), Computer (15).
- Organization pages (17) and Administration (18). A member-readable "chat platforms I can link" read for account-3 (a gap
  in the backend briefs: the Orchestrator places it).
- The timeline's backend (brief 9) and the attention feeds' backend (brief 8).

## Acceptance criteria

1. Every item above matches its `UXR5/after/<key>.png` (dark) and `-lt.png` (light) at 1256 px and 390 px (no horizontal scroll
   at 390). The report lists the pairs compared.
2. No snake_case, UUID or dotted key shows outside `TechnicalDetails`/`<code>` on Check a request, Activity, Audit and Approvals
   (the label guard of brief 2 passes with the new templates).
3. Check a request names the rule and says Blocked/Allowed/asks first in a sentence; the raw verdict is collapsed.
4. Activity part A: sentence rows, filters in words, Technical view behind Expert on `ResponsiveTable`. Part B (after 9): rows
   arrive live, in-progress rows change in place, "n new · Show" when scrolled.
5. Audit rows read as sentences with the key or person name; Critical is red; stacked under 900 px.
6. Add people says how the person opens the workspace before submit; a never-signed-in person shows "Not signed in yet"; only a
   change to Owner asks, with what an Owner can do; Approver/Member changes apply at once with "Saved".
7. API keys line and docs link present; revoke asks; the secret opens `SecretOnce`.
8. Renaming the workspace shows "Saved" and the switcher shows the new name without a reload. Delete asks for the visible name.
9. Apps keeps its card grid, has search and the Connected / Ready to connect / Needs an admin first groups; Skills opens with
   what a skill is and the precedence in a fold; no "home sync".
10. Approvals shows one empty state and an Example card; its buttons explain, change nothing, call nothing.
11. Skeleton rows replace "Loading…"; the slow `/apps` finding is measured and fixed or written up.
12. The switcher shows a disabled state while loading and always opens its list; the guide ticks AI key only for a key that
    answered; the tour dims the page and the card never covers the item; the Welcome reach sentence has the link.
13. Account: no red notification text before the person tried; Create key works; rail has Notifications and SSH keys; RSA paste is
    refused at once and Add stays disabled.
14. Web gates (targeted): typecheck, lint on touched files, the vitest files below, the mocked Playwright specs below with axe.

## Tests

Targeted only; GitHub CI is the full gate. Heavy commands through `/home/alex/orca/workspaces/aurora/heavy.sh <lane> <command>`;
never `localhost:4000` or the aurora-dev cluster.

- **Vitest:** `SimulationResult` sentences per verdict; activity sentence templates and filter words; audit sentence table (every
  action has a template); `ROLE_CAN` and the Owner confirm (only Owner asks); `signed_in` chip; API keys row text; General save and
  the switcher name refresh; Delete workspace visible-name match; Apps grouping, search and sort; approvals empty state and
  example answers; `computeGuide` with a checking key; Welcome reach sentence variants; notifications blocked only after a try;
  SSH key parser (ed25519 ok, rsa/ecdsa/private key refused); account rail entries.
- **Playwright (mocked)** at 1280 and 390 px in both themes, axe, no console errors: Check a request; Activity; Audit; Add people
  → promote to Owner → confirm; Settings rename → switcher; Apps search; Approvals example; switcher on a phone; tour backdrop;
  Account create key and SSH paste.
- **Rust:** `cargo test -p aurora-api` for the two `signed_in` views (targeted, through `heavy.sh`).
- Update existing specs that read the old texts ("Verdict:", "Settings saved.", "Nothing is waiting for you." twice, the slug
  confirm, "Stop 1 of 8").

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

- `docs/14-web-dashboard.md`: §Workspace (Activity, Audit, Settings, Apps, Skills, Approvals), §10 People, §8 (Welcome reach
  variant), §Account (rail, Notifications page, SSH keys), §Onboarding (switcher, guide checking state, tour backdrop).
- `docs/API_CONTRACTS.md` and `docs/05-control-plane-api.md`: the `signed_in` field.
- `docs/PROGRESS.md` and `docs/FEATURES.md`: P5-REFINE-16 row and the item rows, current at every commit.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`) for the web items; the small Rust field (scope 6) by `rust-developer`, or the same Sonnet lane if
trivial. Many pages, but each is bounded and has a mockup.

## Completion report

Fill in when done; P5-REFINE-20 reads it.

- **Items done** (number, finding keys; branch `alexiri/p5-lane6`, one commit per item or group, see `git log`):
  1 Check a request (workspace-1) · 2 part A (workspace-2) · 3 Audit (workspace-3) · 4 Add people dialog and role fold (workspace-4) ·
  5 Owner promotion with `ROLE_CAN.workspace` (workspace-5) · 7 API keys (workspace-6) · 8 General rename, Saved sentence, switcher refresh,
  memory-language label (workspace-7) · 9 Delete by visible name (already built by brief 2: `DangerTab` uses `DeleteWithName`; verified, no change) ·
  10 Apps grid with search and groups, Skills copy and fold (workspace-9, -10) · 11 Approvals one empty state and Example card (workspace-11) ·
  12 skeleton cards / "Loading your accounts…" (workspace-12) · 13 switcher loading state (firstday-2), guide `checking` state (firstday-3, brief 5 had
  merged: reads `verification.state` of the key from the secrets list), tour backdrop and "Step n of m" (firstday-4), Welcome reach sentence (firstday-5) ·
  14 notifications blocked only after a try (account-1) · 15 Create key (account-2) · 17 Account rail and Notifications page (account-4) ·
  18 SSH key validated on paste (account-5) · 19 the Approvals badge half (G1) and the phone dot (G6): refetch on focus and every 30 s, action
  approvals counted, screen-reader words, hidden at zero and for people who cannot decide.
- **Items waiting for a backend brief** (which, what is missing, when picked up):
  - 2 part B (live per-agent timeline, in-progress rows in place, "n new · Show"): brief 9 not merged.
  - 4 "Not signed in yet" **data**: the row, chip, "added …" and Copy link are built and tested against `signed_in === false`; the field
    `AccessBinding.signed_in` (C-1086, built by brief 6) is not in the schema yet, so nothing shows it until -6 merges. Scope 6 (Rust) was not built here, as the
    brief says.
  - 11 / 19 Overview: Needs you rows in brief 8's feed shape, the attention count next to Overview and in the Needs you heading, and the Approvals
    badge read from C-1082: brief 8 not merged.
  - 16 (account-3, chat platforms naming the platform and the bot): C-1089 (brief 6) not merged; the page is unchanged.
  - 2 "Automations" filter option: no field on a request-log row says a schedule or automation made it, so the option is not offered until part B.
- **Deviations from the mockup or this brief and why:**
  - Check a request: the rule name is bold, not a link: there is no rule-editor deep link in the codebase. The dialog keeps its title "Try a request".
  - Add people: the e-mail field is labelled "Who" (the mockup); pronoun "they" instead of "she".
  - Audit: only the workspace tab uses sentences (the shared `AuditPanel` keeps its table for organization and Administration, brief 17/18's pages); 45 actions have
    sentence templates (every action in `AUDIT_ACTION` is covered by a test); the other ~60 critical actions fall back to "<Person> did something (“…”)" until their pages need them.
    A person's name comes from the workspace's people list, a key's from `metadata.name` (the audit writes it) or the key list.
  - Skills: the row line "Written by … · used by …" is not built: a skill has no author or user field in the API. The list and editor layout are unchanged.
  - Activity: the Technical view kept its Decision and Host filters (inside the Expert block); "Automations" is missing (above); the app is named from the apps catalogue.
  - API keys: "API documentation" links to `/v1/openapi.json` (the instance serves it); a rendered docs page does not exist: **please confirm** the address.
  - Apps: "Connected" group is a third group above the two (as the brief says). Skeleton cards, not rows, in the grid.
  - Switcher on the phone: I could not find a handler that navigates into the Default workspace (the drawer only closes on `a[href]`); the likeliest cause was the
    live-looking card tapped while `me` loaded, which is now a busy, disabled card. Needs checking on a real phone.
  - **Slow `/apps` (item 12): not measured.** No throwaway cluster here and the machine was busy (perf rule). Reading `list_apps`
    (`services/aurora-api/src/domains/apps/routes.rs:112`): `service::summary(...).await` and `setup::readiness(...).await` run once per app, sequentially;
    that is the place to batch. Proposed brief: measure `GET /v1/apps?workspace_id=` per app on aurora-ux5, then load the per-app facts in one query.
- **Screenshots compared** (key, theme, width, path of the new screenshot): none taken: no Playwright or browser in this lane. Markup of every mockup was read
  (`UXR5/after/src/<key>.html`); P5-REFINE-20 compares the pairs.
- **Tests run** (commands and counts):
  - Targeted vitest after each item, all green (`pnpm exec vitest run <files>`): activity-sentences + agent-filter + altitude-pages + usage-placement (16), labels + policy (25 → 26),
    audit-sentences + audit-view + audit-detail + admin-audit (26), people-table + add-people + refine-17-org (30), account-settings + general-rename + byo,
    app-families + polish-7 (16), workspace-skills (7), approvals + m5-approvals-chat + parked + status-rail (76), waiting-badges (4), context-switcher (24) + switcher-loading,
    onboarding + onboarding-guide (39), welcome + agent-summary + internet-pages (48), ssh (18) + account-pages-refine16 (6), onboarding (23).
  - `pnpm exec tsc --noEmit`: clean at the account commit. eslint on every touched src and test file: 0 errors.
  - Whole web vitest (`cd web && heavy.sh p5-lane6 pnpm exec vitest run`; shared UI changed: app shell, labels, rail): 2797 passed, 1 failed (`design-guards` read the sample "#214" of the Approvals example as a hex colour); fixed with a unicode escape, `design-guards` + `approvals` re-run: 25/25. So 2798/2798 after the fix, not re-run as one suite.
  - `scripts/guards/run-all.sh`: all guards passed. `pnpm exec tsc --noEmit`: clean after the last change.
  - Not run: cargo (no Rust change), `extract-schema --check` (no API change), Playwright.
- **Test files changed or added:** added `tests/activity-sentences`, `audit-sentences`, `general-rename`, `switcher-loading`, `waiting-badges`, `account-pages-refine16`;
  changed `policy`, `labels`, `altitude-pages`, `usage-placement`, `audit-view`, `people-table`, `add-people`, `account-settings`, `byo`, `app-families`, `polish-7`,
  `workspace-skills`, `approvals`, `context-switcher`, `onboarding`, `onboarding-guide`, `welcome`, `agent-summary`, `ssh` (the profile SSH-link tests became rail tests: the link moved to the rail).
  **Removed:** none.
- **Playwright specs updated by reading (not run here; the nightly or P5-REFINE-20 runs them):** `e2e/first-run.spec.ts` (production: Activity sentence + Details),
  `add-people-journey.spec.ts` (production) and `add-people.spec.ts` (label "Who"), `ia-4.spec.ts` (memory-language label), `onboarding.spec.ts` ("Step 1 of"),
  `welcome-summary.spec.ts` (Welcome reach sentence), `web-push.spec.ts` (Notifications moved to `/account/notifications`). New mocked specs: `e2e/refine-16.spec.ts`, written by reading and **not run** (no browser in the lane; the CI Watcher runs them; expect selector fixes). Per theme and width (1280/390): Activity sentences + Details, Audit sentences + Critical, Add people note + Copy link, Settings rename Saved, Apps search, Approvals empty state + Example (no POST), tour backdrop + Esc, Account Create key dialog; plus the phone switcher, the Approvals badge dropping without reload, and Check a request. Not covered by a spec: the Owner confirm dialog, SSH paste in a browser (vitest covers both), and anything of the waiting items.
- **Open questions:** (1) answered: `/v1/openapi.json` is right; (2) should the shared `AuditPanel` get sentence rows for the organization and Administration
  pages in briefs 17/18; (3) the phone switcher device check is P5-REFINE-20's.

