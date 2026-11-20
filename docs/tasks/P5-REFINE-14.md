# P5-REFINE-14: Automations: one list, schedules with presets, webhooks with "How to call it", deliveries, one thread per run, Paused/On (owner, 2026-11-20)

## Goal

Four tabs (Schedules, Webhooks, Channels, Peers) with four shapes become **one "Automations" page**: "when does Dani
run". UX review 5 found, in bullets:

- **Schedules:** the only way to set a time is a 5-field cron; the 5-minute minimum shows only as an error after submit;
  Run now gives no feedback; Runs says "fired"; the card shows `*/5 * * * * UTC` and "Disabled (user)"; six equal
  buttons per card; Edit opens a card inside the card; the conversation keeps the schedule's old name.
- **Webhooks:** nothing says how to call one (header `X-Signature`, value `sha256=<hex HMAC>`); verification defaults to
  "none" so anyone with the URL can wake the agent; the options read `none / hmac_sha256 / bearer`; Deliveries is an
  unheaded list ("Just now | rejected_signature | 31 B"); the chat shows the safety wrapper and a delivery UUID; Rotate
  and Delete are a red "Confirm ..." button with no consequence text; the page has no header button.
- **Channels:** "Ask alice" with no platform named and no reason.
- Every run of a schedule and every delivery of a webhook piles into one endless conversation.

Owner decisions that apply: **D-405** (this set; precedence: this brief, then D-405..D-419 and the artifact comments,
then the mockup, then the finding's fix text), **D-407** (Pause/Resume replaces Freeze and Turn off for automations;
"Paused/On" are the state words; text that points to an administrator names the role: "Ask an admin"), **D-409** (one
thread per run, folded under its automation), **D-417** (requests with a reason), comment decision (schedule presets
start at **Every 5 minutes**, hint "Schedules run at most every 5 minutes", a too-frequent custom cron is flagged as
you type).

## Linked requirements

- **Artifact (source of truth):** <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>; anchors `#f-<key>`. Workers
  cannot open it; use the local copy.
- **Local copy `UXR5/`** = `/home/alex/orca/workspaces/aurora/design-ux-review-5/`: `findings.md` (authoritative text),
  `after/<key>.png` (dark) and `after/<key>-lt.png` (light) are the visual spec, `after/src/<key>.html` the markup (the
  PNG wins if they differ), `before/<key>-b<n>.png` the Before crops, `screens/<id>.png` the full screenshots.
  Yellow dashed notes are commentary, not UI.
