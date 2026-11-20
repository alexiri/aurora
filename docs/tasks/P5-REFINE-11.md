# P5-REFINE-11: The agent page and chat: Settings button, honest status, a rail that says what it means, several chats, first successes (owner, 2026-11-20)

## Goal

UX review 5 walked the agent page and the chat as Bob and Carol and found 11 chat findings, 2 of them broken, and one
structural problem on the page itself:

- **Configuration is hidden.** Behaviour and Access sit behind an unlabelled "…" in the corner; the per-conversation "…"
  600 px lower uses the same glyph (chat-5).
- **The status lies.** "● Ready" shows while the agent's computer is still booting (layer-above rec 3).
- **The rail is five unlabelled icons** that need a hover and a click (rec 3, rec 4).
- **Archive does nothing visible** (chat-1, broken): the conversation stays in the Active list, also after reload.
- **Sending a long message leaves the transcript scrolled near the top** (chat-2, broken).
- **One chat per person and agent**: no New chat, no Rename; Delete is the only fresh start (chat-3, D-409).
- **Stop leaves no trace** and the header says Working for 10–25 s (chat-4).
- **The status panel squeezes the chat to 300 px** and the composer toolbar wraps to three rows (chat-6).
- **An error shows twice** (chat-7), 13 formatting buttons always show (chat-8), search results are all "Direct chat"
  (chat-9), images are generic file cards (chat-10), the phone opens on the list (chat-11).
- **Nobody marks the first success** (rec 10).

The owner's decisions for this brief:

- **D-408:** a labelled **Settings** button replaces the unlabelled "…" (amends D-382); the tabs are **Chat · Automations ·
  Computer**; the header status follows the **computer's readiness** ("Starting its computer…", **Ready** only when it
  answers, **Asleep**, **Working**).
- **D-409:** **several direct chats** per person and agent, each with fresh history (New chat, Rename, auto-title),
  privacy unchanged; **one thread per run** for schedule runs and webhook deliveries, folded under their automation in
  the conversation list and linked from Runs/Deliveries.
- **D-407:** Pause/Resume replaces Freeze everywhere; text that points to an administrator names the role.
- **D-415:** the rail's "What <Agent> did" is the last five rows of the live agent timeline.

## Linked requirements

- **Artifact (source of truth, owner's comments applied):** <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>
  - `#f-chat-1` … `#f-chat-11`, `#f-access-3` (the header part), `#f-channels-4` (the confirmation after Create is brief 12's);
  - layer-above rec 3 (the page), rec 4 (the timeline), rec 10 (first successes), rec 15 (Pause):
    <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#above>, `UXR5/layer-above.md` recs 3, 4, 10, 15;
  - theme "Stale live state" (`UXR5/priorities-themes.md`), blind spot "Several chats with one agent".
- **Local reference folder (read-only, workers cannot open the artifact):**
  `/home/alex/orca/workspaces/aurora/design-ux-review-5/`. Write it as `UXR5/` from here on. Files: `UXR5/findings.md`
  (authoritative text), `UXR5/layer-above.md`, `UXR5/priorities-themes.md`, `UXR5/after/*.png`, `UXR5/after/src/*.html`,
  `UXR5/before/*.png`, `UXR5/screens/*.png`. Yellow dashed notes in a mockup are commentary, not UI.
- **Per finding** (After dark `UXR5/after/<key>.png`, light `UXR5/after/<key>-lt.png`, markup `UXR5/after/src/<key>.html`,
  Before `UXR5/before/<key>-b1.png`, screens):
  - **chat-1** archive: `UXR5/after/chat-1.png`, `-lt`, `src/chat-1.html`, `UXR5/before/chat-1-b1.png`; screens
    `UXR5/screens/B041-archived-conversation.png`, `B042-archived-still-active.png`, `B045-reload-after-archive.png`.
  - **chat-2** scroll on send: `UXR5/after/chat-2.png`, `-lt`, `src/chat-2.html`, `UXR5/before/chat-2-b1.png`, `chat-2-b2.png`;
    screens `B029-after-long.png`, `B030-long-bottom.png`.
  - **chat-3** several chats: `UXR5/after/chat-3.png`, `-lt`, `src/chat-3.html`, `UXR5/before/chat-3-b1.png`, `chat-3-b2.png`;
    screens `B032-conversation-menu.png`, `B033-agent-actions-menu.png`, `B047-after-delete.png`.
  - **chat-4** Stop: `UXR5/after/chat-4.png`, `-lt`, `src/chat-4.html`, `UXR5/before/chat-4-b1.png`, `chat-4-b2.png`; screens
    `B048-before-stop.png`, `B049-after-stop.png`, `B051-after-stop-30s.png`.
  - **chat-5** Settings button: `UXR5/after/chat-5.png`, `-lt`, `src/chat-5.html`, `UXR5/before/chat-5-b1.png`; screen
    `B033-agent-actions-menu.png`, `C040-agent-actions-menu.png`.
  - **chat-6** overlay and Aa: `UXR5/after/chat-6.png`, `-lt`, `src/chat-6.html`, `UXR5/before/chat-6-b1.png`; screens
    `B040-search-results.png`, `B037-rail-computer.png`.
  - **chat-7** one error box: `UXR5/after/chat-7.png`, `-lt`, `src/chat-7.html`, `UXR5/before/chat-7-b1.png`; screen
    `B189-phone-status.png`.
  - **chat-8** formatting on demand: `UXR5/after/chat-8.png`, `-lt`, `src/chat-8.html`, `UXR5/before/chat-8-b1.png`,
    `chat-8-b2.png`; screens `B018-agent-home.png`, `B024-markdown-preview.png`.
  - **chat-9** search results: `UXR5/after/chat-9.png`, `-lt`, `src/chat-9.html`, `UXR5/before/chat-9-b1.png`; screen
    `B040-search-results.png`.
  - **chat-10** image thumbnail: `UXR5/after/chat-10.png`, `-lt`, `src/chat-10.html`, `UXR5/before/chat-10-b1.png`; screen
    `B028-long-with-attachments.png`.
  - **chat-11** phone opens the chat: `UXR5/after/chat-11.png`, `-lt`, `src/chat-11.html`, `UXR5/before/chat-11-b1.png`,
    `chat-11-b2.png`; screens `B185-phone-chat.png`, `C074-m-chat.png`.
  - **access-3** header "Can't answer" part: `UXR5/after/access-3.png`, `-lt`, `src/access-3.html`,
    `UXR5/before/access-3-b1.png`, `access-3-b2.png`; screens `C041-rex-chat-cant-answer.png`, `C044-rex-agent-access.png`.
  - **Rec 3 rail screens:** `UXR5/screens/B034-rail-needs-you.png` … `B038-rail-about.png`, `B018-agent-home.png`,
    `C003-agent-chat.png`, `C047-computer.png`.
  - **Rec 10** (first successes): no mockup; the words are in `UXR5/layer-above.md` rec 10; screens
    `UXR5/screens/A026-checklist-after-hello.png`, `C038-agent-created.png`, `B152-approvals.png`.
- **Mockup words that are older than the owner's decisions** (D-405: the brief and the decisions win over the mockup):
  `UXR5/after/chat-5.png` still reads **Triggers**, **Freeze Daneel**, "Pauses it until you unfreeze it" and "Delete
  agent…". Use **Automations**, **Pause Daneel** ("Stops it until you resume it") and **Delete Daneel…** (P5-REFINE-1's
  vocabulary and D-407). Everything else in the mockup is to be matched.
- **Decisions:** D-408, D-409, D-407, D-415, D-405, D-406 (altitudes). Existing: D-382 (amended), D-383, D-201, D-230.
- **Contracts used (read the merged text, do not guess names):** P5-REFINE-4's computer readiness field (C-1057..C-1060),
  P5-REFINE-5's "can't answer: no AI key" reason (C-1061..C-1066), P5-REFINE-7's conversations (C-1073..C-1079: several
  direct chats, rename, stopped marker, error event split, per-run threads), P5-REFINE-9's timeline (C-1083, C-1084).
