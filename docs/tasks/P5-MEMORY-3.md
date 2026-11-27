# P5-MEMORY-3: A memory's history you can read: names, context, a real diff, Restore (owner, 2026-11-27)

## Goal

Today History opens below the form as raw rows ("#2 (latest) - bob@example.test (user, api) - 1 min ago"), with no
diff, and "Remove content of #2" disabled without a reason (M-5). Replace it with the review's **mem-4**: History
**replaces the reading view** (with "← Back to the memory"), a **timeline** that names people and context, and for the
selected version a **diff** against the one before, with **Restore this version** and **Remove this version's text…**.

The diff is the **shared `DiffView`** (`web/src/components/ui/diff-view.tsx`, `web/src/lib/line-diff.ts`, built by
FIX-250 for the template review and already used by the skill-library update review). **Do not build another diff
component** (owner's comment on the review, 2026-11-27: "make sure you don't reimplement the component to view these
diffs, or you create one shared one"). If `DiffView` lacks something the mockup needs (a "+3 added · −2 removed in this
version" summary, a compact header), extend it there, for every caller.

Decisions: **D-496**, **D-497(d)** (names and "via", never e-mails). Contracts: **C-1170** (revision fields), **C-1177**
(`&history=1` deep link).

## Linked requirements (look at every image before coding)

- `docs/design/skills-memory/README.md` (binding), `review.md` M-5 and proposal mem-4.
- **After:** `after/mem-4.png` / `-light.png` / `.html`: "Back to the memory · History · Aurora migration project · 3
  versions", rows "**Bob Builder** changed it · 2 min ago · **Current** · through an API key", "**Daneel** changed it ·
  yesterday · in “Release prep”", "**Bob Builder** created it · 25 Nov"; "**What changed** · Compared with the version
  before (Daneel, yesterday)"; **Restore this version**, **Remove this version's text…**; the diff with line numbers;
  "+3 added · −2 removed in this version".
- **Before:** `before/M05-memory-history.png`.
- `docs/14-web-dashboard.md` page pattern **Diff** (~1222) and the confirm ladder (~162–215).

## Wave and order

After **P5-MEMORY-2** (same files, same lane) and **P5-MEMORY-1** (the revision fields).

## Context

- Today: `RevisionHistory` in `web/src/components/memory/memory-tab.tsx:~357`; hooks `useMemoryRevisions`,
  `useRestoreMemory`, `useRedactRevision`, `canRedact` in `web/src/lib/api/memory.ts`. Routes: `GET
  …/memories/{key}/revisions` (not open to emergency readers), `POST …/restore {revision}`, `POST
  …/revisions/{rev}/redact` (422 `cannot_redact_latest`).
- After P5-MEMORY-1 each revision has `author` (`MemoryActor`), `via`, `restored_from`, `redacted`, `content` (null when
  redacted). 50 revisions are kept.

## Scope

1. "…" › **History** in the reading view (P5-MEMORY-2) opens the History panel **in place of** the reading view, with
   "← Back to the memory"; `&history=1` deep-links to it.
2. **Timeline:** newest first; each row an avatar initial, the actor's display name (or the key's name, the agent, the
   template), the verb (**created it**, **changed it**, **restored version n**, "removed the text of version n"), the
   time (`When`), and the context: "through an API key", "in “<conversation>”" (or "in a conversation" when the
   reference is null), "from the template <name> <version>"; **Current** on the newest. The selected row is
   highlighted; the first selection is the newest.
3. **What changed:** "Compared with the version before (<actor>, <when>)"; `DiffView` of the previous version's content
   against the selected one (the first version against empty), "+n added · −n removed in this version". A redacted
   side shows "This version's text was removed" instead of a diff.
4. **Restore this version** (not on Current): restores through the existing route, then returns to the reading view
   with an undo toast ("Restored version n · Undo" restores the one before).
