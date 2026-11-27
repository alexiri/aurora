# P5-MEMORY-2: The Memory page: a calm list, a reading view, editing that fits, search that shows the match, a phone that opens one memory (owner, 2026-11-27)

## Goal

Rebuild the agent's **Settings › Memory** page to match the review's mockups **mem-1, mem-2, mem-3 and mem-6**:

- a list that uses the width, with filter chips, recent-first order and a source line per memory (M-3, M-7);
- a **reading view**: the selected memory rendered as Markdown, with Edit, Lock and a "…" menu, the key, and a line
  saying who added it, how often it changed and when the agent last used it (M-2, M-10);
- **editing that fits**: Title, Key, Description, a tall Write/Preview editor, one Locked switch, the shared save bar
  (M-2, M-4, M-8), and on create a **Key** field filled in from the title and editable until the first save;
- **search that shows the match**: highlighted snippets, a count and Clear, the first match opened at the highlighted
  line (M-1, a bug today);
- **a phone** where the list and the memory are two screens (M-9).

Decisions: **D-496** (precedence; mem-1's designer's notes are not adopted: Edit · Lock · "…" as drawn, the Settings
sub-nav stays), **D-497** (source lines; conversation names only for readers). Contracts: **C-1169, C-1171, C-1177**
(the `?memory=<key>` deep link).

## Linked requirements (look at every image before coding)

- `docs/design/skills-memory/README.md` (how to read a mockup — binding), `review.md` §Memory: M-1, M-2, M-3, M-4, M-7,
  M-8, M-9, M-10 and proposals mem-1, mem-2, mem-3, mem-6.
- **After:** `after/mem-1.png` / `-light.png` / `.html` (list + reading view), `after/mem-2.png` / `-light.png` /
  `.html` (edit mode with the key chip; below it the **new-memory** form with the Key field), `after/mem-3.png` /
  `-light.png` / `.html` (search "rotation"), `after/mem-6.png` / `-light.png` / `.html` (two phone screens).
- **Before:** `before/M01-memory-list.png`, `M02-memory-list-full.png`, `M03-memory-open.png`, `M04-memory-open-full.png`,
  `M06-memory-search.png`, `M07-memory-locked.png`, `M08-memory-new.png`, `M09-memory-untitled.png`,
  `M11-memory-expert.png`, `M12-memory-technical.png`, `P01-phone-memory.png`, `P02-phone-memory-open.png`,
  `P02b-phone-memory-open-bottom.png`.
- `docs/14-web-dashboard.md` §Visual design (lines ~90–1241: shared patterns ~162–215, components ~391–492, page
  patterns ~1210–1241 incl. "Search results: server-highlighted plain-text snippets, never HTML"), the Memory spec
  (~2787–2795), vocabulary (~1306).

## Wave and order

Starts when **P5-MEMORY-1** is merged (it provides `added_by`, `changed_by`, `last_used`, `highlights`).
P5-MEMORY-3 (History) follows in the same lane and the same files. P5-SKILLS-3 reuses the **Write/Preview editor** this
brief adds to the shared set, so merge it early in its own commit if P5-SKILLS-3 is waiting.

## Context (what exists today)

- Everything is in `web/src/components/memory/memory-tab.tsx` (421 lines): `MemoryTab` (empty state, the language
  banner at ~78, search `#mem-search`, two columns `md:layout-aside-lg`), `MemoryView` (read-only, plain text),
  `MemoryEditor` (140 px textarea, title, "Locked" switch on edit vs "Lock it" checkbox on create, `FormActions`, not
  `SaveBar`), `RevisionHistory` (P5-MEMORY-3 replaces it). Expert-only technical details (`memory.key`,
  `altitude-registry.ts:30`). Helpers `size-meter.tsx`, `web/src/lib/memory-key.ts` (key from title; 409 retries with a
  suffix). Client `web/src/lib/api/memory.ts` (no `getMemory` hook; search runs on every keystroke). Mounted from
  `web/src/components/agents/agent-page.tsx:~319` with `readOnly={emergency}`.
- Selection is local state; there is no deep link to one memory. On a phone the editor renders under the list.
- The banner capitalizes the stored value ("En", "Simple"); the language setting lives in Settings › General
  (`general-tab.tsx:87-94`, `memoryLanguageName()` in `web/src/lib/nav/landing.ts:67`).
