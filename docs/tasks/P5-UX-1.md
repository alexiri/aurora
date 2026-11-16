# P5-UX-1: Who can fix this, refusals in words, honest empty states, first-run fixes (owner, 2026-11-15)

## Goal

The first-run walkthrough found pages that misled people about why they could not proceed:
- A page the person may not open said **"Could not reach the server. Check your connection and try
  again."** The API had answered 403.
- A workspace **owner** was told "Ask a workspace owner to add one." right under the Add button.
- The Welcome page's **"Connect an app"** was a 404, and "Invite a teammate" led to a refused page.
- A design review then counted six different ways the specs said "who can help".

This task builds the one component that says who can fix something, uses it for refusals and empty
states, repairs the broken Welcome links and fixes the small first-run defects.

## Linked requirements

- **Design** (D-354): the canvas “Aurora — agents, accounts and access”, https://claude.ai/artifact/3UwmUgrhh6MLCsfqdhTFmQ, boards Read me › Patterns (who can fix this); Workspace › A member's view and Welcome (links); States › Empty states and Loading, refusals, errors. Read each board's note; where a board and the spec disagree, the spec wins.
- D-323, D-349; contracts **C-915, C-916, C-981 (component)**.
- Normative: `14 §Information architecture` "Who can fix this", `14 §Agents, accounts and access` §9
  (first-run fixes), `14 §Visual design`.

## Wave and order

- **Wave 1.** Starts after P5-IA-1 (routes, navigation, `useViewerRoles`) and P5-UX-2 (`who_can` in
  refusals, `contacts`).
- Navigation and role visibility are P5-IA-1's. This task does not edit `app-shell.tsx`.
- It is the first web task of wave 1: P5-IA-2 and P5-UX-4 start after it (they render `WhoCan`, and
  P5-UX-4 edits `components/states.tsx` callers).
- P5-UX-7 rewrites Welcome's content later; this task only repairs its links.

## Context (facts found 2026-11-15)

- **The cause of the network sentence.**
  - `web/src/components/org/org-gate.tsx:24` returns `<ErrorState error={{ code: "forbidden" }} />`, a
    plain object.
  - `errorMessage` (`web/src/lib/api/errors.ts` ~238–245) looks codes up only for `instanceof ApiError`
    and otherwise returns the network sentence. `ERROR_MESSAGES.forbidden` exists (~16).
  - The same pattern is in `components/channels/shared.tsx:96`,
    `components/credentials/workspace-credentials.tsx:18,20` and
    `components/policy/workspace-policy.tsx:41`.
- **Empty states.** `components/states.tsx` ~32–58: `EmptyState` renders `ask` whenever there is no
  `action`. Callers that always pass `ask`: the Accounts lists' predecessors and
  `schedules-tab.tsx:260`. Correct conditional use: `policy-view.tsx:240`.
- **Names.** `useOwnerNames` in `components/onboarding/getting-started.tsx` ~56–66 reads the workspace's
  access list, which needs `can_own`. P5-UX-2's `contacts` replaces it.
- **Welcome.** `getting-started.tsx` ~351–371 `OptionalNext`: "Connect an app" links to a route that
  does not exist; "Invite a teammate" links to an admin-only page.

## Scope