5. **Remove this version's text…** in its "…" (or as drawn): the confirm ladder's typed or two-step confirm for an
   irreversible action; disabled on the current version **with the reason shown** ("The current version can't be
   removed: change the memory first.").
6. Phone: the History panel is its own sub-screen with back to the memory.

## Out of scope

Any API change; history for emergency readers (route closed; hide the menu item in the read-only view).

## Constraints / file ownership

Owns the History part of `web/src/components/memory/**`; may extend `ui/diff-view.tsx`/`lib/line-diff.ts` for all callers
(then their tests: `web/tests/diff-view.test.tsx`, `line-diff.test.ts`, the fix-247/fix-250 specs). Tokens and shared
primitives only; both themes; 390 px without horizontal scroll.

## Acceptance criteria

- With the review's data (the two memories changed twice), History matches `after/mem-4.png` / `-light.png` at 1256 px
  and works at 390 px; no e-mail address anywhere; the diff is `DiffView`.
- Restore and remove work with their confirmations and reasons; the old `RevisionHistory` is gone.

## Tests

Vitest: timeline wording per actor kind and via, "in a conversation" fallback, diff of first/middle/redacted versions,
Restore with undo, the disabled remove with its reason. Playwright (mocked): History open → select → diff → Restore,
both themes, 390 px, axe clean.

## Tests move with the UI (owner, 2026-11-20 — binding)

1. Before each commit that changes visible text, a label, an `aria-label`, a role, a heading, a route or structure:
   `rg -n "<old text>|<old selector>" web/tests web/e2e scripts/e2e` and update every hit **in the same commit**
   (`web/tests/memory-skills.test.tsx` restore/redact ~42–68, `memory-one-box.test.tsx`, the diff tests if you extend
   `DiffView`, `web/e2e/**` mocked and production specs, `support/mock-api.ts`).
2. New expectations use the mockup's words exactly; never loosen, skip, `fixme` or delete a test to get green.
3. Run through `scripts/dev/test.sh` only (D-491): `scripts/dev/test.sh <lane> web vitest <files>` (the whole suite once
   if you change `DiffView`), `scripts/dev/test.sh <lane> web playwright <spec> --project=mocked` for every spec that
   visits Memory or renders `DiffView` (fix-247, fix-250, skill-libraries); lint, typecheck, build through
   `scripts/dev/heavy.sh <lane> …`.
4. Production specs: update by reading; list them. 5. A merge that turns master red with a stale test comes back here.

## Docs to update

`14` (Memory › History; `DiffView` if extended), PROGRESS and FEATURES rows at every commit.

## Suggested agent/model and reason

Sonnet `nextjs-developer` (one panel, reuse of a shared component).

## Completion report

Built 2026-11-27 on `alexiri/p5-skm-web2` after merging master (`6b56bb21`) (one commit, "P5-MEMORY-3: …"). Not
pushed. Web only: no API change, shapes from `schema.d.ts`.

### What changed

- New `components/memory/memory-history.tsx`; `revision-history.tsx` and the `RevisionHistory` export are gone.
  `memory-words.ts` gains the version's actor, verb and context; `memory-tab.tsx` shows History **in place of** the
  reading view (only the detail column's branch changed; P5-MEMORY-4's mount step and `memory-editor.tsx` are untouched);
  `memory-view.tsx` loses "Hide history".
- **Shared `ui/diff-view.tsx`** (approved by the coordinator, option A): an additive prop `wrap`, default `false`. With
  it, a line's cell is `whitespace-pre-wrap wrap-anywhere` instead of `whitespace-pre`. Nothing else in the component
  changed, `lib/line-diff.ts` is unchanged, and only History passes it, so the other callers render as before. Without
  it, in this column the diff scrolled 189 px sideways and the fold row's buttons were cut off.
- "+n added · −n removed in this version" is counted outside `DiffView`, from `buildDiff` (`changeCounts`).

### Readings of the brief to check

1. **"removed the text of version n" is not a row's verb.** A removal writes no version and the API gives neither who
   removed the text nor when, so naming someone would be a guess. The removed version keeps its own verb (the person
   who wrote it did write it) and its context ends "its text was removed".
2. "created it" only for version 1: after pruning, the oldest kept version "changed it" and says "The version before is
   no longer kept, so there is nothing to compare with." (not drawn).
3. Name: the person behind a person, API key or template write (mem-4: "Bob Builder changed it · through an API key");
   else the agent, the key or the template; "Someone" for an unknown writer. A file the agent wrote on its computer
   (`via: harvest`) has no context line: it was not a conversation.
4. **Remove** is the two-step confirm (menu, then a danger dialog), as deleting a memory is; the brief allows typed or
   two-step.
5. **Undo** restores the version that was current before, as one more version; if that fails a toast says "Couldn't
   undo the restore: <reason>". Nothing else is undone by it (the lock never changes).
6. Not drawn, worded here: "The first version", "Remove the text of version n?" and its sentence, "Remove text".

### Comparison with the mockups

Looked at `after/mem-4{,-light}.png` and the HTML before coding and again against the built panel at 1256 and 390 px in
both themes (mocked data). Not committed; `MEMORY_SHOTS=<dir> … web playwright e2e/memory-history.spec.ts` writes
`mem-4-{1256,390}-{dark,light}` (↔ `after/mem-4{,-light}.png`) and `mem-4-menu-…` (the open menu).

| Difference kept | Reason |
|---|---|
| No **Restore this version** while **Current** is selected (the mockup draws it there) | Scope 4: not on Current |
| The menu on Current is disabled and carries the reason | Scope 5 |
| Times are "2 min ago", then "26 Nov, 19:47" ("Nov 26" in the test browser's locale), not "yesterday" / "25 Nov"; the same inside "Compared with the version before (…)" | Scope 2: the time is `When` (`14` §Dates) |
| The diff has two line-number columns, three lines of context with a folded "n unchanged lines" row and its buttons, word highlights, and a **Show the whole file** switch | The shared `DiffView` as it is (D-496, owner: no second diff component; mem-4's text: "same line numbers, folded unchanged lines and word highlights everywhere"). The switch says "file" because it is that component's word |
| For version 1 the diff's header adds "(added)" | `DiffView`'s own mark for a text compared with nothing |
| On a phone, "‹ Back to the memory" is the sub-screen's header and **Current** can wrap under the time; the agent header stays | Scope 6; `SubScreenHeader`; the real shell |
| Initials are tinted by name | The shared `Avatar` |
| List data in the screenshots is one memory | The spec's mock; the list is P5-MEMORY-2's and unchanged |
| Shell, sub-nav entries, ink **New memory** | As in P5-MEMORY-2's report |

### Tests

- New: `tests/memory-history.test.tsx` (23), `e2e/memory-history.spec.ts` (8: 1256 and 390 px, both themes: open,
  select, diff, the disabled remove with its reason, Restore and Undo, remove after asking twice; axe serious+critical,
  no sideways scroll of the page **or of the diff box**, the three fold buttons inside the box). One `wrap` case in
  `tests/diff-view.test.tsx`.
- Changed with the old component: `tests/memory-skills.test.tsx` loses its `RevisionHistory` case (restore, redact with
  confirmation and "disabled on latest" are in `memory-history`; `canRedact` stays); `tests/memory-page.test.tsx` opens
  the History panel instead of the "Revision history" region.
- No production or real-stack spec visits Memory: **none to run nightly**.

### Commands and results

- Fail first: `memory-history` before the panel existed: **22 failed, 1 passed** (the emergency reader never saw
  history). The `wrap` unit case and the diff-box check before the prop: failed (189 px of sideways scroll, 4 runs).
- `test.sh web vitest`, the files that render `DiffView` (directly or through the review dialogs and skill libraries)
  and the Memory ones, 15 files: **243/243**. The whole suite was not run (the coordinator's gate; CI runs it).
- `test.sh web playwright … --project=mocked`: `memory-history` **8/8**, `memory` **14/14**,
  `fix-247-template-review-diff` **4/4**, `agent-tabs` **13/13**. `fix-250` and `skill-libraries` were not run
  (the gate asked for one `DiffView` spec).
- `pnpm typecheck`, eslint on touched files, `cargo fmt --all --check`, `scripts/guards/run-all.sh`,
  `extract-schema.py --check`: all clean on the final tree. `pnpm build` not run.

### Drift guards (gate addition, after the first commit)

`audit-actions`, `arbitrary-values`, `ui-text`, `labels`:
- `arbitrary-values` failed on `border-l-[3px]`, in the History rows and in `memory-list.tsx` (merged with
  P5-MEMORY-2). Master had meanwhile fixed its own with the `row-rail` utility; after merging master both places use
  `row-rail`.
- `audit-actions` failed before that merge on `skill.agent_off` and `skill.agent_on` (P5-SKILLS-1, not this lane's);
  the result after the merge is below. `ui-text` and `labels` pass. No Rust touched, so no clippy.

### Merge of master (P5-MEMORY-4, before handing over)

Conflicts in `memory-list.tsx` (the row class, master's `row-rail` taken) and the PROGRESS and FEATURES rows (both
lanes' states kept). `memory-tab.tsx` and `memory-editor.tsx` merged cleanly: P5-MEMORY-4's chat-draft mount step and
`draft` prop are as merged, beside History's branch of the detail column. After the merge: vitest
(the four drift guards, the Memory and chat-memory files, `diff-view`, design and copy guards, 13 files) **152/152**,
`audit-actions` included (fixed on master meanwhile); mocked Playwright `memory-history`, `memory` and
`chat-memory-cards` **38/38**; `pnpm typecheck` and eslint clean.

### Security notes

No SEC row. Names are the API's display names (never an e-mail; tested); a conversation is named only from its reference
and title and is never linked; an emergency reader is shown no History and no revisions are requested; version texts
reach the page only through `DiffView` as text.