- Primitives to use: `Markdown` (`ui/markdown.tsx`), `SaveBar`/`useSaveBar` (`ui/save-bar.tsx`), `SwitchField`,
  `ChoiceChips`, `Menu`/`RowMenu`-style "…", `useCopy`/`CopyValue`, `HighlightedText` (`ui/highlighted-text.tsx`),
  `SkeletonRows`, `When`, `ConfirmDialog`, `useUndoToast`. No Write/Preview primitive exists (the chat composer has one
  inline, `chat/composer.tsx:559-571`); no phone sub-screen primitive (the chat's `ChatPanel` with `threadOpen`, the
  Files viewer's "Back to folder" are the patterns).

## Scope

1. **List (mem-1).** Search on top; chips **All n · By you n · By <Agent> n · Locked n** (from `added_by`; "By you" =
   added by the signed-in person, any other person or key counts in All only); "Sorted by **Recently changed**" with
   the alternative **Name**; rows: title (untitled → the first line of the content, M-9 of today shows the whole
   content), the description or *No description*, the source line from D-497: **Added by you · 2 days ago**,
   **Added by <Person> · …**, **Added through <key name> · …**, **Learned by <Agent> in “<conversation>” · today**
   (no title → "in a conversation"), **From the template <name> · …**; a lock icon for a locked memory; a dot for
   unsaved changes. The first memory (by the sort) is selected on arrival; there is no empty "Select a memory" column.
2. **The language banner goes** from this page (M-7). The language stays editable in Settings › General; fix that
   page's label to use `memoryLanguageName()` if it shows the raw value.
3. **Reading view (mem-1).** Title, the description under it, the **key chip** (`aurora-migration` in mono with a copy
   button, exactly as in `after/mem-2.png`, here under the title too — the mem-2 proposal says "in reading and edit
   mode"), actions **Edit**, **Lock** (Unlock when locked) and "…" (**History**, **Delete**). The content rendered with
   `Markdown` (headings, checklists as ticked boxes, tables). The meta line at the bottom: **Added by <name> on <date> ·
   Changed <once|twice|n times> · <Agent> last used it in “<conversation>” <when>** (or "<Agent> hasn't used it yet").
   Remove the expert-only "Technical details · Memory key" (M-10) and its altitude row.
4. **Edit mode (mem-2)** in the same column: **Title**; the key chip (read-only, copy) with "<Agent> finds the memory by
   this name. It can't be changed."; **Description** with "<Agent> sees the title and this line first and opens the
   memory only when it fits."; **What should <Agent> remember?** in the new **Write/Preview editor** (below), tall
   (fills the column, about 400 px at 1256 px), the hint "Markdown works: # headings, - lists, - [ ] checklists, tables";
   the size as quiet text "476 of 100,000 characters" with a thin meter; **Locked** switch with "<Agent> can read this
   memory but can't change or delete it. You can still edit it."; the shared **SaveBar** ("Unsaved changes · Discard ·
   Save").
5. **New memory (mem-2, lower form).** Same form, same switch, same bar ("New memory · not saved yet"). Under Title a
   **Key** field: filled from the title as the person types (`where-the-runbooks-live`, the existing key rules and
   `newMemoryKey`), editable (lowercase words joined by dashes; validate as the server does), "Made from the title" while
   untouched, with "<Agent> finds the memory by this name. Change it now if you like: lowercase words joined by dashes.
   It can't be changed after saving." A 409 shows "A memory with this key already exists" on the field (no silent
   suffix once the person edited the key).
6. **Write/Preview editor — add it to the shared set.** `web/src/components/ui/markdown-editor.tsx`: tabs Write/Preview,
   a monospace textarea that grows to its container, the hint line, `Markdown` for Preview, accessible tab semantics,
   both themes. Use it here; P5-SKILLS-3 uses it for skill instructions. Document it in `14` §Components.
7. **Search (mem-3).** Debounced (about 250 ms). Above the results "**1 memory matches “rotation”**" (n memories) and
   **Clear**; snippets with `HighlightedText` from `highlights` (never HTML); the title highlighted from
   `title_highlights`; the first match selected and its reading view scrolled to the first highlight, with a small
   "Match 1 of n" line as drawn. Clear restores the list and the previous selection.
8. **Deep link (C-1177).** `?tab=memory&memory=<key>` selects that memory (and is what the list writes when a row is
   chosen, with `replace`), so a reload keeps it and P5-MEMORY-4's "View" can open it.
9. **Phone (mem-6, M-9).** Below `md`: the list is one screen; tapping a row opens the memory as its own screen with
   "‹ Memory" back, the reading view and **Edit**; Lock and History at the bottom as drawn; Edit opens the editor
   full-width with the save bar. Add a small reusable list/detail sub-screen helper to the shared set if none fits
   (document it).
10. **Read-only (emergency) view** uses the reading view without Edit, Lock, "…", search (its route is not open to
    emergency readers) and the editor.

## Out of scope

History (P5-MEMORY-3); the chat card (P5-MEMORY-4); any API change (P5-MEMORY-1); moving the Settings sub-nav (D-496(c)).

## Constraints / file ownership

Owns `web/src/components/memory/**` (except `RevisionHistory`, which P5-MEMORY-3 replaces — leave it working),
`web/src/lib/memory-key.ts`, `web/src/lib/api/memory.ts`, the new `ui/markdown-editor.tsx` and sub-screen helper,
`altitude-registry.ts` (remove `memory.key` only). Tokens and shared primitives only (`14` §Visual design); both themes;
no horizontal scroll at 390 px.

## Acceptance criteria

- With the review's data (`seed.py`), the page at 1256 px matches `after/mem-1.png` / `-light.png` (list, chips,
  source lines, reading view, meta line), `after/mem-2.png` / `-light.png` in edit and create, `after/mem-3.png` for
  "rotation" (no `<b>` anywhere), and at 390 px matches `after/mem-6.png`. Remaining differences are listed in the
  report with the brief or decision that requires them.
