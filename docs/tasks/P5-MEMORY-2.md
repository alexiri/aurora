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

Built 2026-11-27 on `alexiri/p5-skm-web2` over `10d24f64` (one commit, "P5-MEMORY-2: …"). Not pushed. Web only: no
API change, response shapes from `schema.d.ts`, no migration.

### What changed

- `components/memory`: `memory-tab.tsx` (page state, search, deep link, leave gate), new `memory-list.tsx`,
  `memory-view.tsx`, `memory-editor.tsx`, `memory-words.ts` (source and meta lines), `revision-history.tsx` (moved
  unchanged; still exported from `memory-tab` for P5-MEMORY-3 to replace); `size-meter.tsx` gains `quiet`.
- Shared set: new `ui/markdown-editor.tsx` (**for P5-SKILLS-3**), `ui/list-detail.tsx`, `ui/markdown-marks.ts`;
  `Markdown` gains `marks` and `reading`; `CopyChip`; `HighlightedText tone="solid"`; `PencilIcon`;
  `formatAge`/`formatShortDay`; reading and match styles in `globals.css`.
- Outside the brief's file list, small: `agents/settings-shell.tsx` (a slot for **New memory** beside the heading, and
  hiding the heading and sub-nav on a phone sub-screen), `agents/agent-page.tsx` (no language prop; leaving Memory
  drops `memory`/`history`).
- The banner and the `memory.key` altitude row are gone. Settings › General already uses `memoryLanguageName()`.

### Readings of the brief to check

1. **"By you" and "Added by you"** include a memory added through the signed-in person's **own** API key (the review's
   seed adds everything that way and mem-1 says "Added by you · 2 days ago", "By you 11"; P5-MEMORY-1's data table says
   the same). "Added through <key name>" is another person's key.
2. Not drawn, worded here: "Added · …" (unknown), "Imported · …", "Added through an API key" (no key name, no person),
   "No memories match “q”", "No memories here." (an empty chip), "Nothing to preview yet.".
3. **Match i of n** counts the results ("1 memory matches" → "Match 1 of 1"), not the occurrences in the text.
4. The Locked switch of an existing memory still saves on its own (C-1150, unchanged); Save sends title, description and
   text only.
5. "Leaving asks as other settings pages do": it uses the Files editor's **Leave without saving?** gate (row, New, back,
   settings sub-nav, section tabs) plus the browser's own warning. As with Files, a link outside the agent page (the
   rail) does not ask.

### Comparison with the mockups

Looked at `after/mem-1`, `-2`, `-3`, `-6` (dark and light) before coding and again against screenshots of the built page
at 1256, 1280 and 390 px in both themes (mocked data shaped like `seed.py`, not a seeded cluster). The screenshots are
not committed; `MEMORY_SHOTS=<dir> scripts/dev/test.sh <lane> web playwright e2e/memory.spec.ts --project=mocked`
writes them again (`mem-1-1256-{dark,light}` ↔ `after/mem-1{,-light}.png`, `mem-2-edit-…` and `mem-2-new-…` ↔ `mem-2`,
`mem-3-…` ↔ `mem-3`, `mem-6-{list,memory,edit}-390-…` ↔ `mem-6`).

| Difference kept | Reason |
|---|---|
| Rail, agent header ("Needs you"), Tests / Other agents in the sub-nav, "Showing more controls" | The real shell (README §How to read, 2; D-496(c)). On a phone the agent header stays above both screens, and the list screen keeps "‹ Chat" and the sub-nav |
| **New memory** and **Save** are ink, not teal | The shared primary `Button` (`14` §Visual design) |
| The save bar floats at the bottom of the window, with no amber dot; it can lie over the page in a full-page screenshot | The shared `SaveBar` (scope 4; README 5). Room is left under the form so the last control can be scrolled clear of it |
| An unchanged memory in edit mode has **Done** | A state the mockup does not draw: the shared bar is hidden until a change, so something must leave the form |
| Chips are larger and wrap to two rows at 390 px (mockup: one clipped row) | The shared `ChoiceChips`; `14`: nothing hides in an inner scrollbar |
| The key chip shows in the reading view, at 1256 and 390 px | Scope 3 |
| The title in the reading view wraps to three lines at 1256 px (mockup: two) | The real rail and sub-nav leave the column about 400 px (D-496(c)) |
| Search: the matched word is also marked in result titles | Scope 7 (`title_highlights`) |
| Search: the heading above the first matching line stays visible (and is marked); the mockup's view starts at the line | The whole memory is shown and the page scrolls to the line only when it is out of view; no inner scrollbar. A matching text line is preferred over a heading for the highlighted line |
| Sub-screens below **1100 px**, not only below `md` (768) | Between 768 and 1099 px the rail and sub-nav leave 230–560 px: two columns do not fit, and stacking them is M-9 again |
| "Nov 25" in the screenshots | The browser's locale decides ("25 Nov" in en-GB; `14` §Dates) |
| A locked memory's meta line ends "Locked by <name> on <date>"; on a phone and for an emergency reader a **Locked** badge sits by the key | P6-MEMLOCK's information, otherwise lost where there is no Unlock button; the mockups draw no locked memory open |
| New-memory form: the hint is under the tabs | Same column width as editing; mem-2's lower form is drawn wider |

### Tests

- New: `tests/memory-page.test.tsx` (38), `tests/markdown-editor.test.tsx` (4), `e2e/memory.spec.ts` (14: 1256 and
  1280 px list/read/edit/save, search "rotation", new memory; 390 px two screens; both themes; axe serious+critical and
  no sideways scroll), `e2e/support/memory.ts` (the shared memory routes).
- Removed with its feature: `tests/memory-one-box.test.tsx` (the one-box editor, "Lock it", the expert key,
  `memoryPreview`). Its still-true cases (key rules, 409 retry, size limit, lock switch and "Locked by", `locked` only
  when asked) are in `memory-page.test.tsx`.
- Changed: `e2e/agent-tabs.spec.ts` "locked memories" (lock icon in the list, **Lock** in the reading view, then the
  switch in **Edit**; uses the shared routes).
- Read, no change needed: `agent-empty-states`, `memory-skills`, `agent-page-header`, `dashboard-links`; no production
  or real-stack spec visits the Memory page, so **no production spec to run nightly** for this brief.

### Commands and results

- Fail first: the two new vitest files on `10d24f64`'s code (a temporary detached worktree): **35 failed, 3 passed**
  (38 in `memory-page`; the three are key rules that did not change), and `markdown-editor.test.tsx` could not import.
  The new Playwright spec was not run on the old code.
- `test.sh web vitest`, every file that renders or reads a changed component: **39 files, 1150/1150**; after the last
  `Markdown` change (conformance cases gfm279/280 caught a first version that changed every task list), 17 files
  **908/908**. The whole suite was not run (the dispatch's minimal gate; CI runs it).
- `test.sh web playwright e2e/memory.spec.ts --project=mocked` **14/14** on the final tree;
  `e2e/agent-tabs.spec.ts` **13/13** (run before that last change, which only touches the reading view).
- `pnpm typecheck` and eslint on the touched files: clean. `cargo fmt --all --check` 0, `scripts/guards/run-all.sh`
  passed, `extract-schema.py --check` up to date. `pnpm build` not run (minimal gate).

### Security notes

No SEC row. A conversation is named only from the API's `ConversationRef` and its title; null or untitled reads "in a
conversation"; nothing on the page links to a conversation (tested). Snippets and search marks are text: `HighlightedText`
for rows, and a Markdown step that only splits text nodes after sanitizing; no HTML is built from server strings. The
emergency reader's view calls neither search nor history.
