# P5-MEMORY-4: Say in the chat what the agent remembered (owner, 2026-11-27)

## Goal

When the agent saves a memory, today's transcript says "Used 1 tool · **Deny**" and "done": nothing says what was
saved, and a failed call is badged "Deny" although nobody denied anything (M-6). Implement the review's **mem-5**: a
small **memory card** in the transcript — "**<Agent> remembered** · <title> · <one line>" with **View** and **Undo** —
and, when saving fails, "**<Agent> tried to remember this but couldn't**", the reason in plain words, what it wanted to
add, **Save it yourself** and **Dismiss**. Also stop calling failures "Deny".

Decisions: **D-496**, **D-497**. Contract: **C-1172** (the `memory.saved` / `memory.refused` transcript events,
written by P5-MEMORY-1), **C-1177** (the memory deep link for View).

## Linked requirements (look at every image before coding)

- `docs/design/skills-memory/README.md` (binding), `review.md` M-6 and proposal mem-5.
- **After:** `after/mem-5.png` / `-light.png` / `.html` (a component, 680 px): the success card ("Daneel remembered ·
  Grafana service account · Needs read access to the prod folder. · View · Undo") and the refused card ("Daneel tried to
  remember this but couldn't · “Jira project keys” is locked, so Daneel can't change it. What it wanted to add: · PLAT is
  now the key for the platform team's board. · Save it yourself · Dismiss"), each between the person's message and the
  agent's reply.
- **Before:** `before/C01-chat-memory-saved.png`.
- `docs/14-web-dashboard.md` page pattern **Chat** (~1236), colour meaning and the copy system (~162–215).

## Wave and order

After **P5-MEMORY-1** (the events) and **P5-MEMORY-2** (the deep link `?memory=<key>` that View opens). It touches only
chat files, so it can run in a different lane from P5-MEMORY-3.

## Context

- Tool calls: `web/src/lib/chat/layout.ts` (`ToolCall` ~175, `toolGroupSummary` ~199: "Used N tool(s) · 4.2 s",
  `hasDeny` is true for a deny **or any failed call** — the "Deny" of the review), `web/src/components/chat/tool-call-group.tsx`
  (the `<details>` with the Deny badge and per-row "Failed"), `message-group.tsx` (assembly), approval cards
  `approval-card.tsx`. Nothing renders memory specially today.
- Tool names differ by harness (`mcp__aurora__memory_save`, `aurora__memory_save`, `aurora_memory_save`); after
  P5-MEMORY-1 the API appends `memory.saved` / `memory.refused` events (fields in C-1172), so the web does not need to
  parse tool names or results.
- Memory routes for Undo: `DELETE …/memories/{key}` (created) and `POST …/memories/{key}/restore {revision}` (changed);
  the reader may lack write access (a viewer of the conversation who cannot edit the agent).
- Mock LLM: `deploy/dev/llm-mock/vidaimock/scenarios/anthropic/memory-tool.{sse,body}` calls
  `mcp__aurora__memory_save` with `{"key":"conformance",…}`. In the review (throwaway cluster, mock model) the call
  ended "Failed" and nothing was saved: **find out why** (the memory family is offered on demand; the scenario may call
  it before it is loaded, or the call fails for another reason) and fix the scenario or the product, whichever is wrong.

## Scope

1. **The cards.** Render `memory.saved` and `memory.refused` as cards in the turn, after the tool group, matching
   `after/mem-5.png`: an icon, the heading, the title (bold), the preview line, the actions. The tool group itself
   stays (it is the record of the call) but says **Failed**, not Deny, for a failed call.
2. **View** opens the agent's Memory page at that memory (`?tab=memory&memory=<key>`), for people who may read the
   agent's memories; hidden otherwise.
3. **Undo** (only for people who may edit the agent, and only while the memory's current revision is still the saved
   one — otherwise the button reads "Changed since" and is disabled with that reason): a created memory is deleted, a
   changed one restored to `previous_revision`; then the card says "Undone · Redo" (Redo re-applies the saved revision
   through restore) and a toast confirms. Never undo silently across a newer edit.
4. **Refused:** the reason as the event's `message`; "What it wanted to add:" with the preview; **Save it yourself**
   opens the Memory page's new-memory form prefilled with the key (when free), the title and the preview text (for a
   locked memory: opens that memory in edit mode with the text appended as a draft — the person unlocks or saves as
   they choose); **Dismiss** folds the card to one line for this viewer (local state; it reappears on reload — say so
   in the report if you think it should persist).
5. **Fix `hasDeny`**: "Deny" only for a policy DENY decision; failed calls are "Failed"; update the group summary
   accordingly.
6. **Mock scenario:** reproduce the review's failure (the mocked Playwright stack, or read the supervisor/API code path
   and the scenario); fix it so `scn-memory-tool` saves; add a mocked Playwright spec that shows both cards from
   recorded events.

## Out of scope

The memory page itself (P5-MEMORY-2/3), any change to what tools the agent has, the API (P5-MEMORY-1).

## Constraints / file ownership

Owns `web/src/lib/chat/layout.ts` (the deny/failed rule), `web/src/components/chat/tool-call-group.tsx`, a new
`web/src/components/chat/memory-card.tsx`, the event handling where transcript events become message parts, and the
mock scenario files. Tokens and shared primitives only; both themes; 390 px.

## Acceptance criteria

- A saved memory shows the success card exactly as `after/mem-5.png` (dark and light); View and Undo work with the
  permission rules above; a locked refusal shows the refused card with the reason and the attempted text; failures are
  never called "Deny".
- `scn-memory-tool` saves its memory on a mock-model stack (state how you proved it).

## Tests

Vitest: event → card mapping, Undo/Redo states (created, changed, changed since), permissions (no View/Undo without
access), Failed vs Deny. Playwright (mocked): a transcript with both events, both themes, 390 px, axe clean.

## Tests move with the UI (owner, 2026-11-20 — binding)

1. Before each commit that changes visible text, a badge, a role or structure in the chat: `rg -n "Deny|Used 1 tool|<old
   text>" web/tests web/e2e scripts/e2e` and update every hit **in the same commit** (chat layout tests, tool-group
   tests, approval specs that count "Deny" badges, `web/e2e/**` mocked and production specs, `scripts/e2e/llm*.sh`,
   `tests/e2e-hosted/tests/llm.rs` if the scenario changes).
2. New expectations use the mockup's words exactly; never loosen, skip, `fixme` or delete a test to get green.
3. Run through `scripts/dev/test.sh` only (D-491): vitest files you changed and those rendering the chat; the chat is
   shared UI, so the **whole** vitest suite once (`scripts/dev/test.sh <lane> web vitest`); every Playwright spec that
   visits a chat (`--project=mocked`); lint, typecheck, build through `scripts/dev/heavy.sh <lane> …`.
4. Production specs: update by reading; list them. 5. A merge that turns master red with a stale test comes back here.

## Docs to update

`14` (Chat: memory cards, Failed vs Deny), `10` §Memory (what the person sees), PROGRESS and FEATURES rows.

## Suggested agent/model and reason

Sonnet `nextjs-developer` (chat rendering, permissions in the UI, one mock fix).

## Completion report

Commits; screenshots beside `after/mem-5*.png`; the cause of the review's failed save and the fix; tests changed;
commands and results.
