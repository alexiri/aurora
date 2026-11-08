# P2-UI-5: Chat v2 — rebuild the agent Chat tab as a messaging layout

## Goal
Rebuild the agent page's Chat tab as a two-pane messaging layout:
- a conversation list pane and a thread pane inside a fixed-height panel
- the composer pinned to the bottom of the thread
- messages grouped by sender
- tool calls folded into one row per turn
- conversation actions in a menu

This is a presentation change only. Every behaviour the tab has today stays.

## Linked requirements
- Master spec IDs/sections:
  - `14 §Visual design` › **Agent chat** (the normative description)
  - `14 §Page patterns` (row "Chat")
  - `14 §Workspace` › Chat (behaviour: RFE-009, RFE-010, RFE-012, RFE-015, RFE-018)
  - D-102 (this design) and D-68 (visual identity)
- Feature row: `P2/14§chat-v2` in `docs/FEATURES.md`.
- Requirement summary:
  - The page itself does not scroll, and the composer never moves.
  - List rows are built only from existing `Conversation` fields: `kind`, `title`, `last_activity_at`, `active_turn`, `direct_user`.
  - The visibility note, the retention note and the connection state move into the thread header.
  - Archive, Unarchive, Export and Delete move into a `Menu`.
  - Redact moves into a per-message actions toolbar.

## Context
- **Mock-ups**, which are reference only (the spec wins where they differ):
  - `docs/design/ui-refresh/Agent.html` (light, 1280 px; a running turn, approval, attachments and a queued follow-up)
  - `AgentDark.html` (dark: the Archived list, Reconnecting, the open actions menu, a redacted row, a `no_model_key` error)
  - `AgentPhone.html` (390 px thread view)
  - The README there lists the known differences.
  - Open each one in a browser. Copy the layout and spacing, but use tokens, never the mock-ups' hex values.
- **Current code**:
  - `web/src/components/agents/chat-tab.tsx` (609 lines): `ChatTab`, `ChatPane`, `Transcript`, `EventView`, `ApprovalCard`, `AttachmentLink`, `ErrorNotice`, `ContinueNotice`. It is used only by `agent-page.tsx`.
  - `web/src/components/agents/conversation-actions.tsx`: `ConversationActions`, `RedactButton`, and the delete, redact and export dialogs.
  - Data: `web/src/lib/chat/{transcript.ts,use-transcript.ts}`, plus `web/src/lib/api/{conversations,attachments,approvals,org}.ts`.
- **Primitives to reuse** (`web/src/components/ui`): `Menu` (`menu.tsx`), `Button`, `Badge`, `Tabs` (segmented, for Active / Archived), `Notice`, `Avatar` (`components/agent-avatar.tsx`), `Markdown` (safe mode, `components/apps/markdown`), `ConfirmDialog`, `FormDialog`, and the icons in `ui/icons.tsx` (add the ones you need there).
- **API**: no change. `GET /v1/agents/{agent}/conversations?archived=`, events, the SSE stream, messages, follow-ups (`send-now`), attachments, approvals decide, archive/unarchive, export, redact and delete, all as today (`05 §Conversations and turns`).

## Scope
### In scope
1. **New `web/src/components/chat/`** holding the components named in `14 §Page patterns`:
   - `ChatPanel`
   - `ConversationList` and `ConversationRow`
   - the thread header
   - `MessageGroup` (bubble, meta line, door chip, message-actions toolbar)
   - `ToolCallGroup`
   - the inline approval card, as a compact variant of the existing card
   - attachment card and composer chip
   - typing/activity row, redacted row, error block, queued follow-up row
   - `Composer`

   `chat-tab.tsx` becomes a thin composition over them. Move logic rather than duplicating it; `use-transcript` stays the source of transcript state.
