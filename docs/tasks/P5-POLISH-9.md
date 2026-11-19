# P5-POLISH-9: Organization pages: member views, agents told apart, one apps table, People (owner, 2026-11-19)

## Goal

Make the organization's pages useful to both kinds of reader. UX review 4 found:

- **Members get the owners' forms.** Carol, a member, opens Organization › AI models and gets the full
  rules form, about 2,300 px of checkboxes she can't change ("LLM posture", "Permitted · enforced by
  instance", "Origin: organization ids / workspace ids / team ids"). Internet access shows "SET BY YOUR
  ORGANIZATION" on the organization's own page and "Ask alice" three times.
- **Agents can't be told apart.** The organization's AI key lists three identical "Daneel" buttons:
  Alice's, Bob's and Carol's agents.
- **Apps is two lists on one long page.** Eight near-identical "People can't sign in to X yet · Register
  an OAuth client" rows (letter avatars Go, Co, Gm) sit above a second list of 18 apps with
  "Connect…". A single radio, "Which apps can people connect?", has its own Save at the bottom of a
  2,600 px page. Notion and Linear are "can't sign in" here but "Ready" in the journey (fixed by
  C-1029).
- **People:**
  - the owner is "alice" (the login) with role "owner";
  - Alice's own row offers Suspend and a red Remove;
  - "ACTIVE" caps chips;
  - dates like "11/18/2026, 6:49:12 PM".
- **The workspace list** lists every member's personal workspace by their full name, next to the
  shared "Default".
- **Agents › BYO** shows raw values: "Effective posture: OWNERS_ONLY · set by instance", "Inherit the
  instance floor (owners_only)".

## Linked requirements

- **Design reference (D-380).** Proposals **E1**, **E2**, **E3** and **D6** (organization People) of
  “Aurora UX fixes”, <https://claude.ai/artifact/WXbcpcrkbCLuxMSurpkuQW> (`#e1`, `#e2`, `#e3`, `#d6`).
  - Local copy `/home/alex/orca/workspaces/aurora/design-ux-fixes/`: `after/e1.png`, `after/e2.png`,
    `after/e3.png`, `after/d6.png`, `before/e1.png`, `before/e2.png`, `before/e3.png`,
    `before/d6b.png`, and `after/src/<id>.html`.
  - The review: <https://claude.ai/artifact/33oHaXQpCU3oSVRVmgtY5Z> (screens 302–304, 400–409).
  - Yellow dashed labels are commentary.
- **Decisions:** **D-390** (member views), **D-389** (`workspaceDisplayName`), **D-384** (lock pills),
  **D-386** (setup requests on the app row).
- **Contracts:** **C-1029**, **C-1030**.

## Wave and order

- After **P5-POLISH-2**, **P5-POLISH-3**, **P5-POLISH-4** and **P5-POLISH-7** (agent pills,
  `formatWhen`), and after **P5-POLISH-8** (`workspaceDisplayName`).
- Lane A, last: … → P5-POLISH-7 → **P5-POLISH-9**.

## Context

All paths are under `web/src/`.

- **Organization topics:** `components/org/org-topics.tsx`:
  - AI models :115–136 → `components/governance/org-governance.tsx:302` (`OrgAiModels`, the form :48
    and :85, `KeysNotice` :249);
  - Agents :53 → `org-governance.tsx:325` (`OrgByo`; checkboxes in
    `components/governance/byo-panel.tsx`);
  - Apps & accounts :72 (the Accounts tab :84 uses `AccountsPanel` with organization scope);
  - People :147 (tabs :163–165);
  - Settings :206 → `components/org/settings-view.tsx`.
- **Internet access:** `components/internet/org-internet.tsx:147`.
- **Apps:**
  - "People can't sign in" rows: `components/admin/sign-in-gaps.tsx` (`SignInGaps` :66,
    `OrgSignInGaps` :124);
  - availability `components/apps/availability-view.tsx:65` (`AvailabilityView` :132);
  - `components/apps/org-apps-list.tsx`;
  - `components/apps/oauth-clients-panel.tsx`.
- **People:** `components/org/members-view.tsx` (`MemberRow` :34, role select :62, Suspend/Reactivate
  :120, Remove :131, confirm :167; dates from `fmtDate`).
- **Workspace list:** the organization's home (`/o/<org>`; find the component in `components/org/`).
- **Who can:** `components/ui/who-can.tsx`. Summary sentences: `lib/summary/*` (P5-IA-2's
  `internet.ts`).

## Scope

