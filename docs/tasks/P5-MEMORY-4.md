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

Built 2026-11-27 on `alexiri/p5-memory-4` over master `6b56bb21` (one commit, "P5-MEMORY-4: …"). Not pushed.

### What was built

- `lib/chat/layout.ts`: a `memory` block for `memory.saved` / `memory.refused`, under its turn's tool group. A later
  call of the same turn joins the group above the cards; a card whose event overtook its own `tool.started` gets the
  group put in front of it (P5-MEMORY-1, decision 5). `toolGroupSummary` has `hasDeny` (a `DENY` decision only) and
  `hasFailed`; a failed call is no longer read as denied (nor an after-turn one, nor one with an unknown decision word).
- `lib/chat/transcript.ts`: both types added to the stream's event names (without this they never arrive live).
- `components/chat/memory-card.tsx` (new), wired in `message-group.tsx`; `tool-call-group.tsx` shows Deny / Failed.
- `lib/chat/memory-draft.ts` (new): the in-memory hand-over for **Save it yourself**.
- With the coordinator's go-ahead, in `components/memory` (P5-MEMORY-3's lane; the second to merge resolves it):
  `memory-tab.tsx` opens the form from a waiting draft and `memory-editor.tsx` takes an optional `draft`. Additive;
  without a draft nothing changes. A taken key is dropped; an edit draft for a memory that is gone opens the new form.
- Shared UI: `BookmarkIcon`; `Button` variant `pending`.

### Why the review's save failed, and the fix

The platform tools are on demand by default (`hosted.platform_tools = on_demand`, with `aurora-agent` in the image):
the MCP bridge lists only the core tools. `scn-memory-tool` called `mcp__aurora__memory_save` directly, so Claude Code
answered `No such tool available: mcp__aurora__memory_save` (the "Failed" call) and the scenario then said `done`. The
product is as designed; the **scenario** was wrong. It now looks at the request's tool names: with the MCP tool listed
(the conformance stub bridge, or `platform_tools = all`) it calls it as before, otherwise it runs
`aurora-agent call aurora memory_save key=conformance content='…'` through `Bash`.

**How it was proved** (no full stack was started: no kind cluster, no database, so a stored row on a mock-model stack
was **not** observed):

1. Pinned Claude Code 2.1.319 → pinned VidaiMock serving master's scenario, with a bridge listing only
   `complete_task`: `tool_result` `No such tool available: mcp__aurora__memory_save`, then `done`. Reproduced.
2. The same with the new scenario, the real `aurora-agent` (this tree's `aurora-supervisor agent`, under `bwrap` so
   `/run/aurora/tools.sock` could be a recording fake): the socket got `call` `memory_save`
   `{"key":"conformance","content":"Saved by the conformance run."}` and the tool result was not an error.
3. VidaiMock, curl, both tool lists, streaming and not: the expected `tool_use` and joined input.
4. From there the path is the server's, covered by P5-MEMORY-1's `memory::` tests (not rerun here).

`scripts/e2e/llm-smoke.sh` has a new check for the on-demand branch. Neither it nor `llm.sh` was run (they need
LiteLLM). `tests/e2e-hosted/tests/llm.rs` needs no change by reading (the Bash summary contains `memory_save`, the
stored content is the same); CI does not run it, and it was not run.

### Decisions (accepted by the coordinator, 2026-11-27)

- **Redo of a created memory creates it again** (`POST`, same key, title, description, content, lock), because Undo
  deletes it and a delete removes its revisions: there is nothing to restore. It then reads as added by that person.
  A key taken meanwhile reads **Changed since**. Changed memories use restore, both ways.
- **The Memory page edit** above, with the draft never in the address or storage (pinned in `memory-chat-draft`).

### Choices the brief left open (please check)

1. **Dismiss is local state** (back after a reload), as the brief says. I do not think it needs to persist: the card is
   one line once folded, and persisting it per viewer needs storage or a server field.
2. **Undone is local state too.** After a reload an undone card reads **Changed since**, disabled, with "This memory was
   deleted after this was saved." or "…changed…": the page does not guess that it was this viewer's Undo.