1. **`WhoCan` component (C-981)** in the shared primitives. Input: a `WhoCan` value from the server, or a
   role plus `contacts` from the workspace or organization. Output: the sentence ("Ask Ada Lovelace or
   Alan Turing (organization owners and admins).", "An Aurora administrator can set this up."), or the
   action or link when the viewer can do it themselves. The role phrases are fixed (`14 §Who can fix
   this`):
   - `workspace_owner` → "workspace owners"
   - `workspace_approver` → "workspace owners and approvers"
   - `org_admin` → "organization owners and admins"
   - `org_owner` → "organization owners"
   - `instance_admin` → "an Aurora administrator" (never names)
2. **Refusals (C-915).**
   - Add `forbiddenError(whoCan)` returning an `ApiError`, and use it in every page gate listed above and
     any other bare `{ code }` error object `grep` finds.
   - `errorMessage` maps any object with a string `code` through `ERROR_MESSAGES`.
   - `ErrorState` renders a refused page as "Only <role phrase> can open this page." followed by
     `WhoCan`.
   - "Could not reach the server" stays only for transport failures (`TypeError` from fetch,
     `AbortError`, a non-JSON 5xx).
   - Add `ERROR_MESSAGES` keys for `connect_probe_failed` per cause, `oauth_client_required`,
     `above_account_limit` and `always_not_available`.
3. **Empty states (C-916).** `EmptyState`'s `ask` becomes `ask?: { whoCan; canAct: boolean }`, enforced by
   the type, and renders (through `WhoCan`) only when `!canAct`. Fix every caller; where the viewer can
   act, pass the action. Remove `useOwnerNames`.
   - **A missing main action (C-961).** Add one small component for page headers and list headers: it
     renders the page's main action for people who can use it and the `WhoCan` sentence in the same
     place for people who cannot ("Only workspace owners can add an account. Ask Bob Builder."). Use it
     on the pages that exist now (Accounts, Apps, Internet access, People, Agents, Skills, Schedules).
     Later briefs use it for the actions they add.
4. **Welcome links.** "Connect an app" becomes **Give <Agent> an app** and goes to the default agent's
   Access tab (`routes.ts`); P5-UX-7 points it at the journey. "Invite a teammate" is removed; P5-UX-5
   adds **Add people**. A test checks that every Welcome link resolves to an existing route.
5. **First-run fixes** (`14 §Agents, accounts and access` §9):
   1. The chat's empty state shows "Say hello to <Agent>" and the handle, not the agent's instructions.
   2. The message editor toolbar wraps at any width; no clipped buttons at about 1250 px or at 390 px.
   3. Tab bars show no stray scrollbar widget (fix it in the tab primitive).
   4. Form labels never print "(optional) (optional)" (the label builder).
   5. Help links render **Where do I find this?** without the URL in the text (the field renderer). The
      one exception is the journey's link to an app's token page, **Create a token on <App>**
      (P5-UX-3).
   6. The agent header names the computer's state in words: Off, Starting, On, Stopping, Failed, with the
      reason on hover or tap.
   7. The admin landing never flashes "You are not in an organization yet" before Administration ›
      Overview (gate the root page's empty-organization view on the setup-status query).
6. **Tests.**
   - Vitest: `WhoCan` for each role, with names, without names, and when the viewer can act;
     `errorMessage` over `ApiError`, plain objects and transport errors; `EmptyState` typing (a
     `// @ts-expect-error` case for `ask` without `canAct`); each fix.
   - Playwright:
     - a member opening `/o/<org>/people` by direct URL reads "Only organization owners and admins can
       open this page. Ask …" with the admins' names;
     - a workspace owner's empty Accounts list shows its Add action and no "Ask" line;
     - a workspace member's shows "Only workspace owners can add an account. Ask <owner names>." and no
       button.

## Constraints / file ownership

- Owns: the `WhoCan` component; `org-gate.tsx` and the other gate callers; `states.tsx`; `errors.ts`
  (later briefs append keys); the empty-state callers; `getting-started.tsx` (`OptionalNext` links only);
  the files of the seven fixes.
- `14 §Visual design` for every changed screen.

## Acceptance criteria

1. A refused page never shows the network sentence; a real network failure still does.
2. Every "who can help" sentence this task touches is rendered by `WhoCan`.
3. No empty state asks someone else when the viewer can act.
4. No Welcome link leads to a 403 or a 404.
5. Each of the seven fixes has a test or a screenshot check in the narrow-route suite.
6. Typecheck, lint, vitest and `npm run build` green; axe and narrow-route suites, both themes.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). Many small, well-located changes in shared components; the risk is in the
shared primitives (`EmptyState`, tabs, errors).

## Completion report (lane `p5-apps`, worker 20)

Status: Done (2026-11-17). Commits `P5-UX-1: …` (code) and the gate fix below.

**What changed.**
- `lib/who-can.ts` (pure: role phrases, action words, `askSentence`, `onlySentence`, `whoCanFromContacts`, `forbiddenError`, `whoCanOf`, `orgRoleFor`), `components/ui/who-can.tsx` (`WhoCan`: the link when `you_can`, else "Only <role> can <action>. Ask <names>." with `lead`, or "Ask <names> (<role>)."), `components/ui/main-action.tsx` (`MainAction`: the action for people who can use it, the `WhoCan` sentence in the same place otherwise, C-961), `lib/api/who-can.ts` (`useWhoCan` from a workspace's or organization's `contacts`).
- Refusals (C-915): every page gate (`org-gate.tsx`, `workspace-topics.tsx`, `workspace-policy.tsx`, the approvals page) returns `forbiddenError(whoCanFromContacts(...))`; the unused `channels/shared.tsx` `Forbidden` is removed; `errorMessage` maps any value with a string `code`, answers the network sentence only for a `TypeError`, an `AbortError`, `network_error` or a non-JSON 5xx (`isTransportFailure`), and words `connect_probe_failed` per `details.cause`; new `ERROR_MESSAGES` keys `connect_probe_failed`, `oauth_client_required`, `above_account_limit`, `always_not_available`. `ErrorState` reads "Only <role phrase> can open this page." plus `WhoCan` for a refusal that carries `who_can`.
- Empty states (C-916): `EmptyState.ask?: { whoCan, canAct }`, shown through `WhoCan` only when `!canAct`, never beside the action; callers fixed (Accounts lists, connections, schedules, policy rules); `useOwnerNames` and `ownerAsk` removed (the Welcome guide uses the workspace's `contacts`). `MainAction` on Accounts (websites, AI keys, both credential views), Agents (Create agent), Schedules (New schedule); the policy page's view-only notice and the member's Accounts notice are `WhoCan` sentences. Pages still to adopt `MainAction` as they gain actions: Apps catalogue, Skills editor, People (their gates already refuse in words).
- Welcome links: **Give <Agent> an app** (the default agent's Access tab, owners only), "Invite a teammate" removed (`optionalNextLinks`, tested against the `src/app` route tree).
- First-run fixes: chat empty state "Say hello to <Agent>" + `@handle` (brief removed); formatting row and toolbar wrap (`e2e/chat-composer.spec.ts` updated from "scrolls inside itself" to "wraps"); tab strips hide the scrollbar widget; `fieldLabel` says "(optional)" once; help link text "Where do I find this?"; agent header pill "Computer: Off|Starting|On|Stopping|Failed" with the reason on hover and tap; the root page shows `Loading…` (not "You are not in an organization yet") until the destination is decided.

**Gates (level: targeted + batch; no Rust touched).** `pnpm typecheck`, `pnpm lint` (0 errors; 3 pre-existing warnings), vitest 113 files / 1977 tests, `pnpm build`, Playwright: full mocked project 935 passed (43 min under load 20–39; the only failures were `chat-composer` at 390 px asserting the old scrolling strip — updated and rerun alone, 16 passed — and the four `real-stack` specs, which need the kind stack and were not run) incl. stage4-axe, stage4-routes, ia-navigation, who-can (new, 12), chat-v2 (+ formatting toolbar at 1250 and 390 px), llm-key-modes, credential-access; production project 8 passed. New tests: `tests/who-can.test.tsx` (25), `tests/home-landing.test.tsx` (2; the flash test fails with the fix disabled — checked), `e2e/who-can.spec.ts`. Existing tests changed because behaviour changed on purpose: onboarding (owners from `contacts`, `EmptyState.ask`), chat-v2 (hello card), agent-sandbox-failure (computer pill), m5-tabs (a schedule is created by an owner).

**Process note.** A few edits were made with one-line `sed` substitutions (two identical `ask=` lines, the Welcome call site, test strings) and one heredoc append to `e2e/chat-v2.spec.ts` instead of Edit/Write; each is visible in the diff.
