# P5-POLISH-5: The connect journey, honest and steady (owner, 2026-11-19)

## Goal

Rebuild the connect journey ("Give Daneel an app") so that every step tells the truth and nothing jumps
around. This is where the owner's two screenshots came from. P5-POLISH-1 fixes the shared bugs; this
brief fixes the journey itself. UX review 4 found:

- **App step:** all 16 apps sit under "READY TO CONNECT", including the six that can't be connected.
  The order is neither alphabetical nor grouped, there is no search, three Google apps share one "G",
  and a connected calendar is offered as if new.
- **Access step:** the title appends the host ("Give Daneel GitHub · github.com"), and the button row
  is right-aligned.
- **Sign in step:**
  - the token step doesn't say which permissions to give for the access just chosen;
  - "Advanced: other ways to connect, scopes" holds only a Setup guide link;
  - a failed connect empties the secret field and shows a technical error at the bottom;
  - for an app that needs an admin, the step is a dead end;
  - the button row is left-aligned.
- **Done step:** the confirmation is hidden (fixed by P5-POLISH-1). What remains says nothing about the
  access level and offers nothing to try.
- **Everywhere:** the dialog re-centres at every step, so the title and buttons move. On a phone it is
  a floating card with margins and a two-line title.

## Linked requirements

- **Design reference (D-380).** Proposals **B1**, **B2** (the person's side), **B3**, **B4** and **B5**
  of “Aurora UX fixes”, <https://claude.ai/artifact/WXbcpcrkbCLuxMSurpkuQW> (`#b1` … `#b5`).
  - Local copy `/home/alex/orca/workspaces/aurora/design-ux-fixes/`. Read `README.txt`, then:
    - `after/b1.png`: App step with search, Ready, Already connected and "Needs an admin first";
    - `after/b2.png`, top: the asked state;
    - `after/b3.png`: phone sheet on the Access step;
    - `after/b4.png`: GitHub sign-in with numbered steps, permission hints and an inline error with the
      value kept;
    - `after/b5.png`: Done with confirmation, Try it and three buttons;
    - the matching `before/` crops, and `after/src/b*.html` for exact spacing.
  - The review: <https://claude.ai/artifact/33oHaXQpCU3oSVRVmgtY5Z> (screens 250–258, 503–504,
    605–606).
  - The owner's comments are already applied in the artifact:
    - an Already-connected row reads "Calendar link · connected, Daneel can't use it yet", with **Give
      access**;
    - the choice rows are only as tall as their text;
    - below 560 px the Done footer wraps with **Try it in chat** full width on top.
  - Yellow dashed labels are commentary.
- **Decisions and contracts:** **D-388** (journey), **D-386** (asking), **D-387** (package text), **C-1029**
  (`readiness`, `accounts`, `summary`), **C-1030** (ask and withdraw), **C-1031** (`permission_hints`,
  `first_prompt`), **C-1035** (`?draft=`).
- **Normative docs:** P5-UX-3's brief and `14 §Agents, accounts and access` §3 stay normative where this
  brief doesn't change them: steps, skips, entry points, the grant on connect (C-988) and `grant_failed`.

## Wave and order

- After **P5-POLISH-1** (Dialog rows, `sheet`, `fixedHeight`, the stepper slot), **P5-POLISH-2**
  (`EmptyState`, `StatusLine`), **P5-POLISH-3** (C-1029, C-1031) and **P5-POLISH-4** (C-1030).
- Lane B: P5-POLISH-3 → P5-POLISH-4 → **P5-POLISH-5**. Lane B is Rust until here; this brief is web
  only.

## Context

All paths are under `web/src/`.

- **Journey:** `components/apps/connect-journey/`.
  - `connect-journey.tsx`: dialog, step state, Done at :426–480; "Open chat" :456;
    "Give {who} another app" :471; Done :478.
  - `app-step.tsx` :93:
    - grouping by `productKey` :105–107;
    - buckets ready/connected/blocked :133–135, keyed on `connection_count` and `planBlocked()` :23;
    - headings "Ready to connect" :154, "Already connected" :170, "Needs setup by an admin" :175;
    - `PlanProbe` :28 loads a plan only on focus or click: the cause of "everything ready".
  - `sign-in-step.tsx` :78:
    - form :270, `DynamicForm` :289;
    - the "Advanced: other ways to connect, scopes" `<details>` :298–354;
    - errors as `Notice role="alert"` :356–362 (`connectFailure`, `logic.ts:183`);
    - **secret fields reset to "" in the `finally` at :234–240**;
    - `WebsiteStep` :381.
  - `logic.ts`: steps and skips, Done sentences (`doneSentence` :76–86), tick list, payload, C-958
    sentences.
  - `org-journey.tsx` (organization mode, P5-UX-11): it shares `Steps` and must get the same frame.
  - `advanced-fields.tsx`, `use-sign-in-window.ts`.