3. **No compare-and-set.** The memory routes take no expected revision. The card reads the memory again at the click and
   stops if it moved; an edit in the milliseconds between that read and the write is not detected. A restore keeps the
   overwritten text in History; a delete does not. Closing this needs an API precondition (not in this brief).
4. A memory remade under the same key restarts at revision 1, so the card also compares `created_at` (with the event's
   time until it wrote the memory itself).
5. **Who may:** own access to the workspace (`my_role`) reads and edits; an emergency reader gets View only and the card
   makes **no** memory request for them (those reads are recorded). The server checks every request. **View** is hidden
   once the memory is known to be gone. The refused card has no View (not in the mockup).
6. The preview is the first 200 characters of what the agent sent (C-1172), so for a long text **Save it yourself**
   starts with only that much; on a locked memory it is appended even if it repeats text already there.

### Comparison with `after/mem-5.png` / `-light.png`

Captured by the mocked spec (`AURORA_E2E_SHOTS=<dir>`; the conversation log in a tall window at 1280 px and 390 px,
dark and light) and compared before coding and before handing over. Icon, tile, eyebrow, title, line, buttons, amber
card, preview box, words and the 570 px width match. Differences kept:

- The tool group stays above each card (brief, scope 1); the mockup draws none.
- Bubbles, avatar, names, times, the pane around them and the "answered and used 1 tool" banner are the app's own.
- Text is the app's scale (14 px where the mockup has 13 px), so the refusal sentence wraps before "add:".
- Radii and colours are the app's tokens; the card in dark sits on the canvas colour, like the tool group above it.
- At 390 px (no mockup) the actions go under the text and the preview wraps (one line from 768 px).
- States the mockup does not draw: "Undone · Redo", "Changed since" with its reason, and the folded refusal (**Show**).

### Tests

- New: `tests/memory-card.test.tsx` (events → cards, Undo/Redo for created and changed, changed since, a newer edit at
  the click, failure, permissions, Save it yourself, Dismiss); `tests/memory-chat-draft.test.tsx` (the Memory page);
  `e2e/chat-memory-cards.spec.ts` (both themes, 1280 and 390 px, axe, no horizontal scroll, View and Save it yourself
  through to the Memory page).
- Changed with the behaviour: `tests/chat-layout.test.ts` (Deny / Failed; an after-turn failed call is `TOOL`, it was
  asserted as `DENY`; memory blocks), `tests/chat-v2.test.tsx` (Failed in the summary; a new policy-deny case).
- Production specs: none shows the changed words (`rg` over `web/e2e` and `scripts/e2e`); none edited or run.

### Commands and results

- Fail first: `memory-card`, `chat-layout`, `chat-v2` before the code: **8 failed, 54 passed**, and `memory-card` did
  not load (no module). `memory-chat-draft` before the Memory page edit: **4 failed, 1 passed** (the "no draft" case).
- `test.sh web vitest`, 16 files (the new ones, memory, every chat file, `status-rail`): **250/250**. Guards
  `copy-guard`, `design-guards`, `person-name-guard`, `button-link-contrast`: **34/34**.
- `test.sh web playwright e2e/chat-memory-cards.spec.ts --project=mocked`: **16/16** (last run after the last edit).
- `pnpm typecheck`; eslint on the touched files: clean. `cargo fmt --all --check` 0; `guards/run-all.sh` passed;
  `extract-schema.py --check` up to date. `shellcheck -S warning llm-smoke.sh` clean (older info notes remain).
- The Orchestrator's drift guards (`audit-actions`, `arbitrary-values`, `ui-text`, `labels`): **21 passed, 2 failed,
  both already on the base commit in files this lane did not touch**: `arbitrary-values` →
  `components/memory/memory-list.tsx:126` (`border-l-[3px]`, P5-MEMORY-2); `audit-actions` → `skill.agent_off`,
  `skill.agent_on` have no entry (P5-SKILLS-1).
- No Rust changed, so no crate clippy or Rust test. Not run: whole vitest, other Playwright specs, `pnpm build`.

### Security notes

No SEC row. The cards render server text as plain text (no Markdown or HTML). They add no permission: every action is an
existing memory route with the person's own rights. Memory text goes to the Memory page in memory only, never into the
address, storage or a log.
