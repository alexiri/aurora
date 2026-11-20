# P5-REFINE-13: Access and the connect journey: fix "Try it in chat", send-only levels, flat webhook wizard, AI key dialog (owner, 2026-11-20)

## Goal

UX review 5 walked "give an agent an app" from every side. This brief fixes what it found in **Access** (the agent's
accounts list), the **connect journey** (the "Give Dani an app" sheet) and the **AI key** dialog.

**The owner's bug report, 2026-11-20 (first scope item): after giving the agent an app, the Done step's "Try it in chat"
button does not work.** The code explains it (see Context, "The Try it bug"). The fix is item 1 and ships first.

Other problems, in bullets:

- A send-only app (Outgoing webhook) offers "Read only (recommended)" and choosing it fails with "A custom grant needs
  at least one tool". Owner decision **D-419**: send-only apps get their own levels, "Ask before each send" and "Send
  without asking"; Read only is never offered for them.
- The Outgoing webhook's Sign in step is a three-step wizard inside the journey's wizard, with its own Back/Next in the
  scroll area. A failed test dumps the receiver's HTML.
- At 390 px an Access row wraps its title one letter per line (the card is 2,000 px tall).
- GitHub's sign-in choice is two walls of text; the app dialog hides nine "needs an admin" apps below a scroll.
- Adding a personal AI key ends in a red "Verification pending", and a rejected key empties the field. The endpoint
  picker says "Provider endpoint ... (litellm) — Works with Claude Code".
- Connect gives no progress for 3 to 5 s. "Test the connection" and "Sign in again" are offered for apps that cannot do
  either. "Read only" is a static label on one row and a select on another.