1. **Member views (E1, D-390).** For a viewer who cannot change the page (members, and owners of
   another organization viewing read-only where that happens), render a summary instead of the form.
   - **AI models**, as in `after/e1.png`:
     - the title;
     - a sentence: "Agents in <Org> think with the organization's key, <key name>. You can also add
       your own key in your workspace." Vary it with the rules: whether workspace keys are allowed;
       with no organization key, "Agents in <Org> use keys you add in your workspace.";
     - the organization's keys as rows (an AI tile, the name, "Any model the gateway serves · default
       <model>" or the allowed models);
     - a `WhoCan` line: "Owners and admins change these rules. <Name> is an owner." with **Ask**.
     - **Ask** opens the existing contact or mention path, or a mail link to the owner's address shown
       as text. No new request kind.
   - **Internet access:** the sentence from `summary/internet.ts` ("Agents here can reach any website.
     Nothing asks you first."), the limits as rows with a `LockPill` ("Set by Administration" where
     the instance sets them), and the `WhoCan` line. No "Ask …" under every row.
   - **Agents (BYO):** one sentence ("Only workspace owners can run agents outside Aurora. Their
     tokens never expire unless an owner sets a limit.") and the `WhoCan` line.
   - Owners and admins keep the forms. Make the owners' BYO form say values in words: "Owners only",
     not `OWNERS_ONLY`; "Same as Administration (owners only)", not "Inherit the instance floor
     (owners_only)". This is a mapping table, not a reword of the page.
2. **Tell agents apart (E2).**
   - Outside its own workspace, an agent is always "<Agent> · <workspaceDisplayName>" ("Daneel · Bob's
     workspace").
   - In the organization's Accounts:
     - an account used by more than three agents shows a **Used by <n> agents** select that expands
       to the pills;
     - three or fewer show the pills directly.
     - The starter rule is said in words under them: "New agents get this key automatically." when the
       key is the organization's starter key (C-922).
   - The agent pill component (P5-POLISH-7) gains a `workspaceLabel` prop. Use it here and wherever an
     agent from another workspace is listed.
3. **One apps table (E3).** Organization › Apps & accounts › Apps becomes, as in `after/e3.png`:
   - **A summary note** (pending style, because it has an action): "<n> apps need a sign-in app before
     people can connect them. One Google sign-in covers Calendar, Drive and Gmail." with **Show them**,
     which sets the filter. It names the providers that serve several apps, from the OAuth client
     keys (`client_key`) shared by apps.
   - **A filter** (segmented): All <n> · Needs setup <n> · Off <n>.
   - **A table** (`card-c`, full width): App (the icon and name) · Status · People may connect.
     - **Status:** "Ready" (ok dot) or "Needs <Provider> sign-in" (attention dot) with **Set up**,
       which opens the OAuth-client form for that provider's `client_key`, prefilled.
     - **People may connect:** a `Switch` that saves on change, with a toast and undo. This replaces
       the radio and its Save (`AvailabilityView`). Keep "People can connect every app" as the
       organization-wide default; a switch turned off makes the organization's availability "only the
       apps switched on" (the existing model).
   - **Open setup requests** (C-1030) show on their app's row: "Bob asked" (or "Bob and Carol asked"),
     with **Set up** and **Not now** (decline).
   - **Variants of one product** (Cloud, Data Center, Enterprise Server) are one row named after the
     product, with the hosts as a line under it ("Cloud · Data Center"). Their status is the worst of
     the variants.
   - Remove the separate "People can't sign in to X yet" list from this page. Its information is in
     the table. Keep `SignInGaps` for Administration if it is used there (P5-POLISH-10 decides).
4. **People, organization (D6).** The same table rules as P5-POLISH-7's workspace People:
   - Person: display name and e-mail, "(you)" on your own row;
   - Role: a select in sentence case;
   - Joined: `formatWhen`;
   - Status: only when not active ("Suspended"), as a neutral `Badge`;
   - a … menu with **Suspend** and **Remove from <Org>…**;
   - no menu, role change or removal on your own row;
   - the last owner can't be lowered.
   - Pending invitations keep their section. "Email is not configured: invitation links are written to
     the server log." becomes the `Unavailable` pattern only if inviting really can't work; otherwise
     keep one neutral sentence.
5. **The organization's workspace list.** Use `workspaceDisplayName`. Sort shared workspaces first,
   then personal workspaces under a "Personal workspaces" heading, with their owners' names.
6. **Settings.** The organization's Settings page uses P5-POLISH-2's settings cards and switches (no
   more work than composing them).
7. **Tests:**
   - **Vitest:**
     - member versus owner rendering for each of the three pages;
     - the BYO value mapping;
     - agent pills with workspace labels and "Used by n agents";
     - the apps table (filter, provider grouping in the note, Set up opening the right `client_key`,
       switch save and undo, variants as one row, setup requests on rows);
     - People own-row rules.
   - **Playwright** (mocked), 1280 and 390 px, both themes, axe, no console errors:
     - Carol sees the AI models summary;
     - Alice switches Sentry off and on;
     - Alice sees "Bob asked" and declines.

## Out of scope

- Administration (P5-POLISH-10).
- The general wording pass. The BYO mapping is a value-to-words table only.

## Constraints / file ownership

- **Owns:**
  - `components/org/*`;
  - `components/governance/{org-governance,byo-panel}.tsx` (member views and the value mapping);
  - `components/internet/org-internet.tsx`;
  - `components/apps/{availability-view,org-apps-list}.tsx` and a new `components/apps/org-apps-table.tsx`;
  - the organization usage of `sign-in-gaps.tsx`.
- Uses the shared pieces and the agent pill; extends the pill only with `workspaceLabel`.

## Acceptance criteria

1. Carol sees summaries on AI models, Internet access and Agents, never a disabled form (`after/e1.png`).
2. The organization's key shows "Daneel · Alice's workspace", "Daneel · Bob's workspace" and "Daneel ·
   Carol's workspace" (`after/e2.png`).
3. Apps is one table with status, Set up and a switch per app, with requests on their rows; the switch
   saves without a Save button (`after/e3.png`).
4. People: no menu on your own row, display names, relative dates.
5. Web gates: typecheck, lint, vitest, build, Playwright mocked (axe, narrow-route, both themes).
   Report the counts.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). Several pages with role-dependent rendering and an availability model that
must not change meaning.