- The key is visible in reading and edit mode with a working copy button, settable on create, fixed after.
- Create and edit share one form, one Locked switch and the SaveBar; leaving with unsaved changes asks as other
  settings pages do.
- No "Select a memory" column, no language banner, no expert-only key.

## Tests

Vitest: list chips and counts, source-line wording per actor kind (incl. "in a conversation" when `conversation` is
null), reading view (Markdown rendered, meta line), edit/create (key derived, editable, 409 on the field), SaveBar,
search highlights from ranges, deep link, read-only view; the new `MarkdownEditor`. Playwright (mocked): one spec walking
list → read → edit → save, search "rotation", and the phone screens at 390 px in both themes (axe clean).

## Tests move with the UI (owner, 2026-11-20 — binding)

1. Before each commit that changes visible text, a label, an `aria-label`, a role, a heading, a route or `?tab=`, a
   page's structure: `rg -n "<old text>|<old selector>" web/tests web/e2e scripts/e2e` and update every hit **in the
   same commit**: `web/tests/memory-one-box.test.tsx`, `memory-skills.test.tsx`, `agent-empty-states.test.tsx`,
   `agent-page-header.test.tsx`, `dashboard-links.test.tsx`, the export/import/template tests; `web/e2e/agent-tabs.spec.ts`
   (locked memories, 390 px), `save-template.spec.ts`, `support/mock-api.ts` (add memory routes there rather than per
   spec where several specs need them); production specs.
2. New expectations use the mockup's words exactly; never loosen an assertion to accept old or new; never skip, `fixme`
   or delete a test to get green.
3. Run through `scripts/dev/test.sh` only (D-491): `scripts/dev/test.sh <lane> web vitest <files>` for every file you
   changed and every file rendering a component you changed; this brief adds a shared primitive, so run the **whole**
   vitest suite once (`scripts/dev/test.sh <lane> web vitest`); `scripts/dev/test.sh <lane> web playwright <spec>
   --project=mocked` for every spec visiting the Memory page. `pnpm lint`, `pnpm typecheck` and `pnpm build` through
   `scripts/dev/heavy.sh <lane> …`.
4. Production specs: update by reading; list them in the report.
5. A merge that turns master's CI red because a test expects the old UI goes back to this lane.

## Docs to update

`14` (Memory page spec, the new `MarkdownEditor` and sub-screen helper in §Components, the altitude row removed),
`API_CONTRACTS.md` C-1177 (the deep link, as built), PROGRESS and FEATURES rows at every commit.

## Suggested agent/model and reason

Sonnet `nextjs-developer`: a multi-part page rebuild with a new shared primitive.

## Completion report

Commits; screenshots at 1256 px (dark, light) and 390 px beside the mockup paths; every remaining difference with its
reason; test files changed and why; commands and results; production specs to run nightly.
