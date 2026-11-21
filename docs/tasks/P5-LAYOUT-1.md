# P5-LAYOUT-1: Pages use the full content width (owner, 2026-11-21)

## Goal

The owner, looking at Apps & accounts at 1920 px: "we're wasting lots of horizontal space, what's the point of limiting
things with `.max-w-page`?" Most list and detail pages are capped at `max-w-page` (1160 px, `--container-page: 72.5rem`
in `web/src/app/globals.css`), so on a wide screen a third of the content area stays empty while the Agents page and the
agent page (FIX-129) already span it.

`max-w-page` was meant to keep lines readable. That job belongs to the text and the forms, not to the page: a card grid
or a list of cards reads fine at any width if the grid adds columns. So:

- **No page caps its own width any more** (D-439). The page wrapper spans the shell's content area, as the agent page
  and the Agents page already do.
- **What must stay readable caps itself**: running text paragraphs (`max-w-prose`), a single form (its column of fields,
  `max-w-page-narrow`), a dialog (as today). A page that *is* one form or one reading column (Your account, a package's
  detail page, an organization's single-form tabs) keeps `max-w-page-narrow` on that form, not on the page.
- **Card grids grow with the width**: one shared `CardGrid` primitive, 1 column below `sm`, 2 from `sm`, 3 from `lg`,
  4 from `2xl` (1536 px viewport). Apps uses it in this brief; P5-LAYOUT-2 (accounts) and P5-LAYOUT-3 (agents) reuse it.

## Linked requirements

- **Decision:** D-439 (this brief). Amends `docs/14-web-dashboard.md` §Visual design, page header bullet ("Page widths
  are theme tokens…", around line 353): `max-w-page` is retired; the rule becomes the one above.
- **Feature row:** `P5/14§layout-wide` in `docs/FEATURES.md`.
- No API change, no contract.

## Wave and order

- After **P5-REFINE-20** has passed (the owner: "to be done after the UX review passes").
- **Lane A** first: P5-LAYOUT-1 → P5-LAYOUT-2 → P5-LAYOUT-3. P5-LAYOUT-2 and -3 need `CardGrid` from this brief.
- Lane B (P5-LAYOUT-5 → P5-LAYOUT-4) runs beside it. `components/approvals/approvals-view.tsx` is touched by this brief
  (one class) and by P5-LAYOUT-4: whichever merges second rebases.

## Context

All paths under `web/src/`, master 55b1bdb8.

- Tokens: `app/globals.css` :139–142 (`--container-page`, `--container-page-narrow`, with the comment pointing at docs/14).
- `components/topic-page.tsx`: `TopicPage` defaults `width = "max-w-page"` (:31) and wraps the page in
  `${width} space-y-4`. Every topic page (Apps & accounts, Internet access, People, Settings…) goes through it.
  `components/org/org-topics.tsx:297` passes `width="max-w-page-narrow"`.
- 46 files reference `max-w-page` or `max-w-page-narrow` (`rg -l "max-w-page" web/src`). Examples: `app/o/[org]/page.tsx:97`,
  `components/admin/*-view.tsx`, `components/admin/settings/*-view.tsx`, `components/account/admin-*-view.tsx`,
  `components/approvals/activity-view.tsx:104`, `components/skills/org-skills.tsx:20`, `components/admin/admin-topics.tsx:40`,
  `app/account/layout.tsx:9` (narrow), `components/apps/availability-view.tsx:139` (narrow),
  `components/admin/plugin-detail-view.tsx:150,166` (narrow).
- Other page-level caps: `components/approvals/approvals-view.tsx:446` (`max-w-5xl`), `components/admin/runners-view.tsx:106`
  (`max-w-4xl`), `components/agents/connect-tab.tsx:73` (`max-w-3xl`), `components/admin/settings/governance-settings-view.tsx:120`
  (`max-w-5xl` on a section). `max-w-3xl` on a single paragraph, notice or headline (setup-view :612, app-setup-page :106,
  the settings notices) is a text measure and stays.
- The Apps grid: `components/apps/apps-tab.tsx:248` and its skeleton :295, `grid gap-4 sm:grid-cols-2 lg:grid-cols-3`.
- Tests: `web/tests/arbitrary-values.test.ts` (its header comment names `max-w-page`); `web/e2e/workspace-pages.spec.ts`
  (1280 and 390 px, no horizontal scroll, axe); the per-level route sweeps `web/e2e/stage4-*.spec.ts`.

## Scope

1. **`CardGrid`** in `components/ui/card-grid.tsx`: a `<ul>` with `grid gap-4 sm:grid-cols-2 lg:grid-cols-3 2xl:grid-cols-4`,
   passing through `aria-label`, `className` and children (`<li>` items). Export it; add it to docs/14's component list.
   Apps (`apps-tab.tsx` grid and its skeleton) uses it. Its cards keep their look (`rounded-card border border-line
   bg-surface p-4 shadow-card sm:p-5`, actions at the foot with `mt-auto`).
2. **Retire `max-w-page`.** Remove `--container-page` from `globals.css` and every `max-w-page` class (not
   `max-w-page-narrow`). `TopicPage`'s `width` prop defaults to no cap (`""`); keep the prop for the narrow form pages.
   Page wrappers that only carried `max-w-page` keep their spacing classes (`space-y-6` etc.).
3. **Keep `max-w-page-narrow` only on a form or a reading column**, never on a whole page that holds lists or cards. Go
   through its current users: `app/account/layout.tsx` (Your account: a column of forms, keep), `availability-view.tsx`,
   `plugin-detail-view.tsx`, `admin-users-view.tsx:25`, `org-topics.tsx:297`. Keep it where the page is one form or one
   column of text; drop it where the page holds a table or a list (say which in the report).
4. **The other page caps** listed in Context (`max-w-5xl`/`max-w-4xl`/`max-w-3xl` on a page or section wrapper) go too.
   Approvals' list then spans the content width like every other list. A `max-w-*` on one paragraph, notice or headline
   stays (it is a text measure).
5. **Readable text inside wide pages.** Where removing the cap leaves a long paragraph running across 1600 px (page
   descriptions under `PageHeader`, explanatory paragraphs at the top of a tab, empty states), give that paragraph
   `max-w-prose`. Do it in the shared place where one exists (`PageHeader`'s description, `EmptyState`), not page by page.
   Forms inside a now-wide page keep their field widths (fields already size themselves; check Settings › General,
   Internet access, Administration settings sections, the connect journey pages).
6. **Tables** (`ResponsiveTable`, `DataTable`) fill the width they get; nothing to change unless a column now stretches
   absurdly (a one-word column 600 px wide): then give that column a width class from the existing set, not an arbitrary
   value.
7. **docs/14**: rewrite the "Page widths are theme tokens…" sentence (§Visual design, page header bullet) to D-439's rule
   and add `CardGrid` to the components list; fix the `globals.css` comment.

## Out of scope

- Accounts and agents as cards (P5-LAYOUT-2, -3). The sidebar width, the shell, the agent page (already full width).
- Any colour, font or spacing token other than the width tokens.

## Acceptance criteria

- `rg "max-w-page\b|max-w-page[^-]" web/src` finds nothing; `--container-page:` is gone from `globals.css`;
  `max-w-page-narrow` remains only on forms or reading columns, each listed in the report.
- At 1920 × 1080 (sidebar open), Workspace Apps & accounts (both tabs), Internet access, People, Settings, Skills, Activity,
  Approvals, Organization pages and Administration list pages use the whole content area: the right edge of the page
  content is within the shell's padding of the viewport's right edge. Apps shows 4 cards per row at 1920 px, 3 at
  1280 px, 2 at 768 px, 1 at 390 px.
- No horizontal scroll at 390, 768, 1280, 1920 px on the pages above, in both themes.
- No paragraph of running text is wider than `max-w-prose` on those pages at 1920 px.
- `web/tests/arbitrary-values.test.ts` passes without new allow-list entries.

## Tests

- vitest: `CardGrid` renders a list with its label and the column classes; Apps renders its cards inside it.
- Playwright (mocked): extend `web/e2e/workspace-pages.spec.ts` with a 1920 px pass over Apps & accounts and Internet
  access that asserts the page content's right edge is near the viewport's (within 48 px) and that Apps' first row holds 4
  cards; keep the 1280/390 checks.

## Tests move with the UI (owner, 2026-11-20 — binding)

The owner: "I don't want a ton of broken tests because a button was renamed and the tests look for the old label." In
**every commit** of this brief:

1. **Find every test that sees what you change**: search `web/tests/**`, `web/e2e/**` (mocked **and** `production`
   projects), `scripts/e2e/*.sh`, Rust tests that assert messages or shapes, and fixtures/mocks for the old classes,
   strings, selectors and the components you touched (`rg -n "<old>" web/tests web/e2e scripts/e2e services crates`).
2. **Update them in the same commit.** Never loosen an assertion so old and new both pass; never `.skip`/`test.fixme`/delete
   a test to get green. A test is removed only with the feature it covers, and the report says so.
3. **Run what you touched and what references it** through `heavy.sh`. This brief changes shared UI (every page wrapper,
   `TopicPage`, `PageHeader`): run the **whole** web vitest suite once before handing over
   (`cd web && /home/alex/orca/workspaces/aurora/heavy.sh <lane> pnpm exec vitest run`). Production specs can't run in a
   lane: update them by reading and list them in the report.
4. New behaviour gets new tests; old tests are kept and updated.
5. The completion report lists every test file changed and why, the commands run with their results, and the
   production specs left to the nightly.
6. A merge that turns master red because a test still expects the old UI is a send-back to this lane.

## Docs to update

- `docs/14-web-dashboard.md` §Visual design (page widths, components list: `CardGrid`).
- `web/src/app/globals.css` comment.
- Not PROGRESS or FEATURES (the Orchestrator updates them at merge).

## Suggested agent/model and reason

`nextjs-developer` (Sonnet): many files but one mechanical rule plus a small primitive; the judgement calls (which
narrow uses stay, where text needs `max-w-prose`) are listed in the report for review.

## Completion report

Files changed; every `max-w-page-narrow` kept or removed and why; paragraphs given `max-w-prose`; screenshots at 1920,
1280 and 390 px of Apps & accounts, Internet access and one Administration list page in both themes; test files changed;
commands and results; production specs not run.
