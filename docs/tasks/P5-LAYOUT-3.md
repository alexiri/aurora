# P5-LAYOUT-3: Agents as cards on Overview and on the Agents page (owner, 2026-11-21)

## Goal

The owner: "In the Overview, make the agents be displayed as boxes instead of rows. Same in the Agents page." Today the
workspace Overview lists agents as rows in one bordered list, and the Agents page as full-width `ListRow`s; on a wide
screen each row is a name and a handle across the whole page. `docs/14` §Screens already says Overview's agents "show as
a card grid"; the code drifted.

After this brief both pages show agents as cards in the shared `CardGrid` (the same grid as Apps and Accounts), built from
one `AgentCard` component.

## Linked requirements

- **Decision:** D-440 (accounts and agents show as cards in the shared `CardGrid`).
- **Docs:** `docs/14-web-dashboard.md` §Visual design › Screens › Workspace Overview ("Agents show as a card grid…":
  bring it in line with what is built: no "New agent" tile, the **New agent** button stays in the section header;
  the card's contents below) and §Workspace (the Agents page).
- **Feature row:** `P5/14§layout-cards` in `docs/FEATURES.md`.
- No API change, no contract.

## Wave and order

- **Lane A**, after **P5-LAYOUT-2**: 1 → 2 → **3** (needs `CardGrid` from 1; follows 2's card frame).
- Lane B's P5-LAYOUT-5 edits `components/agents/agents-view.tsx` (removes the BYO row). This brief does not touch that
  file except, if needed, the props it passes to `AgentsList`; whichever merges second rebases.

## Context

All paths under `web/src/`, master 55b1bdb8.

- **Overview:** `components/overview/overview-view.tsx` :123–147. `Section id="agents" title="Agents"` with
  `actions={<CreateAgentDialog wsId={wsId} />}`; a `<ul aria-label="Agents">` with `divide-y`; each `<li>`: a `Link`
  to the agent page with `Avatar` 40, the name, `AgentSummary` (C-986: the same sentences as Welcome and Access), the
  "BYO disabled by …" badge; and a **Chat** button-link (`aria-label="Chat with <name>"`).
- **Agents page:** `components/agents/agents-view.tsx` renders `AgentsList` (`components/agents-list.tsx`):
  `ItemList`/`ListRow` (`components/ui/section.tsx`) with `AgentAvatar` 40, name link (`id="agent-link-<id>"`),
  `@handle` (mono), the needs-you `AttentionChip` (P5-REFINE-20 G5, `actionId` points at the name link), the kind chip
  (`lib/agent-chip`), "BYO disabled by …", and `TokenBadges` for owners. Its empty state is `EmptyState "No agents yet"`.
  `AgentsList` is only used by the Agents page (the organization's lists use `org/org-agents-list.tsx`, not changed).
- `AgentSummary` (`components/agents/agent-summary.tsx`) needs `wsId`, `orgId` and the agent; Overview already renders
  it per agent, so the Agents page can too (same queries, already cached when coming from Overview).
- `CardGrid` (`components/ui/card-grid.tsx`, P5-LAYOUT-1); the card frame of Apps and Accounts
  (`flex h-full flex-col gap-3 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5`).
- Tests that see these lists: `web/tests/agents.test.tsx`, `agent-needs.test.tsx`, `agent-tokens.test.tsx`,
  `components.test.tsx`, `ui-foundations.test.tsx`, `onboarding.test.tsx`, `avatar.test.tsx`, `byo.test.tsx`;
  `web/e2e/workspace-pages.spec.ts`, `m1.spec.ts`, `first-run.spec.ts`, `onboarding.spec.ts`, `try-it-in-chat.spec.ts`,
  `agent-page-p5.spec.ts` (search them for "Chat with", `aria-label: "Agents"`, `agent-link-`).

## Scope

1. **`AgentCard`** in `components/agents/agent-card.tsx`, an `<li>` in the card frame:
   - **Head:** avatar (40), the name as the link to the agent page (keep `id="agent-link-<id>"`), `@handle` under it in
     mono, and on the right the needs-you `AttentionChip` when the count is above 0 (same words and `actionId`).
   - **Badges line:** kind chip, "BYO disabled by …", token badges (owners), as today; omitted when empty.
   - **Summary:** `AgentSummary` (C-986) when `wsId`/`orgId` are given.
   - **Foot** (`mt-auto`): **Chat** (outline, `aria-label="Chat with <name>"`, to the agent page) when the caller asks
     for it.
2. **Overview** renders its agents in a `CardGrid aria-label="Agents"` of `AgentCard`s with summary and **Chat**; the
   section header keeps **New agent**. Loading, error and empty states stay.
3. **Agents page:** `AgentsList` renders a `CardGrid aria-label="Agents"` of `AgentCard`s with the summary (pass `wsId`
   and `orgId` from `agents-view.tsx`) and **Chat**. `ItemList`/`ListRow` stay in `ui/section.tsx` for their other users.
4. Both pages: 4 cards per row at 1920 px, 3 at 1280, 2 at 768, 1 at 390 (the `CardGrid` steps). A workspace with one
   agent shows one card at the start of the row, not stretched across the page.
5. **docs/14:** Overview's bullet describes the card (avatar, name, `@handle`, needs chip, badges, summary, Chat) and the
   header button instead of a tile; the Agents page says the same cards.

## Out of scope

- The organization's agents lists (`org-agents-list.tsx`), the agent page, the agent picker, Welcome.
- The BYO row above the Agents list (P5-LAYOUT-5 removes it).

## Acceptance criteria

- Overview and Agents show agents as cards in the grid, same frame as Apps and Accounts, both themes; no horizontal scroll
  at 390, 768, 1280, 1920 px.
- Every card shows what the row showed on that page; Overview's and the Agents page's cards look the same.
- The needs-you chip, the token badges (owners only) and the "BYO disabled" badge still appear on the same conditions.
- "Chat with <name>" still opens the agent page.

## Tests

- vitest: `AgentCard` renders the name link, handle, chip at a count above 0 and none at 0, badges on their conditions,
  the summary, and Chat; `AgentsList` renders a list labelled "Agents" with one `listitem` per agent.
- Playwright (mocked): Overview and Agents at 1920 and 390 px: cards per row (4 with four agents in the fixture, 1) and
  no horizontal scroll.

## Tests move with the UI (owner, 2026-11-20 — binding)

The owner: "I don't want a ton of broken tests because a button was renamed and the tests look for the old label." In
**every commit** of this brief:

1. **Find every test that sees what you change**: search `web/tests/**`, `web/e2e/**` (mocked **and** `production`
   projects), `scripts/e2e/*.sh`, Rust tests that assert messages or shapes, and fixtures/mocks for the old strings,
   selectors, structure and the components you touched (`rg -n "<old>" web/tests web/e2e scripts/e2e services crates`).
2. **Update them in the same commit.** Never loosen an assertion so old and new both pass; never `.skip`/`test.fixme`/delete
   a test to get green. A test is removed only with the feature it covers, and the report says so.
3. **Run what you touched and what references it** through `heavy.sh`. Overview and the Agents page are on most first-run
   journeys: run the **whole** web vitest suite once before handing over (`cd web &&
   /home/alex/orca/workspaces/aurora/heavy.sh <lane> pnpm exec vitest run`). Production specs can't run in a lane
   (`first-run.sh`, `kind.sh` visit both pages): update them by reading and list them in the report.
4. New behaviour gets new tests; old tests are kept and updated.
5. The completion report lists every test file changed and why, the commands run with their results, and the
   production specs left to the nightly.
6. A merge that turns master red because a test still expects the old UI is a send-back to this lane.

## Docs to update

- `docs/14-web-dashboard.md` §Visual design › Screens (Workspace Overview) and §Workspace (Agents).
- Not PROGRESS or FEATURES (the Orchestrator updates them at merge).

## Suggested agent/model and reason

`nextjs-developer` (Sonnet): one new component used by two pages, with first-run tests to move.

## Completion report

Files changed; screenshots of Overview and Agents at 1920, 1280 and 390 px in both themes, with one agent and with four;
test files changed; commands and results; production specs not run.

### Report (lane A, 2026-11-21)

- **Commits:** fe5c897e (AgentCard, Overview, AgentsList, tests, docs/14); f5a77aae (agents-view.tsx: passes `wsId`/`orgId`, its own commit so LAYOUT-5's BYO removal rebases cleanly).
- **Files:** new `components/agents/agent-card.tsx`; changed `agents-list.tsx`, `overview/overview-view.tsx`, `agents/agents-view.tsx`, docs/14 (Overview bullet, Agents list).
- **Overview and the Agents page show the same card**: Overview now also shows the needs-you chip (via `useAgentNeedsCounts`) and owners' token badges, which its old rows lacked.
- **768 px:** the grid's container beside the sidebar is ~450 px, below `@2xl`, so 768 shows one column (the container rule, 3f196cde), not the brief's "2 at 768". 4 / 3 / 1 / 1 at 1920 / 1280 / 768 / 390.
- **Tests:** new `tests/agent-card.test.tsx`, `e2e/agent-cards.spec.ts` (both pages, both themes, 1920/1280/768/390, four agents and one agent, no sideways scroll); `e2e/workspace-pages.spec.ts` test title. Other specs (`m1`, `agent-needs`, onboarding, first-run, try-it-in-chat) use `list "Agents"`, `agent-needs-count`, `Chat with`: unchanged and still valid.
- **Run:** whole web vitest 3421/3422 (the one failure, `connect-journey.test.tsx`, timed out under load and passes alone, 33/33); `pnpm typecheck` clean; eslint on touched files clean; mocked Playwright agent-cards, agent-needs, workspace-pages: 70 passed; `m1.spec` real-stack project can't run here (no API).
- **Production specs not run:** `first-run.sh`, `kind.sh` (read: use `list "Agents"` / Chat links, unaffected).
- **Screenshots (not in git):** /tmp/claude-1000/-home-alex-orca-workspaces-aurora-p5-polish-a/5907c2a7-eb59-474f-a6e9-cd042563b3b4/scratchpad/shots/ (`<overview|agents>-<1|4>-<light|dark>-<1920|1280|768|390>.png`, 32 files).
