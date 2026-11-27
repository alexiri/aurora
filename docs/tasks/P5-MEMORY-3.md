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

Commits; screenshots (1256 dark/light, 390) beside `after/mem-4*.png`; differences with reasons; tests changed; commands
and results.