- **Normative docs:** `docs/14-web-dashboard.md` § Workspace › Agent page (phase 5), `§Agent chat`, `§Vocabulary`.

## Wave and order

- **Wave 2, lane A, after P5-REFINE-1** (vocabulary: Automations, Pause, "What … did") **and P5-REFINE-2/3 have merged**
  (primitives and the altitude primitive). Lane A: 1 → **11** → 15 → 19.
- **Items that wait for a backend brief** (do the rest first, in this order, and record in the completion report which
  were done after which merge):

  | Item | Needs |
  |---|---|
  | 1. Settings button, tabs, Settings page shell, Pause | nothing (after -1, -2, -3) |
  | 3. Rail labels, headings, overlay below 1400 px | nothing |
  | 4. chat-2, chat-6, chat-8, chat-9, chat-10, chat-11 | nothing |
  | 5. chat-1 (the Active/Archived fix) | nothing |
  | 6. chat-7 (one error box, the technical line under Details) | nothing (uses the one error event; the split event of -7 makes the text cleaner) |
  | 7. Header status from the computer's readiness | **P5-REFINE-4** |
  | 8. "Can't answer: <reason>" header text; the chat's "has no AI model yet" state | **P5-REFINE-5** (and **-8** for the Ask reason); the fix button is P5-REFINE-12's |
  | 9. Several chats: New chat, Rename, auto-titles, list grouping with per-run threads, search over them | **P5-REFINE-7** |
  | 10. chat-4 Stop keeps the partial answer ("You stopped this answer") | **P5-REFINE-7** |
  | 11. "What <Agent> did" = last five timeline rows, live | **P5-REFINE-9** (ship the heading first) |
  | 12. First-success cards | nothing for the first app/first approval/first run; "first answer" uses the person's first finished turn |

## Context

All paths are under `web/src/` unless stated; line numbers are for master c0fe7d10.

- **Agent page:** `components/agents/agent-page.tsx`:
  - header :161–219, the `AgentState` line :187, the "Agent actions" menu :199–218 (`freezeInMenu` :124, `configItems`
    :133, the trigger `size-11` icon-only `MoreIcon` :204), section tabs :221, config page header and back link :247–256,
    page bodies :268–320;
  - `components/agents/sections.ts` (`SECTIONS` :67–73: chat, behaviour [instructions, memory, skills, evals], triggers
    [schedules, webhooks, channels, peers], computer [...], access; `PAGE_LABEL` :47; `resolvePage` :120);
  - `components/agents/agent-state.tsx` (`AgentState` :25: Ready/Working/Needs you/Can't answer from model readiness,
    approvals and `active_turn`; the computer is deliberately not a state today, P5-POLISH-6);
  - Pause: `components/freeze/freeze-sections` (`AgentFreeze`, still called Freeze in code; the words come from
    P5-REFINE-1); delete: `components/agents/delete-zone.tsx`.