2. **Layout.**
   - The panel fills the viewport below the agent header and tabs (at least 560 px). Use CSS only (`dvh` and flex, no JS measuring), and no inline `style=` (P2-UI-4's guard).
   - From 768 px there are two panes. Below 768 px the list and the thread are two views.
   - The selected conversation lives in the URL (a search param such as `?c=<id>`, with no param meaning the caller's direct chat). Back and forward work, and a deep link opens the conversation.
3. **Conversation list.**
   - The title filter runs on the client.
   - Active / Archived uses the segmented `Tabs`.
   - Kind icon and tint, then title, kind line or Working dot, then relative time.
   - Ordering and the owners-only "Other members' direct chats" group follow `14`.
   - Keep the empty-archived text.
4. **Thread header.**
   - One meta line: visibility, kind, archived date and retention. It replaces the two `Notice` banners.
   - A `Live` / `Reconnecting…` pill fed by the existing stream state.
   - The actions `Menu` (Archive or Unarchive, Export JSON, Export with files (zip), separator, then Delete conversation… in `danger`). It reuses the existing dialogs unchanged.
5. **Transcript.**
   - Date dividers, sender grouping (same sender within 5 minutes) and door chips.
   - Person and agent bubbles as in `14`, with markdown in safe mode.
   - Message actions (Copy message; More → Redact…) on hover, focus or tap.
   - Copy uses `navigator.clipboard` and shows a status toast or inline "Copied". When the clipboard is unavailable, the item is hidden, not broken.
6. **Turn content.**
   - `ToolCallGroup`: open while the turn runs, collapsed after it ends, `DENY` in the summary when the group holds a denied or failed call.
   - Inline approval:
     - a `HELD` badge and the countdown
     - the request line and the rule name
     - Approve and Deny for `can_approve` users
     - "Waiting for an approver" with no buttons for everyone else (RFE-012)
     - one line with the decision chip once decided
   - Activity row, redacted row, and the error block with its deep link (the current `ErrorNotice` mapping, `no_model_key` → LLM keys).
   - The continue notice (`ContinueNotice`) keeps its behaviour, restyled as a notice row in the thread.
7. **Attachments.** Message file cards and image thumbnails, the "Expired" chip (C-196), and composer chips with progress, Remove and the failed reason (`uploadErrorMessage`).
8. **Composer.**
   - Queued follow-up rows with "Send now" above the box.
   - Auto-growing textarea (2 to 8 lines).
   - Attach, the hint (hidden below 768 px), Stop while a turn runs, and Send (disabled while empty).
   - The placeholder depends on the turn state.
   - A single row of 44 px controls at 390 px.
   - Enter sends and Shift+Enter adds a new line, as today.
   - **The RFE-018 `Idempotency-Key` handling stays byte-for-byte**: one key per composed message, reused on automatic retries, and a new key after an edit or a deliberate resend.
   - On another member's direct conversation the composer is replaced by the read-only/audited `Notice`. A frozen agent keeps today's disabled behaviour.
9. **Empty conversation**: the large avatar, "Start a conversation with <name>", the agent's brief when set, and focus in the composer.
10. **Tests**: update and extend vitest (see Acceptance). Update Playwright `e2e/first-run.spec.ts` selectors only where the markup changed. Add a `chat-v2` Playwright spec for the layout checks below.

### Out of scope
- Any API, schema, OpenAPI or SSE change.
- Unread counts, previews and presence.
- A conversation details pane, reactions, reply/forward, rich-text formatting, emoji and voice.
- Other agent tabs; the agent header beyond the 44 px avatar and one-line badges.
- The Approvals page (its card stays; only a compact variant is added).
- Redesigning the redact, delete or export dialogs (they are reused as they are).

## Constraints / file ownership
- **Owns:**
  - `web/src/components/chat/**` (new)
  - `web/src/components/agents/chat-tab.tsx`
  - `web/src/components/agents/conversation-actions.tsx` (moving the triggers into the menu and toolbar; dialogs unchanged)
  - the chat part of `web/src/components/agents/agent-page.tsx` (header avatar size and badge line only)
  - `web/src/components/ui/icons.tsx` (adding icons)
  - `web/src/lib/chat/**` (only if grouping or day helpers fit there; pure functions with unit tests)
  - chat tests in `web/tests/{chat,m5-approvals-chat,conversation-lifecycle,direct-visibility}.test.tsx`
  - new `web/tests/chat-*.test.tsx`
  - `web/e2e/first-run.spec.ts` (selectors only)
  - new `web/e2e/chat-v2.spec.ts`
- **Do not touch** `web/package.json` or `pnpm-lock.yaml` (no new dependencies), `globals.css` tokens, other screens, or `docs/` (the coordinator updates the spec if you find a gap, so report it).
- Tokens only: no raw hex and no Tailwind palette colours. No inline `style=`. Both themes. No horizontal page scroll at 390 px (code blocks and long hosts scroll or wrap inside themselves).
- Untrusted text (model output, titles, door names, file names) is rendered through `Markdown` safe mode or as plain text. Never use `dangerouslySetInnerHTML`.
- Accessibility (WCAG 2.1 AA):
  - the transcript is `role="log"` with `aria-live="polite"`
  - the activity row is `role="status"`
  - the error block is `role="alert"`
  - every icon-only button has an `aria-label`
  - the menu and the toolbar are keyboard reachable (Tab and arrow keys through Radix), and the toolbar appears on `:focus-within`
  - the selected row has `aria-current`
  - touch targets are at least 44 px at 390 px
- Decision: D-102 is written. If you need a further choice, report it; do not take a new number.

## Acceptance criteria
- [ ] Every element in `14 §Visual design` › Agent chat is present, in light and dark, at 1280 px and 390 px. A light/dark × 1280/390 screenshot of each state is saved outside the repo in `/tmp/aurora-screens/p2-ui-5/` and listed in the report. The states are: running turn with a held approval, archived list with the open actions menu, empty conversation, owner read-only view, error block, phone list view, and phone thread view.
- [ ] At 1280 px the page does not scroll while the transcript is longer than the panel, and the composer stays at the same position. Asserted in `e2e/chat-v2.spec.ts`: `document.scrollingElement.scrollHeight <= innerHeight`, and the composer's bounding box is unchanged after 30 messages are injected.
- [ ] At 390 px there is no horizontal page scroll in either view. The back button returns to the list, and the browser back button does too, through the URL param.
- [ ] Every behaviour kept, each with a test:
  - RFE-009: direct visibility, the owners' read-only group with no composer, and 404 handling
  - RFE-010: archive, unarchive, delete with typed confirmation, both exports, and redact with reason and reset
  - RFE-012: a non-approver sees "Waiting for an approver" and no buttons
  - RFE-015: upload, thumbnail, failed chip, expired chip
  - follow-ups with "Send now", Stop, the live activity line, door chips, and `no_model_key` → LLM keys
  - RFE-018: the same `Idempotency-Key` on an automatic retry and a new one after an edit (the existing test still passes unmodified)
- [ ] New unit tests: sender grouping (5-minute window and sender changes), day dividers (Today, Yesterday, weekday, date), list ordering (own direct chat first, then `last_activity_at` desc), the title filter, the `ToolCallGroup` summary (count, duration, `DENY` when present), and the Working dot from `active_turn`.
- [ ] The existing vitest suites and `e2e/` specs pass. Selectors change only where the markup changed by design, and no assertion is removed.
- [ ] P2-UI-4's guards pass on the chat screens: token guard, axe 0 serious/critical in both themes, theme test, narrow-shell test, and the `style=` guard.

## Verification
- Commands (in `web/`): `pnpm install --frozen-lockfile && pnpm typecheck && pnpm lint && pnpm test && pnpm build`, then `pnpm exec playwright test e2e/chat-v2.spec.ts e2e/first-run.spec.ts` plus P2-UI-4's specs. From the repo root: `bash scripts/e2e/m1.sh` and `bash scripts/guards/run-all.sh`.
- Expected results: everything green. Against `pnpm start`, the `Content-Security-Policy` header is identical to master's (`curl -sI`).

## Dependencies
- Blocked by: **P2-UI-4 merged**. It sweeps `web/src/**` (including `chat-tab.tsx`) and adds the guards this lane must pass; starting earlier means two lanes editing the same files.
- Unblocks: AUDIT-002 (phase-2 exit includes this row).

## Suggested agent/model and reason
`nextjs-developer` (Sonnet): a multi-file React refactor with state, SSE-driven rendering and accessibility work, but no architecture or contract decisions. Escalate to the coordinator if a requirement in `14` cannot be met from the current API data.

## Completion report
- Files changed:
- Tests/checks (exact commands and results):
- Screenshots (paths, per state × theme × width):
- Spec gaps or deviations found (for the coordinator):
- Remaining risks:
