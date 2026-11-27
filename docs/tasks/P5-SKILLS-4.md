# P5-SKILLS-4: Adding a skill from a library: libraries on the left, only what can be added, a preview, then Add (owner, 2026-11-27)

## Goal

Today's picker lists the 8 skills that cannot be added (in red, some "about 0 characters") among the 11 that can, and
shows nothing of a skill before adding it (S-10). Implement the review's **skill-5**: a dialog with the **libraries on
the left** (each with its skill count, and **Add a library**), the **addable skills** on the right with search, the
unavailable ones **folded** into "**n can't be added · Why?**", and a **preview** of the selected skill (rendered
instructions, files) with **Add to <Agent>** (or <Workspace>/<Org> on those pages).

Decisions: **D-496**, D-498 (display names). Contract: **C-1176** (`blocked` per tier, `budget`, `skills_offered`, from
P5-SKILLS-1).

## Linked requirements (look at every image before coding)

- `docs/design/skills-memory/README.md` (binding), `review.md` S-10 and proposal skill-5.
- **After:** `after/skill-5.png` / `-light.png` / `.html`: dialog "**Add a skill from a library** · Skills are pinned to
  the version you see here."; left "Libraries · Anthropic skills 17 skills · Acme team recipes 5 skills · Platform tools
  9 skills · Add a library"; right: search, rows "Algorithmic art · Create generative art with p5.js, … · 4 files · 19
  KB", an "Added" tag on a skill already added (pdf), "8 can't be added · Why?" folded at the bottom; the preview of the
  selected skill ("Brand guidelines · Use it to apply …", rendered instructions, files SKILL.md 1.9 KB, LICENSE.txt 0.3
  KB, "Pinned to e4bb5d9 when added."); footer "Adds a copy to Daneel. You can turn it off or remove it later." · Cancel ·
  **Add to Daneel**. The page behind the dialog is illustrative.
- **Before:** `before/L07-add-from-library.png`, `L08-library-skill-preview.png`, `S08-agent-skills-after-library.png`.
- `docs/14-web-dashboard.md` dialogs (top-anchored, sizes; ~162–215), org Skills and Libraries (~1766–1812).

## Wave and order

After **P5-SKILLS-1** (C-1176) and **P5-SKILLS-3** (the Add skill menu opens this dialog; same `skills/` folder). Can
overlap with P5-SKILLS-3's last commits if file ownership is respected.

## Context (what exists today)

- `web/src/components/skills/library.tsx`: `AddFromLibrary` (~378), `LibraryBrowserDialog` (~416: picker, search,
  list, preview, Add), `AddAction` (~314), `activeLibraries` (~368). Client `web/src/lib/api/skill-libraries.ts`
  (`useLibraryCatalogue`, `useLibrarySkill`, `LIBRARY_BUDGET` hard-coded ~22, `shortSha`). Browse route `GET
  /v1/skill-libraries/{id}/skills?q=&cursor=&org=` → `LibrarySkillItem {path, name, description, valid, reason, size,
  files, added}`; after P5-SKILLS-1 also `blocked` and `budget` for the tier asked (`?tier=agent&agent=…`).
- Members see only a library's `id`, `label`, `status`, `scope`.

## Scope

