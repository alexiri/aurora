# P5-POLISH-11: Final review: compare every fixed screen with its mockup and send back what's off (owner, 2026-11-19)

## Goal

After P5-POLISH-1..10 are merged, walk the real app as the review did and compare each screen with its
"After" mockup. Check that UX review 4's problems are gone, and send back precise changes for whatever
still differs. The owner asked for this step explicitly: "review all the changes, compare them to the
mockups on the artifacts and send back any needed changes."

This brief is a review, not an implementation. Small fixes in an area nobody is editing may be made
directly (see Scope 5). Everything else goes back to the Orchestrator as a list of changes for the
owning brief's lane.

## Linked requirements

- **The reference (D-380):**
  - “Aurora UX fixes”, <https://claude.ai/artifact/WXbcpcrkbCLuxMSurpkuQW> (proposals A1..F3, the
    owner's comments applied);
  - “Aurora UX review 4”, <https://claude.ai/artifact/33oHaXQpCU3oSVRVmgtY5Z> (100 screens and every
    finding, tagged Broken / Confusing flow / Rough edge / Words);
  - the redesign canvas (D-354), <https://claude.ai/artifact/3UwmUgrhh6MLCsfqdhTFmQ>, for screens the
    fixes didn't touch.
- **Local copy** (workers can't open artifacts): `/home/alex/orca/workspaces/aurora/design-ux-fixes/`.
  - Read `README.txt`.
  - `after/<id>.png` and `after/src/<id>.html` are the targets, `before/<id>.png` today's screens.
  - `review/index.html` holds the review's findings, with screenshots in `review/img/` (each
    screenshot's name starts with the screen number).
  - The canvas copy is `/home/alex/orca/workspaces/aurora/design-canvas/`.
- **Spec:** `14 §Visual design` › "UX fixes" (the rules), D-380..D-393, C-1029..C-1036, and the briefs
  P5-POLISH-1..10 with their completion reports.

## Wave and order

- **Last.** Starts when P5-POLISH-1..10 are all merged to master.
- A fresh lane (Opus or Sonnet with a browser). It does not share a worktree with an implementation
  lane.
- **The loop:** the changes it sends back are implemented in the owning lanes. This review then
  re-checks only those screens, until nothing is left or the owner decides.

## Environment (read first)

- **Never use the shared `aurora-dev` kind cluster, `localhost:4000`, or the `aurora` database in
  `aurora-dev-pg`** (owner's standing rule). Bring up a **throwaway** cluster from your worktree with
  `deploy/dev/kind-dev.sh up --llm-mock --connector-fake`, using `AURORA_DEV_CLUSTER=<your-name>` and
  `AURORA_DEV_PORT=<free port>`.
  - Check `df -h /home` first: a cold cluster takes about 25 minutes and several GB.
  - Tear it down at the end (`kind-dev.sh down` with the same variables) and say so in the report.
- **Data to create**, as the review did:
  - run `kind-dev.sh bootstrap-admin alice@example.test`;
  - save the placement (shared organization "Users", members join as Member, a personal workspace
    each);
  - add an organization AI key on the LiteLLM mock;
  - sign in as Bob Builder and Carol Member through the development sign-in;
  - connect a Calendar link (ICS) account for Bob. The review used Google's public US holidays
    calendar, `https://calendar.google.com/calendar/ical/en.usa%23holiday%40group.v.calendar.google.com/public/basic.ics`;
  - add a second organization with several workspaces to see the switcher's multi-org case;
  - with no Google sign-in app registered, ask for Google Calendar as Bob (C-1030).
- **Browser:** use Orca's browser (`orca` CLI, the `orca-cli` skill) or Playwright, at 1440×900 and
  390×844, in **dark and light** themes. Capture full pages by scrolling and stitching, not with a tall
  viewport (tall viewports repeat tiles).
- Save every capture under `/home/alex/orca/workspaces/aurora/design-ux-fixes/verify/<id>-<theme>-<width>.png`.

## Scope

1. **Per proposal, compare with the mockup** (A1, A2, B1–B5, C1–C6, D1–D6, E1–E8, F1–F3):
   - Open the same screen with the same data the mockup shows. The mockups' sample texts are
     illustrative where the brief didn't quote them.
   - Compare against `after/<id>.png` for:
     - **layout:** order, grouping, columns, widths, alignment, and the 64 px header alignment in C4;
     - **components:** the right primitive (LockPill, StatusLine, Switch, LinkCard, EmptyState,
       Unavailable, AttentionChip);
     - **states:** empty, failing, asked, member versus owner, frozen;
     - **every interactive path** the proposal describes:
       - journey steps;
       - Ask and Undo;
       - Try it in chat with a draft;
       - the … menu;
       - the access select;
       - the switcher's search and organization drill-in;
       - the checklist toast;
       - Approve without expanding;
       - the apps table switch.
   - Check the **rules** of `14` › "UX fixes" on every page you visit, fixed or not:
     - no amber without an action;
     - at most one chip per card, in sentence case;
     - health lines only from real calls (cause a failure: give Bob's GitHub account a bad token and
       check that "Failed … : …" appears and stays until a success);
     - dialogs with three rows and no covered first line;
     - one focus ring;
     - no CSP violation or console error;
     - no horizontal scroll at 390 px;
     - both themes readable (contrast).
2. **Re-check UX review 4's findings.** Go through `review/index.html`. For each finding tagged Broken,
   Confusing flow or Rough edge, mark it **fixed**, **still there** or **changed** (a new problem in
   its place). Leave Words findings for the wording pass, but list any that a P5-POLISH screen
   reintroduced.
3. **Look for new problems** the fixes introduced: text cut off, unreadable buttons, overflow, broken
   links, mismatched labels, layout jumps, dead ends. These are the kinds of problems the owner asked
   to catch.
4. **The report** (in this brief's completion report and as a file
   `/home/alex/orca/workspaces/aurora/design-ux-fixes/verify/REPORT.md`):
   - a table with one row per proposal: ID · verdict (matches / minor differences / needs changes) ·
     screenshots (paths) · notes;
   - a list of **changes to send back**. Each change has:
     - the owning brief (P5-POLISH-n) and the file or component;
     - what is wrong, with the screenshot path and the mockup path;
     - what it should be, precisely: a measurable change such as "the footer's primary button sits at
       the right edge on the Sign in step (it is at the left)";
     - the severity: broken / flow / rough;
   - the re-check table of the review's findings.
5. **Small direct fixes.** A change of a few lines inside one component may be fixed in this lane
   (with tests) when no running lane owns that file. Typical examples are a wrong token, a missing
   `aria-label` or a spacing value. List each fix in the report. Everything else is sent back.
6. **Send back.** Message the Orchestrator (SendMessage `Orchestrator`) with the list of changes
   grouped by owning brief, so it can queue them in those lanes. The Orchestrator also tells the
   coordinator. After they merge, re-check those screens (repeat 1 and 4 for them only) and append a
   "Pass 2" section to the report.

## Out of scope

- The general wording pass, apart from listing regressions.
- New proposals. Ideas beyond the mockups go in a short "Further ideas" list at the end of the report,
  for the owner.

## Acceptance criteria

1. Every proposal A1..F3 has a verdict with dark and light screenshots at 1440 px, and at 390 px where
   the proposal has a phone view (B3, D4, and every full page).
2. Every Broken, Confusing flow and Rough edge finding of UX review 4 is marked fixed, still there or
   changed.
3. The changes to send back are precise and owned, and the Orchestrator has them.
4. The throwaway cluster was used, not `aurora-dev`, and was torn down.
5. After the sent-back changes are merged, a second pass confirms them. Nothing marked "needs changes"
   remains without an owner decision.

## Suggested agent/model and reason

Opus, or Sonnet (`sonnet-integration-reviewer`) with browser access. Judging against a design and
writing precise changes needs a careful eye. Read-only except for the small fixes in Scope 5.

## Completion report — pass 1 (lane `p5-polish-c`, 2026-11-19)

**How.** Master at `d83d396f` (P5-POLISH-1..10 and the CI Watcher's Channels fix). Throwaway kind cluster
`AURORA_DEV_CLUSTER=p5pc AURORA_DEV_PORT=4610` (`up --llm-mock --connector-fake`, web image rebuilt after the
merge), never `aurora-dev`/:4000. Data as the review: alice bootstrapped, placement saved through the checklist
(shared org "Users", Member, personal workspace each), org AI key "Users AI gateway" added through the checklist
dialog, Bob and Carol signed in via the dev IdP, Bob said hello (mock answered "pong"), Bob connected "Team
calendar" (ICS, Google US holidays), Bob asked for Google Calendar (no Google sign-in app: C-1030), a second org
"Acme Labs" with 8 workspaces and 6 more shared workspaces in Users. Playwright (Chromium) at 1440×900 and
390×844, dark and light (`colorScheme`), full pages by scrolling and stitching; on every capture: console errors,
CSP violations (`securitypolicyviolation`), 4xx `/v1` responses, horizontal overflow at 390 px, axe wcag2a/aa.
Captures and `REPORT.md` (this section) are in the lane's scratchpad
(`/tmp/claude-1000/-home-alex-orca-workspaces-aurora-p5-polish-c/bd53a237-0474-4705-b41d-93ed407d92fa/scratchpad/shots/`,
`<id>-<theme>-<width>.png`, step shots `…-<step>.png`), not in `design-ux-fixes/` (the coordinator's rule).

**Automated rules, every page visited (≈40 pages × 4):** axe wcag2a/aa — no violation anywhere; horizontal
scroll at 390 px — none; CSP violations — none (A1/F3 fixed). Console errors left: see the 403/400 items below.
Not exercisable on this stack: F2's failure path (the mock LLM makes no tool calls, so no account ever gets a
real call; no valid GitHub token to connect an account and then break it) and the Channels tab "with an
integration" (the fake connector stays "Waiting for the connector", so no org can add an integration).
Light-theme captures were checked by axe contrast and spot-viewed (Welcome, journey sheet); no light-only defect.

### Verdicts

| ID | Verdict | Screenshots (`shots/`) | Notes |
|---|---|---|---|
| A1 | matches | b4-ics-*, b5-done-*, e4-keydialog-* | three rows everywhere; minor: AI-key dialog body is a card (P5-POLISH-7 #6), Close gets a ring (P5-POLISH-1 #1) |
| A2 | matches | b5-done-dark-1440 | "Try it in chat" readable in both themes |
| B1 | minor differences | b1-appstep-*, b1-search-*, b1-already-* | groups, search, Ask; empty "Ready · 0" while searching; one "G" icon; "Already connected" not exercised (Bob's only account is already Daneel's) |
| B2 | matches | b2-ask-*-asked/undone, b2-alice-overview-*, b2-alice-orghome-* | Ask → asked line with Undo → Ask; Alice's Needs you with Set up / Not now; org table "Bob asked" |
| B3 | matches | b4-github-*-access, b4-ics-light-390 | fixed frame, Back left / action right, full-screen sheet with pinned footer at 390 |
| B4 | matches | b4-github-*-token, b4-github-* | numbered steps, hints for Read only, error under the field, value kept masked |
| B5 | needs changes | b5-done-dark-1440, b5-draft-dark-1440 | no level for a one-level app; `?draft=` fills, not sent |
| C1 | needs changes | c1-access-*, c1-access2-* | no level on the ICS row; AI key row has an "Ask alice" line and no AI tile |
| C2 | minor differences | c2-menu-* | menu grouping and Delete text match; menu not aligned to its button |
| C3 | matches | c3-instructions-*, c3-savebar-* | save bar only after a change; minor: two help lines under Model |
| C4 | minor differences | c4-hello-*, c4-menu-*, c4-rail-* | menu, Ready/Working, no computer flicker, model calls filtered; amber Needs-you icon when empty, lone dot |
| C5 | minor differences | c5-computer-* | no Wake it now (real gap), "awake 0 min" |
| C6 | minor differences | c6-* | Webhooks two buttons, Channels who-can, Peers two empty states |
| D1 | needs changes | d1-accounts-*, d1-pill-*, d1-add-* | two chips, no level on pills, Add on its own row, dialog body card |
| D2 | minor differences | d2-apps-* | status + button per app match; not alphabetical; one "G" icon |
| D3 | matches | d3-approvals-* | one list, empty state explains |
| D4 | needs changes | d4-welcome-*, d4-switcher-* | Welcome matches; switcher menu cut by a fixed max height; "Y" avatar |
| D5 | needs changes | d5-overview-*, b2-alice-overview-* | sentence ignores Needs you; Recent activity shows the model call |
| D6 | needs changes | d6-people-*, d6-org-people-* | Groups in a personal workspace; inherited row inline; 403; org People matches |
| E1 | needs changes (broken) | e1-carol-ai-*, e1-carol-internet-*, e1-carol-agents-* | "no key of its own" is false; Agents is a red error box |
| E2 | minor differences | e2-org-accounts-* | workspace labels match; "AI key" chip; sentence lines missing |
| E3 | matches | e3-org-apps-*, b2-alice-orghome-* | banner, filters, table, switches, asked row; not alphabetical |
| E4 | needs changes | e4-fresh-*, e4-placed, e4-toast2, e4-keydialog-* | Sign-in step never done on dev; layout details; toast/stay verified in code (8 s) |
| E5 | matches | e5-admin-apps-* | "No app is waiting.", From Aurora 14 + "11 more" |
| E6 | matches | e6-computers-*, e6-chat-*, acme-chat | Unavailable cards; admin connector row still raw |
| E7 | minor differences | e7-defaults-*, e7-signin-*, e7-users-*, e4-choose | full-width rows and switches; "Default:" chips on every row; New users form half width |
| E8 | minor differences | e8-health-*, every admin shot | rail matches; health view repeats eight per-app rows |
| F1 | minor differences | f1-internet-*, acme-chat, e6-chat-* | lock pills match; amber notices without an action on Chat platforms |
| F2 | needs changes | d1-accounts-*, e2-org-accounts-* | kind chips remain; failure path not exercisable here |
| F3 | matches | f3-acctmenu-*, c4-hello-* | one ring, composer border only, menu without frame, name+e-mail header |

### Changes to send back (by owning brief)

**P5-POLISH-5 (connect journey)**
1. *flow* — Done for a one-level app says "Daneel can now use Team calendar." with no level (`b5-done-dark-1440`
   vs `after/b5.png`). `connect-journey.tsx:458` passes `done.level` = null when the app has no write tools; pass
   `"read"` when `levelsForApp(app)` is empty, so `doneSentence` says "can now read Team calendar.", and start the
   line under it with the level label: "Read only. You can change this on Daneel's Access tab."
2. *rough* — App step search "goo": "Ready to connect · 0 / No app matches" stays above the two matches
   (`b1-search-dark-1440`). In `app-step.tsx` hide a group with no match while a query is set; show "No app
   matches" only when every group is empty.

**P5-POLISH-6 (agent pages)**
1. *flow* — The ICS row on Access shows no access level (`c2-menu-dark-1440`, `after/c1.png` "Read only"). The
   API gives a read-only app `levels: []` and grant level `use`; `access-control.tsx:110–130` renders nothing for
   that. Render "Read only" (plain text, the row's right side) when `account.levels` is empty. Same root makes D1's
   pills and B5 levelless.
2. *rough* — AI key row on Access has the line "Ask alice (organization owners and admins)." and no "AI" tile
   (`c1-access2-dark-1440`). Expected: AI tile, "Shared by your organization · Daneel thinks with scn-pong", no
   who-can line (C1: who manages it is on the account page).
3. *rough* — "What Daneel can do" and the Overview agent row say "Daneel can read Calendar link (ICS)"; name the
   account: "Daneel can read Team calendar." (`after/c1.png`, `after/d5.png`).
4. *rough* — Status rail "Needs you" heading icon is amber while it says "Nothing is waiting for you."
   (`c4-hello-dark-1440`): neutral icon when nothing waits (F1).
5. *rough* — Webhooks shows "New webhook" twice while empty (header and empty state, `c6-webhooks-dark-1440`):
   keep only the empty state's button until there is a webhook.
6. *flow* — Channels: who-can line says "Only organization owners and admins can connect an app. Ask alice." with
   no button (`c6-channels-*`); `after/c6.png`: "alice can connect a chat platform." with **Ask**.
7. *rough* — Peers shows two stacked empty states ("No agent links", "No peer tasks", `c6-peers-dark-1440`): one
   empty state while both are empty (C6 rule).
8. *rough* — Computer usage "18 tokens · awake 0 min" right after the computer ran (`c5-computer-dark-1440`,
   finding 30 still there): when `awake_seconds` is 0 but turns > 0 say "awake under a minute"
   (`usage-card.tsx:42`); the mockup's "This month" card is a plain section today.
9. *rough* — Every chat open logs `GET /v1/orgs/{org}/retention` 403 for a member (`chat-tab.tsx:130`): call
   `useRetention` only for organization owners/admins.
10. *rough* — Account row … menu (`account-menu.tsx:81`) is pushed to 4 px from the viewport edge instead of its
    right edge lining up with the … button (`c2-menu-dark-1440`): `align="end"` on the menu content.
11. Gaps, still real (owner/API decision): **Wake it now** — no route wakes a stopped computer (C-610 retry is
    only from `failed`, restart only while running); "sleeps after <n> minutes" — `SandboxView` has no idle time.

**P5-POLISH-7 (workspace pages)**
1. *flow* — Overview sentence "Daneel is ready. Nothing needs you." above a Needs you panel with 1 request
   (`b2-alice-overview-dark-1440`): build the sentence from the Needs you items too ("1 request needs you.").
2. *flow* — Overview Recent activity shows the agent's model call: "allow · POST
   litellm.aurora-dev-mock…/v1/messages" (`d5-overview-dark-1440` vs `after/d5.png`). Filter calls to model
   endpoints (as the status rail does) and describe actions in words.
3. *rough* — Accounts: "Users AI gateway" carries two chips ("AI key", "Shared by Users"), "Team calendar" an
   "App" chip (`d1-accounts-dark-1440`, F2: a kind is its icon). Expected (`after/d1.png`): AI tile, "Shared by your
   organization · managed by alice", no chips.
4. *rough* — Accounts sentence "2 accounts." lacks "Daneel uses both."; **Add** sits on its own row under it
   instead of on the sentence's line (`after/d1.png`). Same on the org Accounts tab.
5. *flow* — Agent pill popover on a read-only account offers only "Remove from Daneel" (`d1-pill-dark-1440`); show
   the level ("Read only") as in C2 (root: P5-POLISH-6 #1).
6. *rough* — "Add an AI key" dialog (and the checklist's): `LlmKeyForm` keeps its bordered card inside the dialog
   body and its Cancel/Add key buttons inside that card (`d1-add-dark-1440`): inside a dialog, drop the card border
   and render the buttons in the dialog's footer row (A1).
7. *flow* — People in Bob's personal workspace shows "Groups · No group is bound to this workspace"
   (`d6-people-dark-1440`, finding 52 still there): Groups only in shared workspaces. The inherited owner reads
   "Owner (organization)" inline; `after/d6.png`: a reason line "Has access as an owner of Users".
8. *rough* — People logs `GET /v1/orgs/{org}/members?status=active` 403 for a member: fetch it only for org admins.
9. *rough* — Apps catalogue is in package-id order ("Calendar link (ICS)" after Gmail, `d2-apps-dark-1440`): sort
   by display name.
10. *rough* — Welcome guide shows a "Done" chip beside "AI key" (`d4-welcome-*`): the check is enough (F2).

**P5-POLISH-8 (navigation)**
1. *flow* — The switcher menu has a fixed max height (~320 px) and scrolls inside: at 1440×900 the "Other
   organizations" rows and "New workspace in Users" are hidden, and after the drill-in the search box scrolls away
   (`d4-switcher-dark-1440-open`, `d4-switcher-dark-1440`; `after/d4.png` shows the whole menu). Size it to the
   available height (`--radix-popover-content-available-height` / dropdown equivalent).
2. *rough* — "Your workspace" has a "Y" avatar; `after/d4.png` uses the owner's initial ("B").

**P5-POLISH-9 (organization pages)**
1. *broken* — Carol's Organization › AI models says "The organization has no key of its own." while Users has
   "Users AI gateway" (`e1-carol-ai-dark-1440`). `GET /v1/orgs/{org}/accounts` is 403 for members, so the summary
   concludes there is none. Use a source a member may read (her workspace's accounts list carries the
   org-shared key) or have the API expose the org key's name to members. Then render `after/e1.png`: one key row
   ("Users AI gateway · Any model the gateway serves · default scn-pong") and "Owners and admins change these
   rules. alice is an owner." with **Ask**, instead of the seven operator rows of "Limits from above".
2. *flow* — Carol's Organization › Agents is a red error box "Only organization owners and admins can open this
   page." (`e1-carol-agents-dark-1440`): a member view (sentence + who-can, neutral, D-390), or no rail entry.
3. *rough* — Org Accounts: "AI key" chip; missing "1 account, shared with everyone in Users." and "New agents get
   this key automatically." (`e2-org-accounts-dark-1440` vs `after/e2.png`).
4. *rough* — Org Apps table is in package-id order (`e3-org-apps-dark-1440`): sort by name.
5. *rough* — Org home lists slugs in mono on every row (finding 58, `org-home-dark-1440`): drop them.

**P5-POLISH-10 (administration)**
1. *flow* — Checklist step Sign-in is never done on the dev stack ("Set up", the amber current row,
   `e4-placed-dark-1440`) because `roles` is `warning: no_role_mappings`, though the dev provider is registered
   and alice is an admin by the e-mail list. Count Sign-in done when a provider exists and the admin signed in;
   the role-mapping warning goes to the health view. The brief's button is always **Review**.
2. *rough* — Checklist layout vs `after/e4.png`: rows sit in an inner padded card (the amber band is inset);
   "n of 5" is under the sentence, not right-aligned on its line; the current step's button is outline, not filled.
3. *flow* — Health view "Needs you" lists eight near-identical "People can't sign in to X yet … Register an OAuth
   client" rows with letter avatars (`e8-health-dark-1440`) — the list E3 removed. Group by sign-in app ("Google
   sign-in: Calendar, Drive, Gmail · Register") with package icons, or one line to Organization › Apps "Needs setup".
4. *rough* — People › New users: the placement form is a ~480 px column of fieldsets, not full-width setting rows
   (`e4-choose-dark-1440`, E7).
5. *rough* — Settings rows carry a "Default: …" chip on nearly every row (Agents › Defaults, Sign-in sessions,
   AI models; `e7-defaults-dark-1440`): F2 allows one chip per card; put the default in the help text.
6. *rough* — Agents › Computers: the "Whether previews are on…" note is half width; "Data one shared preview may
   serve a day 5368709120 bytes" is a bare number (show GB).
7. *rough* — Admin › Chat platforms: connector row "fake · fake · Self-hosted · Waiting for the connector" (raw id,
   three chips) and an amber notice "The shared bootstrap token is refused…" with no action (F1, `e6-chat-*`).
   The organization page `acme-chat` also has an amber "An instance administrator needs to add Mattermost first.
   Ask: alice." without a button.
8. Health item 2 (left open by P5-POLISH-10) explained: the dev chart gives the API pod no egress to
   `aurora-dev-mock` (`deploy/kind/values-llm-mock.yaml:27` says so; the API NetworkPolicy allows only postgres,
   hookhost, gateway, dev-idp and public IPs). The probe fails ("did not answer from the API") and the key's verify
   waits 10 s (`POST /v1/orgs/{org}/secrets` 201 after 10 049 ms). Fix in the dev values: an API egress rule to
   the mock namespace like the gateway's (`values-llm-mock.yaml:40`) — owner to place (deploy/kind).
9. Gap "needs a token / Google sign-in" on From Aurora rows: still real until the admin list API carries the
   sign-in method.

**P5-POLISH-2 (shared pieces)**
1. *rough* — `StatusLine` (`status-line.tsx:39–41`, `flex-wrap`): a long child wraps below the dot, leaving the
   dot alone on a line (status rail Computer `c4-hello-dark-1440`, Admin › AI models endpoint warning
   `e7-ai-models-dark-1440`). Give the text span `min-w-0 flex-1` so it wraps beside the dot.

**P5-POLISH-1 (dialogs)**
1. *rough* — A dialog opened from a menu shows the focus ring on its Close button (`d1-add-dark-1440`, finding 103
   still there): Radix autofocuses Close; focus the first field or the content (`onOpenAutoFocus`).

**P5-POLISH-3 (packages)**
1. *rough* — Google Calendar, Drive and Gmail share one "G" icon in the journey, the catalogue and the org table
   (B1/D2 ask for product icons).

### Fixed directly in this lane (Scope 5)

- `web/src/components/admin/setup-view.tsx`: the checklist no longer requests `GET /v1/orgs//secrets` (400 and a
  console error on /admin before the placement named an org) — `useSecrets(…, !!org)`; the checklist's AI-key
  dialog uses P5-POLISH-7's buttons (Cancel, **Add key**), closing that half of P5-POLISH-10's "swap the AI-key
  Dialog" caveat (the body card remains, P5-POLISH-7 #6). Test: `tests/setup-checklist.test.tsx` (new "asks for
  no organization's keys before the placement names an organization" — seen failing without the fix; the toast
  test uses the dialog's Add key and checks Cancel); `e2e/p5-polish-10.spec.ts`, `e2e/onboarding-journeys.spec.ts`
  click **Add key**. Gates: vitest file 11/11, `pnpm typecheck` clean, eslint on the four files clean. Playwright
  not run (CI is the gate).

### Gaps from the completion reports

- No "Wake it now", no idle minutes: real (P5-POLISH-6 #11). No toast primitive: real (the checklist's toast is a
  status Notice that hides after 8 s, enough for E4). AI-key dialog swap: closed for the buttons (above).
  Workspace People Suspend / Joined column: not re-verified (no shared workspace with a second member was set up).

### UX review 4 findings, re-checked (Broken / Confusing flow / Rough edge)

Fixed: 250 (all four but the shared "G"), 251 guide and alignment, 252 (all), 253, 254 (both), 255 GitHub, 257,
200 composer ring, 201, 202, 210 (both), 211, 212, 213 (both), 221, 222, 229, 240 model plumbing, 241 chip/status,
242 menu, 100 (both), 101 (both), 104/120 Requests box, 105 cut text and chip, 105 status per app, 123, 106 (both),
107, 109 legend and role case, 303 form, 402, 403 rows/variants/radio, 406 own row, 010 (both), 011 (both), 013
bare values, 013 Optional, 013 builtins pending, 015 toast, 020 (both), 022, 024 (both), 050 (both), 028 (both),
031, 033 provider, 034 (one home), 060 (both), 503, 504, 602, 603, 605 (both), 606.
Changed: 200 rail icons → tooltips on hover; 240 Remove gone but an "Ask alice" line stays (P5-POLISH-6 #2); 242
menu now 4 px from the viewport edge (P5-POLISH-6 #10); 303 no form, but a false "no key" sentence (P5-POLISH-9
#1); 400 shared first, slugs remain; 403 rows gone from org Apps but reappear on the admin Health view
(P5-POLISH-10 #3); 121 dialog → popover, still only removal (P5-POLISH-7 #5); 015 "did not answer" explained
(dev network policy, P5-POLISH-10 #8).
Still there: 200 "You and the a…" truncated preview; 230 "awake 0 min"; 241/104/120 no access level (P5-POLISH-6
#1); 109 Groups in a personal workspace and Alice inline (P5-POLISH-7 #7); 250/105 one "G" icon (P5-POLISH-3);
302 "AI models" balance-scale icon; 406 "Email is not configured" line (dev, informational); 605 Close's focus
ring (P5-POLISH-1).
Not checked: 111 API keys expiry input, 302 All workspaces, 405 org Internet (owner), 408/409 org Data/Settings,
013 confirmation after placement Save, 026 Self-hosted, 029 AI models Rules, 030 admin Internet, 033 directory
sync, 036 Retention, 043 All settings, 103 workspace Skills.
Words regressions: none introduced by P5-POLISH seen; "Delete agent…" (mockup "Delete Daneel…"), "Shared by
Users" (mockup "Shared by your organization").

### Further ideas (owner)

- The agent's Access "Add an AI key" button beside "Give Daneel an app" is not in C1; keep it or move it into Add.
- A dev-stack switch so the LLM mock can call one app tool would let F2's "Failed … : …" path be reviewed.

**Left for pass 2:** re-check the screens above after the owning lanes merge; Carol's views of Channels with an
integration (needs a running chat connector), F2's failure path, the not-checked findings.
**Cluster:** torn down at the end of pass 1 (`kind-dev.sh down`, same variables); pass 2 brings it up again.