- **Chat tab:** `components/agents/chat-tab.tsx`:
  - `ChatTab` :76; `useSelectedConversation` :54 (`?c=`); `useDirectConversation` (`const direct`, near :89) and the list `useConversations(agentId,
    archived)` :111; **the chat-1 cause:** `activeConvs` :142–145 and `groups` :153–157 both **prepend `directConv`**
    when it is not in the list, so an archived direct conversation is still shown under Active (the API returns it from
    `POST /agents/{agent}/conversations/direct`, `domains/conversations/service.rs:166`, archived or not);
  - the 15 s refetch :139–144; the rail context `ctx` :235–250; `ChatPanel` :272 (`rail`, `list`, `thread`).
  - `components/chat/chat-panel.tsx`: `RAIL_OVERLAY_BELOW = 1100` :8 (**chat-6 wants 1400**), the thread/list switch
    `threadOpen` :63–69 (**chat-11**: on a phone Chat opens on the list).
  - `components/chat/conversation-list.tsx` (`ConversationRow` :38, `ConversationList` :93), `lib/chat/layout.ts`
    (`conversationTitle` :64–67 returns "Direct chat" or the person's name, `groupConversations` :94, `KIND` :54).
  - Thread: `components/chat/chat-thread.tsx` (Stop :180–195, `stopErr` :68), `thread-header.tsx`,
    `transcript-view.tsx` (stick-to-bottom :58–61, jump pill :64–85 and :166–172), `message-group.tsx`
    (`error` events :285–300, `TurnError` :304, `TurnErrorWithRetry` :310), `status-rows.tsx` (`ErrorBlock`,
    `errorCopy` in `lib/chat/transcript.ts`), `attachments.tsx` (image thumbnail :65 exists: verify before changing),
    `composer.tsx` (the `Aa` choice :160, formatting row :496–530, text field :445, `Preview` :484 with
    `max-h-44 overflow-y-auto`), `search/search-results.tsx` (titles from `conversationTitle`).
  - Conversation actions menu: `components/agents/conversation-actions.tsx` (Archive :99–103, Delete :115).
- **Status rail:** `components/agents/status-rail/status-rail.tsx` (`StatusParts` :40–72 with the five parts; collapsed
  strip :134–205 with the icon buttons and hover-only `Tip` :162–170; expanded panel :207–260; `StatusSheet` :276),
  `parts.tsx` (`NeedsYouList` :101, `DoingNow` :139, `RecentCalls` :188, `ComputerPart` :220, `AboutPart` :269),
  `status-data.ts` (`useAgentStatus` :138, `actionsOnly` :64, `sandboxLook` :93, `computerWord` :104), `rail-state.ts`.
- **First-success state:** `lib/onboarding/guide.ts`, `use-guide.ts`; the person's server-side flags are
  `user_onboarding.notices_seen text[]` (`PATCH /v1/me/onboarding { notice_seen }`, at most 50 keys, C-914 area; the
  "What moved" notice stops using it in P5-REFINE-1).
- **API hooks:** `lib/api/conversations.ts` (`useDirectConversation` :122, `useConversations` :125), `lib/api/sandbox*`
  (`useSandbox`), `lib/api/credential-grants.ts` (`useModelReadiness`), `lib/api/timeline.ts` (from P5-REFINE-9).
- **Primitives to use** (P5-REFINE-2 and earlier): `Tabs`, `Menu`, `Dialog`, `ConfirmDialog`, `StatusLine`, `Notice`,
  `EmptyState`, `Altitude` (P5-REFINE-3), `labels.ts` (P5-REFINE-2), toast.
- **Existing tests:** `web/tests/agent-*.test.tsx`, `chat-*.test.tsx`, `status-rail*.test.tsx`; Playwright
  `web/e2e/agent-tabs.spec.ts`, `agent-page-p5.spec.ts`, `chat-v2.spec.ts`, `chat-composer.spec.ts`, `chat-search.spec.ts`.

## Scope

Words are the mockup's unless noted. "Matches" always means both themes at 1256 px and 390 px.

1. **Settings button, tabs, the Settings page (chat-5, D-408, rec 3).** Mockup: `UXR5/after/chat-5.png`, `-lt`,
   `src/chat-5.html`; Before `UXR5/before/chat-5-b1.png`.
   - The tab bar shows **Chat · Automations · Computer** (hosted) or **Computer** (BYO). The trigger section's
     label comes from P5-REFINE-1 (`Triggers` → `Automations`); its pages are unchanged until P5-REFINE-14.
   - Beside the tabs: a **labelled button, "Settings", with a gear icon** (not an icon alone, not "…"). Mockup: outline
     button, accent outline when a Settings page is open (the same highlight the "…" has today, the `triggerClass` in `agent-page.tsx`).
     At 390 px the button keeps its label; the tab row and the button wrap into two rows, never one clipped row.
   - **The button opens the Settings page directly** (on Instructions; a BYO agent the same). **Coordinator's decision,
     2026-11-20:** the owner accepted layer-above rec 3 ("Ok, add Settings button"), and D-408 records it: Settings is
     one page with a sub-nav, and **Pause and Delete sit in a danger zone at the bottom of the Settings page**, not in a
     menu. The chat-5 mockup still draws the old menu; follow the mockup only for the button's look (outline, gear,
     label, accent outline while a Settings page is open), not for the menu. The sub-nav items carry the mockup's
     descriptions as their one-line lead: **Instructions** ("Name, instructions, model"), **Apps & access** ("Apps,
     accounts and the AI key it uses"), **Memory** ("What it remembers"), **Skills** ("What it knows how to do"); at the
     **Expert altitude** (D-406, the `Altitude` primitive) also **Tests** and **Other agents**. The danger zone:
     **Pause <Agent>** ("Stops it until you resume it"; **Resume <Agent>** when paused) and **Delete <Agent>…** in danger
     ink, with today's flows, permissions (workspace owners) and confirm dialogs (`AgentFreeze`, `DeleteAgentDialog`),
     shown under every Settings sub-page.
   - **The Settings page** (every menu item opens it): the existing config page header (back link "Chat", the page name,
     the one-line lead, `agent-page.tsx:225–236`) stays; under it a **left sub-nav** on desktop (Instructions · Apps &
     access · Memory · Skills · Tests* · Other agents*; * = Expert) and the same list as the segmented/overflow `Tabs`
     under 768 px (no horizontal scroll at 390 px). `?tab=` ids are unchanged (`instructions`, `access`, `memory`,
     `skills`, `evals`, `peers`): old links open the right Settings page. A BYO agent opens on Instructions as before.
   - `sections.ts`: a `settings` section replaces `behaviour` and `access` (pages: instructions, access, memory, skills,
     evals, peers); `peers` leaves the triggers section here (its page id is kept; P5-REFINE-12 renames and fills it, P5-REFINE-14
     removes it from the Automations list). `SectionId` and every user of `SECTION_OF` follow; update the tour stops that
     pointed at the old tabs. Rec 3's seventh sub-nav entry, **Advanced**, is **not built**: the mockup
     `UXR5/after/behaviour-5.png` keeps "Advanced" as a fold on Settings › Instructions (visible at the Expert altitude), and
     the mockup wins over the rec (D-405). P5-REFINE-12 owns that fold.
   - A member without owner rights sees the Settings pages read-only as today, without Pause or Delete.

2. **Header: name, state, kind (D-408, rec 3).** The header keeps the avatar, the name, the handle and **one state** with
   a `StatusLine` dot and a word.
   - **Words:** **Ready** (only when the computer answers), **Starting its computer…** (with a spinner, `aria-live`
     polite), **Asleep** (it wakes on the next message), **Working**, **Needs you**, **Can't answer: <reason>**.
   - **Order** (most urgent first, extending `agentStateOf`): Can't answer → Needs you → Working → Starting its computer…
     → Asleep → Ready. A BYO agent shows no computer states.
   - The computer's state comes from the **readiness field of P5-REFINE-4** (read its contract; do not derive "answers"
     from the sandbox state). Until that field exists, keep today's words and do not show Starting.
   - "Can't answer: <reason>" uses the reason P5-REFINE-5 exposes ("no AI key", "the key stopped working", "choose a model").
     The primary action ("Give Rex the Company key", access-3) is **P5-REFINE-12's**: render the state with an
     `action?: ReactNode` slot next to it and leave the slot empty here. Mockup `UXR5/after/access-3.png` for the words.
   - **An agent with no AI model (D-414; after P5-REFINE-5).** When the organization has **no starter key** and the agent has
     no usable key, the conversation's empty state (and the composer's disabled state) says **"<Agent> has no AI model yet"**
     with one action: **Add a key** (a link to the organization's AI keys, for organization owners and admins) or **Ask an
     admin** (everyone else: the `AskAdmin` control, subject `ai_key`, with its optional reason from P5-REFINE-8). Mockup
     for the header and card: `UXR5/after/access-3.png`. It replaces the chat error "Open AI keys, Agent access, Fix it in
     the guide" (screen `UXR5/screens/C041-rex-chat-cant-answer.png`).
   - The mobile "Status" button and the approvals banner (`chat-tab.tsx:279–292`) are unchanged.

3. **The rail says what it means (rec 3; chat-6).** Mockups: `UXR5/after/chat-6.png` (the expanded panel's words: "Status",
   NEEDS YOU "Nothing is waiting for you.", DOING NOW "Idle"), rec 3's text; screens `B034`…`B038`.
   - **Collapsed strip:** each icon has a **visible text label** under it (three-letter label, `Need`, `Now`, `Did`; or the
     full word where it fits at 56 px, as the designer's note says "icon + 3-letter label") in addition to the hover tip and
     the aria label.
   - **Expanded panel and sheet:** three headings in this order: **Needs you · Doing now · What <Agent> did**.
     "Recent actions" is renamed (P5-REFINE-1 owns the string; here the heading and its description change: no
     "thinking isn't listed" apology). The **Computer** part becomes **one line under Doing now** ("on its computer ·
     Open", linking to the Computer tab, with the state in words as in item 2); the **About** part **moves into Settings**
     (the rail no longer has it; "Edit instructions" lives on Settings › Instructions).
   - **"What <Agent> did"** shows the last five timeline rows as sentences (P5-REFINE-9; the sentence per kind from
     `labels.ts`), each a link to its target, with "All" linking to the workspace Activity filtered to the agent. **Until
     P5-REFINE-9 has merged**, keep today's list (`RecentCalls`) under the new heading, and wire the timeline in the
     same file in a later commit. When the timeline stream is available the rows update **live**; in-progress rows change
     in place (`useTimelineStream`).
   - **Overlay threshold (chat-6):** the expanded panel **overlays the transcript** below **1400 px** (change
     `RAIL_OVERLAY_BELOW` 1100 → 1400 in `chat-panel.tsx`) instead of squeezing the chat to 300 px. Esc and the close
     button collapse it (as today for the overlay mode).

4. **Composer and transcript (chat-6, chat-8, chat-2, chat-10, chat-9, chat-7).**
   - **chat-8** `UXR5/after/chat-8.png`: the composer shows **one row** ("Message Daneel…", `Aa`, attach `+`, "Enter to
     send · Shift+Enter for a new line"). **Aa** opens the formatting toolbar (B I S H </> 1. • Link, Write | Preview); a
     second press closes it. The choice is remembered. **Preview grows with its content** (no 170 px box with its own
     scrollbar: remove `max-h-44`/`overflow-y-auto` on the Preview at :484). **Verify first:** `composer.tsx:160`
     already has an `Aa` choice from P5-UX-1; change only what differs from the mockup.
   - **chat-6** `UXR5/after/chat-6.png`: with the Status panel open and search results showing, the composer's toolbar
     stays **one row** (`Aa`, `+`, hint); at narrow widths the hint shortens or hides, nothing wraps to three rows.
   - **chat-2** `UXR5/after/chat-2.png`: **after sending**, the transcript scrolls to the bottom so the person's message
     and the answer are in view, **always**; "Jump to latest" is not shown. New messages while the person was at the
     bottom keep following; while scrolled up they do not (as `transcript-view.tsx:58–85` does). Fix the cause of the
     transcript staying near the top after a long message with attachments (find it: the stick flag is not set on
     send, or the attachments' images change the height after the scroll). Test with a long message and two attachments.
   - **chat-10** `UXR5/after/chat-10.png`: an image attachment is an **inline thumbnail with its name and size** ("pic.png ·
     179 B · Image"); other files keep the file card ("notes.txt · 44 B · File"). **Verify first:** `attachments.tsx:65`
     already renders an `<img>`; find out why the review still saw a generic card (the thumbnail may need `loading`
     state, the content type may arrive as `application/octet-stream`, or the review ran before the fix) and fix what is wrong.
   - **chat-9** `UXR5/after/chat-9.png`: each search result shows **the title of its conversation** (not "Direct chat"
     for all), **a sentence of context** around the match with the match in bold ("…Reply with just **pong** and nothing
     else, then summarise the quarterly plan…"), who spoke ("Daneel: **pong**") and the **full date and time**
     ("19 Nov 23:50"). Quote-replies show 'Replying to "…"' as in the mockup.
   - **chat-7** `UXR5/after/chat-7.png`: an error shows **once**, as **one red box** with "Something went wrong. Try
     again." (plain sentence from `lib/labels.ts`), a **Try again** button where retry applies, and the technical line
     ("API Error: 400 No connected db.") **only under Details** ("Hide details" when open). The agent's own bubble that
     repeats the error is not rendered for an `error` turn. After P5-REFINE-7, read the person-facing sentence from the
     split event; before it, derive the sentence from the code with `errorCopy` and keep `message` under Details.

5. **chat-1: archive does what it says** (`UXR5/after/chat-1.png`).
   - An archived conversation **leaves the Active list at once and after a reload**, appears under **Archived**, and the
     thread shows **"Archived 19 Nov 2026 · read only"**, a **Restore** button and "This conversation is archived. Restore
     it to write again."; **the composer is gone**.
   - **Undo toast**: after Archive, "Conversation archived · Undo" (toast with Undo, the ladder of P5-REFINE-2: undoable
     needs no confirmation).
   - Empty Active list: "No active conversations" / "Start a new chat to talk with Daneel." / **New chat** (the button works
     after P5-REFINE-7; before, it opens the person's chat).
   - **Fix** the two places that prepend `directConv` (`chat-tab.tsx:142–145`, `:153–157`) so an archived direct
     conversation is not injected into Active. Keep the API as is: the direct chat is **archivable** (the designer's mock).

6. **Several chats (chat-3, D-409; after P5-REFINE-7).** Mockup `UXR5/after/chat-3.png`, `-lt`, `src/chat-3.html`.
   - The list header: **"Conversations"** and a **New chat** button (plus icon). A new chat starts with **fresh history**
     and opens at once in the thread with the composer focused; the previous chats stay in the list.
   - **Titles come from the first message** (the API auto-titles, P5-REFINE-7); each row: title, "You and Daneel · 23:50"
     (or "Yesterday", "17 Nov"), the **…** per-row menu: **Rename**, **Archive**, a separator, **Delete this conversation…**
     (danger ink; today's confirm dialog stays). **Rename** edits the title in place (Enter saves, Esc cancels, shows
     "Saved" per P5-REFINE-2's inline pattern) and calls the rename route of P5-REFINE-7.
   - **The list grouping** (`lib/chat/layout.ts` `groupConversations`): the person's chats newest first; **runs** of a
     schedule or webhook **fold under their automation** ("Morning summary · 3 runs ▸", expanding to the run threads,
     each titled "Morning summary · 19 Nov 09:00"); the owner-visible members' chats stay in their own group as today.
   - **`useDirectConversation`** (get-or-create the one direct chat) stops being the default: the Chat tab opens the
     person's **latest active chat**, or creates the first one when there is none. Remove the `directConv` special cases
     that chat-1 also touches. `?c=` deep links keep working (search permalinks, notifications).
   - D-230's search scope covers these threads (P5-REFINE-7 changes the API); the web needs no change beyond showing the
     thread title in a result (item 4, chat-9).
   - The per-run thread's link from the automation's Runs table is P5-REFINE-14's.

7. **chat-4: Stop keeps what was written (after P5-REFINE-7).** Mockup `UXR5/after/chat-4.png`.
   - Pressing Stop keeps **the text written so far** in the transcript and adds the line **"You stopped this answer"**
     (muted, centered under the partial answer, from the **stopped marker event** of P5-REFINE-7). The header goes back
     to **Ready at once** (clear the busy state locally on Stop; the Working chip, the `active_turn` row state and the rail's
     Doing now follow the next event). Stop is disabled with "Stopping…" while the call is in flight.
   - The error path (`stopErr`, `chat-thread.tsx:68`) keeps its message.

8. **chat-11: the phone opens the chat** (`UXR5/after/chat-11.png`). On a phone the Chat tab opens **the person's
   conversation with the composer ready** ("Message Dani…"), not the list; the list is **one tap back** (a "Conversations"
   control in the thread header, as the mockup's top-left). Change `threadOpen`'s default in `chat-tab.tsx` /
   `chat-panel.tsx:63–69` for widths under 768 px only. A deep link `?c=` and the Archived view still work.

9. **First successes (rec 10, `UXR5/layer-above.md` rec 10, screens `A026`, `C038`).** Four cards in chat, **shown once each
   and dismissed per person**, server-side with the existing `PATCH /v1/me/onboarding { notice_seen }` keys
   `first_answer`, `first_app`, `first_approval`, `first_run` (check the key format the API accepts; if it rejects these, ask
   the Orchestrator for the key rule rather than storing in `localStorage`). Words:
   - after the first answer: **"Daneel answered. It thought with the Company key and changed nothing outside this chat."**
     with **See what it did** (opens the rail's "What Daneel did");
   - after the first app: **"Daneel can now read My calendar. Try: 'What's on my calendar this week?'"** (the app's
     `first_prompt`, D-387, with a button that puts it in the composer, not sent);
   - after the first approval: **"You allowed it once. Next time Daneel will ask again; choose Always allow when you trust it."**;
   - after the first schedule run: **"Morning summary ran at 09:00."** with **Open the run** (the run's thread, after
     P5-REFINE-7).
   - Each card is a `Notice` with a **Got it** button, never confused with a real approval (no "Live" pill; a sample card
     is labelled "Example", and these are not samples). They are rendered in the thread under the event that triggered
     them, **not** in the rail. The getting-started guide ticks "Try it" from the same event: "first turn finished"
     (P5-REFINE-18 owns the checklist UI; here only the flag and the card).

10. **Pause everywhere on this page** (D-407): every "Freeze/Frozen/Unfreeze" string on the agent page, the banner under the
    header and the Settings menu reads **Pause / Paused / Resume**. The strings come from P5-REFINE-1; this item checks
    none is left on the pages this brief edits.

11. **Needs-you counts on the agent (owner, 2026-11-20; P5-REFINE-20 row G5).** The owner reported the navigation's
    count badges don't work. The status rail's **Needs you** shows a count (not only a dot) on its icon when collapsed
    and in its heading when open: this agent's approvals and access requests waiting for the viewer; the agent's row in
    the Agents list shows the same count. Live from the approvals stream the page already opens. Same rules as
    P5-REFINE-16 item 19 (no badge at zero, only for those who can decide, screen-reader text, fits at 390 px), with a
    vitest and a mocked Playwright spec where the count drops after deciding.

## Out of scope

- Settings page contents (instructions, memory, skills, tests, new agent, Other agents body): **P5-REFINE-12**.
- The "Give Rex the Company key" button and the Access page: P5-REFINE-12 (access-3 UI).
- The Automations list and its pages (Schedules, Webhooks, Channels): P5-REFINE-14. The Computer tab, Files, Terminal,
  SSH: P5-REFINE-15.
- The workspace Activity page and the Technical view: P5-REFINE-16.
- Command palette and shortcuts (`n` new chat, `g a`): P5-REFINE-19.
- The `labels.ts` table and the shared primitives (P5-REFINE-2); the altitude primitive (P5-REFINE-3).

## Constraints / file ownership

- **Owns:** `components/agents/{agent-page,sections,agent-state,chat-tab,conversation-actions}.ts(x)`,
  `components/agents/status-rail/**`, `components/chat/**`, `lib/chat/**`, the sub-nav shell of the Settings page (a new
  `components/agents/settings-shell.tsx`), the first-success cards (a new `components/chat/first-success.tsx`).
- Do **not** edit `instructions-tab.tsx`, `memory`, `skills`, `evals`, `peers-tab.tsx`, `create-agent-dialog.tsx`,
  `access-tab.tsx` or `components/accounts/**` (P5-REFINE-12). Do not edit `schedules`, `webhooks`, `channels` (P5-REFINE-14).
- P5-REFINE-12 edits one line in `agent-page.tsx` (where `AgentState` is rendered, to pass its `action`): keep it easy to
  merge, and merge first.
- Use tokens and the shared primitives only; no one-off styling (`docs/14 §Visual design`). Both themes; no horizontal
  scroll at 390 px; touch targets ≥ 44 px on a phone.
- Heavy commands through `/home/alex/orca/workspaces/aurora/heavy.sh <lane> <command>`.

## Acceptance criteria

1. The agent page shows **Chat · Automations · Computer** and a **labelled Settings button**; the menu matches
   `UXR5/after/chat-5.png` (dark) and `-lt` (light) at 1256 and 390 px with the words fixed as stated; Pause and Delete are
   below the divider and work; every old `?tab=` link opens the right Settings page; Settings pages show the sub-nav.
2. The header state matches the order and words of item 2; with the computer starting it says "Starting its computer…"
   and never **Ready** until the readiness field says the computer answers (after P5-REFINE-4).
3. The rail has visible labels, the headings **Needs you · Doing now · What <Agent> did**, no About part, and overlays the
   transcript below 1400 px; at 1280 px with search results and the panel open the composer is **one row** (Match
   `UXR5/after/chat-6.png`, both themes).
4. Archiving moves the conversation to Archived at once and after reload; the archived thread is read-only with Restore and
   an Undo toast; no composer. (Match `UXR5/after/chat-1.png`.)
5. Sending a long message with two attachments leaves the latest messages in view and no "Jump to latest" (Match chat-2).
6. The composer shows one row until Aa is pressed; Preview grows (Match chat-8). Images are inline thumbnails (chat-10).
   Search results show a context sentence and the full date (chat-9). An error shows once with Details (chat-7).
7. After P5-REFINE-7: New chat, Rename, auto-titles and run threads folded under their automation (Match chat-3); Stop
   keeps the partial answer and "You stopped this answer" and the header returns to Ready at once (Match chat-4).
8. On a phone Chat opens on the conversation with the composer ready; the list is one tap back (Match chat-11 at 390 px).
9. The four first-success cards appear once each and not again after Got it, also after a reload.
10. Gates: `pnpm typecheck`, `pnpm lint`, targeted vitest, targeted Playwright (mocked) with axe and no console errors, both
    themes, at 1280 and 390 px. GitHub CI is the full gate. Report counts.

## Tests

Targeted only. Heavy commands through `heavy.sh`.

- **Vitest** (`web/tests/`): sections and the Settings menu per role and altitude (member, owner, Expert); old `?tab=` ids
  resolve; `agentStateOf` order incl. Starting and Asleep; `groupConversations` with several chats, archived and run
  folding; the chat-1 regression (an archived direct conversation is **not** in the Active groups); title and context
  sentence for a search result; the stopped marker row; the first-success card shown once per key; rail labels and
  headings; overlay threshold constant.
- **Playwright (mocked)** (`web/e2e/`), 1280 and 390 px, both themes, axe, no console errors:
  - Settings menu opens each page; old links; Pause and Delete reachable below the divider (extend `agent-tabs.spec.ts`);
  - archive then reload (the list is right), Restore, the Undo toast (extend `chat-v2.spec.ts`);
  - send a long message with two attachments and assert the last message is in the viewport;
  - Aa toggles the toolbar; the composer stays one row with the Status panel open and search results showing;
  - New chat and Rename, Stop keeps the partial text (after P5-REFINE-7, mocked events);
  - the phone opens the conversation, the list one tap back.
- Update the existing specs that clicked the "…" or the Behaviour/Access tabs.

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

- `docs/14-web-dashboard.md` § Workspace › Agent page: the Settings button, the three tabs, the Settings page, the header
  state words, the rail's three headings and 1400 px overlay, several chats, first successes; supersede the D-382 bullet.
- `docs/14` §Agent chat (several chats, Stop, errors, search results); `§Vocabulary` (Pause).
- `docs/PROGRESS.md`, `docs/FEATURES.md` rows (chat-1..chat-11 with their status; note which items waited for which brief).

## Suggested agent/model and reason

Sonnet (`nextjs-developer`). Many components on the most-used page, navigation changes, a live status mapping and state
that must be exactly right. Not Haiku; Opus review at integration only for the Settings decision.

## Completion report

Lane A, branch `alexiri/p5-polish-a`, master at 6cbf3a12 then re-merged at 4c92cc7d (brief 13 item 1 and brief 4 are in).
**Status: partly done; items 6, 7, 11 and the live part of 3 wait (list below).**

- **Items done** (commit ids on the branch):
  - **1 Settings button, tabs, Settings page, danger zone** — `bbaa62e9` (after -1, -2, -3). `sections.ts` has a `settings` section
    (instructions, access, memory, skills, evals, peers); `components/agents/settings-shell.tsx` is the frame (back link, name, lead,
    left sub-nav that is the segmented `Tabs` under 768 px, danger zone). `?tab=` ids unchanged.
  - **3 Rail** — `1986b503`: Need · Now · Did labels, headings Needs you · Doing now · What <Agent> did, Computer as one line under
    Doing now, About removed, `RAIL_OVERLAY_BELOW` 1100 → 1400. "What <Agent> did" still shows today's request list (waits for -9).
  - **2 + 7(header) Header state** — `63f8023e`: `agentStateOf` order Can't answer → Needs you → Working → Starting its computer… → Asleep →
    Ready from `SandboxView.computer` (after -4, C-1059); "Can't answer: <reason>" from `answer-readiness` (after -5, C-1062); `action` slot left empty
    for -12. **Done after -4 and -5 merged.**
  - **4, 5, 8** — `f2ff5544` (plus `wip` commit): chat-8 (one row until Aa, Preview grows), chat-6 (hint truncates, 1400 overlay), chat-2
    (scroll to bottom on send via `onSent`, again after image load, no Jump pill), chat-10, chat-9, chat-7, chat-1 (archived thread read-only,
    Restore, Undo toast, empty Active list with New chat), chat-11 (phone opens the conversation).
  - **9 First successes** — see the final commit: `lib/chat/first-success.ts`, `components/chat/first-success.tsx`.
  - **10 Pause everywhere** — the agent page, banner and Settings read Pause/Resume (strings from -1); checked, none left.
  - **Send-back from the coordinator (P5-REFINE-1 failures)** — `c154e215` (agent-evals, app-setup, internet-access specs); altitude `?tab=evals`
    is fixed by the Settings page (its heading is "Tests"); the 390 px tablist overflow: the Computer sub-tabs now use `overflow="menu"`
    (More menu) — **please have the CI Watcher run `agent-tabs.spec.ts` and `files.spec.ts` at 390 px** (Playwright is refused in this lane).
- **Waiting (not done, with what they wait for):** item 6 several chats / Rename / auto-titles / run-thread folding / `useDirectConversation`
  no longer the default → **P5-REFINE-7**; item 7 Stop keeps the partial answer + "You stopped this answer" → **-7**; item 3's live
  "What <Agent> did" (last five timeline rows, `useTimelineStream`) → **-9**; item 8's "<Agent> has no AI model yet" card with Add a key / Ask an
  admin (header words are done) → needs **-8**'s Ask reason and is -12's button; chat-7's person-facing sentence from the split event → -7;
  item 11 the Needs-you count on the agent's Agents-list row (`C-1082`) → **-8** (the rail's own count on the icon and in the heading already
  existed and is kept); the first-run card's "Open the run" button → -7.
- **Deviations from this brief, and why:**
  - Settings: followed the coordinator's decision (button opens the page; Pause and Delete in the danger zone at the bottom), not the
    mockup's menu. Pause in the zone is an outline button; Delete a danger button.
  - Rail: Terminal, Files and SSH buttons left the rail with the Computer part (the brief makes it one line); they are pages of the Computer tab.
    `RailContext.terminalAvailable/sshAvailable` are now unused (kept to avoid churn).
  - Header: a computer that failed to start reads "Can't answer: its computer couldn't start" (not in the brief's word list; "Ready" would be false).
  - First-success keys are dashed (`first-answer`, …) because the API accepts only lowercase letters, digits and dashes (so no question to the Orchestrator was needed).
    The first-answer card does not name the key ("thought with the Company key"): members cannot see it; it says "changed nothing outside this chat" only when the
    turn used no tools, else "answered and used N tools". The cards sit at the end of the transcript (under the latest rows), not pinned under the exact event.
    A card appears only for a success from the last 24 hours.
  - chat-9: no "Replying to …" line — `SearchItem` carries no reply information (needs an API field). Titles of direct chats stay "Direct chat" until -7 auto-titles.
  - chat-7: the error event's `message` and the agent bubble that repeats it (same turn, text equal to or contained in the message) are handled client-side; -7's split event makes it cleaner.
- **Verify-first findings:** chat-8: `Aa` existed (P5-UX-1) but the row was **open by default from 768 px** — now closed at every width, remembered once chosen.
  chat-10: `attachments.tsx` did render an `<img>`, but a person's own message names attachments by id only, so no content type reached `canThumbnail` and
  uploads typed `application/octet-stream` were file cards: the card now reads the type from the HEAD response and the file name, and shows one card
  (thumbnail over name · size · Image) instead of a thumbnail plus a card.
- **Screenshots compared:** none taken (Playwright and a browser are refused in this lane); P5-REFINE-20 compares chat-1..chat-11 and access-3 against the mockups.
- **Gate counts:** typecheck clean; eslint clean on the touched trees; full web vitest **191 files, 2737 tests** (all passed after fixing one test's query-client seed; the fixed file re-ran green, 6/6);
  `scripts/guards/run-all.sh`: all guards passed. No Rust or API change, so `cargo fmt/clippy` and `extract-schema --check` were not run. Playwright mocked specs: edited by reading, **not run**.
- **Tests changed / added:** changed `agent-page-header`, `agent-sections`, `status-rail`, `ssh-sessions` (rail SSH button removed with the feature: replaced by a computer-line test),
  `agent-sandbox-failure`, `chat-search`, `chat-v2`, `fix-170`; added `agent-state`, `refine-11-chat`, `first-success`; specs changed `agent-tabs`, `agent-config`,
  `agent-page-p5`, `dialogs`, `dialogs.prod`, `chat-v2`, `chat-composer`, `chat-search`, `agent-evals`, `app-setup`, `internet-access`; added `refine-11-chat.spec.ts`.
  Removed: the `dialogs.spec` menu-ring assertion (the agent actions menu is gone) and the rail's Terminal/Files/SSH/About assertions (those controls left the rail).
- **Production specs only the nightly runs:** `dialogs.prod.spec.ts` (Settings → Delete flow, read not run).
- **Open questions for the owner:** none new; the Settings page's other sub-pages keep their old bodies until -12.