- **Findings (anchor `https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#f-<key>`):**
  - `schedules-1` · After `UXR5/after/schedules-1.png`, `schedules-1-lt.png`, `after/src/schedules-1.html` · Before
    `UXR5/before/schedules-1-b1.png` · Screens `UXR5/screens/C004-triggers.png`, `C005-schedule-dialog.png`.
  - `schedules-2` · After `schedules-2.png`, `schedules-2-lt.png`, `src/schedules-2.html` · Before `schedules-2-b1.png` ·
    Screens `C006-schedule-bad-cron.png`, `C007-schedule-created.png`.
  - `schedules-3` · After `schedules-3.png`, `schedules-3-lt.png`, `src/schedules-3.html` · Before `schedules-3-b1.png`,
    `schedules-3-b2.png` · Screens `C009-schedule-run-now.png`, `C010-schedule-runs.png`, `C011-chat-after-run.png`,
    `C012-schedule-conversation.png`.
  - `schedules-4` · After `schedules-4.png`, `schedules-4-lt.png`, `src/schedules-4.html` · Before `schedules-4-b1.png`,
    `schedules-4-b2.png` · Screens `C008-schedule-created.png`, `C015-schedule-off.png`.
  - `schedules-5` · After `schedules-5.png`, `schedules-5-lt.png` (**no `src` file; it is the same mockup as
    `schedules-1`, use `src/schedules-1.html`**) · Before `schedules-5-b1.png` · Screens `C005-schedule-dialog.png`.
  - `schedules-6` · After `schedules-6.png`, `schedules-6-lt.png`, `src/schedules-6.html` · Before `schedules-6-b1.png`,
    `schedules-6-b2.png` · Screens `C013-schedule-edit.png`, `C016-schedule-reassign.png`, `C070-m-schedules.png`.
  - `schedules-7` · After `schedules-7.png`, `schedules-7-lt.png`, `src/schedules-7.html` · Before `schedules-7-b1.png`,
    `schedules-7-b2.png` · Screens `C014-schedule-saved.png`, `C093-chat-after-deletes.png`.
  - `webhooks-1` · After `webhooks-1.png`, `webhooks-1-lt.png`, `src/webhooks-1.html` · Before `webhooks-1-b1.png`,
    `webhooks-1-b2.png` · Screens `C019-webhook-form-filled.png`, `C021-webhook-secret-shown.png`,
    `C027-webhook-edit.png`.
  - `webhooks-2` · After `webhooks-2.png`, `webhooks-2-lt.png`, `src/webhooks-2.html` · Before `webhooks-2-b1.png` ·
    Screens `C018-webhook-form.png`, `C020-webhook-created.png`.
  - `webhooks-3` · After `webhooks-3.png`, `webhooks-3-lt.png`, `src/webhooks-3.html` · Before `webhooks-3-b1.png` ·
    Screens `C025-webhook-deliveries.png`, `C072-m-webhooks-deliveries.png`.
  - `webhooks-4` · After `webhooks-4.png`, `webhooks-4-lt.png`, `src/webhooks-4.html` · Before `webhooks-4-b1.png`,
    `webhooks-4-b2.png` · Screens `C026-webhook-conversation.png`, `C075-m-webhook-conversation.png`.
  - `webhooks-5` · After `webhooks-5.png`, `webhooks-5-lt.png`, `src/webhooks-5.html` · Before `webhooks-5-b1.png`,
    `webhooks-5-b2.png` · Screens `C028-webhook-secret-rotated.png`, `C030-webhook-rotate-url.png`,
    `C032-webhook-delete-confirm.png`.
  - `webhooks-6` · After `webhooks-6.png`, `webhooks-6-lt.png`, `src/webhooks-6.html` · Before `webhooks-6-b1.png`,
    `webhooks-6-b2.png` · Screens `C017-webhooks-empty.png`, `C021-webhook-secret-shown.png`.
  - `channels-3` · After `channels-3.png`, `channels-3-lt.png`, `src/channels-3.html` · Before `channels-3-b1.png`,
    `channels-3-b2.png` · Screens `C033-channels.png`, `C034-ask-alice.png`.
  - Not in this brief: `channels-1` (Link an agent, P5-REFINE-12), `channels-2`, `channels-4` (P5-REFINE-12).
- **Layer-above rec 12** (`UXR5/layer-above.md`, anchor `https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#above`):
  one list, runs are threads, Peers → Settings › Other agents; **rec 15** (the stop ladder: Pause/Resume for automations,
  Rotate and Delete dialogs); **rec 1** (cron and request ids are Expert); rec 10 (first run card, owned by P5-REFINE-11).
- **Decisions:** D-405, D-406, D-407, D-409, D-417. **Contracts:** C-1073..C-1079 (P5-REFINE-7: per-run threads, Run now,
  Runs outcomes, next three run times, title sync, deleted marker, webhook signed by default and deliveries), C-1080..C-1082
  (P5-REFINE-8: ask with a reason). Read them in `docs/API_CONTRACTS.md` once merged; this brief names fields by meaning
  only.
- **Docs:** `docs/10-*` §Schedules, §Webhooks, §Conversations; **FIX-192** (commit `5ffaa929`: a schedule created from a
  chat thread answers in that thread; **keep it**); `docs/14-web-dashboard.md` §Agent page, §Triggers.

## Wave and order

- Lane B, after **P5-REFINE-13** and **P5-REFINE-7** (backend), and after **P5-REFINE-1** (renames), **-2** (primitives),
  **-3** (Altitude). P5-REFINE-11 owns the agent page's tab bar (Chat · Automations · Computer, Settings button); this
  brief supplies the **Automations page** and registers its page id. Whoever lands second rebases `sections.ts`.
- **Items that wait for a backend brief** (do the rest first; until it merges keep today's behaviour for the dependent
  part and say so in the completion report):
  - **P5-REFINE-7:** the per-run thread links (item 6), Run now's "Open the conversation" (item 5), Runs outcomes
    (item 5), "next three run times" (item 3), title sync and the deleted marker (item 8), Signed with a server-made
    secret (item 9), Deliveries with result words, reason, duplicate and conversation (item 12), the webhook message
    display without the wrapper (item 11).
  - **P5-REFINE-8:** the reason field on Ask (item 14).
- Items 1 to 4, 7, 9 (the form without a server-made secret), 10, 13 and the words of 14 do not wait.

## Context

All paths are under `web/src/`; line numbers are master `c0fe7d10`.