- **API client:** `lib/api/apps.ts`: `useApps` :102 (`GET /v1/apps?workspace_id=`), `useConnectPlan`
  :340, `useAppInstances` :51. Setup-request hooks: `lib/api/app-setup-requests.ts` (P5-POLISH-4).
- **Dialog:** `components/ui/dialog.tsx` after P5-POLISH-1: `header` slot, `footer`,
  `variant="sheet"`, `fixedHeight`.
- **Chat:** the agent page `app/o/[org]/w/[ws]/agents/[handle]/page.tsx` → `components/agents/agent-page.tsx`;
  composer `components/chat/composer.tsx`.
- **Tests:**
  - `tests/connect-journey.test.tsx` (22 cases);
  - `e2e/connect-journey.spec.ts` (5 cases × 2 themes × 2 widths);
  - `e2e/support/mock-api.ts` serves `connect-plan` and apps.

## Scope

1. **Frame (B3, D-388).**
   - Use `DialogContent` with `variant="sheet"` and `fixedHeight="600px"`; the stepper goes in the
     `header` slot.
   - The footer is always **Back** on the left (absent on the first step) and the step's one action on
     the right:
     - **Next** on Access;
     - **Connect**, or **Sign in to <App>**, on Sign in;
     - **Try it in chat** on Done, with **Another app** and **Close** to its left (the Done order in
       `after/b5.png`: Close at the left edge, a spacer, **Another app**, **Try it in chat**).
   - **On the phone sheet:**
     - the header has a back arrow (same as Back) and the close button;
     - the title stays on one line (truncate with an ellipsis and the full title in `title`);
     - below 560 px the Done footer wraps, with **Try it in chat** full width on top.
   - **Title:** "Give <Agent> an app" on the App step; "<App> for <Agent>" on the later steps. The host
     moves into the Sign in step and is shown only when a family has more than one host (the FIX-135
     deployment choice).
   - Apply the same frame to `org-journey.tsx`.
