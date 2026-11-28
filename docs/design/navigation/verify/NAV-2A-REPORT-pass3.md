# Agent pages (nav-2a) verification, pass 3 (P5-NAV-4): T-1 and a re-measure

Date: 2026-11-28. Reviewer lane, no product code changed.

## Result

**Clean: all 12 rows are Match.** T-1 is fixed, CI is green on `1e9f0c34`, and moving the rules into a layer changed
none of the measurements.

## Environment

- Commit checked: `1e9f0c34` (master with the layer fix `285b04a4` and P5-NAV-1..3). `aurora-mem5` (:4530) redeployed
  with `kind-dev.sh up --llm-mock --ssh` through `heavy.sh`, then `kubectl rollout restart` of every deployment. **Still
  up.** `aurora-dev` and `aurora-skm` untouched; no volume pruned.

## T-1: the full-height pane rules — fixed

The rules are in `@layer components`, and the chat panel's and file editor's phone heights are `max-md:`.
`tests/design-guards.test.ts` on `1e9f0c34`, through `test.sh`: **17 passed**.

## Re-measure (same results as pass 2)

**With Bob's emergency-access notice dismissed**, page scroll in px at 1280×800 and 1256×900, with more controls on and
off:

| Page | Scroll |
|---|---|
| Terminal, Files, Chat | 0 in all four cases; less than 32 px left under the page, no sideways scroll |
| File editor (AGENTS.md open) | 0 in all four cases (pane 296 and 396 px with more controls on, 308 and 408 px off) |

- **With the notice showing**, the only scroll is **18 px at 1280×800**: Terminal with more controls on, and Chat with
  them on or off. The pane is then at its smallest height (288 px; Chat 560 px), where `14` lets the page scroll.
- At 390 px the page scrolls, as intended: Terminal 297 px and the editor 327 px.
- **BYO Connect, axe:** no finding at both widths and themes. The only sideways scroll is inside the code boxes.

## CI

Run 46092801164 for `1e9f0c34`: **success** in every job that ran (two release and pull-request jobs were skipped).
`web`: 4,130 unit tests passed in 290 files; the Playwright runs passed 2,188 (4 skipped) and 13. So the mocked browser
suite has now run on the terminal fix.

## Registry

Tests: **Match** (was Needs changes). The other 11 rows stay Match from pass 2. P5-NAV-1 and the agent pages are done.

## Still with the owner (from pass 1)

The agent "…" menu (D-507(a) against P5-NAV-1's "don't invent one"), and Chat on a phone opening the conversation rather
than the list. Both counted as Match.
