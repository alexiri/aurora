# P5-REFINE-1: Vocabulary and renames, and "What moved" removed (owner, 2026-11-20)

## Goal

UX review 5 found one concept with several names and the machinery in people's sentences. This brief
lands **every rename of the layer-above vocabulary table at once** (D-407) and removes "What moved".
Labels change; routes, ids, API codes and `?tab=` values do not.

What is wrong now:

- **Several names for one thing** (theme "Several names for one thing", rec 2): Behaviour / Brief,
  Triggers, Evals / Judge / Checked as, Peers, Access, "Tighten…", "Rules people wrote", "Check what
  would happen" as a button, Freeze, "Disabled (user)", OAuth client / sign-in app, connector /
  channels, "Provider endpoint", "Gate command", "Redact…", "Memory language", "Default agent".
- **Freeze** is used for agents, workspaces and organizations. The owner chose **Pause / Resume**
  everywhere, including organizations (D-407). The API (`frozen`, 423 `frozen`, `/freeze` routes) does
  not change.
- **Text that points to an administrator names a person** ("Ask alice", "managed by alice", "Needs
  alice first"). Instance and organization admins can be several people (priority 7). The text names
  the **role** (D-407).
- **"What moved"** shows a green banner of renames to a brand-new person (firstday-1). There are no
  users yet, so it is removed entirely (D-407).

Decisions that bind this brief: **D-405** (precedence: this brief, then the owner's decisions, then the
mockup, then the finding's fix text), **D-407** (all renames together, Pause/Resume, roles not names,
What moved removed).

## Linked requirements

- **The review (source of truth, owner's comments applied):**
  <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>. Workers cannot open artifacts, so read the local
  copy `/home/alex/orca/workspaces/aurora/design-ux-review-5/` (called `UXR5/` from here on).
  - `UXR5/layer-above.md`, rec 2 and the table "Proposed vocabulary" at its end; rec 15 (the "stop it"
    ladder); rec 16 (Redact, Memory language, Default agent, What moved). Anchor
    <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#above>.
  - `UXR5/priorities-themes.md`: theme "Several names for one thing"; priority 7 (role wording).
    Anchors `#themes`, `#top`.
- **Findings with a mockup** (artifact anchor `https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#f-<key>`;
  After renders `UXR5/after/<key>.png` dark and `UXR5/after/<key>-lt.png` light; markup
  `UXR5/after/src/<key>.html`; Before `UXR5/before/<key>-b1.png`; main screens `UXR5/screens/<id>.png`):
  - **firstday-1**: Before `UXR5/before/firstday-1-b1.png`; screen `UXR5/screens/B014-what-moved.png`.
  - **channels-3** (wording part): Before `UXR5/before/channels-3-b1.png`, `-b2.png`; screens
    `UXR5/screens/C033-channels.png`, `C034-ask-alice.png`.
  - **schedules-4** (wording part): Before `UXR5/before/schedules-4-b1.png`, `-b2.png`; screens
    `UXR5/screens/C008-schedule-created.png`, `C015-schedule-off.png`.
  - **webhooks-2** (option words only): Before `UXR5/before/webhooks-2-b1.png`; screens
    `UXR5/screens/C018-webhook-form.png`, `C020-webhook-created.png`.
  - The full finding text is in `UXR5/findings.md` (search the key). The designer's notes there are
    binding where this brief does not say otherwise.
- **Screens for the layer-above renames** (Before only; no mockup per word):
  `UXR5/screens/B033-agent-actions-menu.png`, `B145-agent-chip-menu.png`, `C036-new-agent.png`,
  `C041-rex-chat-cant-answer.png`, `C010-schedule-runs.png`, `C025-webhook-deliveries.png`,
  `C015-schedule-off.png`, `B179-danger-zone.png`, `A031-admin-agents.png`, `B003-first-landing.png`,
  `B014-what-moved.png`.
- **Decisions:** D-405, D-407 (and D-230, D-338, D-382 are amended by this set; this brief only
  touches their words). Existing: D-341/C-974 (words test), C-987 (notices; its web part is retired).
- **Spec:** `14` §Information architecture › Vocabulary; §Settings in words.

## Wave and order

- Wave 1, **lane A**, starts immediately. It is the first web brief; P5-REFINE-2 rebases on it where
  both touch a component (this brief merges first, since it only changes strings).
- Later briefs (11, 12, 13, 14, 15, 16, 17, 18) quote these words. They must not reintroduce a retired
  word; the extended words test (Scope 8) enforces it.

## Context

Paths are under `web/src/` unless noted. Line numbers are master `c0fe7d10`.

- **What moved:**
  - `components/onboarding/renames-notice.tsx` (the notice; reads `onboarding.notices_seen`);
  - `lib/moved.ts` (the rename table `movedRows`, `RENAMES_NOTICE = "renames-2026-11"`, `MOVED_EVENT`,
    `showWhatMoved`);
  - `components/app-shell.tsx`: import :13 (`showWhatMoved`), :23 (`RenamesNotice`), the menu item
    :163–166 ("What moved" with `ScrollIcon`), the render :512;
  - tests: `web/tests/renames-notice.test.tsx`, `web/tests/context-switcher.test.tsx` :290 and :298 (menu
    item lists), `web/e2e/ia-navigation.spec.ts` :84–111.
  - Server: `user_onboarding.notices_seen` and `PATCH /v1/me/onboarding { notice_seen }` stay (C-987);
    the Rust upsert still seeds `renames-2026-11` for a new row (`crates/aurora-db/src/repo/onboarding.rs`
    `apply`). **Remove that seed too** (coordinator, 2026-11-20: the owner asked to get rid of all of the "What moved"
    functionality): drop `renames-2026-11` from the upsert's default and its Rust test expectation; existing rows that
    carry the key keep it (harmless, no migration); `notices_seen` itself and C-987 stay for other notices.
- **Labels:**
  - `components/agents/sections.ts`: `PAGE_LABEL` :47–64 (Evals, Peers, Access, Computer: Connect),
    `SECTIONS` :67–73 ("Behaviour", "Triggers", "Access");
  - `components/agents/agent-page.tsx` :127–150 (menu items "Behaviour", "Access", `configTitle`);
  - `components/agents/instructions-tab.tsx` (field "Brief" :170, `a-brief`), `create-agent-dialog.tsx`
    :123 ("Brief (optional)");
  - `components/agents/evals/*` (`evals-tab.tsx`, `questions-section.tsx` :86 "Checked as",
    `judge-section.tsx` "Judge settings", "Judge key", "Judge model", `copy.ts` :54, `run-detail.tsx`,
    `fingerprint-diff.ts` :31–32);
  - `components/peers/peers-tab.tsx` ("Peer tasks" :113);
  - `components/ui/scope-ladder.tsx` :47 (`tightenLabel = "Tighten…"`), :65 "Limits from above",
    :104 "Tightened below"; `components/agents/workspace-byo.tsx` :25;
    `components/workspace/workspace-topics.tsx` :72;
  - `components/internet/internet-level.tsx` :67, :300, :407 ("Rules people wrote"), :503–510 (button
    "Check what would happen"); `components/policy/simulate-panel.tsx` :86, :132;
  - `components/governance/controls.tsx` :19–23 ("Set by Administration"), `components/ui/lock-pill.tsx`
    :10, `components/admin/secret-stores-view.tsx` :339 and `components/admin/settings/*` ("Set in Helm");
  - `components/schedules/schedules-tab.tsx` :118 ("Cron (5 fields)"), :125 ("Gate command (optional)"),
    :82 `anchored` and `components/automations/owner-controls.tsx` (`AnchorSwitch`);
  - `components/credentials/llm-form.tsx` :229 ("Provider endpoint");
  - `components/chat/message-group.tsx` :126 ("Redact…"), `components/agents/conversation-actions.tsx` :152
    :182–184 (`RedactDialog`: "Redact this message?", "Redact");
  - `components/create-workspace-form.tsx` :92, `components/settings/general-tab.tsx` :57 ("Memory language");
  - `components/agents/status-rail/parts.tsx` :299 ("Default agent"), the agents list "Default" badge
    (`grep -rn '"Default"' components/agents`), `components/channels/account-channels-view.tsx` :83;
  - OAuth client wording: `components/admin/sign-in-gaps.tsx` :17–131, `components/admin/plugin-detail-view.tsx`
    :198–202, `components/apps/oauth-clients-panel.tsx` :191, :330;
  - chat platforms: `components/channels/*` (`connector-blocked.tsx` :114), `components/connectors/*`,
    `components/admin/*chat*`, `lib/routes.ts` topic label `chat-platforms`.
- **Freeze words:** `components/freeze/freeze-panel.tsx` (:60–62 consequence lines, :125–127 "Unfreeze …",
  :136, :167 "Pauses it until you unfreeze it", :182–183 dialog), `freeze-sections.tsx` :65–82,
  `frozen-entity-banner.tsx`, `components/admin/orgs-view.tsx` :36–99, :126–141,
  `components/admin/org-detail-view.tsx` :73, `components/admin/settings/governance-settings-view.tsx` :155,
  `components/admin/audit-view.tsx` :29, `components/org/settings-view.tsx` :236.
- **Admins by name:**
  - `components/ask-admin.tsx` :63 ("Ask {name ?? "an admin"}");
  - `components/apps/apps-tab.tsx` :45 (`Needs ${admin ?? "an admin"} first`), :114 ("Ask {firstName}");
  - `components/accounts/account-row.tsx` :185–189 ("managed by ${names}");
  - `components/channels/connector-blocked.tsx` :114 (`adminNames`);
  - `lib/who-can.ts`: `askSentence` :63–70 names up to three people ("Ask Ada Lovelace…"), `ROLE_PHRASE`;
    `components/ui/who-can.tsx`.
- **Words test:** `web/tests/ui-text.test.ts` (`RETIRED` :87, `RETIRED_LABEL`, `VOCABULARY_EXEMPT` :119 is
  empty today). Glossary: `docs/14-web-dashboard.md` §Vocabulary (:1021–1055).

## Scope

Every item changes the visible word only. Names are the ones in the table; where the table says
"sentence under it", keep the old words as the explanation.

1. **Remove "What moved" (firstday-1).**
   - Artifact `#f-firstday-1`; After `UXR5/after/firstday-1.png` / `-lt.png`, markup
     `UXR5/after/src/firstday-1.html`; Before `UXR5/before/firstday-1-b1.png`;
     screen `UXR5/screens/B014-what-moved.png`.
   - Delete `renames-notice.tsx`, `lib/moved.ts`, the account-menu item and its `ScrollIcon` import, the
     `<RenamesNotice />` render and its imports, `web/tests/renames-notice.test.tsx`, and the What moved
     cases in `context-switcher.test.tsx` and `ia-navigation.spec.ts` (the menu lists become "Your
     account", [Administration], "Show me around", "Sign out").
   - A new person (and an old one) sees a plain Welcome with no banner. Nothing else in the account menu
     moves.
   - `routes.ts` helpers are not removed (other code uses them). Remove only what `lib/moved.ts` alone
     used.
2. **The vocabulary table (rec 2, D-407), all rows.** Refs for every row: `UXR5/layer-above.md` rec 2 and
   the table at its end; artifact `#above`; Before screens as listed in Linked requirements.

   | Today | Visible word after | Where (see Context) |
   |---|---|---|
   | Behaviour (menu item and page title) | **Instructions** | `agent-page.tsx` menu + `configTitle`, `sections.ts` |
   | Brief (field, create dialog) | **Instructions** ("Instructions (optional)" in the create dialog) | `instructions-tab.tsx`, `create-agent-dialog.tsx` |
   | Triggers (tab) | **Automations** | `sections.ts` `SECTIONS` |
   | Evals, "Evals" page, Judge, Judge key/model, Checked as | **Tests**, **Grader**, "Grader key", "Grader model", **Checked by** | `evals/*`, `sections.ts` |
   | Peers, "Peer tasks" | **Other agents**, "Tasks from other agents" | `peers-tab.tsx`, `sections.ts` |
   | Access (tab, page, menu) | **Apps & access** | `sections.ts`, `agent-page.tsx` |
   | Limits from above | **Set for you by <who>** (the actor named as WhoCan names it: "an Aurora administrator", "your organization's admins") | `scope-ladder.tsx` |
   | "Set by Administration", "Set by your organization", "Set in Helm" (lock pill owner text on policy pages) | **Set for you by an Aurora administrator** / **… by your organization's admins**; Helm-set values: **Set by the installation** (the "Set in Helm" page name is brief 18's) | `governance/controls.tsx`, `lock-pill.tsx` callers |
   | Tighten… | **Restrict further…** | `scope-ladder.tsx` :47 |
   | Tightened below | **Restricted further below** | `scope-ladder.tsx` :104 |
   | Rules people wrote | **Your rules** | `internet-level.tsx` |
   | Check what would happen (button) | **Try a request**; the section keeps "Check what would happen" as the sentence under it | `internet-level.tsx` :503, `simulate-panel.tsx` |
   | Disabled (user), Enabled (automations) | **Paused / On** (accounts, rules, apps keep Turn off/on) | `schedules-tab.tsx`, `webhooks-tab.tsx`, `lib/api` status words |
   | Verification none / hmac_sha256 / bearer (option labels) | **Signed (recommended) · Password in the header · Anyone with the address** (webhooks-2, words only; the default and the Generate button are brief 14) | `webhook/*`, `webhooks-tab.tsx` |
   | Deliveries accepted / rejected_signature / disabled | **Received · Refused: wrong signature · Refused: paused · Duplicate, ignored** | `webhooks-tab.tsx` deliveries |
   | Runs "fired" | **Answered · Skipped by the gate · Failed · Running** | `schedules-tab.tsx` runs |
   | Gate command, Anchor to workspace | **Only run when… (a check command)** · **Keep running if I leave** | `schedules-tab.tsx` :125, `owner-controls.tsx` |
   | Provider endpoint (AI key dialog) | **Which service is this key for?** | `credentials/llm-form.tsx` :229 |
   | OAuth client, Google sign-in (as a thing a person sets up) | **Sign-in app**, everywhere ("OAuth client" stays only as the Administration tab id and in code) | `sign-in-gaps.tsx`, `plugin-detail-view.tsx`, `oauth-clients-panel.tsx` |
   | Connector, connector host, integration, Channels (the platform a person connects) | **Chat platform**; the machinery page (Administration) says **Chat platform host** | `components/channels`, `components/connectors`, admin |
   | Workspace address, URL name, slug | **Web address** (shown as the full URL; never typed to confirm, see brief 2) | `settings/general-tab.tsx`, `create-workspace-form.tsx` |
   | Redact…, "Redact this message?", "Redacting…" | **Remove this message's content…**, "Remove this message's content?", "Removing…" | `message-group.tsx` :126, `conversation-actions.tsx` |
   | Memory language | **Language <Agent> remembers in** (the agent's name; in the workspace form "Language agents remember in") | `create-workspace-form.tsx`, `general-tab.tsx` |
   | Default agent, "Default" badge | **Came with this workspace** | `status-rail/parts.tsx` :299, agents list badge |
   | Technical details for members | no word change here; hiding it from members is brief 3 (altitudes) | — |
   | Harness: Claude Code / "fake" | no change here (brief 12 and 18) | — |

   Keep the old word anywhere it is **code** (route, `?tab=`, test id, API field, enum value).
3. **Pause and Resume replace Freeze in every visible string (rec 15, D-407).**
   - Artifact `#above` (rec 15); screens `UXR5/screens/B145-agent-chip-menu.png`,
     `UXR5/screens/B179-danger-zone.png`.
   - Words: Freeze → **Pause**, Unfreeze → **Resume**, Frozen → **Paused**, "Freezing…" → "Pausing…",
     "Freeze organization" → "Pause organization". Dialog titles: "Pause Daneel?", "Pause <workspace>?",
     "Pause <organization>?". The dialog says what stops (keep today's consequence lines, with
     "resume" for "unfreeze": "Schedules, webhooks and chat presences pause; they resume when you
     resume it."). Keep the reason field.
   - The section titles in Settings: "Pause this workspace", "Pause this organization". Banners:
     "Paused by <name> on <date>: <reason>".
   - **Organizations too**: `orgs-view.tsx`, `org-detail-view.tsx`, `governance-settings-view.tsx`,
     `org/settings-view.tsx` :236 ("Only organization owners can pause or delete the organization."),
     the Administration filter "Paused organizations only".
   - Do not change `lib/api/freeze.ts`, route names, `frozen` fields, the 423 `frozen` code or the
     test ids. The error text for 423 reads "<Agent> is paused".
   - Ladder rung words that already exist stay: Turn off/on (accounts, rules, apps, chat platforms),
     Suspend/Reinstate (people), Remove from <Agent>, Rotate, Delete.
4. **Administrators by role, never by login (priority 7, D-407).**
   - Artifact `#top` priority 7; `#f-channels-3`; screens `UXR5/screens/C034-ask-alice.png`,
     `UXR5/screens/C033-channels.png`.
   - Rewrite every place that names a person where it points at "the people who can do this":
     - "Ask alice" / "Ask {firstName}" → **Ask an admin** (instance level: "Ask an Aurora administrator";
       organization level: "Ask an organization admin"; workspace: "Ask a workspace owner");
     - "Needs alice first" → **Needs an admin first**;
     - "managed by alice" → **Managed by your organization's admins**; instance level **Managed by an
       Aurora administrator**;
     - `askSentence` in `lib/who-can.ts`: with names, "Ask Ada Lovelace or Alan Turing" is replaced by
       the role phrase: "Ask an organization admin." (`ROLE_PHRASE` singular forms). Names are no
       longer printed anywhere an admin is meant. The data (`who_can.people`) stays; it is just not
       shown.
     - `ask-admin.tsx`: the button reads **Ask an admin**; the confirmation reads "Asked the admins.
       You'll see it here when it's done."
   - A **person's own name** stays where the person is the subject ("Bob asked", "Invited by Alice
     Admin") and in people lists.
   - Routing of the request ("goes to every admin who can act") is brief 8's backend; this brief changes
     words only and keeps today's call.
5. **channels-3 wording.** Artifact `#f-channels-3`; After `UXR5/after/channels-3.png` / `-lt.png`;
   markup `UXR5/after/src/channels-3.html`. In the Channels who-can line: "Ask an admin to connect Slack,
   Teams or Mattermost." (the three platform names are the types the instance supports; read them from
   the supported-connector list where it exists, else say "a chat platform"). The optional reason field
   and the routing are brief 14 (UI) and 8 (data). Today's "Ask alice" button reads **Ask an admin**.
6. **schedules-4 wording.** Artifact `#f-schedules-4`; After `UXR5/after/schedules-4.png` / `-lt.png`;
   markup `UXR5/after/src/schedules-4.html`. A schedule that is off reads **Paused**, one that is on reads
   **On** (the card status, the menu item "Pause" / "Resume", the toast). "Disabled (user)" and
   "Enabled" no longer appear. The plain-language cadence and "next at 23:55" are brief 14.
7. **Copy that other briefs will rewrite:** do **not** rebuild pages here. Only replace words. If a
   string sits in a component that a later brief replaces (agent page menu, schedules/webhooks tabs), still
   change the word now so the vocabulary test passes on master.
8. **Glossary and words test.**
   - `docs/14-web-dashboard.md` §Vocabulary: replace the table with the full glossary: the existing rows
     plus every row of Scope 2, Pause/Resume, "Ask an admin" rules, and a new column **Introduced at**
     (Use / Set up / Expert, from brief 3's table; "minute 1 / 5 / 6 / first week / on demand" from rec 2's
     order of introduction as a short list under the table). Say that "Say" is binding and "Not" lists
     the retired words.
   - `web/tests/ui-text.test.ts`: extend `RETIRED` (capitalised whole words, as today) with: Freeze,
     Unfreeze, Frozen (as a visible word), Triggers, Evals, Peers, Tighten, "Rules people wrote",
     "Disabled (user)", "Gate command", "Provider endpoint", "Redact", "Limits from above",
     "Default agent", "OAuth client" (allowed only in a `string` that is a tab id or inside `<code>`;
     the test already reads strings, so list exact exempt strings), and the patterns `/\bAsk [a-z]+\b/`
     is **not** used (too loose). Add a test "never names a person where an admin is meant": fails on the
     literals "Ask alice", "managed by" followed by a template expression of names, and on
     `askSentence` being given `people` names (unit test of `askSentence`: it never prints a name).
   - `VOCABULARY_EXEMPT` stays empty.

## Out of scope

- Moving pages, new tabs, the Settings button, the one Automations list, the rail headings (briefs 11,
  12, 14).
- New copy for the rewritten screens (the mockup words are in the area briefs).
- Any API, route, `?tab=` id, enum or test-id change. Renaming `Freeze` code symbols.
- The Rust seed of `renames-2026-11` and the `notices_seen` column.
- Hiding Expert words from members (brief 3).

## Acceptance criteria

1. A new account's Welcome has no "What moved" banner and the account menu has no "What moved" item; the
   files `renames-notice.tsx` and `lib/moved.ts` do not exist; `rg "What moved|showWhatMoved|RenamesNotice|movedRows" web/src web/tests web/e2e` finds nothing.
2. Every row of the Scope 2 table is visible as the new word where the old one was, in both themes,
   checked on: agent page menu, Instructions, Tests page, Other agents, workspace Internet access
   (Your rules, Try a request, Restrict further…), Settings, the AI key dialog, Channels, a message's
   "…" menu.
3. `rg -n "Freez|Unfreez|Frozen" web/src/components web/src/app` finds only code (imports, symbols, test
   ids, comments) and no visible string; the pause dialog for an agent, a workspace and an organization
   each says what stops.
4. No visible string names a person where "an admin" is meant; `askSentence` never prints a name.
5. A schedule that is off reads Paused, on reads On; no "Disabled (user)".
6. The extended `ui-text.test.ts` passes with an empty `VOCABULARY_EXEMPT`, and fails when a retired word
   is added to a component (show this once locally).
7. Matches `UXR5/after/firstday-1.png` (dark) and `-lt.png` (light) at 1256 and 390 px (plain Welcome);
   `UXR5/after/channels-3.png`/`-lt.png` and `UXR5/after/schedules-4.png`/`-lt.png` for the **words** that
   this brief owns (the rest of those mockups is brief 14's).
8. Web gates for the touched files: typecheck, eslint, the vitest files named below, build is left to CI.

## Tests

Targeted only. Heavy commands run as `/home/alex/orca/workspaces/aurora/heavy.sh <lane> <command>`;
GitHub CI is the full gate.

- Vitest: `ui-text.test.ts` (extended), `who-can` (new: `askSentence` prints roles, never names),
  `context-switcher.test.tsx` (menu items), the tests of every component whose string changed
  (`freeze*`, `scope-ladder`, `internet-level`, `schedules`, `webhooks`, `evals`, `peers`, `agent-page`,
  `apps-tab`, `account-row`, `ask-admin`). Update their expected strings; do not loosen them.
- Playwright (mocked): update `ia-navigation.spec.ts`; run one spec that opens the agent menu and the
  Pause dialog.
- Run only what you touched.

## Docs to update

- `docs/14-web-dashboard.md`: §Vocabulary (glossary, Introduced at), §Information architecture (remove
  the "Pages have new names" notice paragraph and `lib/moved.ts`), the Freeze bullets of the agent page
  and Settings (now Pause).
- `docs/10-*` §Freezing an agent: add one line "Shown as Pause/Resume in the dashboard (D-407); the
  semantics and codes are unchanged".
- `docs/API_CONTRACTS.md` C-987: note "the web no longer shows the renames notice (D-407); the field
  stays".
- `docs/PROGRESS.md` and `docs/FEATURES.md` rows for P5-REFINE-1.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). Many files, all string changes, with a test that must pass on the whole
tree.

## Completion report

Fill in when done (P5-REFINE-20 reads this):

Lane A, branch `alexiri/p5-polish-a` (commits listed in the lane's worker_done message).

- **Items done** (Scope 1–8):
  - **1 What moved:** `renames-notice.tsx`, `lib/moved.ts`, the account-menu item, the render, `renames-notice.test.tsx`, the What moved cases in `context-switcher.test.tsx`, `ia-navigation.spec.ts` and `p5-polish-8-switcher.spec.ts` are gone; the Rust upsert and the "no row" answer no longer seed `renames-2026-11` (IT `onboarding` updated and green). `rg "What moved|showWhatMoved|RenamesNotice|movedRows" web/src web/tests web/e2e` finds nothing but the new words test. Commit "item 1".
  - **2 Vocabulary table:** every row except those listed under Deviations: Instructions, Automations, Tests/Grader/Checked by, Other agents, Apps & access (tab, page, menu and the "… tab" sentences), Set for you by …, Set by the installation, Restrict further…, Restricted further below, Your rules, Try a request, Paused/On, webhook verification and delivery words, run outcome words, Only run when…, Keep running if I leave, Which service is this key for?, Sign-in app, Chat platform / Chat platform host, Web address (create form), Remove this message's content… (also the memory revision's "Remove content of #n"), Language agents remember in, Came with this workspace.
  - **3 Pause/Resume:** agent menu, dialogs ("Pause Daneel?"), buttons, banners ("Paused by …"), Settings section titles, the 423-related texts in the web (`Files`, `Terminal`, `SSH`, Tests), organizations in Administration and Settings, "Paused organizations only". API names, routes, test ids and `lib/api/freeze.ts` are unchanged.
  - **4/5 Admins by role:** `askSentence` prints the role only ("Ask an organization admin."); `Ask an admin`, "Asked the admins. You'll see it here when it's done.", "Needs an admin first", "Managed by your organization's admins" / "an Aurora administrator", chat platform blocked states ("Ask an Aurora administrator."), the apps journey and the member Chat platforms page ("Only organization owners and admins can connect a chat platform." + **Ask an admin**).
  - **6 schedules-4 words:** a schedule/webhook that is off reads **Paused**, on reads **On**; "Disabled (user)" is gone.
  - **8 Glossary and words test:** `docs/14` §Vocabulary has the full glossary with the **Introduced at** column and the order of introduction; `ui-text.test.ts` has the extended `RETIRED`, an "OAuth client" test and a "never names a person where an admin is meant" test; `VOCABULARY_EXEMPT` stays empty. `who-can.test.tsx` tests that `askSentence` never prints a name. Shown once locally: a string "Freeze this agent" in a component made the words test fail.
- **Deviations from the brief and why:**
  - **Run outcome "Running"** is not shown: `ScheduleRun.outcome` only says how a run started (`fired`, `gated_quiet`, `gate_failed`, `skipped_unauthorized`, `coalesced`), so `fired` reads "Answered" for now. The turn's state arrives with brief 14.
  - **Webhook "Duplicate, ignored"** is in the word table but the API never logs duplicates, so no row can show it today.
  - **Memory language** reads "Language agents remember in" in both places (the workspace Settings page has no single agent to name).
  - **The channels-3 "Slack, Teams or Mattermost" names** are not shown: no supported-chat-platform list is available to a member, so the sentence says "a chat platform" (the brief's fallback). The optional reason field is brief 14's.
  - **Connector counts** ("2 connections") and "Chat platform" words in the Administration pages were renamed without a mockup (no mockup exists for them).
  - **Not touched (server text):** the 423 message text and other messages that come from the Rust API ("agent is frozen" style) are the server's; only web strings changed. The DB column default of `user_onboarding.notices_seen` keeps `renames-2026-11` (the upsert never relies on it; no migration, as the brief says).
  - `docs/05` has no change (no API change); `web/openapi.json`, `schema.d.ts` and `spec_schema.json` are untouched (`extract-schema --check` passes).
- **Screenshots compared** (path, dark/light, 1256/390 px, verdict): none. This lane changed strings only and no cluster was started (rule: no aurora-dev, no Playwright); the words of `UXR5/after/firstday-1`, `channels-3` and `schedules-4` were matched from their markup (`after/src/*.html`), the layout parts belong to briefs 11/14. P5-REFINE-20 compares the pixels.
- **Words I could not place** (a string that sits in a component another brief rewrites): the schedules and webhooks tabs ("Cron (5 fields)" stays: the plain-language cadence is brief 14); "Set in Helm" stays inside sentences of the Administration settings pages (the page's own name is brief 18's); server-side texts (above); "Users" as an organization name in `setup-copy` ("none is set up for this workspace, Users or Aurora") is data, not a word.
- **Tests run** (all through heavy.sh):
  - `pnpm vitest run` on the 68 test files that mention a changed word, plus `ui-text` and `memory-skills`: 68 files, 850 tests, all passed.
  - `pnpm typecheck`: clean. `pnpm eslint` on the touched files: 0 errors (3 old warnings in files I touched: `terminal.spec.ts`, `account-row.tsx`, `accounts.test.tsx`).
  - Rust: `cargo fmt --check` ok; `cargo clippy -p aurora-db -p aurora-api --all-targets -- -D warnings` clean; `cargo test -p aurora-api --test it onboarding` 4 passed (throwaway Postgres, stopped afterwards).
  - `python3 scripts/extract-schema.py --check` up to date; `scripts/guards/run-all.sh` all guards passed.
  - Playwright specs edited for the new words (not run, per the rules): `ia-navigation`, `p5-polish-8-switcher`, `agent-config`, `agent-tabs`, `agent-page-p5`, `agent-evals`, `chat-v2`, `connect-journey`, `app-setup`, `internet-access`, `workspace-pages`, `ia-3`, `ia-4`, `who-can`, `add-people`, `credential-access`, `first-run`, `p5-polish-10`, `welcome-summary`, `accounts`, `onboarding-journeys`, `llm-key-modes`, `admin-secret-stores`, `admin-settings`, `stage4-axe`, `fix-127-mattermost`, `support/connectors`, `files`, `terminal`.
- **Open questions:** none blocking. For the owner/Orchestrator: should the server's own message texts that say "frozen" (423 `frozen`, approvals resume reasons) read "paused" too (a Rust-only follow-up)?