2. **App step (B1).**
   - Build the groups from `useApps` alone (`readiness`, `accounts`, C-1029). No per-tile plan probe
     for grouping. Keep the plan fetch for the Sign in step.
   - **Search field** at the top ("Search apps"): filters all groups by app name and summary,
     client-side.
   - **Ready to connect · <n>:** `readiness.state === "ready"` and no givable account. Tiles sorted
     alphabetically, with **Another website or API** last (dashed).
   - **Already connected:** apps with an account in `accounts` that the agent cannot use yet. It is a
     list row:
     - the account name;
     - "<App> · connected, <Agent> can't use it yet";
     - **Give access**, which goes to the Access step with that account and saves only the agent's
       access (P5-UX-3's existing-account path, C-980).
     - An account the agent can already use is not listed here (it is on the agent's Access tab).
   - **Needs an admin first · <n>:** `needs_admin`, one list row per app:
     - the icon;
     - the app name;
     - the reason in one line from `needs`: "Alice Admin needs to set up Google sign-in" (an OAuth
       client, named after the provider), "An admin needs to add a server" (an instance), or "Needs
       setup by an admin" (admin material);
     - for apps of the same sign-in provider after the first, "Same Google sign-in";
     - **Ask <First name>** (the first name in `who_can`, otherwise "Ask an admin").
   - **Icons:** each app's own icon from the package (`/v1/apps/{app}/icon`). Calendar, Drive and Gmail
     get their product icons; if the package ships one icon for several apps, report it and use the
     package's per-app icons once P5-POLISH-3 adds them. Do not draw letter tiles where an icon exists.
3. **Asking (B2, the person's side).**
   - **Ask <Name>** calls C-1030 and turns the row into the asked state of `after/b2.png` (top):
     - a green dot;
     - "Asked just now. You'll get a notice when it's ready, and <Agent>'s Access tab will offer it.";
     - **Undo** (withdraw).
   - `readiness.requested.by_me` restores that state on reopen.
   - A Sign in step can still be reached for a `needs_admin` app through a deep link
     (`?connect=<app>`). There it shows the same sentence and **Ask <Name>** button, not a disabled
     Connect.
4. **Sign in step (B4).**
   - Numbered steps for token methods, as in `after/b4.png`:
     1. "Create a <kind> token on <App>" with an **Open <App>** button (external-link icon) to the
        method's token page;
     2. "Give it these permissions, for **<access level>**:" followed by the method's
        `permission_hints` for the chosen level, in a code-style block (one per line). With no hints,
        fall back to the current `scope_presets` tick line;
     3. "Paste the token" with the field.
   - **Errors:**
     - shown under the field they concern (`aria-describedby`), in the danger ink, with the field's
       border in danger;
     - the C-958 sentence for the app ("GitHub refused this token: it has expired. Create a new one
       and paste it here.");
     - a network error says what to check, in words.
   - **Keep typed values after a failure, secrets included:**
     - remove the reset at `sign-in-step.tsx:234–240`;
     - secrets stay in component state only, masked, with **Show** (FIX-173's `SecretInput` if merged,
       otherwise the existing secret input);
     - they are never written to storage or the URL, and cleared when the dialog closes or connects.
     - Note this in the brief's completion report: it changes P5-UX-3's "typed values that are not
       secret survive".
   - **Guide:** a plain link at the end of the step ("Step-by-step guide"), opening the package's
     guide. The `<details>` "Advanced: other ways to connect, scopes" exists only when the app has
     more than one usable way to sign in. It then becomes a choice at the top of the step ("Sign in
     with: Token · GitHub app", the existing `ChoiceCards`). Scopes beyond the level's presets and the
     OAuth client select stay under a small **More options** disclosure for the people who need them.
5. **Done (B5).**
   - A check icon in an accent circle.
   - "<Agent> can now <read|use|…> <account name>", from the Done sentences of `14` §3, naming the
     account.
   - Under it: "<Access level>. You can change this on <Agent>'s Access tab." (the tab name links
     there).
   - A **Try it** block (`--subtle` background) with the package's `first_prompt` in quotes.
   - **Try it in chat** (primary) opens the agent page with `?draft=<first_prompt>` (C-1035).
     Implement C-1035 in the agent page:
     - open a new conversation;
     - fill the composer;
     - focus it; never send;
     - remove the parameter from the URL;
     - at most 500 characters.
   - With no `first_prompt`, the button reads **Open chat** and opens the chat without a draft.
   - `grant_failed` keeps its retry (P5-UX-3).
6. **Tests:**
   - Update `connect-journey.test.tsx`:
     - groups from `readiness`, search filter, alphabetical order;
     - Already connected → Give access saves only the access;
     - Ask, Undo, and the asked state from `requested.by_me`;
     - permission hints for each level, and the fallback;
     - an error keeps the typed token and is shown under the field;
     - Done names the account and level;
     - Try it builds `?draft=`;
     - title rules.
   - Vitest for C-1035 in the agent page (a new conversation, a filled composer, nothing sent, the
     parameter removed).
   - Update `e2e/connect-journey.spec.ts` and the mock API (`readiness` in `/v1/apps`, the setup-request
     routes):
     - the six `needs_admin` apps appear only under "Needs an admin first";
     - Ask then Undo;
     - GitHub Read only shows the hints, and a refused token keeps the field;
     - Done → Try it in chat opens a new conversation with the draft;
     - the dialog's header and footer don't move between steps (compare bounding boxes);
     - at 390 px it is a full-screen sheet.
   - Both themes, 1280 and 390 px, axe, no console errors.

## Out of scope

- The Rust side (P5-POLISH-3, -4).
- Words outside the journey. The general wording pass is separate. Inside the journey, use the
  mockups' words where this brief quotes them.

## Constraints / file ownership

- **Owns:** `components/apps/connect-journey/*`, the journey parts of `lib/api/apps.ts`, the agent
  page's `?draft=` handling (a small hook in `components/agents/` plus the composer's
  `initialValue`), `tests/connect-journey.test.tsx`, `e2e/connect-journey.spec.ts` and its mock-API
  additions.
- Uses, and does not change, `Dialog` (P5-POLISH-1), the shared pieces (P5-POLISH-2) and the
  setup-request hooks (P5-POLISH-4).
- Tokens and primitives only.

## Acceptance criteria

1. On the dev stack (no Google sign-in app), the App step lists Google Calendar, Drive, Gmail,
   Microsoft 365, Notion and Linear only under "Needs an admin first", each with **Ask <Name>**. Asking
   shows the asked state, and the owner sees the request (P5-POLISH-4). Match `after/b1.png` and
   `after/b2.png`.
2. Giving Daneel GitHub with Read only shows the four permission hints. A refused token keeps the
   pasted value and shows the error under the field. Match `after/b4.png`.
3. Done reads "Daneel can now read Team calendar" and "Read only. …", with **Try it in chat** opening
   a new conversation with "What's on my calendar this week?" in the composer, unsent. Match
   `after/b5.png`.
4. The header and footer stay put across steps at 1280 px. At 390 px the journey is a full-screen sheet
   with a one-line title (`after/b3.png`).
5. Web gates: typecheck, lint, vitest, build, Playwright mocked (journey spec, axe, narrow-route, both
   themes). Report the counts.

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). A multi-step dialog with several data sources and an OAuth window round
trip that must keep working.

## Completion report — review pass 1 (P5-POLISH-11)

- **#1 done.** A one-level app (no write tools) ends with `level: "read"`: "Daneel can now read Team calendar." and
  "Read only. You can change this on Daneel's Access tab." (custom-only apps keep no level).
- **#2 done.** While a search is set, a group with no match is hidden; "No app matches" shows only when every group
  is empty (`tests/connect-journey.test.tsx`).
- Gates: typecheck, `connect-journey` + `org-accounts` vitest (42 passed), Playwright `connect-journey.spec.ts` passed.
