# P5-POLISH-7: Workspace pages: Accounts, Apps, Approvals, Overview, People, Internet access, Welcome's links (owner, 2026-11-19)

## Goal

Make the workspace pages say what's true and what a person can do next. UX review 4 found:

- **Accounts:**
  - each account shows a bare "Daneel" button with no access level, and it opens a dialog that only
    offers removal;
  - an empty "Requests · No proposals are waiting for a value" box is always there;
  - from the Add menu, apps open a dialog but an AI key opens an inline card-in-card form ("Provider
    endpoint", "Add LLM key").
- **Apps:**
  - every card says "0 connections" and offers the same Details and Connect, whether the app is ready,
    connected or blocked on an admin;
  - descriptions are cut mid-word;
  - the Google apps share one "G";
  - a "CUSTOM ACCESS ONLY" caps chip pushes a card out of line.
- **Approvals:** two lists, "Waiting" and "Action approvals", with no explanation of the difference.
- **Overview:**
  - the title is the workspace name, with no sentence;
  - the agent card has HOSTED and DEFAULT chips, and the "New agent" placeholder card is as big as a
    real agent;
  - "Live" floats far from Recent activity, and the guide card repeats Welcome's.
- **People:**
  - a role legend paragraph, then a table, then a second "Members with access" table that repeats Alice;
  - a Groups section in a personal workspace;
  - the role select shows lowercase "owner";
  - dates show seconds.
- **Internet access:** amber "SET BY YOUR ORGANIZATION" chips (P5-POLISH-2 replaces them), "Ask alice"
  under every limit, an unexplained History button, and a full "Check what would happen" form at the
  bottom.
- **Welcome's "Next, when you are ready" cards** look like text boxes.

## Linked requirements

- **Design reference (D-380).** Proposals **D1**, **D2**, **D3**, **D5**, **D6** (workspace part), **F1**
  (Internet access page) and the Welcome cards of **D4** in “Aurora UX fixes”,
  <https://claude.ai/artifact/WXbcpcrkbCLuxMSurpkuQW>. These are full desktop pages.
  - Local copy `/home/alex/orca/workspaces/aurora/design-ux-fixes/`:
    - `after/d1.png`, `after/d2.png`, `after/d3.png`, `after/d5.png`, `after/d6.png`, `after/f1.png`;
    - `after/d4.png`: Welcome, the "Next" cards;
    - the `before/` crops, and `after/src/<id>.html`.
  - The review: <https://claude.ai/artifact/33oHaXQpCU3oSVRVmgtY5Z> (screens 100–123).
  - The owner's comments, applied in the artifact:
    - an app and an account are different things: the Apps tab is the whole catalogue with each app's
      status; Accounts is what you have;
    - Suspend and Remove live in each row's … menu (D6), never on your own row.
  - Yellow dashed labels are commentary.
- **Decisions:** **D-383** (health), **D-384** (amber, chips), **D-386** (Ask), **D-388**
  (readiness).
- **Contracts:** **C-1029** (`readiness`, `accounts`, `summary`), **C-1030** (ask and withdraw).
  `14 §Agents, accounts and access` (the Accounts list, the Apps tab), `14 §Information architecture`.

## Wave and order

- After **P5-POLISH-6** (account row, access select, health line), **P5-POLISH-3** and **P5-POLISH-4**,
  in lane A: … → P5-POLISH-6 → **P5-POLISH-7** → P5-POLISH-9.
- P5-POLISH-8 (navigation) may merge before or after. This brief does not touch the rail, the page
  header's level pill or the Welcome card sentences (P5-POLISH-8 owns those).

## Context

All paths are under `web/src/`.

- **Apps & accounts:**
  - `components/workspace/workspace-topics.tsx:38` (`WorkspaceAppsPage`);
  - Accounts `components/accounts/accounts-panel.tsx` (`RequestsBanner` :29, the Requests section
    :32–38) and `accounts-list.tsx` (Add menu :96–107; `LlmKeyForm` inline at :126; more "Connect an
    app or website" buttons at :158, :172);
  - `account-row.tsx` (`AgentEntry` :30; the "<Agent> and <Account>" dialog :40, rendered :184–185);
  - `components/credentials/proposals.tsx:57` ("No proposals are waiting for a value");
  - AI key form `components/credentials/llm-form.tsx:82` (`LlmKeyForm`, modes in `llm-key-modes.ts`).
- **Apps tab:** `components/apps/apps-tab.tsx` (connection count :36/54, "Custom access only" badge
  :61, Details :68, Connect :74, `AppsTab` :81).
- **Approvals:**
  - `components/approvals/approvals-view.tsx:416` (tabs :423);
  - `PendingTab` :106 (`usePendingApprovals`, `lib/api/approvals.ts:80`; stream :135);
  - `HistoryTab` :348;
  - the Action approvals section :435–438 (`ActionPending` :255, `usePendingActionApprovals`
    `lib/api/action-approvals.ts:34`; `ActionHistory` :289);
  - `OwnSendApprovals` (`own-send-approvals.tsx`).
- **Overview:** `components/overview/overview-view.tsx` (title :79, `NeedsYou` :81, agents :83–111,
  chips :96–98, `LiveIndicator` :29 used at :123, Getting started :114–116),
  `components/overview/needs-you.tsx`.
- **People:**
  - workspace `components/workspace/workspace-topics.tsx:76` → `components/settings/access-tab.tsx`
    (`ROLE_HELP` :31, legend :238, table :250);
  - `components/settings/group-access.tsx` ("Members with access" :144).
- **Internet access:** the workspace page in `components/internet/` (find `WorkspaceInternet`). It holds
  the History button and the "Check what would happen" simulator.
- **Welcome:** `components/onboarding/getting-started.tsx:379–395` (the "Next" cards, links from
  `optionalNextLinks` :347, hand-styled).
- **Shared pieces:** P5-POLISH-2 (`LinkCard`, `StatusLine`, `EmptyState`, `LockPill`, `formatWhen`);
  P5-POLISH-6 (account row, access select, health mapping).

## Scope

1. **Accounts (D1).**
   - **Page sentence** above the list, built from state: "<n> accounts. <Agent> uses <both/all/n of
     them>.", "No accounts yet. …" with the empty state, or "1 account needs you." when one is failing
     (that case takes precedence). Use the `sent-s` style.
   - **Each row:** the app icon, the account name, the health line (P5-POLISH-6) and one **agent pill**
     per agent that can use it: avatar, "<Agent> · <access level>". Pressing a pill opens the access
     select of P5-POLISH-6 for that agent (a popover, not a dialog).
   - For the AI key, the pill shows only the agent name (keys have no levels), and the row says
     "Shared by your organization · managed by <Name>".
   - The "<Agent> and <Account>" dialog goes away.
   - **Requests** appear only while there are requests (agent access requests, P5-UX-17). Agent secret
     proposals keep their place but render nothing when empty.
   - **Add** (a menu): "Connect an app or website" opens the journey; **"Add an AI key" opens a
     dialog** (`DialogContent`, title "Add an AI key", body `LlmKeyForm`, footer **Cancel** /
     **Add key**) instead of the inline card. The form's contents are unchanged except the button
     label.
2. **Apps (D2).**
   - **Cards in a three-column grid**, as in `after/d2.png`. Each card has:
     - the app's own icon (36 px) and name;
     - the `summary` (C-1031), or the description clamped to two lines with a full-text tooltip;
     - one status line from `readiness`, with the button that follows it:

       | Status line | Button |
       |---|---|
       | "Ready · connect with a token / key / sign-in" (from the suggested method's kind) | **Connect** (primary) |
       | "Connected · <account name>" (or "<n> accounts") | **Connect another** |
       | "Needs <Name> first" | **Ask <First name>** (C-1030, with the asked state and Undo as in the journey) |
   - Details opens from the card's name (link) or a … menu, not a second button.
   - No "0 connections".
   - "Custom access only" becomes a line on Details ("You choose exactly what agents may do in
     <App>."), not a chip.
   - Apps switched off for the organization are not listed.
3. **Approvals (D3).**
   - **One list** on the Waiting tab, merging gateway approvals and action approvals (client-side
     merge of the two hooks, newest first; or one API list if one exists).
   - Each row:
     - the app or site icon;
     - "<Agent> wants to <action sentence>" (the sentences P5-UX-16 already produces);
     - "<App> · <account> · asked <formatWhen>";
     - **Details**, **Block** and **Allow once** (and **Always allow** where P5-UX-16 offers it).
     - Same decision hooks as today.
   - **Page sentence:** "<n> requests are waiting for you." or the empty state "Nothing is waiting for
     you. When <Agent> asks before a change, the request appears here and on Overview."
   - **History** merges the same way.
   - `OwnSendApprovals` (posts as you) stays a separate section titled for what it is, shown only when
     it has items.
4. **Overview (D5).**
   - Title **Overview**.
   - **Sentence from state:** "<Agent> is ready. Nothing needs you." / "<n> things need you." /
     "<Agent> is working on <n> conversations." (the canvas Overview board's sentence;
     `design-canvas/project/Overview.dc.html`).
   - **Agents** as list rows: avatar, name, the agent summary (`AgentSummary` short form) and **Chat**.
     **New agent** becomes a button by the section heading (where allowed).
   - No HOSTED or DEFAULT chips.
   - "Recent activity" with the **Live** dot next to its heading.
   - The guide card shows on Overview only after Welcome stops being the landing page, until the guide
     is done. Never on both pages at once.
5. **People, workspace (D6).**
   - **One table** with columns Person, Role, Joined:
     - **Person:** avatar, name and e-mail, "(you)" on your own row;
     - **Role:** a select ("Owner", "Member"; sentence case) where you may change it, plain text
       otherwise;
     - **Joined:** `formatWhen`, exact in the tooltip;
     - a … menu with **Suspend** ("Can't sign in to this workspace until you resume them") and
       **Remove from workspace…** (danger, with confirmation).
   - **Your own row** has no menu and no role change. The last owner's role can't be lowered (the
     server already refuses; hide the option).
   - **Inherited access:** people with access through the organization appear as rows with the reason
     under their name ("Has access as an owner of Users"), with no menu. Remove the separate "Members
     with access" table.
   - The role legend moves into a "What can each role do?" disclosure under the table.
   - **Groups** show only in shared workspaces.
6. **Internet access, workspace page (F1 page).** Match `after/f1.png`:
   - the page sentence;
   - **Common changes** cards;
   - **Rules people wrote**;
   - **Limits from <Org>** with `LockPill`s (P5-POLISH-2) and no "Ask …" lines;
   - **History** explained by its label ("History of changes");
   - "Check what would happen" collapsed into a button that opens the simulator in a dialog.
7. **Welcome's links (D4 cards).** "Next, when you're ready" cards become `LinkCard`s (title, one line,
   arrow, hover and focus). Same links (`routes.ts`).
8. **Tests:**
   - **Vitest:**
     - the Accounts sentence variants;
     - agent pill text per level, and the AI key pill;
     - Requests hidden when empty;
     - the AI-key dialog;
     - Apps status lines and buttons for each readiness state, including Ask/Undo;
     - Approvals merge order and the empty sentence;
     - Overview sentences;
     - People: own row without a menu, inherited row, last owner;
     - the Internet simulator in a dialog.
   - **Playwright** (mocked), 1280 and 390 px, both themes, axe, no console errors:
     - give access through a pill;
     - add an AI key in the dialog;
     - Ask for Google Calendar from the Apps tab;
     - approve from the merged list;
     - suspend Carol from the menu.
   - Update the specs that relied on the old layout.

## Out of scope

- The rail, the level pill and Welcome's card sentences (P5-POLISH-8).
- The organization pages (P5-POLISH-9).
- The general wording pass.

## Constraints / file ownership

- **Owns:**
  - `components/accounts/{accounts-panel,accounts-list}.tsx` (page level; the row is P5-POLISH-6's);
  - `components/credentials/{llm-form,proposals}.tsx` (dialog placement and empty rendering);
  - `components/apps/apps-tab.tsx`;
  - `components/approvals/*`;
  - `components/overview/overview-view.tsx`; `needs-you.tsx` only where P5-POLISH-4 did not already
    change it;
  - `components/settings/{access-tab,group-access}.tsx`;
  - the workspace Internet access page;
  - `components/onboarding/getting-started.tsx` (the Next cards).
- Uses the shared pieces; does not change them.

## Acceptance criteria

1. Accounts matches `after/d1.png`:
   - the sentence;
   - pills with access levels;
   - no empty Requests box;
   - Add an AI key in a dialog.
2. Apps matches `after/d2.png`: a status line and the right button per app, no "0 connections", no cut
   descriptions, product icons.
3. Approvals is one list (`after/d3.png`). Overview opens with a sentence and lists agents as rows
   (`after/d5.png`). People is one table with row menus and no menu on your own row (`after/d6.png`).
   Internet access matches `after/f1.png`.
4. Web gates: typecheck, lint, vitest, build, Playwright mocked (axe, narrow-route, both themes).
   Report the counts.

## Completion report — review pass 1 (P5-POLISH-11)

- **#1 done.** The Overview sentence counts the same items as the Needs you panel, app setup requests included.
- **#2 done.** Recent activity drops the agent's model calls (`actionsOnly`, as the status rail) and reads as a sentence:
  "Daneel used create issue in Team tracker." / "Daneel reached host." / "tried to …" when refused. Open: the mockup's
  verbs ("read Team calendar") and app icon — a log row has a tool id and a connection id, no access kind or app id.
- **#3 done** (in the P5-POLISH-6 commit, same row code): AI tile, "Shared by your organization · managed by alice",
  no chips (the shared and kind chips are gone).
- **#4 done.** "2 accounts. Daneel uses both." (one agent that uses every account; workspace list) and **Add** on the
  sentence's line, on the workspace and the organization Accounts tab.
- **#5 done** through P5-POLISH-6 #1: the pill popover shows "Read only".
- **#6 done.** "Add an AI key" (and the setup checklist's dialog): no card border, Cancel and Add key in the dialog's
  footer row (`LlmKeyForm footerTarget`, `useFooterSlot`). Playwright specs click the footer's button.
- **#7 done.** Groups only in a shared workspace (`GET /v1/workspaces/{ws}` carries no `personal_of`, so the page reads
  it from the organization's workspace list); the inherited owner reads "Owner" with "Has access as an owner of <Org>".
- **#8 done.** The org member list is fetched only for organization owners and admins.
- **#9 done.** Apps catalogue sorted by display name.
- **#10 done.** No "Done" chip on guide steps.
- Tests: vitest `polish-7` (sentences, activity line), `account-settings`; Playwright `accounts`, `credential-access`,
  `p5-polish-10`, `onboarding-journeys`.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). Several pages, each moderate, sharing the account pieces built in
P5-POLISH-6.