- **Agent page** `components/agents/agent-page.tsx`: tab content switch :280–320 (`schedules` :299, `webhooks` :300,
  `peers` :301, `channels` :302); `components/agents/sections.ts` (`SECTIONS` :67–73: `triggers` holds schedules,
  webhooks, channels, peers; `AgentPageId`, `PAGE_LABEL`).
- **Schedules** `components/schedules/schedules-tab.tsx` (271 lines): `Runs` :30, `ScheduleForm` :59–148 (Name, Prompt,
  "Cron (5 fields)" :119, "Timezone" free text :122, Gate command :127, Gate description :130, Anchor checkbox :135,
  Enabled checkbox :138), `ScheduleRow` :150–221 (cron `<code>` :174, "Enabled/Disabled (reason)" badge :176, "Next: ...
  Last: ..." :181, six buttons :199–217, delete confirm button :209), `SchedulesTab` :223.
  API: `lib/api/schedules.ts` (`useSchedules`, `useScheduleRuns`, `useRunScheduleNow` :57).
- **Webhooks** `components/webhooks/webhooks-tab.tsx` (281 lines): `KINDS = ["none","hmac_sha256","bearer"]` :30,
  `Deliveries` :32–54 (a table with no headings), `WebhookForm` :56–139 (Verification select :105, Header :112, "Shared
  secret" :115), `WebhookRow` :141–230 (confirm buttons :186–219), `WebhooksTab` :232 with `OneTimeKeyDialog`
  ("Copy your webhook URL"). API: `lib/api/webhooks.ts`. One-time secret dialog `components/account/one-time-key-dialog.tsx`
  (**replace by the shared `SecretOnce` of P5-REFINE-2**).
- **Channels** `components/channels/agent-channels-tab.tsx`: the empty state :446–457 uses `AskAdmin`
  (`components/ask-admin.tsx:17`: today it sends only the subject, no text; label "Ask <first name>" :70);
  `NoIntegration` :44; Presences :246, Bindings :273, Reach :348.
- **Peers** `components/peers/peers-tab.tsx` (118 lines): stays as it is here; its home becomes Settings › Other agents
  (P5-REFINE-12, P5-REFINE-10). This brief only removes it from the Automations page and keeps `?tab=peers` working.
- **Shared pieces** to reuse: `ReassignPanel`, `AnchorSwitch`, `OwnerLabel` (the schedules and webhooks rows import
  them), `components/ui/{menu,tabs,select,confirm-dialog,data-table,empty-state?,status-line,badge}.tsx`,
  `lib/format` date helpers (`formatWhen`, P5-REFINE-2), `lib/labels.ts` (P5-REFINE-2), `ResponsiveTable`
  (P5-REFINE-2), `SecretOnce` (P5-REFINE-2), `<Altitude>` (P5-REFINE-3).
- **Conversation list** `components/chat/conversation-list.tsx`, `lib/chat/layout.ts:56–57` (`kindLabel`: Schedule,
  Webhook); the grouping and "New chat" are P5-REFINE-11's.
- **Tests:** `tests/schedules*.test.tsx`, `tests/webhooks*.test.tsx`, `e2e/webhook.spec.ts`, `e2e/webhook-stack.spec.ts`,
  `e2e/support/webhook.ts`, `e2e/support/mock-api.ts`, `e2e/agent-tabs.spec.ts`.

## Scope

1. **One "Automations" page (rec 12).**
   Anchors `#above` (rec 12), `#f-webhooks-6` · After `UXR5/after/webhooks-6.png`, `webhooks-6-lt.png`, `after/src/webhooks-6.html`
   (the page frame) · Before `webhooks-6-b1.png`, `webhooks-6-b2.png` · Screens `C004-triggers.png`,
   `C017-webhooks-empty.png`, `C033-channels.png`, `C035-peers.png`.
   - New page id `automations` (`?tab=automations`; label "Automations") in `sections.ts` and a component
     `components/automations/automations-tab.tsx`. The old ids `schedules`, `webhooks`, `channels` open Automations
     (`?tab=schedules` scrolls to and filters to schedules; same for the others). `peers` is not part of Automations.
   - **Page frame (webhooks-6):** title "Automations", subtitle "When Dani runs: on a schedule, when something calls its
     address, or in a chat.", header button **New automation**. Same frame for every state.
   - **One list.** Each row: name · kind icon (clock, link, chat bubble) · **when** in words · state **On** / **Paused**
     · last run with its outcome · one primary action by kind (**Run now** for a schedule, **How to call it** for a
     webhook, **Open channel** for a channel) · a "…" menu (item 7). Examples of "when": "Every weekday at 09:00 ·
     Zurich", "When something arrives at its address", "In #ops on Mattermost".
   - A filter row above the list: **All · Schedules · Webhooks · Chats** (counts), keyboard accessible.
   - **New automation** opens a dialog that asks the **kind first**, one sentence each: "On a schedule — Dani runs at set
     times.", "When something calls it — an outside system sends Dani a request.", "In a chat — Dani joins a channel on a
     chat platform." (the last only when an organization has a chat platform; otherwise the sentence of item 14 and no
     choice). Choosing continues into the form of item 3 or 9.
   - Empty state: "No automations yet" / "Dani can run on a schedule, when something calls it, or in a chat." / **New
     automation**.
   - The channels body (presences, space bindings, reach requests, "Show progress in chat") moves under a "Chats" row
     group or the row's expand; keep every function; they are Set up items (`<Altitude level="setup">`), and Space
     bindings and Reach requests are Expert (P5-REFINE-3's table).

2. **States in words: On / Paused (schedules-4; D-407).**
   Anchor `#f-schedules-4` · After `UXR5/after/schedules-4.png`, `schedules-4-lt.png`, `after/src/schedules-4.html` · Before
   `schedules-4-b1.png`, `schedules-4-b2.png` · Screens `C008`, `C015`.
   - The badge says **On** or **Paused** (never "Enabled", "Disabled (user)", "Running"; **the decision wins over the
     mockup's "Running"**). A paused row says "Every 5 minutes · paused, won't run until resumed" and "Paused by Carol
     Member" (from `disabled_reason`/actor, in words; `authorization` keeps its Notice: "Paused because its owner lost
     access. Reassign it to a current member to bring it back.").
   - The action is **Pause** / **Resume** (menu, item 7); no confirmation; a toast "Paused Morning summary · **Undo**" through
     `useUndoToast()` (`components/ui/`, built by P5-REFINE-2; rec 15, rung "Stop temporarily"). Same for webhooks and chats.
   - The cadence is a sentence: "Every 5 minutes · next at 23:55 (Zurich time)" from the schedule's cron and time zone
     (`lib/cron.ts`, item 3); no raw cron outside Expert.

3. **Schedule form with presets and a time picker (schedules-1, schedules-2, schedules-5).**
   Anchors `#f-schedules-1`, `#f-schedules-2`, `#f-schedules-5` · After `UXR5/after/schedules-1.png`, `schedules-1-lt.png`,
   `schedules-2.png`, `schedules-2-lt.png`, `schedules-5.png`, `schedules-5-lt.png`; markup `after/src/schedules-1.html`,
   `schedules-2.html` · Before `schedules-1-b1.png`, `schedules-2-b1.png`, `schedules-5-b1.png` · Screens `C005`, `C006`,
   `C007`.
   - Fields in this order: **Name** ("Morning summary"), **Prompt** ("Summarise what changed overnight and tell me what
     needs me."), **When**, then **Advanced**.
   - **When** is a segmented row of presets: **Every 5 minutes · Hourly · Every day · Weekdays · Weekly · Custom**. There is
     **no preset shorter than 5 minutes** ("Every minute" is not offered). Every day, Weekdays and Weekly show "at" and a
     time picker (24 h, default 09:00); Weekly adds a day select; Hourly adds "at minute". The time zone is a **searchable
     select** (Europe/Zurich · Acme workspace), defaulting to the **workspace's time zone** (`defaultTimezone`); the
     workspace name sits in the option's detail; a person may choose another zone. Raw "IANA name" text goes.
   - **Summary line** under it: "Every day at 09:00 · next: tomorrow 09:00 (Zurich time). Schedules run at most every
     5 minutes." For Every 5 minutes: "Every 5 minutes · next 23:55, 00:00, 00:05." (**next three run times**: from the
     API when P5-REFINE-7 has merged, in the workspace's time zone; before that show only `next_fire_at` as one time).
   - **Custom** keeps the 5-field cron in a monospace field (an **Expert** control: `<Altitude level="expert">`; a person
     without the switch sees Custom disabled with "Needs more controls · Show", and an existing custom cron is shown as
     read-only text "Custom schedule" in the form). **As you type**, a cron that runs more often than every 5 minutes is
     flagged under the field in red: "Too often: a schedule can run at most every 5 minutes. Try */5 * * * *." (pure
     function `lib/cron.ts` `minGapMinutes(cron)`; conservative; also covers lists and steps in the minute field;
     the API remains the authority and its error is shown the same way).
   - **Advanced** (collapsed, `<Altitude level="setup">`): "Only wake when a check finds something" with the gate command
     and its description (replaces "Gate command"/"Gate description"), one sentence: "Dani is only woken when this
     command reports something to do."; "Keep running without you" (the anchor switch; replaces "Anchor to workspace
     (outlives any single member)"), one sentence: "The schedule keeps running even if you leave the workspace." Labels from
     `lib/labels.ts`; `Enabled` checkbox on edit becomes the **On / Paused** control of item 2.
   - Buttons **Create schedule** / **Cancel**; the save pattern of P5-REFINE-2 for edits.
   - `lib/cron.ts` also parses the presets back from an existing cron (so Edit opens on the right preset) and writes the
     sentence for the row; unknown crons read "Custom schedule" with the raw cron under Technical details.

4. **Schedule row: two buttons and a menu (schedules-6).**
   Anchor `#f-schedules-6` · After `UXR5/after/schedules-6.png`, `schedules-6-lt.png`, `after/src/schedules-6.html` · Before
   `schedules-6-b1.png`, `schedules-6-b2.png` · Screens `C013`, `C016`, `C070-m-schedules.png`.
   The row's buttons are **Run now** and **Edit**. The "…" menu holds **Runs**, **Pause**/**Resume**, **Reassign…**, **Keep
   running without you** (anchor), **Delete…** (danger ink, after a separator). **Designer's note (follows):** below 640 px
   **Runs stays a visible button** on the row and only Reassign and Delete go into the menu. **Edit opens in place**,
   replacing the card's text, not a card inside the card. **Reassign** opens as a dialog (it no longer opens above and
   pushes its button away). Delete uses the shared `ConfirmDialog` with the consequence: "Delete Morning summary? It
   stops running. Its past conversations are kept." (rec 15); the inline "Confirm delete" button is gone.

5. **Run now and Runs (schedules-3). Waits for P5-REFINE-7.**
   Anchor `#f-schedules-3` · After `UXR5/after/schedules-3.png`, `schedules-3-lt.png`, `after/src/schedules-3.html` · Before
   `schedules-3-b1.png`, `schedules-3-b2.png` · Screens `C009`, `C010`, `C011`, `C012`.
   - **Run now** shows at once a status line under the row (toast on a phone): "Started · **Open the conversation**",
     where the link opens the run's own thread (`?tab=chat&c=<conversation id>`, the id from the run the API now returns).
     Disabled while the request is pending; if the run is skipped by its gate the line says so.
   - **Runs** is a table (shared `ResponsiveTable`: stacked rows below 900 px) with columns **When · Outcome ·
     Conversation**: "23:55 · Answered · *Every 5 minutes ping · 23:55*", "23:50 · Skipped by the gate · nothing to
     report · None", "23:45 · Failed · computer didn't wake · Open details". Outcome words: **Answered, Skipped by the gate,
     Failed, Running** (`lib/labels.ts`; reason in words; "Open details" goes to Technical details, Expert). The
     conversation cell links to the run's thread. Times use `formatWhen`.

6. **One thread per run; Open the thread (D-409). Waits for P5-REFINE-7.**
   Anchor `#above` rec 12; `#f-schedules-3`, `#f-webhooks-3`.
   - Every link to a run or a delivery goes to **that run's own conversation**, titled "Morning summary · 20 Nov 09:00"
     (title from the API). Runs, Deliveries and Run now all use the one helper `automationThreadHref(agent, conversationId)`.
   - A schedule created from a chat thread keeps its FIX-192 behaviour (the answer also goes to the origin thread); the
     dashboard's run row for it still links to the run's thread, and the row says "Also posts in #ops" when it has an
     origin link (from the API). Do not change FIX-192's server behaviour.
   - The conversation list folding ("Morning summary · 12 runs") is P5-REFINE-11; this brief only supplies the hrefs.

7. **The "…" menu on every row (all kinds).**
   Menu items by kind, in this order, each with the word from `lib/labels.ts`:
   schedule: Runs · Pause/Resume · Reassign… · Keep running without you · Delete… ;
   webhook: Edit · How to call it · Deliveries · Rotate URL… · New secret… · Pause/Resume · Reassign… · Delete… ;
   chat: Open channel · Pause/Resume (when supported) · Remove from channel…. Danger items after a separator. On a phone
   the primary action and Runs/Deliveries stay visible (designer's note of schedules-6).

8. **The conversation follows the schedule's name; "Schedule deleted" (schedules-7). Waits for P5-REFINE-7.**
   Anchor `#f-schedules-7` · After `UXR5/after/schedules-7.png`, `schedules-7-lt.png`, `after/src/schedules-7.html` · Before
   `schedules-7-b1.png`, `schedules-7-b2.png` · Screens `C014`, `C093`.
   The list row and the header of a run's conversation show the live title (the API keeps it in sync) with "Schedule ·
   was "Every minute ping", renamed with it" as the first rename note, and, after the schedule is deleted, a **Schedule
   deleted** mark on the row and, in the thread header, "It was started by a schedule that no longer exists. No new
   runs." (`kindLabel` in `lib/chat/layout.ts:56`). Same for a deleted webhook ("Webhook deleted"). If P5-REFINE-11 has
   not merged the list changes, add the mark to the existing list row and header.

9. **Webhook form: who may call it; signed by default; Generate (webhooks-2).**
   Anchor `#f-webhooks-2` · After `UXR5/after/webhooks-2.png`, `webhooks-2-lt.png`, `after/src/webhooks-2.html` · Before
   `webhooks-2-b1.png` · Screens `C018`, `C020`.
   - The field "Verification" becomes **"Who may call this webhook?"**, three choice cards:
     **Signed (recommended)** — "Only senders that know the secret can wake Dani." (default, replaces `hmac_sha256`);
     **Bearer token** — "The sender puts a token in the Authorization header."; **Anyone with the URL** — "No check. Use
     only for testing." (replaces `none`; shown last, with a pending-tone note when chosen). Enum values never show.
   - **Secret** (for Signed and Bearer): a field with a **Generate** button that fills a random 32-character secret
     (`crypto.getRandomValues`) with the hint "Generated for you, 32 characters. You'll see it once, so copy it to the
     sender." Leaving it empty on Signed is allowed once P5-REFINE-7 has merged (the server makes one and returns it);
     before then **Create webhook** needs a secret and Generate fills it. The "8 to 512 bytes" rule is not shown unless
     it is violated.
   - **Header** (optional) moves under **Advanced** with the placeholder `X-Signature` and its default stated.
   - Fields: **Name**, **Instructions** ("Tells Dani what to do with each delivery."), then the choice, secret, Advanced
     ("Keep running without you" anchor for owners). **Create webhook** / **Cancel**.
   - After creating, the **one-time dialog** (`SecretOnce`, P5-REFINE-2): the address and the secret each in a box that fits
     the whole value on one line (no mid-token wrap; wider dialog, 640 px; the box scrolls horizontally or breaks at
     `/` only on a phone), **Copy** → **Copied**, the "closing it loses the secret" warning, and the **How to call it**
     panel (item 10) below. This is the `webhooks-6` bottom mockup ("Your secret ... Shown once. The box fits the whole value.").

10. **"How to call it" panel (webhooks-1).**
    Anchor `#f-webhooks-1` · After `UXR5/after/webhooks-1.png`, `webhooks-1-lt.png`, `after/src/webhooks-1.html` · Before
    `webhooks-1-b1.png`, `webhooks-1-b2.png` · Screens `C019`, `C021`, `C027`.
    - Panel titled **How to call it** with **Copy** on the right. Text: "Send a POST with the body signed in the
      `X-Signature` header: `sha256=` followed by the hex HMAC-SHA256 of the body." (Bearer: "Send a POST with the header
      `Authorization: Bearer <token>`." Anyone: "Send a POST to the address.")
    - Tabs **curl · Python · Node**; each a real, copyable snippet using the real address and the real header name;
      curl as in the mockup (`BODY=...`, `SIG=$(printf %s "$BODY" | openssl dgst -sha256 -hmac "$SECRET" | sed 's/^.* //')`,
      `curl -X POST <url> -H "Content-Type: application/json" -H "X-Signature: sha256=$SIG" -d "$BODY"`). The secret is
      never in the snippet: it reads `$SECRET` / an environment variable. The three snippets live in
      `lib/webhook-snippets.ts` with a unit test (the HMAC they compute matches `crypto` for a fixed body).
    - Shown **on creation** (inside the one-time dialog, with the full address) and **on the card** ("How to call it" row
      action or menu item). On the card the full address was shown once, so the snippet uses the prefix with `…` and the
      line "The full address was shown once. Rotate it to get a new one." The CLI/`curl` snippet is Expert only for the
      Copy-as-curl shortcut (P5-REFINE-19); the panel itself is Set up.

11. **The webhook delivery message in chat (webhooks-4). Waits for P5-REFINE-7.**
    Anchor `#f-webhooks-4` · After `UXR5/after/webhooks-4.png`, `webhooks-4-lt.png`, `after/src/webhooks-4.html` · Before
    `webhooks-4-b1.png`, `webhooks-4-b2.png` · Screens `C026`, `C075-m-webhook-conversation.png`.
    The first message of a delivery's thread reads **"Webhook Deploy notice · delivery"** with "23:58 · signed and
    accepted" under it and the payload in a code block (`{"event": "deploy", ...}`), then a collapsed **Technical details**
    (Expert). The safety wrapper text and the delivery id are **not** rendered: the API returns a separate display payload
    (C-1076..C-1079); the agent still receives the wrapper. Add the renderer for the message kind to the chat transcript
    (`components/chat/` message renderer; keep the change small and name the file in the completion report).

12. **Deliveries table (webhooks-3). Result words wait for P5-REFINE-7.**
    Anchor `#f-webhooks-3` · After `UXR5/after/webhooks-3.png`, `webhooks-3-lt.png`, `after/src/webhooks-3.html` · Before
    `webhooks-3-b1.png` · Screens `C025`, `C072-m-webhooks-deliveries.png`.
    `Deliveries` (`webhooks-tab.tsx:32`) becomes a `ResponsiveTable` with headings **When · Result · Size · Conversation**.
    Results: **Accepted**, **Refused: wrong signature** (and the other refusals in words), **Duplicate, ignored**; the
    conversation cell links to the delivery's thread (item 6), "None" when there is none. A refused or duplicate row is
    quiet, not red, except a refusal in red text of the Contrast rules. Times via `formatWhen`. Stacked rows below 900 px.

13. **Rotate, new secret and Delete as dialogs (webhooks-5).**
    Anchor `#f-webhooks-5` · After `UXR5/after/webhooks-5.png`, `webhooks-5-lt.png`, `after/src/webhooks-5.html` · Before
    `webhooks-5-b1.png`, `webhooks-5-b2.png` · Screens `C028`, `C030`, `C032`.
    Three `ConfirmDialog`s (P5-REFINE-2), no inline red buttons:
    - **Rotate the URL?** "The old URL stops working now. Update every sender that uses it. The new URL is shown once."
      [Cancel] [Rotate URL]
    - **Make a new secret?** "Signatures made with the old secret are refused from now on. Update the sender with the new one
      first, or deliveries will fail." [Cancel] [Make new secret] (the secret is rotated from the menu, not hidden in
      Edit); the new secret shows once in `SecretOnce`.
    - **Delete Deploy notice?** "The webhook and its delivery history are removed. Senders that call it get an error.
      Conversations it started are kept." [Cancel] [Delete webhook]. The item's visible name is in the title; the confirmation
      uses the ladder's Delete rung as P5-REFINE-2 builds it (rec 15: a dialog with the consequence; if that primitive
      asks for the item's visible name typed, follow it; the mockup shows a plain dialog, so use the plain dialog unless
      P5-REFINE-2's table says otherwise and note the choice in the completion report).
    Do the same for schedules' Delete (item 4).

14. **"Ask an admin" on Chats, with a reason (channels-3). Reason waits for P5-REFINE-8.**
    Anchor `#f-channels-3` · After `UXR5/after/channels-3.png`, `channels-3-lt.png`, `after/src/channels-3.html` · Before
    `channels-3-b1.png`, `channels-3-b2.png` · Screens `C033`, `C034`.
    The empty Chats state says: "Dani isn't in any chat yet" / "Only organization owners and admins can connect a chat
    platform." / **Ask an admin** "to connect Slack, Teams or Mattermost" (the platforms the instance has connectors for,
    from the connector list) / a field **"Why do you need it?"** (optional, placeholder "So Dani can answer in #ops") /
    **Send request**. After sending: "Asked an admin · **Undo**". The request goes to **every admin who can do it** (D-407,
    D-417); the name of one admin never appears in the button. `AskAdmin` takes `reason` (max 500 characters) once
    C-1080..C-1082 exist; before then ship the words and platform names and hide the field.

## Out of scope

- The conversation list's folding and New chat (P5-REFINE-11). Peers' new home and the "Link an agent" button
  (P5-REFINE-12, P5-REFINE-10). Bulk actions and Copy as curl shortcut (P5-REFINE-19).
- Any server behaviour (P5-REFINE-7, -8). FIX-192's server logic stays.
- The attention feed (P5-REFINE-8, -16).

## Acceptance criteria

1. The Automations page matches `UXR5/after/webhooks-6.png` (frame) in both themes at 1256 and 390 px; the old tab ids
   still open it; the page has one header button and one list.
2. **Schedules:** matches `schedules-1.png`/`-lt.png`, `schedules-2.png`/`-lt.png`, `schedules-4.png`/`-lt.png`,
   `schedules-6.png`/`-lt.png` at 1256 and 390 px. There is no preset below 5 minutes; typing `* * * * *` shows the red
   "Too often" line before submit; presets round-trip through `lib/cron.ts` (unit tests for every preset and for 20 odd
   crons); the time zone defaults to the workspace's; at 390 px Runs is visible and only Reassign and Delete are in the menu.
3. **Run now and Runs** (after P5-REFINE-7): `schedules-3.png`/`-lt.png`; the link in "Started · Open the conversation" opens
   Chat on the run's thread (`c=<id>`), proven by a mocked Playwright click-through; outcome words from the table only.
4. **States:** the only state words rendered are On, Paused (and "Running" for an in-flight run). A test greps the
   rendered Automations page for `Enabled`, `Disabled`, `hmac_sha256`, `rejected_signature`, `none`, a cron string outside
   Technical details, and a UUID.
5. **Webhooks:** matches `webhooks-1.png`, `webhooks-2.png`, `webhooks-3.png`, `webhooks-4.png`, `webhooks-5.png`, `webhooks-6.png`
   (each with `-lt`) at 1256 and 390 px. Signed is the default; Generate fills 32 characters; the three snippets show the real
   address and header and never the secret; Copy puts the snippet on the clipboard (Playwright clipboard check).
6. **Dialogs:** Rotate, New secret and Delete say what changes (exact sentences above); Escape and Cancel change nothing.
7. **Conversations** (after P5-REFINE-7): a renamed schedule's thread shows the new title; a deleted schedule's thread shows
   "Schedule deleted".
8. **Channels:** matches `channels-3.png`/`-lt.png`; the request sent includes the reason (after P5-REFINE-8); no admin's name
   in the button.
9. No horizontal scroll at 390 px, axe with no rule disabled, no console errors, both themes; no snake_case/UUID outside
   Technical details (the labels guard).
10. Screens in the completion report: each key's Before crop beside the new screenshot and the After mockup.

## Tests

Targeted only. Heavy commands through `/home/alex/orca/workspaces/aurora/heavy.sh <lane> <command>`.

- vitest: `lib/cron.ts` (new, many cases), `lib/webhook-snippets.ts` (new), schedule form (presets, too-frequent, zone default),
  schedule row and menu, webhook form (default Signed, Generate), deliveries table words, the Automations list (kinds,
  filter, empty state), `AskAdmin` with a reason.
- Playwright (mocked): update `e2e/agent-tabs.spec.ts`, `e2e/webhook.spec.ts`, `e2e/webhook-stack.spec.ts`; new
  `e2e/automations.spec.ts` (create a schedule by preset, Run now, open the run thread, pause and Undo, create a webhook,
  copy the snippet, rotate with the dialog, deliveries). Both themes at 1280 and 390 px with axe; screenshots under
  `/tmp/aurora-screens/p5-refine-14/` (outside the repo).
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

- `docs/14-web-dashboard.md` §Agent page (Automations page, rows, kinds), a new §Automations (presets, cron rule, How to call it,
  deliveries, dialogs), §Vocabulary (On/Paused, Pause/Resume, Accepted/Refused/Duplicate).
- `docs/10-*` §Schedules/§Webhooks only to point at the dashboard wording if a sentence there names "Enabled/Disabled".
- `docs/PROGRESS.md` row P5-REFINE-14 and `docs/FEATURES.md` rows for schedules, webhooks and channels, both current at every
  commit.

## Completion report

Fill in when done (P5-REFINE-20 reads it).

- Items done, with commit: 1 · 2 · 3 · 4 · 5 · 6 · 7 · 8 · 9 · 10 · 11 · 12 · 13 · 14.
- Items that waited for P5-REFINE-7 or -8 and what shipped meanwhile:
- The file the webhook message renderer went into (item 11):
- Deviations and why (the On/Paused choice over the mockup's "Running" is already decided):
- Screenshots compared (Before / After / mine, per finding key, both themes, 1256 and 390 px):
- Open questions:
