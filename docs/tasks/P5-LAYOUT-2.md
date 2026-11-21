# P5-LAYOUT-2: Accounts as cards in a grid, like Apps (owner, 2026-11-21)

## Goal

The owner, on Apps & accounts › Accounts: "Accounts should be displayed as boxes, not rows, just like apps are." Today
each account is a full-width bordered row stacked in one column (`<ul className="space-y-3">`), so at 1920 px a row
holds an icon, a name and one pill across 1100 px of empty card. The Apps tab next to it is a grid of cards.

After this brief the main Accounts list (a workspace's and an organization's) is a `CardGrid` of account cards, the same
grid, card frame and spacing as Apps. Nothing an account row shows or does is lost; it is laid out for a card about
360–420 px wide.

## Linked requirements

- **Decision:** D-440 (accounts and agents show as cards in the shared `CardGrid`; an agent's Access tab keeps its one
  bordered list).
- **Docs:** `docs/14-web-dashboard.md` §Agents, accounts and access › 1. Accounts ("A row shows:" becomes "A card
  shows:", same contents). `14 §Workspace pages` (P5-REFINE-16) says Apps keeps its grid; Accounts now matches it.
- **Feature row:** `P5/14§layout-cards` in `docs/FEATURES.md`.
- No API change, no contract.

## Wave and order

- **Lane A**, after **P5-LAYOUT-1** has merged (it adds `CardGrid` and removes the page cap): 1 → **2** → 3.
- `components/accounts/account-row.tsx` and `accounts-list.tsx` are this brief's alone in the set.

## Context

All paths under `web/src/`, master 55b1bdb8.

- `components/accounts/accounts-list.tsx`: `AccountsList` (one list for a workspace's Accounts tab, an organization's
  Accounts tab, an agent's Access tab with `agent`, and an app's setup page with `appId`). The main list renders
  `<ul className="space-y-3" aria-label="Accounts">{given.map(row)}</ul>` (around :262); the agent's tab renders a
  `Card` with `divide-y` (P5-POLISH-11 C1) and the "Not given to <Agent>" fold. The sentence + **Add** line, the bulk
  "Select all" and `AccountsBulkBar` sit above the list.
- `components/accounts/account-row.tsx`: `AccountRow` (:160). Outside an agent's page the `<li>` is already a bordered
  card (`space-y-3 rounded-card border border-line bg-surface p-4`, `aria-label={account.name}`). First line: bulk
  checkbox (Expert, P5-REFINE-19), icon (`AppIcon` 32 / `AiKeyTile`), title with `AppLink` and " · <label>", health line,
  starter-key line, the level/menu block with a phone line break (:240, `max-sm:basis-full`), `AccountMenu` (the "…"
  button). Then "Shared by …", the "who can use it" pills (`ul aria-label="Who can use it"`, or the "Used by <n> agents"
  fold when there are many), and `TechnicalDetails` (Expert) at the bottom.
- The Apps card frame to match: `components/apps/apps-tab.tsx:92` (`flex flex-col gap-3 rounded-card border border-line
  bg-surface p-4 shadow-card sm:p-5`, actions at the foot with `mt-auto`).
- `CardGrid` (`components/ui/card-grid.tsx`, from P5-LAYOUT-1): 1/2/3/4 columns at base/`sm`/`lg`/`2xl`.
- Tests that see the list: `web/tests/accounts.test.tsx`, `polish-7.test.tsx`, `credential-free-apps.test.tsx`,
  `connect-dialog.test.tsx`, `dashboard-links.test.tsx`; `web/e2e/accounts.spec.ts`, `credential-access.spec.ts`,
  `org-accounts*.spec.ts`, `refine-19*.spec.ts` (bulk), `workspace-pages.spec.ts`.

## Scope

1. **The grid.** In `AccountsList`, the main list (no `agent`, no `appId`) renders `given` in a `CardGrid` with
   `aria-label="Accounts"`. Each `AccountRow` stays an `<li aria-label={account.name}>` so role queries keep working.
   An app's setup page (`appId`) uses the same grid. The agent's Access tab (`agent`) is **unchanged**: one bordered
   list with dividers and the "Not given to …" fold.
2. **The card** (outside an agent's page), laid out for ~380 px, in the Apps card frame
   (`flex h-full flex-col gap-3 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5`):
   - **Head:** (bulk checkbox when on) icon, then the name and label (wrapping, `break-words`), and the "…" menu pinned
     top-right. The name's line never pushes the menu to a new line; the phone break at :240 is no longer needed in a
     card and goes.
   - **Health line** under the name, then the starter-key line and "Shared by …" where they apply.
   - **Who can use it:** the agent pills, wrapping inside the card; the "Used by <n> agents" fold keeps its threshold.
   - **Foot** (`mt-auto`): `TechnicalDetails` when shown, so the folds line up across a row. Opening it grows that row
     (the grid's rows stretch, like Apps); nothing else moves.
   - The account's **Reconnect / Fix** action from the health line stays where the health line puts it.
3. **Order and sentence unchanged**: needs-attention first as today, the sentence line and **Add** above the grid, the
   bulk bar above the grid.
4. **Skeleton.** While the list loads (`Loading variant="rows"`), show card skeletons in the same grid (as Apps' skeleton
   does, `apps-tab.tsx:290–307`), not rows. Reuse that skeleton card if it can be shared; otherwise make one shared
   `CardSkeleton` in `components/ui/` and use it in both.
5. **docs/14 §1 Accounts:** "A row shows" → "A card shows" with the same list, plus one line: "The workspace's and the
   organization's Accounts are a grid of cards like Apps (`CardGrid`); an agent's Access tab keeps one bordered list."

## Out of scope

- What a card says (texts, health wording, menu items): unchanged.
- The agent's Access tab layout; the organization's member views of agents (`org-agents-list.tsx`).

## Acceptance criteria

- Workspace Apps & accounts › Accounts and the organization's Accounts tab show accounts as cards in the same grid as
  Apps: 4 per row at 1920 px, 3 at 1280, 2 at 768, 1 at 390; both themes; no horizontal scroll at any of them.
- Every card shows what the row showed (icon, name and label, health line, starter and shared lines, pills or "Used by",
  menu, Technical details at Expert, bulk checkbox at Expert); the menu is top-right on every width.
- A card with a long name ("Calendar link (ICS) · Google calendar") wraps its name inside the card without overflow.
- Bulk select (Expert) still selects, the bar still acts on the picked accounts.
- An agent's Access tab looks exactly as before.
- The loading state shows card skeletons in the grid.

## Tests

- vitest: the main list renders a `CardGrid` labelled "Accounts" with one `listitem` per account; the agent's Access tab
  still renders its single bordered list; the menu button is inside each card's head.
- Playwright (mocked): in `accounts.spec.ts`, at 1920 and 390 px: the cards' count per row (4 and 1) and no horizontal
  scroll; open Technical details on one card (Expert) and check its neighbours keep their content.

## Tests move with the UI (owner, 2026-11-20 — binding)

The owner: "I don't want a ton of broken tests because a button was renamed and the tests look for the old label." In
**every commit** of this brief:

1. **Find every test that sees what you change**: search `web/tests/**`, `web/e2e/**` (mocked **and** `production`
   projects), `scripts/e2e/*.sh`, Rust tests that assert messages or shapes, and fixtures/mocks for the old strings,
   selectors, structure and the components you touched (`rg -n "<old>" web/tests web/e2e scripts/e2e services crates`).
2. **Update them in the same commit.** Never loosen an assertion so old and new both pass; never `.skip`/`test.fixme`/delete
   a test to get green. A test is removed only with the feature it covers, and the report says so.
3. **Run what you touched and what references it** through `heavy.sh`: every vitest file that renders `AccountsList` or
   `AccountRow`, and every mocked Playwright spec that visits Accounts. `AccountRow` is shared by four pages: run the
   **whole** web vitest suite once before handing over (`cd web && /home/alex/orca/workspaces/aurora/heavy.sh <lane>
   pnpm exec vitest run`). Production specs can't run in a lane: update them by reading and list them in the report.
4. New behaviour gets new tests; old tests are kept and updated.
5. The completion report lists every test file changed and why, the commands run with their results, and the
   production specs left to the nightly.
6. A merge that turns master red because a test still expects the old UI is a send-back to this lane.

## Docs to update

- `docs/14-web-dashboard.md` §Agents, accounts and access › 1. Accounts.
- Not PROGRESS or FEATURES (the Orchestrator updates them at merge).

## Suggested agent/model and reason

`nextjs-developer` (Sonnet): one component's layout and one list, with several tests to move.

## Completion report

Files changed; screenshots of the Accounts tab at 1920, 1280, 768 and 390 px in both themes, one with Expert on (bulk
and Technical details open); the organization's Accounts tab at 1280 px; test files changed; commands and results;
production specs not run.
