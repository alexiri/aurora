# P5-LAYOUT-4: The approvals example sits in a closed fold (owner, 2026-11-21)

## Goal

The owner, on Approvals with nothing waiting: "the example looks a lot like something the user actually has to act on.
Hide it in a collapsible section, like the Skills page." Today, under "Nothing is waiting for you.", the sample card
(P5-REFINE-16, workspace-11, rec 10) is drawn open with the amber `pending-line` border, a live-looking question and three
working buttons; its only hint is a small caps label above it. Amber means "needs you" everywhere else (D-384).

After this brief the example is inside a closed fold, styled like Skills' **How skills combine**: one line
"What an approval looks like" that opens to the sample. The sample loses the amber border.

## Linked requirements

- **Decision:** D-441 (the example is a closed fold; never amber).
- **Docs:** `docs/14-web-dashboard.md` §Workspace pages (P5-REFINE-16), the sentence "**Approvals** shows 'Nothing is
  waiting for you.' once and an **Example** card … only while nothing is waiting": becomes "… and, below it, a closed
  fold **What an approval looks like** holding the example card (built from the real question, never counted or sent;
  each answer explains itself; never amber)".
- **Feature row:** `P5/14§layout-quiet` in `docs/FEATURES.md`.
- No API change, no contract.

## Wave and order

- **Lane B**, after **P5-LAYOUT-5**: 5 → **4**. Small.
- P5-LAYOUT-1 (lane A) changes one class in `components/approvals/approvals-view.tsx` (the page's `max-w-5xl`); whichever
  merges second rebases.

## Context

All paths under `web/src/`, master 55b1bdb8.

- `components/approvals/approval-example.tsx`: `ApprovalExample` builds a sample `Approval` and renders
  `<section aria-label="Example approval" data-testid="approval-example">` with a caps label
  "Example · this is what {agentName} asks before a change" and a card `rounded-card border border-pending-line bg-surface
  p-4 shadow-card` holding `ApprovalQuestion` (`onDecide` shows `exampleExplanation(...)` in `data-testid="example-says"`).
- `components/approvals/approvals-view.tsx` :216–223: when `total === 0`, `EmptyState "Nothing is waiting for you."`
  then `<ApprovalExample …/>`. The example never shows while something is waiting (keep that).
- The fold to copy: `components/skills/workspace-skills.tsx:32–38`, `<details className="mt-6 rounded-card border
  border-line bg-surface px-5 py-3 text-sm" data-testid="skills-combine"><summary className="cursor-pointer font-semibold
  text-ink">How skills combine</summary>…</details>`. If a shared fold primitive exists by the time you start (check
  `components/ui/` for a `Fold`/`Disclosure`), use it; otherwise use the same classes. Do not add a third copy of the
  classes if the Skills fold and this one can share a small primitive: then add `components/ui/fold.tsx` and use it in both.
- Tests: `web/tests/approvals.test.tsx` :142–151 (checks "Example · this is what" and clicks the three buttons);
  `web/e2e/refine-16.spec.ts` :109–111 (clicks Block in the example).

## Scope

1. **The fold.** `ApprovalExample` renders a closed `<details data-testid="approval-example">` in the Skills fold style,
   with the summary **What an approval looks like**. Closed on every visit (no remembered state).
2. **Inside**, in this order: one line "This is what {agentName} asks before it changes something. Try the buttons:
   nothing is sent."; then the sample card with a **neutral** border (`border-line`, not `border-pending-line`); then the
   explanation line after a click (`example-says`, unchanged texts). The caps label "Example · this is what …" goes (the
   summary and the line replace it).
3. The fold sits below the empty state with the page's normal spacing; on a phone it is full width.
4. **Accessibility:** the summary is the fold's name; the sample keeps `aria-label="Example approval"` on its inner
   section so screen readers still hear it is an example.

## Out of scope

- The empty state's text, the real approval cards, the History tab.

## Acceptance criteria

- With nothing waiting, Approvals shows "Nothing is waiting for you." and one closed line **What an approval looks
  like**; no button and no amber border is visible until it is opened.
- Opened, the sample's three buttons each show their explanation and send nothing (no request in the network log).
- With something waiting, neither the fold nor the sample is on the page.
- Both themes; 390 px without horizontal scroll.

## Tests

- vitest (`approvals.test.tsx`): the fold is closed at first (the buttons are not visible), opening it shows the line
  and the sample; the three explanations as today; the sample's border is not the pending one; no fold when something
  waits.
- Playwright (`refine-16.spec.ts`): open the fold, click Block, read the explanation.

## Tests move with the UI (owner, 2026-11-20 — binding)

The owner: "I don't want a ton of broken tests because a button was renamed and the tests look for the old label." In
**every commit** of this brief:

1. **Find every test that sees what you change**: search `web/tests/**`, `web/e2e/**` (mocked **and** `production`
   projects), `scripts/e2e/*.sh`, Rust tests that assert messages or shapes, and fixtures/mocks for the old strings
   ("Example · this is what", `approval-example`, `example-says`, `skills-combine` if you share the fold) and the
   components you touched (`rg -n "<old>" web/tests web/e2e scripts/e2e services crates`).
2. **Update them in the same commit.** Never loosen an assertion so old and new both pass; never `.skip`/`test.fixme`/delete
   a test to get green. A test is removed only with the feature it covers, and the report says so.
3. **Run what you touched and what references it** through `heavy.sh`: the vitest files above and every one that renders
   Approvals or Skills; the mocked Playwright specs that visit Approvals (`rg -l "approvals" web/e2e`). If you add a
   shared `Fold` primitive, run the **whole** web vitest suite once (`cd web &&
   /home/alex/orca/workspaces/aurora/heavy.sh <lane> pnpm exec vitest run`). Production specs can't run in a lane:
   update them by reading and list them in the report.
4. New behaviour gets new tests; old tests are kept and updated.
5. The completion report lists every test file changed and why, the commands run with their results, and the
   production specs left to the nightly.
6. A merge that turns master red because a test still expects the old UI is a send-back to this lane.

## Docs to update

- `docs/14-web-dashboard.md` §Workspace pages (the Approvals sentence); if a `Fold` primitive is added, the components
  list in §Visual design.
- Not PROGRESS or FEATURES (the Orchestrator updates them at merge).

## Suggested agent/model and reason

`haiku-worker` would do the component; give it to the lane's usual worker with the tests (Sonnet) since two specs move.

## Completion report

Files changed; screenshots of Approvals with nothing waiting (fold closed, fold open) in both themes and at 390 px; test
files changed; commands and results; production specs not run.