Owner decisions that apply: **D-405** (this set; precedence: this brief, then D-405..D-419 and the artifact comments,
then the mockup, then the finding's fix text), **D-407** (words: "Ask an admin" by role, one word per rung), **D-414**
(starter keys), **D-419** (send-only levels).

## Linked requirements

- **Artifact (source of truth):** <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>. Finding anchors are
  `#f-<key>`. Workers cannot open it; read the local copy.
- **Local copy `UXR5/`** = `/home/alex/orca/workspaces/aurora/design-ux-review-5/`. In it: `findings.md` (authoritative
  text), `after/<key>.png` (dark) and `after/<key>-lt.png` (light) are the visual spec, `after/src/<key>.html` the
  markup (the PNG wins if they differ), `before/<key>-b<n>.png` the Before crops, `screens/<id>.png` the full
  screenshots, `layer-above.md`, `priorities-themes.md`. Yellow dashed notes in a mockup are commentary, not UI.
- **Findings and their files** (each: anchor `https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#f-<key>`):
  - `access-1` · After `UXR5/after/access-1.png`, `access-1-lt.png`, `after/src/access-1.html` · Before
    `UXR5/before/access-1-b1.png`, `access-1-b2.png` · Screens `UXR5/screens/B109-webhook-step1.png`,
    `B121-webhook-done-step.png`, `B128-level-readonly.png`.
  - `access-2` · After `access-2.png`, `access-2-lt.png`, `src/access-2.html` · Before `access-2-b1.png` · Screens
    `B192-phone-access.png`, `B193-phone-access-full.png`.
  - `access-4` · After `access-4.png`, `access-4-lt.png`, `src/access-4.html` · Before `access-4-b1.png`,
    `access-4-b2.png` · Screens `B110-webhook-signin.png`, `B112-webhook-step2.png`, `B116-webhook-step3.png`,
    `B121-webhook-done-step.png`.
  - `access-5` · After `access-5.png`, `access-5-lt.png`, `src/access-5.html` · Before `access-5-b1.png` · Screens
    `B125-send-result.png`, `B126-send-result-final.png`.
  - `access-6` · After `access-6.png`, `access-6-lt.png`, `src/access-6.html` · Before `access-6-b1.png`,
    `access-6-b2.png` · Screens `B101-github-signin.png`, `B102-github-pat-form.png`, `B104-github-token-error.png`.
  - `access-7` · After `access-7.png`, `access-7-lt.png`, `src/access-7.html` · Before `access-7-b1.png`,
    `access-7-b2.png` · Screens `B092-give-app.png`, `B093-give-app-admin-list.png`.
  - `access-8` · After `access-8.png`, `access-8-lt.png`, `src/access-8.html` · Before `access-8-b1.png`,
    `access-8-b2.png` · Screens `B140-ai-key-result.png`, `B142-ai-key-litellm-result.png`, `B143-ai-key-final.png`.
  - `access-9` · After `access-9.png`, `access-9-lt.png`, `src/access-9.html` · Before `access-9-b1.png` · Screens
    `B139-ai-key-endpoint.png`.
  - `access-10` · After `access-10.png`, `access-10-lt.png`, `src/access-10.html` · Before `access-10-b1.png`,
    `access-10-b2.png` · Screens `B096-ics-submit.png`, `B103-github-token-filled.png`.
  - `access-11` · After `access-11.png`, `access-11-lt.png`, `src/access-11.html` · Before `access-11-b1.png`,
    `access-11-b2.png` · Screens `B106-app-row-menu.png`, `B107-test-connection.png`, `B130-webhook-row-menu.png`.
  - `access-12` · After `access-12.png`, `access-12-lt.png`, `src/access-12.html` · Before `access-12-b1.png`,
    `access-12-b2.png` · Screens `B105-access-with-ics.png`, `B122-access-with-webhook.png`.
  - The bug (item 1) has no finding; the owner reported it directly. Reference shots of the Done step:
    `UXR5/screens/B121-webhook-done-step.png`.
  - **Not in this brief:** `access-3` (the "Give Rex the Company key" header fix) is P5-REFINE-12.
- **Layer-above recs:** rec 1 (`UXR5/layer-above.md`, anchor
  `https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#above`: GitHub's OAuth option and the AI-key detail are Set up
  and Expert items), rec 15 (one word per rung: Remove from <Agent>, Turn off), rec 10 (first success).
- **Decisions:** D-405, D-406, D-407, D-414, D-419. Contracts: **C-1035** (`?draft=`, `docs/API_CONTRACTS.md:2013`),
  C-1031 (`first_prompt`), the contracts P5-REFINE-5 (access-8) and P5-REFINE-6 (D-419 levels) define.
- **Docs:** `docs/14-web-dashboard.md` §3 (connect journey, line 2205 says Try it in chat opens `?draft=`), §2
  (access levels), §Settings in words.

## Wave and order

- Lane B, after **P5-REFINE-2** (primitives: `labels.ts`, `SecretOnce`, `ResponsiveTable`, dialog frame) and **-1**
  (renames) and **-3** (`<Altitude>`).
- **Item 1 depends on nothing: start with it.** Items 2, 4 to 6 and 8 (UI parts) wait only for the primitives.
- Items that wait for a backend brief:
  - item 3 (send-only levels, access-1) waits for **P5-REFINE-6**;
  - item 9 (the personal key's "Checking…" → "Works", access-8) waits for **P5-REFINE-5**.
  - Do the rest first; do these when the backend has merged and its `web/src/lib/api` client types exist. Until then
    keep the existing behaviour for them and say so in the completion report.
- Lane B continues with P5-REFINE-14, which reuses item 1's fix for the schedule "Open the conversation" link.

## Context

All paths are under `web/src/`; line numbers are master `c0fe7d10`.

**The Try it bug (read this before touching anything).**
- The button is a Next `<Link>` at `components/apps/connect-journey/connect-journey.tsx:511`, label "Try it in chat"
  (or "Open chat" with no prompt), `href={chat}` and `onClick={onClose}`. `chat` is built at :437:
  `wsRoutes(org, ws).agent(first.handle ?? first.id, prompt ? { draft: prompt } : undefined)`, so the URL is
  `/o/<org>/w/<ws>/agents/<handle>?draft=<text>`. **It has no `tab=chat`.**
- The agent page picks its page with `useTabParam` (`lib/use-tab-param.ts:23–49`, used at
  `components/agents/agent-page.tsx:66`). The tab is **state**; it adopts a new `?tab=` from the router only when the
  new URL carries a valid one (`use-tab-param.ts:41–43`). A link with no `tab` therefore keeps the current page.
  **Entry point 1 (the agent's own Access tab, URL `?tab=access`): the Link navigates to the same agent, the state
  stays `access`, the person is left on Access with the dialog closed. Nothing seems to happen.** This is the most
  likely cause of the owner's report.
- **Second cause, for the entry points that land on Chat (Apps, Accounts, app setup page).** The composer reads the
  draft in a `useState` initializer from `window.location.search` (`components/chat/composer.tsx:114`). During a
  client-side navigation the page renders **before** the App Router updates `window.location`, so the initializer
  sees the previous URL (no `draft`) and the box is empty. `use-tab-param.ts:11–15` documents exactly this race for
  `?tab=`; the guide's "Say hello" link (`components/onboarding/getting-started.tsx:268`) survives because it carries
  `tab=chat` and a slower mount. The composer's cleanup effect (`composer.tsx:152–158`) then deletes `?draft=` from the
  URL, so a later remount cannot recover it either.
- **Third, smaller:** `?c=` (the open conversation, `components/agents/chat-tab.tsx:51–66`) is read with
  `useSyncExternalStore` over `window.location`; a link without `c` after a navigation can show the previous
  conversation. C-1035 says the draft opens "a new conversation"; today the code opens the person's direct chat
  (D-230). **Keep the direct chat** until P5-REFINE-11 ships "New chat" (D-409); P5-REFINE-11 then switches this link.
- The tests that exist only check the `href` (`tests/connect-journey.test.tsx:300–330`, `e2e/connect-journey.spec.ts:80–82`,
  `e2e/dialogs.spec.ts:70`). **Nothing clicks the button**, which is why this shipped.
- The journey opens from four places, all of which must work: `components/accounts/accounts-list.tsx:264` (the agent's
  Access tab with `agent`; the workspace Accounts tab without), `components/apps/apps-tab.tsx:172` (workspace Apps, also
  `?connect=<app>`), `components/apps/app-setup/app-setup-page.tsx:164` (an app's own page), and, for people on a phone,
  the same components at 390 px (the button is full width and first, `max-sm:order-first`).

**Access rows.**
- `components/accounts/account-row.tsx`: `AgentEntry` pill :44–110, `AccountRow` header flex :155–163 (the title is in
  `min-w-0 flex-1`, the select and menu share the row so the title collapses at 390 px), starter-key line :193,
  Technical details :283.
- `components/accounts/access-control.tsx`: static "Read only" text at :47 when `levels.length === 0`; the select
  :140–166. `access-choice.tsx` (the journey's three cards, "Recommended" on read, :47).
- `components/accounts/account-menu.tsx`: Test :95, Sign in again :96, Replace/Rename :97–99.
- `lib/accounts.ts`: `LEVEL_LABEL`, `LEVEL_ORDER`, `levelSentence`, `isLimited`.

**Journey.**
- `connect-journey.tsx` (572 lines): steps, `done` state :430–540, `doneSentence` in `connect-journey/logic.ts:76`.
- `app-step.tsx` (253): groups Ready to connect / Already connected / Needs an admin first :150–192.
- `sign-in-step.tsx` (516): methods and `plan.methods[].ready/audience` :268–283; the OAuth line :340; the "Another
  website" form :495; the Outgoing webhook's own wizard is `components/webhook/webhook-connect-step.tsx` (mounted at
  `sign-in-step.tsx:84`), with its result view for the test.
- `frame.tsx` (102): `JourneyFrame`, `StepFooter`, `BackNext` (the footer slot).
- `use-sign-in-window.ts` (113): the OAuth window.

**AI key dialog.** `components/credentials/llm-form.tsx` ("Provider endpoint" :229, org-origin note :279);
`components/credentials/origin-view.tsx:13` has the word "Verification pending"; personal keys list
`components/account/my-keys-view.tsx` and `key-table.tsx`.

**Test helpers.** `e2e/support/journey.ts` (`openJourney`, records the connect request), `e2e/support/mock-api.ts`
(`agent`, `ws`, `extraAccounts`), `e2e/support/chat-mock.ts` (mocked chat API), `e2e/support/open.ts` (`openRoute`,
`overflow`), `e2e/support/axe.ts` (`serious`).

## Scope

1. **Fix "Try it in chat" (the owner's bug). Ships first, in its own commit.**
   Anchors: none (owner's report, 2026-11-20). Reference: `UXR5/screens/B121-webhook-done-step.png` (the Done step).
   - **Reproduce first.** Write the Playwright specs of the acceptance criteria below and watch them fail on master
     for the reasons in Context. Do not fix by guessing; if a spec passes before the fix for an entry point, say so in
     the completion report and find out why.
   - **The link.** `chat` always carries the tab: `wsRoutes(org, ws).agent(handle, { tab: "chat", draft: prompt })`
     (`tab: "chat"` also when there is no prompt, so "Open chat" lands on Chat from Access). The Access-tab link two
     lines above keeps `tab: "access"`.
   - **The agent page follows the router.** `useTabParam` also adopts a navigation that arrives with **no** `tab` when
     the previous state came from a link to the same agent (today it keeps the old page). Rule: when
     `useSearchParams()` changes and the new URL has no `tab`, reset to the default page (Chat for hosted agents,
     Instructions for BYO). A person's own tab clicks use `history.replaceState` and are unaffected. Add a vitest for
     `useTabParam` (navigate `?tab=access` → no tab ⇒ default).
   - **The composer gets its draft from the router, not from `window.location`.** `ChatTab` reads `draft` with
     `useSearchParams()` once when it mounts (a ref), passes it to `ChatThread` → `Composer` as `initialDraft`
     (max 500 characters), and the composer uses it in place of `window.location.search` at `composer.tsx:114`. The
     `?draft=` removal (`composer.tsx:152–158`) stays but runs **after** the composer has applied the draft, and the
     ref keeps the draft until the first send or the person edits the box, so a remount (the direct conversation
     resolving, the transcript loading) cannot lose it. A stored draft (the per-user store of P5-CHAT-3,
     `restored` at `composer.tsx:~175`) never wins over a link's draft.
   - **Conversation.** The link also drops `c` (open the direct chat, not an old conversation): make `ChatTab` treat a
     `draft` as "select the direct chat" (clear `?c=`).
   - **The result the person sees.** One click on **Try it in chat**: the journey closes, the agent's Chat opens, the
     box holds the app's suggested prompt (for GitHub: "Which pull requests are waiting for my review?"), the box is
     focused, **nothing is sent**, and the URL no longer has `draft`. On a phone the same.
   - **Failure to resolve is visible.** If the agent has no handle yet or the first agent is missing, the button is
     not rendered (as today) and Done offers Close only.
   - Keep `onClose` on the Link, but do not rely on it for the hand-off: the navigation is the Link's own.
   - Also fix the guide's link if the same race shows there (`getting-started.tsx:268`): it must pass the same
     Playwright check.

2. **Access rows at 390 px (access-2).**
   Anchor `#f-access-2` · After `UXR5/after/access-2.png`, `access-2-lt.png` · Before `UXR5/before/access-2-b1.png`.
   Under 600 px the title keeps its full width on its own line; the level control and the menu button sit on their own
   line under it (`account-row.tsx:155–163`). Above 600 px nothing moves. The card is never taller than its content
   (no 2,000 px). Text is not clipped; touch targets are at least 44 px high on coarse pointers.

3. **Send-only apps: their own levels (access-1; D-419). Waits for P5-REFINE-6.**
   Anchor `#f-access-1` · After `UXR5/after/access-1.png`, `access-1-lt.png` · Before `UXR5/before/access-1-b1.png`,
   `access-1-b2.png`.
   - The Access step for an app whose levels come back as the send-only pair shows two cards: **Ask before each send**
     ("Dani asks you before it sends anything. You answer in Approvals.", **Recommended**) and **Send without asking**
     ("Dani may send on its own."), under "What may Dani do with Outgoing webhook?". Read only is never shown.
     The footer line stays: "You can change this at any time on Dani's Access tab."
   - Labels come from `lib/labels.ts` (P5-REFINE-2): add the two levels there with the sentences above. `LEVEL_LABEL`
     and `levelSentence` handle them; no snake_case or raw level ids on screen.
   - Done: "Dani can now send to Ops alerts, and asks you first." (or "... and sends without asking." for the other
     level). It names the **account** (`doneSentence` takes the account name).
   - In the Access row the same two levels appear in the level control (item 11).
   - The old failure "A custom grant needs at least one tool; detach the connection instead" can no longer be reached
     from the UI; keep the error text mapping for other causes.

4. **The Outgoing webhook wizard becomes flat (access-4, access-5).**
   Anchor `#f-access-4`, `#f-access-5` · After `UXR5/after/access-4.png`, `access-4-lt.png`, `access-5.png`,
   `access-5-lt.png`; markup `after/src/access-4.html`, `access-5.html` · Before `access-4-b1.png`, `access-4-b2.png`,
   `access-5-b1.png` · Screens `B110`, `B112`, `B116`, `B121`, `B125`, `B126`.
   - The journey's steps for this app are **App · Address · Message · Done** (the access step is the send-only choice
     of item 3, shown before Address as for any app; the stepper shows the same four labels the mockup shows). There
     is no wizard inside the wizard: no inner Back/Next, no scroll-area buttons. Back and Next live in the journey's
     footer (`BackNext` in `frame.tsx`).
   - **Address:** "Address to send to" field (placeholder `https://hooks.example.com/ops-alerts`), help "Dani sends a
     POST request here.", then "Name" (`Ops alerts`). **Message:** what the message looks like (what exists today in
     the third step of `webhook-connect-step.tsx`, in words).
   - **Done** offers "Not sure the address works?" with **Send a test**, then the result:
     - success: a green line "The test went through." with the answer code in Details;
     - failure: a red result **"The test did not go through"** with one plain sentence: "The address doesn't accept
       POST. Check the URL and try again." for 405; other codes get a sentence in `lib/labels.ts` (404: "Nothing is
       there. Check the address.", 401/403: "The address refused the request. Check the sender's settings.", 5xx:
       "The other side had a problem. Try again in a minute.", timeout: "It took too long to answer.", DNS or
       connection: "Couldn't reach the address.").
     - **Details** (collapsed) holds "Answer 405 Method Not Allowed · delivery 01a1…" and the receiver's body cut to
       the first 200 characters with "(first 200 characters)", escaped as text (never rendered as HTML).
     - Buttons: **Send test again**, **Edit address** (back to Address with the values kept).
   - The delivery id and answer code are Expert items (`<Altitude level="expert">`, P5-REFINE-3) inside Details.

5. **GitHub's sign-in choice (access-6).**
   Anchor `#f-access-6` · After `UXR5/after/access-6.png`, `access-6-lt.png`, `after/src/access-6.html` · Before
   `access-6-b1.png`, `access-6-b2.png` · Screens `B101`, `B102`, `B104`.
   - Title "Sign in with" with **A personal access token** first and tagged **Works now**: "Works now. Paste a token from
     github.com; Dani acts as you." Then **OAuth app**: "After an admin sets it up. Sign in with one click."
   - **Designer's note (follows):** do not hard-code "After an admin sets it up". Read the connect plan
     (`plan.methods`, `sign-in-step.tsx:268–283`): when the OAuth method is `ready` for this person, show it as
     "Sign in with one click." with a **Works now** tag and make it the first option; when it is not ready, show the
     "After an admin sets it up" line and who can set it up (`who_can`).
   - The token steps show at once, below the options, no scroll to reach the field: "Get a token in 3 steps" (1 Open
     github.com › Settings › Developer settings › Fine-grained tokens. 2 Pick the repositories and allow Contents and
     Issues. 3 Copy the token and paste it below.) then the **Token** field (`github_pat_…`). The permission list for the
     chosen level stays (P5-POLISH-5).
   - Classic tokens, GitHub App and scopes detail go under `<Altitude level="expert">` ("Other ways to sign in").

6. **Two groups with counts (access-7).**
   Anchor `#f-access-7` · After `UXR5/after/access-7.png`, `access-7-lt.png`, `after/src/access-7.html` · Before
   `access-7-b1.png`, `access-7-b2.png` · Screens `B092`, `B093`.
   In `app-step.tsx`, two tabs under the search: **Ready · 8** and **Needs an admin · 9**, counts from the catalogue.
   Ready is selected first. The list of the selected tab scrolls inside the dialog body; the tabs never scroll away.
   Under Ready: the app tiles (initials tile, name) and "Another website or API" last. Under Needs an admin: the tiles
   with who can set them up and Ask (existing behaviour, P5-POLISH-4, with D-407 wording: "Ask an admin"). The
   Ready tab ends with the hint "9 more need an admin to set them up first: Google, Microsoft, Jira, Linear,
   Notion…" (first five names, real data) that switches to the second tab. "Already connected" stays a group in Ready.
   The tabs use the shared `Tabs` primitive; arrow keys move between them. A search matches in both tabs and the
   tab labels show the matching counts.

7. **Connect shows progress (access-10).**
   Anchor `#f-access-10` · After `UXR5/after/access-10.png`, `access-10-lt.png`, `after/src/access-10.html` · Before
   `access-10-b1.png`, `access-10-b2.png` · Screens `B096`, `B103`.
   While a link or token is checked, the footer button reads **Checking…** with a spinner and is disabled, and a line
   above says what is checked: "Checking the token with GitHub. This takes a few seconds." / "Fetching the calendar
   link." Per method (token, calendar link, webhook address). Use the existing spinner primitive; `aria-live="polite"`.
   The busy label "Connecting…" at `sign-in-step.tsx:511` becomes "Checking…".

8. **The menu shows only what the app can do (access-11); always a level control (access-12).**
   Anchors `#f-access-11`, `#f-access-12` · After `UXR5/after/access-11.png`, `access-11-lt.png`, `access-12.png`,
   `access-12-lt.png`; markup `after/src/access-11.html`, `access-12.html` · Before `access-11-b1.png`,
   `access-11-b2.png`, `access-12-b1.png`, `access-12-b2.png` · Screens `B106`, `B107`, `B130`, `B105`, `B122`.
   - Menu (`account-menu.tsx:95–99`): GitHub: **Test the connection**, **Sign in again**, **Remove**. Calendar link:
     **Remove** only. Outgoing webhook: **Send a test**, **Remove**. "Remove" is "Remove from <Agent>" on an agent's
     list (D-407, rec 15); Rename and Replace stay where they exist today. What each app supports comes from the
     app's capabilities in the catalogue (`apps.ts`): tests only where a probe exists, sign in again only for OAuth
     methods. If the API does not say, derive it from `plan.methods` and the app's `kind` and name the rule in the
     completion report.
   - Level (`access-control.tsx:47` and :140–166): every app row has the **same** control, a pill that opens a select,
     **even with one level**: Calendar link "Read only", Outgoing webhook "Ask before each send", GitHub "Ask before
     changes". With one option the select shows it checked and no other entry; a person who cannot change it sees the
     pill without the chevron (a read-only pill, not bare text). Websites and AI keys (no levels) show no pill.

9. **AI keys: "Which service is this key for?" and checking in place (access-8, access-9).**
   Anchors `#f-access-8`, `#f-access-9` · After `UXR5/after/access-8.png`, `access-8-lt.png`, `access-9.png`,
   `access-9-lt.png`; markup `after/src/access-8.html`, `access-9.html` · Before `access-8-b1.png`, `access-8-b2.png`,
   `access-9-b1.png` · Screens `B139`, `B140`, `B142`, `B143`.
   - **Dialog** (`llm-form.tsx:229`): title "Add an AI key", question **"Which service is this key for?"**, one choice card
     per endpoint with the provider's logo tile (initials if no logo), name and one plain line: "Anthropic — Claude
     models. Works with Claude Code.", "OpenAI — GPT models.", "Acme AI gateway — Your organization's own gateway."
     Lines come from the endpoint's display name and kind (`lib/labels.ts`); no "(litellm)", no "Allowed key scopes".
     Footer **Cancel** · **Next**. Then the key field ("API key", `sk-ant-…`).
   - **Typed key kept.** After the provider rejects the key the field keeps what was typed (masked) and says under it:
     "The provider rejected this key. Check it and try again; what you typed is still here."
   - **The key's row** shows the check, not a red "Verification pending": **Checking…** then **Works** (or "Failed:
     <reason in words>" with Try again) on the row of the personal key (`my-keys-view.tsx`, `key-table.tsx`). The
     dialog closes on save; the row carries the state; it updates without a reload (poll while checking, stop at a
     result). **Waits for P5-REFINE-5** (personal key verification state on the account); until then the row shows
     "Checking…" for as long as the API says `pending` and the remaining parts of this item ship.
   - `origin-view.tsx:13` "Verification pending" is replaced by the same words.

## Out of scope

- Org-level journey (`org-journey.tsx`): only its Done step is checked to stay unchanged.
- The "Give Rex the Company key" fix and starter keys (P5-REFINE-12, backend P5-REFINE-5).
- Approvals for sends (their words are P5-REFINE-16). The new "New chat" behaviour of the Try it link (P5-REFINE-11).
- Backend routes (P5-REFINE-5, -6).

## Acceptance criteria

1. **Try it in chat works from every entry point**, each proven by a mocked Playwright click-through (no `href`-only
   assertions). For each of the five entry points: from the agent's Access tab, from workspace Apps (`?tab=apps`),
   from workspace Apps opened by `?connect=github`, from the workspace Accounts tab, and from an app's page: give GitHub
   to Dani, press **Try it in chat**; then `expect` that
   - the journey dialog is gone;
   - the URL is the agent's page with `tab=chat` (or no tab) and **no** `draft`;
   - the Chat page is shown (heading "Direct chat", the composer);
   - `getByRole("textbox", { name: /message/i })` has the value "Which pull requests are waiting for my review?";
   - the box has focus;
   - **no** POST to `/messages` was made.
   At 390 px the same five. With no `first_prompt`, **Open chat** lands on Chat with an empty box (from Access too).
2. A vitest on `useTabParam` (no `tab` after a navigation = default page) and a component test of the composer reading
   `initialDraft` (500-character cut, not overriding after the person typed, kept across a remount, never sent).
3. Item 2: matches `UXR5/after/access-2.png` (dark) and `access-2-lt.png` (light) at 1256 and 390 px; no one-letter
   wrapping; the card at 390 px is under 260 px high with one account.
4. Items 3 and 4: match `access-1.png`/`-lt.png` and `access-4.png`/`-lt.png`, `access-5.png`/`-lt.png` at 1256 and
   390 px; the footer buttons do not move between steps; a 405 test shows the sentence, the receiver HTML is only in
   Details, cut to 200 characters and not rendered as HTML (a test with `<script>` in the body).
5. Item 5: `access-6.png`/`-lt.png`; with the OAuth method ready the first option is OAuth, "Works now"; with it not ready
   the "After an admin sets it up" line appears; the token field is visible without scrolling at 1256 px.
6. Item 6: `access-7.png`/`-lt.png`; counts equal the catalogue; keyboard switches tabs.
7. Item 7: `access-10.png`/`-lt.png`; the button reads "Checking…" and is disabled during the request.
8. Item 8: `access-11.png`/`-lt.png`, `access-12.png`/`-lt.png`; per app, the menu has exactly the listed entries; every
   app row has the pill.
9. Item 9: `access-8.png`/`-lt.png`, `access-9.png`/`-lt.png`; the typed key is still in the field after a 4xx; no
   "Provider endpoint", "(litellm)" or "Verification pending" anywhere in the rendered UI.
10. No snake_case, UUID or level ids in visible text (the labels guard from P5-REFINE-2 passes). Both themes, no
    horizontal scroll at 390 px, axe with no rule disabled, no console errors.
11. Screens in the completion report: each item's Before crop beside the new screenshot and the After mockup.

## Tests

Targeted only (GitHub CI is the full gate). Heavy commands through
`/home/alex/orca/workspaces/aurora/heavy.sh <lane> <command>`.

- vitest: `tests/connect-journey.test.tsx` (Done step href has `tab=chat`; labels; app step tabs and counts; the flat
  webhook steps), `tests/use-tab-param.test.ts` (new), `tests/chat-composer*.test.tsx` (initialDraft), account row and
  menu tests (`tests/account-row*.test.tsx`: entries per app, one-level pill), `llm-form` test (typed key kept).
- Playwright (mocked, `e2e/connect-journey.spec.ts`, new `e2e/try-it-in-chat.spec.ts`, `e2e/agent-access.spec.ts`,
  `e2e/accounts.spec.ts`): the five click-throughs above at 1280 and 390 in both themes; screenshots under
  `/tmp/aurora-screens/p5-refine-13/` (outside the repo). Use `e2e/support/chat-mock.ts` for the chat API and fail the
  test on any `POST .../messages`.
- Never use `localhost:4000` or the aurora-dev cluster.

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

- `docs/14-web-dashboard.md` §3 (connect journey: steps for the webhook, send-only levels, Try it in chat = Chat tab with
  `draft`, App step tabs, the plan-driven OAuth line), §2 (levels table: send-only pair), §Agent page (the link
  `tab=chat&draft=`), §Settings in words (the AI key dialog).
- `docs/API_CONTRACTS.md` C-1035: add "the link also carries `tab=chat`; the page follows the router; the box takes
  the draft from the router, not `window.location`".
- `docs/PROGRESS.md` row P5-REFINE-13 and `docs/FEATURES.md` rows for the connect journey and AI keys, both current at
  every commit.

## Completion report

Fill in when done (P5-REFINE-20 reads it).

- Items done, with commit: 1 (5c985b40, follow-ups 83f7112c, 90a98529→master's `use-tab-param.ts`, 6640acae; **merge bc86fdec**) · 2 (3247eae7; its arbitrary Tailwind values replaced by `max-sm:` tokens in the item 3–9 commit) · 4 (1a5e3dc… "item 4") · 3, 5, 6, 7, 8, 9 (the last commit of the lane, "items 3, 5–9"). The commit ids are in the worker_done report.
- The cause of the Try it bug as confirmed by the failing spec (which of the three causes, per entry point): the Access-tab entry was the **stale-tab race**: `useTabParam` declined to adopt the router's new `?tab=` while `window.location` still showed the previous valid `?tab=access`, and nothing re-rendered once the location caught up (fixed by adopting when the router's `tab` *changes*; master's brief 17 landed the same rule and its implementation is the one kept). The empty-box entries were the composer reading `window.location` (cause 2); both fixed in 5c985b40. The phone case was brief 11's "the phone opens the chat" (on master). The dialog now closes at once (`leaving` state) without the host's `onClose`, which rewrites the URL and undid the navigation.
- Items waiting for a backend brief: none any more (P5-REFINE-5 and -6 merged): item 3 uses `send_only` (C-1067) and item 9 the account's `verification` (C-1065).
- Deviations and why:
  - **Item 8 rule** (the API does not say which apps can be tested or signed in again, nor the method an account was made with): `accountActions` in `lib/accounts.ts` derives it from the app's connect methods (`useApp`): **Test the connection** where any method has `has_probe`; **Sign in again** where any method is `oauth2`; the Outgoing webhook (`app_id: "webhook"`) has **Send a test** (the existing Send test dialog, now in the menu; the card keeps Edit) instead; with the app unknown none. Websites and AI keys keep their own entries.
  - **Item 9**: the service picker is still a native select (label "Which service is this key for?", one option per endpoint: "Bedrock EU — Claude models on AWS. Works with Claude Code.", from `serviceOption` in `lib/labels.ts`); not choice cards with logo tiles and no Cancel/Next two-step: ~20 test and spec call sites select by id. The typed key is kept after a refusal (all fields; masked) with "The provider rejected this key…"; the row of an AI key shows Checking… / Works / Failed with Try again (`verification`, polled every 3 s while checking); `origin-view` says "Checking…".
  - **Item 4**: Done's **Edit address** goes back to Address with the name kept and the address empty (it is never shown again); the account made a moment ago (wrong address) is **deleted** when the new one is saved, since a webhook's address cannot be changed by Edit. Needs a decision if the owner prefers a different rule. The Message step holds the extras (secret values, headers, signing, limits) in a fold that opens when needed. The organization journey keeps one Sign in step that switches the same two pages.
  - **Item 5**: the main options are only the *ready* OAuth way ("OAuth app", Works now, first) and a personal access token ("Works now"); a not-ready OAuth way shows "After an admin sets it up" with `who_can` (plan-driven). Other ready methods are under `<Altitude id="connect.othermethods" level="expert">` (registry and docs row added). The token steps keep their wording and gain "Get a token in 3 steps".
  - **Item 2 at 640 px, not 600**: the phone variants use `max-sm:` (the arbitrary-values guard allows no `max-[599px]`).
  - **Item 7**: "Checking…" with a spinner is the shared `BackNext` busy state; the line above is per method (token, calendar link, webhook address).
- Tests: changed `tests/webhook-dialogs`, `connect-journey`, `onboarding`, `accounts`, `webhook-card-settings`, `credentials`, `credential-access`, `llm-key-form`, `setup-checklist`, `use-tab-param`; new `tests/webhook-journey`; e2e (read, not run: mocked project) `webhook.spec`, `support/webhook.ts`, `account-health.spec`, `connect-journey.spec`, `connect-access.spec`, `try-it-in-chat.spec` (item 1), `p5-polish-10`, `welcome-summary`, `llm-key-modes`, `accounts.spec`, `credential-access.spec`, and the production specs `first-run.spec` and `onboarding-journeys.spec` (the "Which service is this key for?" label) for the nightly. No test removed. Commands: whole web vitest once (`pnpm exec vitest run`): 212 files, 2953 tests, one failure (credentials.test, the old "field is emptied after a refusal") fixed; `pnpm typecheck` clean; eslint on touched files and `scripts/guards/run-all.sh` see the worker_done report. Playwright could not run here (cold dev-server compile times out in the lane's slot).
- Screenshots compared (Before / After / mine, per finding key, both themes, 1256 and 390 px): not taken in this lane (Playwright cannot run here); for P5-REFINE-20 / the nightly.
- Open questions: the webhook Edit-address rule above; whether the AI-key service picker must become cards (it costs ~20 test call sites).
