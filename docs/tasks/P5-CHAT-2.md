# P5-CHAT-2: Reply, permalinks and "Jump to latest" — web — **medium, phase 5, Next.js**

## Goal
The transcript side of D-228: reply to a specific message, see what a message replies to, jump to it, link
to it, and get back to the bottom. Built to the P5-CHAT-0 boards **Reply**, **Navigation** and **Phone**
and the `14` "Agent chat" text P5-CHAT-0 writes (normative; where a board and `14` disagree, `14` wins).

## Depends on
P5-CHAT-0 (boards + spec), P5-CHAT-1 (C-725 API, regenerated `schema.d.ts`), P5-AGENT-2 (the panel and rail
this sits in), FIX-137 (renderer, used inside quote strips). After FIX-099 and FIX-117 (same chat files).

## Scope
1. **Message actions** (`components/chat/message-group.tsx` toolbar): Reply on agent messages, person
   messages and agent files; Copy link; existing Copy message and More (Redact…). `R` on a focused message
   replies. Touch: tap shows the toolbar (as today).
2. **Reply bar** in the composer (`components/chat/composer.tsx`, only the bar and the `reply_to` field of the
   send call): author + two-line excerpt + ✕; Esc cancels; focus moves to the text field.
3. **Quote strip** above a bubble whose `turn.queued` has `reply_to`: author, clamped excerpt rendered with
   the chat renderer (links inert inside the strip), click/Enter jumps; `{ redacted: true }` → "Original
   message redacted".
4. **Jump and highlight**: scroll to the target, loading older history pages as needed (bounded; if not found
   after the conversation's start, say "Message not available"); highlight 2 s (`accent-soft`, reduced
   motion respected).
5. **Permalinks**: Copy link puts `<origin>/…/agents/<agent>?c=<conversation>&m=<seq>` on the clipboard;
   opening it selects the conversation and jumps; an unknown or redacted `m` shows a neutral notice.
6. **Jump to latest**: while scrolled more than one screen above the bottom, a pill "Jump to latest" with
   "· N new" when new durable messages arrived; click scrolls to the bottom; the transcript does not
   auto-scroll while the person reads history.
7. Errors: 422 `reply_target_invalid` → the reply bar shows "You can't reply to that message any more" and
   the text is kept.

## Owns
`components/chat/{message-group,transcript-view,chat-thread}.tsx`, the reply-bar part of `composer.tsx`
(P5-CHAT-3 takes the file after this lane), `lib/api/conversations.ts` (or wherever the send call lives),
new `components/chat/{quote-strip,reply-bar,jump-to-latest}.tsx`, tests below. **Not** P5-AGENT-2's files
(`chat-panel`, `conversation-list`, `approval-card`, `components/agents/**`). No `package.json` change.

## Acceptance criteria
- [ ] Unit (`web/tests/`): reply bar set/cancel/Esc; quote strip normal/redacted/long excerpt; permalink
      parse; jump-to-latest counter; reply action absent on tool groups, approvals, notices, errors.
- [ ] Playwright (`e2e/chat-reply.spec.ts`): reply to an agent message → sent message shows the strip →
      click jumps and highlights; permalink opens the right message in a fresh page; scrolled-up pill counts
      new messages; 390 px no horizontal scroll; axe clean in both themes.
- [ ] `pnpm typecheck`, `pnpm lint`, `pnpm test`, `pnpm build`; existing `chat-v2` tests updated, green.

## Suggested agent/model and reason
Sonnet (`nextjs-developer`).
