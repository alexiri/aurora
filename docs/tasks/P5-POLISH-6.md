# P5-POLISH-6: Agent pages: tabs and menu, Access, Instructions, chat status, Computer, empty states, honest health (owner, 2026-11-19)

## Goal

Make the agent's pages about the agent, not about the engine. UX review 4 found:

- **Access** opens with a good summary sentence, then half a page of "Model readiness" (roles, three
  amber "Large/Medium/Small is not set" warnings, endpoint names). The accounts come last, and an
  account doesn't show the access it was given.
- **Account rows:** each card has "Remove access" plus a More menu with Give access (already given),
  Rename, Reconnect, Test, Turn off and Remove. Two removals do different things.
- **Instructions:** a 1,900 px form (Brief, Harness, model roles, Subagents, three amber warnings,
  Effort, Proactive messaging), with Save at the very bottom.
- **Chat:**
  - the right rail is five unlabelled icons;
  - the header flips "Computer: Off → On → Stopping" after one message;
  - the status panel lists the agent's own model call as a tool call.
- **Computer:** a lowercase "stopped" with a Restart button, a runner address with an IP, a Kubernetes
  start log, and missing Terminal, Files and Previews tabs with no explanation.
- **Empty states:** Memory and Skills show a small "No … yet" box beside "Select a … to edit it.".
  Channels shows three empty sections under a note that nothing can be attached. Webhooks' empty state
  has no button.
- **Health lines** say "Works" when a test passed or nothing failed.

The owner's comments added two rules:

- **Behaviour and Access are configuration.** They leave the tabs for the … menu, with Freeze and
  Delete (D-382).
- **A health line says only what a real call proved** (D-383).

## Linked requirements

- **Design reference (D-380).** Proposals **C1**–**C6** and **F2** of “Aurora UX fixes”,
  <https://claude.ai/artifact/WXbcpcrkbCLuxMSurpkuQW> (`#c1` … `#c6`, `#f2`). These are full desktop
  pages.
  - Local copy `/home/alex/orca/workspaces/aurora/design-ux-fixes/`:
    - `after/c1.png`: Access as a configuration page;
    - `after/c2.png`: the access select and the row menu;
    - `after/c3.png`: Behaviour › Instructions;
    - `after/c4.png`: Chat with the … menu open and the status panel;
    - `after/c5.png`: Computer;
    - `after/c6.png`: Memory and Channels empty states;
    - `after/f2.png`: health lines, including a failure row.
    - Also the `before/` crops, and `after/src/c*.html` for the exact layout.
  - The review: <https://claude.ai/artifact/33oHaXQpCU3oSVRVmgtY5Z> (screens 200–242, 602).
  - The owner's comments are applied in the artifact:
    - the "Direct chat" and "Status" headers share one 64 px height, so their bottom borders form one
      line;
    - the … menu holds Behaviour, Access, a divider, **Freeze Daneel** and **Delete Daneel…**;
    - every "Works" became "Worked … ago".
  - Yellow dashed labels are commentary.
- **Decisions:** **D-382**, **D-383**, **D-385**, **D-384**.
- **Contracts:** **C-1036** (tabs and menu, web), **C-1033** (health fields, P5-POLISH-3), **C-1034**
  (role warnings move to the endpoint). `14` › Agent page (phase 5) and the new D-382 bullet there.

## Wave and order

- After **P5-POLISH-1**, **P5-POLISH-2** and **P5-POLISH-3** (health fields), in lane A:
  P5-POLISH-1 → P5-POLISH-2 → **P5-POLISH-6** → P5-POLISH-7 → P5-POLISH-9.
- P5-POLISH-7 reuses this brief's account row (access select, menu, health line).

## Context

All paths are under `web/src/`.

- **Agent page:**
  - `components/agents/agent-page.tsx`: header :162–223, the tab bar `<Tabs label="Agent sections"
    overflow="menu">` :213, sub-page tabs :238;
  - "Agent actions" menu :146–210: Freeze through `AgentFreeze inMenu`
    (`components/freeze/freeze-sections`), "Delete agent…" (`components/agents/delete-zone.tsx`
    `DeleteAgentDialog`), owners only;
  - `SandboxPill` :44–70 ("Computer: {computerWord}", `status-rail/status-data.ts:99`).
- **Sections:** `components/agents/sections.ts` (`SECTIONS` :67–73, `PAGE_LABEL` :47,
  `isPageAvailable` :91–100). Feature flags at `agent-page.tsx:82–90`. Routes are in `lib/routes.ts`.