1. **Dialog layout** as drawn (`Dialog`, size 2xl, top-anchored): header and its one-line explanation; left column the
   active libraries with "n skills" (C-1176 `skills_offered`), the selected one highlighted, **Add a library** for people
   who may add one (opens P5-SKILLS-2's Add a library dialog or links to the Libraries tab), nothing for others.
2. **Right column:** search (server `q`); the addable skills (display name per D-498(b), description, "n files · size");
   a skill already at this tier shows **Added** (not addable again); skills with `blocked` or `valid: false` are **not
   in the main list**: they fold into "**n can't be added · Why?**", which opens to rows with the reason in plain words
   ("A skill named pdf is already on Daneel.", "It would exceed Daneel's 32 KB of library skills.", the existing
   validity reasons).
3. **Preview** of the selected skill (`useLibrarySkill`): the "Use it to …" line, the instructions rendered with
   `Markdown`, the files with sizes (each viewable read-only), "Pinned to <sha> when added.".
4. **Footer:** "Adds a copy to <Agent>. You can turn it off or remove it later." (workspace/org: "Adds it to <Workspace|Org>'s
   skills …"), Cancel, **Add to <Agent|Workspace|Org>** (disabled with the reason when blocked); success closes the
   dialog, selects the new skill in the list and opens its panel; a 409/422 from the server shows on the preview in plain
   words.
5. **Budget:** use the server's `budget` (C-1176); remove `LIBRARY_BUDGET`'s hard-coded numbers.
6. Phone: libraries become a select above the list; the preview is a sub-screen with "‹ Skills"; no horizontal scroll.

## Out of scope

Libraries management (P5-SKILLS-2), the skill panel and update review (P5-SKILLS-3), any API change.

## Constraints / file ownership

Owns `AddFromLibrary`/`LibraryBrowserDialog`/`AddAction` in `library.tsx` (move them to `skills/add-from-library.tsx`
if that helps) and `web/src/lib/api/skill-libraries.ts` (browse and budget). Leave `UpdateReviewDialog` and the notices
to P5-SKILLS-3's use. Tokens and shared primitives only; both themes; 390 px.

## Acceptance criteria

- With the review's library added, the dialog matches `after/skill-5.png` / `-light.png` at 1256 px; the unavailable
  skills are folded with reasons; the preview shows rendered instructions and files; Add works for agent, workspace and
  organization tiers; no hard-coded budget remains.

## Tests

Vitest: list split (addable / Added / folded with reasons per `blocked` kind), preview, Add per tier, server errors,
permissions (no Add a library for members). Playwright (mocked, `support/skill-libraries.ts`): open from the Add skill
menu → pick → preview → add; both themes; 390 px; axe clean.

## Tests move with the UI (owner, 2026-11-20 — binding)

1. Before each commit that changes visible text, a label, a role or structure: `rg -n "Add from library|<old text>|<old
   selector>" web/tests web/e2e scripts/e2e` and update every hit **in the same commit** (`web/tests/skill-library.test.tsx`,
   `skill-libraries-ui.test.tsx`, `web/e2e/skill-libraries.spec.ts`, `support/skill-libraries.ts`, `support/mock-api.ts`,
   production specs).
2. New expectations use the mockup's words exactly; never loosen, skip, `fixme` or delete a test to get green.
3. Run through `scripts/dev/test.sh` only (D-491): vitest files you changed and those rendering them; every Playwright
   spec visiting skills or libraries (`--project=mocked`); lint, typecheck, build through `scripts/dev/heavy.sh <lane> …`.
4. Production specs: update by reading; list them. 5. A merge that turns master red with a stale test comes back here.

## Docs to update

`14` (Add from a library), PROGRESS and FEATURES rows at every commit.

## Suggested agent/model and reason

Sonnet `nextjs-developer` (one dialog, data from new fields).

## Completion report

Built 2026-11-27 on `alexiri/p5-skills-3` over `3876d74f`. **Not merged, not pushed.** Web only: no API, DTO,
migration or Rust change. One commit ("P5-SKILLS-4: …"), then a merge of master (FIX-276).

### What changed

- New `components/skills/add-from-library.tsx`: `LibraryBrowserDialog` (add and browse), `AddFromLibrary`,
  `useCanAddFromLibrary`, `activeLibraries`, and the pure `splitCatalogue` / `blockedWords`. The old browser,
  preview, "Already added" chips and `AddAction` left `library.tsx`, which keeps the update review and the notice.
- `lib/api/skill-libraries.ts`: the browse takes where the skill would go (`BrowseFor`, C-1176); `LIBRARY_BUDGET`
  is gone and `BudgetTier` is a plain type. **No budget number is left in the dashboard**, so the update review's
  over-budget sentence no longer says "N skills, K KiB" ("Approving would exceed this <tier>'s library budget.
  Remove a library skill or reject this update.").
- `skills-list.tsx` passes the target (scope, name, the ids kept there); the client-side budget sum is gone.
- `ui/dialog.tsx`, two small additions: `bodyClassName` (columns that scroll on their own), and `fixedHeight` no
  longer throws where `matchMedia` is missing (jsdom). `lib/utils.ts` not touched (FIX-276).

### Readings of the brief to check

1. **The preview's first line is the library's description as written**, without "Use it to" (accepted at
   P5-SKILLS-3's review for library text; the mockup shows the prefix).
2. **Added** means this very library skill is kept at this level (an `added` id among the page's own). Added to
   another agent shows nothing. An added row can be read; Add is off with "Already added to <name>.".
3. A different skill with the same folder name is folded ("A skill named <name> is already on <target>.").
4. The budget has a size and a number of skills. The size has the brief's sentence; the number keeps the server's
   own. Add is also off when the files as read are larger than the listing's estimate and no longer fit.
5. "n files · k KB" is the server's size (characters of body and files) over 1024.
6. Beside the list the first addable skill is previewed at once, so the third column is never empty. Below 768 px
   nothing is chosen for the person.
7. **The instance's libraries keep a "Shared with every organization" heading** (D-450(f); the mockup's list is
   flat). With none shared the column is the mockup's.
8. **Add a library** opens P5-SKLIB-3's dialog over this one, for owners and admins of the organization.
9. The fold counts what is loaded: with **Load more** still offered it can grow.
10. **Browse skills** (a library card) is the same layout, read-only, with Close; there **Added** means added
    anywhere the viewer can see.

### Comparison with the mockups

Looked at `after/skill-5.png` and `-light.png` (and the HTML for the words, `before/` L07 in the review's text)
before coding, and again against the built dialog at 390, 768, 1256 and 1280 px in both themes, on mocked data
shaped like the mockup (three libraries, seven listed, eight that can't be added).
`SKILLS_SHOTS=<dir> scripts/dev/test.sh <lane> web playwright e2e/skills.spec.ts --project=mocked -g "from a library"`
writes them again (`skill-5-1256-{dark,light}` ↔ `after/skill-5{,-light}.png`, `skill-5-why-…`, `skill-5-list-390-…`,
`skill-5-preview-390-…`).

| Difference kept | Reason |
|---|---|
| The dialog is 56 rem wide (mockup about 1000 px), so rows cut their description a few words sooner | The brief: size `2xl` |
| At 1256×900 the dialog stops at its cap and the last row sits under the pinned line (mockup: all seven fit) | The shared dialog is anchored 12 % down and never taller than the window allows (`14`) |
| The preview's line has no "Use it to" | Reading 1 |
| The closed fold's chevron points right, and down when open (the mockup draws it down while closed) | The app's folds, as the editor's Files line in skill-4 |
| "Doc coauthoring" (mockup "Doc co-authoring") | D-498(b): the folder name made readable; library skills have no title |
| The sample picture line "image: a pixel" | Test data: it shows that outside text fetches nothing |
| Title and text sizes, the close button, Cancel | Shared `DialogContent` and `Button` |
| The page behind, the rail and the names | Illustrative |

### Tests

- New: `tests/add-from-library.test.tsx` (16: libraries and counts, the browse's tier, the split with every
  `blocked` kind, the preview and that it fetches nothing, Add for agent, workspace and organization, Added, the two
  late refusals, 409 and 422, search, **Add a library** for admins and not members, the narrow screens);
  `e2e/skills.spec.ts` gains the journey at 390, 768, 1256 and 1280 px in both themes with axe and no sideways
  scroll (page and dialog); `e2e/support/skills.ts` the mockup's libraries.
- Changed with the UI, no case dropped: `skill-library` (folded instead of disabled rows, the new preview and
  button; the client-side "26 of 25 skills" case is now the server's room), `skill-libraries-ui` (the left list
  instead of the select on a wide screen, **Add to Acme**), e2e `skill-libraries` and its support.
- Production specs and `scripts/e2e`: searched, no string or selector of this dialog; none to run nightly.

### Commands and results

- Fail first, on `3876d74f` (a temporary detached worktree): the new vitest file cannot import the module; with its
  one imported constant written out so the cases run, **16 of 16 failed**.
- Before the merge of master: `test.sh … web vitest` on 21 files (this dialog's, the skills pages', every test of a
  shared dialog, the four drift guards and the source guards) **259/259**; `test.sh … web playwright skills
  skill-libraries --project=mocked` **116/116**; `pnpm typecheck`, eslint on touched files, `cargo fmt --all
  --check`, `scripts/guards/run-all.sh`, `extract-schema.py --check`: pass. No Rust changed (no clippy), no local
  `pnpm build`, no whole suite (CI).
- After the merge of master (`47c62f90`, FIX-276: no conflict; `cn()` now keeps `text-label`, and the skill chips
  keep their own `text-xs`): vitest on 12 files with `cn` and the four drift guards **170/170**; Playwright
  `skills skill-libraries` **116/116**; typecheck, eslint, fmt, guards and schema pass. Commits: `cd11744a` (the
  work), `47c62f90` (the merge), and the one that adds these lines.

### Security notes

No SEC row. Everything from a repository (labels, names, descriptions, files) is React text; the instructions are
`Markdown images={false}`, so previewing a skill fetches nothing (vitest and the browser journey). The server decides
what may be added; the buttons only follow it and the role in `/v1/me`.
