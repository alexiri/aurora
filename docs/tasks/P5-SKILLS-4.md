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

Commits; screenshots beside `after/skill-5*.png`; differences with reasons; tests changed; commands and results.