- **Status rail:**
  - `components/agents/status-rail/status-rail.tsx`: parts :57–83, `StatusRail` :111, `StatusSheet`
    (mobile) :276;
  - `status-rail/parts.tsx`: `NeedsYouList` :101, `DoingNow` :139, `RecentCalls` :187,
    `ComputerPart` :220, `AboutPart` :269.
- **Access:**
  - `components/agents/access-tab.tsx`;
  - `agent-summary.tsx:33`;
  - `model-readiness.tsx:120` (`ModelReadinessSection`);
  - `components/accounts/accounts-list.tsx:29`;
  - `account-row.tsx` (`AccountRow` :65, `AgentEntry` :30, "Remove access" :50 and :213, "Technical
    details" :191, `connection-details.tsx`);
  - `account-menu.tsx:26` (Rename :80, Reconnect :81, Test :84, Turn off/on :88, Remove :91);
  - `access-choice.tsx`, `access-control.tsx`;
  - `account-health.tsx:13`, `lib/account-health.ts` (`healthLine`, `testOutcomeText`).
- **Instructions:**
  - `components/agents/instructions-tab.tsx`: Brief :132, Harness :137, Default model :156,
    `ModelRolesFields` :193, Effort :208, Proactive messaging :218, Save :233;
  - `model-roles-fields.tsx:24` (Subagents use :118, warnings :66–70);
  - `lib/model-roles.ts:32–41`.
- **Other pages:**
  - Memory `components/memory/memory-tab.tsx:38`; Skills `components/skills/skills-tab.tsx:6`;
    Evals `components/agents/evals/evals-tab.tsx:19`;
  - Schedules `components/schedules/schedules-tab.tsx:232`; Webhooks
    `components/webhooks/webhooks-tab.tsx:245`; Channels `components/channels/agent-channels-tab.tsx:430`
    (Presences :244/251, Space bindings :279, Reach requests :354); Peers
    `components/peers/peers-tab.tsx:98`;
  - Computer `components/agents/computer-tab.tsx:167` (state and Restart :201–213, `UsageCard` :216,
    Last start attempt :79, Start log :131, Awake because :298, Processes :313).
- **Shared pieces** from P5-POLISH-2: `EmptyState`, `MasterDetail`, `StatusLine`, `LockPill`,
  `Switch`, `AttentionChip`, `formatWhen`.

## Scope

1. **Tabs and the … menu (C4 first bullet, D-382, C-1036).**
   - The section tab bar shows **Chat · Triggers · Computer** (hosted) or **Computer** (BYO).
   - The … button opens a menu, as in `after/c4.png`:
     - **Behaviour** ("Name, instructions, model, memory, skills");
     - **Access** ("Apps, accounts and the AI key it uses");
     - a separator;
     - **Freeze <Agent>** ("Pauses it until you unfreeze it") or **Unfreeze <Agent>** when frozen;
     - **Delete <Agent>…** in danger ink.
   - Freeze and Delete keep their existing flows and permissions.
   - Members without owner rights see Behaviour and Access read-only, as today, with no Freeze or
     Delete.
   - **On a Behaviour or Access page:**
     - no tab is selected and the … button shows the accent outline;
     - under the header: a back link "Chat" (to the agent's Chat, or Computer for BYO), the page name
       (`st`) and one sentence: Access "What <Agent> can use: apps, accounts and its AI key."; Behaviour
       "How <Agent> works: its name, instructions, model, memory and skills.";
     - Behaviour keeps its segmented row Instructions · Memory · Skills · Evals.
   - `?tab=` ids are unchanged. Old links (`?tab=access`, `?tab=instructions`) open the configuration
     page. A BYO agent opens on Behaviour › Instructions as before.
   - Update `sections.ts`, `routes.ts` helpers and the tour stops that pointed at the old tabs.
   - On a phone the same menu is in the … button. The tab row is Chat · Triggers · Computer, with no
     "More" overflow needed.
2. **Header and chat status (C4).**
   - The header shows one agent state: **Ready**, **Working**, **Needs you**, **Can't answer**. Use a
     `StatusLine` dot plus the word, from the existing status data. "Can't answer" means the model is
     unusable (model readiness not ready, or the AI key's health failing).
   - Remove `SandboxPill` from the header. Computer start and stop are not header states. A start that
     takes longer than 5 s shows as a quiet line in the conversation ("Starting Daneel's computer…"),
     removed when the turn starts.
   - **Status rail**, collapsed: each icon has a visible label on hover and focus (tooltip) and an
     `aria-label`.
   - **Status rail**, expanded: labelled sections without caps chips, in this order:
     - **Needs you**;
     - **Doing now**;
     - **Recent actions**: "Only actions in apps and websites; thinking isn't listed.". Calls to the
       agent's model endpoint are filtered out (match the AI key's endpoint host or the request kind);
     - **Computer**: "Asleep. It wakes when Daneel needs it.", "Awake", "Starting…", or "Couldn't
       start: <reason>" with **Open Computer**;
     - **About**: the instructions excerpt and **Edit instructions**.
   - Drop the HOSTED/DEFAULT chips (the kind shows in About as text when it matters).
   - The "Direct chat" header and the "Status" header are the same height (64 px), so their borders
     line up.
3. **Access (C1, D-385).**
   - Order: the summary card ("What <Agent> can do"), then **Apps and accounts** with **Give <Agent> an
     app** (primary) on the right of the heading, then the list.
   - Remove `ModelReadinessSection` from the Access tab. Keep the component if Behaviour's model select
     uses parts of it.
   - The AI key is one row:
     - an AI tile;
     - the key name;
     - "Shared by your organization · <Agent> thinks with <model>", or "… through this key";
     - its health line.
     - A model-readiness problem shows on this row as "Can't answer: <reason>" with its fix link (for
       example "Choose a model" → Instructions › Model).
   - Each account row, as `after/c1.png`:
     - the app icon;
     - the account name (and the app name when the account name is not the app);
     - the health line (D-383);
     - the access control: the select when the app offers more than one level, plain text ("Read only")
       when it offers one;
     - the … row menu.
   - The organization's key has no Remove for someone who cannot change it. Who manages it shows in the
     row menu as a disabled line ("Managed by Alice Admin"), or as a `LockPill`.
4. **Access select and row menu (C2).**
   - The access control is a select: Read only · Ask before changes · Change without asking ·
     Custom… · What exactly?. **What exactly?** opens the actions table from `14` §2. It is the same
     control as today's `AccessControl` (`access-control.tsx`), drawn as a select per `after/c2.png`.
     Keep the three-way switch where the journey uses it.
   - The row menu, in this order:
     - **Test the connection**;
     - **Sign in again** (Reconnect);
     - **Rename**;
     - a separator;
     - **Remove from <Agent>** ("The account stays for other agents");
     - **Delete account…** in danger ink ("Also removes it from <other agents>", naming them; with
       none, "Nobody else uses it").
   - Drop **Give access** from this menu (the agent already has access) and **Turn off** (D-382's
     menu note: removing from the agent covers it).
   - Delete keeps its confirmation.
   - The row's separate "Remove access" button goes away.
   - "Technical details" stays as a disclosure under the row, collapsed.
5. **Health lines (F2, D-383).**
   - Rewrite `lib/account-health.ts` for C-1033's states:
     - `working` → "Worked <formatWhen(last_ok_at)>";
     - `failing` → "Failed <formatWhen(last_error_at)>: <reason in words>", with the fix button
       (**Reconnect** for `rejected`/`expired`, **Try again** for `unreachable`);
     - `not_used` → "Not used yet" (with a **Test** action when the app has a probe);
     - `sign_in_expired` → "Sign-in expired" with **Sign in again**;
     - `disabled` → "Turned off".
   - A test result shows only as "Test passed <when>" or "Test failed <when>: <reason>" next to
     `not_used`. A passed test never shows as "Worked".
   - Render with `StatusLine`. Apply it everywhere `AccountHealth` is shown:
     - account rows (agent Access and workspace Accounts);
     - the agent header attention;
     - the line above the message box (P5-UX-14);
     - Overview's Needs you.
6. **Behaviour › Instructions (C3, D-385).**
   - One card (max width 720 px):
     - avatar and Name side by side (click the avatar to change it; today's upload control becomes
       the avatar's own menu);
     - **Instructions** (the textarea; the field today labelled "Brief", same data) with help "What
       <Agent> should do and how. It reads these before every conversation.";
     - **Model**: one select, "Workspace default · <model>" or a model the key serves (today's Default
       model field);
     - **<Agent> may message you first**: a switch, "For example when a schedule finds something."
       (today's Proactive messaging: on = Ask first or Allowed per its current mapping; keep the stored
       values and map them, with the mapping written in the completion report);
     - **Advanced: model roles, effort, agent program**: a collapsed disclosure holding Harness
       (read-only text), the three roles, Subagents use and Effort.
   - Remove the three role warnings from this page (D-385). P5-POLISH-10 shows them on the endpoint.
   - **Save:** a save bar appears at the bottom of the viewport only when something changed ("Unsaved
     changes", **Discard**, **Save**, as on the settings boards). There is no Save button at the end of
     the page.
7. **Computer (C5).**
   - **Top card**, one sentence state with the right action:
     - "<Agent>'s computer is asleep" with **Wake it now** ("It wakes by itself when <Agent> needs it
       and sleeps after <n> minutes without work. Its files stay.", from the configured idle time);
     - "… is awake" with **Restart**;
     - "… is starting";
     - "… couldn't start" with the failure in words and **Try again**.
   - **Usage:** one card "This month" with "<n> conversation turns" and "<tokens> tokens · awake
     <time>" (from the existing usage data). Replace the four caps tiles.
   - **Missing pages:** when Terminal, Files, Previews or SSH are unavailable, one `WhoCan` line saying
     which are not set up and who can turn them on ("An Aurora administrator", or an organization owner
     for Previews when that is the switch).
   - **Technical details** (collapsed): Last start attempt, the start log, Awake because and Processes,
     unchanged inside.
8. **Empty states (C6).**
   - Use `EmptyState` and `MasterDetail` (P5-POLISH-2) on Memory, Skills, Evals, Schedules, Webhooks,
     Channels and Peers.
   - **Memory:** "<Agent> doesn't remember anything yet" / "It saves notes here as you work together.
     You can also write one yourself." / **New memory**. No "Select a memory" pane while empty.
   - **Skills:** the same pattern. "Skills this agent receives" shows only when there are some.
   - **Webhooks:** the empty state has **New webhook**, as Schedules has **New schedule**.
   - **Channels:** when the organization has no chat integration, the whole tab is one empty state:
     "<Agent> isn't in any chat yet" / "Your organization hasn't connected a chat platform. Once it
     has, you can add <Agent> to channels here." / a who-can line with **Ask**.
     - Presences, Space bindings and Reach requests appear only when there is an integration.
     - **Show progress in chat** appears only then too.
   - **Evals:** a fresh agent shows one empty state ("Add a question you know the answer to…" with
     **Add a question**). Judge, Runs and Schedules appear after the first question.
9. **Tests:**
   - **Vitest:**
     - sections (three tabs; menu items per role; frozen state);
     - configuration page header and back link;
     - access select and row menu items, including Delete naming other agents;
     - the health line mapping table (every C-1033 state, test results never "Worked");
     - Instructions dirty state and the save bar;
     - the Computer states;
     - each empty state (no detail pane while empty).
   - **Playwright** (mocked), at 1280 and 390 px in both themes with axe and no console errors:
     - open Access and Behaviour from the menu;
     - old `?tab=` links;
     - change an account's access through the select;
     - Instructions save bar;
     - the Channels empty state when no integration exists.
   - Update existing specs that clicked the Behaviour or Access tabs.

## Out of scope

- Workspace Accounts and Apps pages (P5-POLISH-7). They reuse the account row from here.
- Words outside the texts quoted here; the wording pass is separate.

## Constraints / file ownership

- **Owns:**
  - `components/agents/**` (agent page, sections, status rail, access tab, instructions, computer
    tab, model roles);
  - `components/accounts/{account-row,account-menu,account-health,accounts-list,access-control}.tsx`;
  - `lib/account-health.ts`;
  - the agent-tab components of memory, skills, evals, schedules, webhooks, channels and peers
    (empty states and section visibility only);
  - the agent parts of `lib/routes.ts`.
- Uses P5-POLISH-1 and P5-POLISH-2 pieces; does not change them.
- Coordinate with FIX-172 (harness settings) if it is still running: it edits the Instructions harness
  field. Its field moves into Advanced here.

## Acceptance criteria

1. The agent page shows Chat · Triggers · Computer; Behaviour and Access open from the … menu, with
   Freeze and Delete below a divider; old links work. Match `after/c4.png`.
2. Access matches `after/c1.png`:
   - the summary;
   - the accounts with health and access;
   - the AI key as one row;
   - no Model readiness block and no role warnings.
3. A failing account reads "Failed <when>: <reason>" with its fix until a successful call; a passed
   test never shows "Worked" (C-1033 fixtures).
4. Instructions fits one screen at 1280×900 with Advanced collapsed; Save appears only after a change.
   Match `after/c3.png`.
5. Computer, Memory, Skills, Webhooks, Channels and Evals show the new states (`after/c5.png`,
   `after/c6.png`).
6. Web gates: typecheck, lint, vitest, build, Playwright mocked (axe, narrow-route, both themes).
   Report the counts.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). Many components on the most-used page, a navigation change and a status
mapping that must be exactly right.
