# Agent pages (nav-2a) verification, pass 2 (P5-NAV-4)

Date: 2026-11-28. Reviewer lane, no product code changed (no direct fixes made).

## Result

**Not clean yet: 12 rows, 11 Match, 0 Minor, 1 Needs changes (Tests).** Both items from pass 1 are fixed on the live
stack: the Terminal, Files and file-editor pages no longer scroll, and the BYO Connect page has no axe finding. But
**CI is red because of the terminal fix itself**: its new rules break the `design-guards` unit test (T-1).

## Environment

- Commit checked: `f7e5769b` (local master with `31bd1e9a`, the terminal fix `579cbf0f`). It doesn't hold P5-NAV-2 or
  FIX-281, which merged later.
- `aurora-mem5` (:4530) redeployed from it with `kind-dev.sh up --llm-mock --ssh` through `heavy.sh`. **Again `up` left
  the old Pods running** (23 minutes old after the new web image was loaded), so I ran `kubectl rollout restart` on every
  deployment. **Still up** for P5-SKILLS-5 pass 3. `aurora-dev` and `aurora-skm` untouched; no volume pruned.
- Playwright against the cluster. The measurements are the page's scroll height less the viewport, and the gap under the
  agent page (32 px is the page padding).

## C-1: full-height panes (rows Computer, Tests) — fixed

**With Bob's "had emergency read access" notice dismissed** (it lasts seven days after a grant), page scroll in px:

| Viewport | More controls | Terminal | Files | Chat |
|---|---|---|---|---|
| 1280×800 | on | 0 | 0 | 0 |
| 1280×800 | off | 0 | 0 | 0 |
| 1256×900 | on | 0 | 0 | 0 |
| 1256×900 | off | 0 | 0 | 0 |

Each left exactly 32 px under the page, with no sideways scroll. Before the fix, Terminal scrolled 122, 8 and 36 px.

- 300 lines of real output (`seq 1 300`) scroll inside the terminal; a long file list scrolls inside its own box.
- The **file editor** (AGENTS.md open): 0 px at 1280×800 and 1256×900.
- Resizing an open window (notice showing): the pane follows, 370 px at 900 high, 288 px (its smallest, page scrolls
  58 px) at 760, 470 px at 1000, and the shell still answered afterwards (`tput lines` read 26 at 470 px). A fresh load
  at 1280×760 with the notice dismissed gives a 312 px pane and no scroll.
- Short windows and phones scroll, as intended: Terminal 136 px at 1280×600; at 390 px the page scrolls to the pane.
- The full-window terminal doesn't scroll (pane 624 px at 1280×800).
- **With the notice showing**, only two cases scroll, by **18 px at 1280×800**: Terminal with more controls on, and
  Chat. The pane is then at its smallest height (288 px; Chat 560 px), where `14` lets the page scroll. Files and
  1256×900 don't scroll. I counted this as Match; if the seven-day notice should never cause a scroll, the smallest
  heights need to give.

## C-2: code boxes (row Rules) — fixed

BYO Connect, both widths and themes: **no axe finding**. The four code boxes are named regions in the tab order; the
container config box scrolls with the arrow keys and shows a focus ring. The "new token" dialog (after **Regenerate
token** on the scratch BYO agent) has its box named and focusable, with no axe finding.

## Neighbours

- **The header still doesn't move** and the width is unchanged: name at y=130 and tab row at 170 on Chat, Automations,
  Files, Terminal, Details, Connect, Memory, Manage and the BYO agent at 1256 px (231 and 275 at 390 px; the BYO name at
  229, as in pass 1); every panel 936 px (358 px).
- No console or CSP error apart from the two terminal WebSocket handshakes while a cold computer wakes (pass 1). Sideways
  scroll only in code boxes, the editor's long lines and the phone pill row.

## Registry

| Row | Pass 1 | Pass 2 |
|---|---|---|
| Computer | Needs changes (C-1) | **Match** |
| Rules | Minor (C-2) | **Match** (see T-1: the fix breaks a design guard, counted under Tests) |
| Tests | Needs changes (C-1) | **Needs changes** (T-1) |
| Header, Tabs, Second row, Settings pages, Chat, Automations, Phone, Addresses, N-1..N-10 | Match | Match (Chat re-measured; the header and width re-checked) |

## Change list

### The terminal fix (`579cbf0f`, ci-fix-terminal)

**T-1 · broken (CI red) · row Tests**

- File: `web/src/app/globals.css`, the "full-height pane" rules.
- Differs: `tests/design-guards.test.ts` › "layer guard › has no unlayered element selector in globals.css" **fails**:
  `main:has([data-fill-pane])`, `main :has([data-fill-pane])` and `main [data-fill-pane]` are element selectors outside
  a cascade layer. CI's `web` job stops there (runs 46089544984 for `b5755ed5` and 46090081104 for `6e1ac7aa`: 1
  failed, 4110 and 4113 passed), so **the mocked Playwright suite didn't run in either**. I reproduced it locally on
  `f7e5769b` with `scripts/dev/test.sh … web vitest tests/design-guards.test.ts` (1 failed, 16 passed).
- Should be: the rules pass the guard (put them in the layer the guard expects, or select without the element) without
  loosening it, and the pages keep today's measurements. A layered rule loses to utilities, so re-measure after moving
  it.
- Because CI didn't reach Playwright, I ran `e2e/terminal.spec.ts` and `e2e/agent-fill-pane.spec.ts` myself on
  `f7e5769b` (mocked, through `test.sh`): **53 passed**.

## CI

`31bd1e9a`'s own run was cancelled by the next push. `b5755ed5` and `6e1ac7aa` fail `web` only on T-1 as far as the job
got; their Rust jobs were passing or still running when I looked. No `.skip` was added by the fix.

## Still with the owner (from pass 1)

The agent "…" menu (D-507(a) against P5-NAV-1's "don't invent one"), and Chat on a phone opening the conversation rather
than the list. Both counted as Match.

## For pass 3

T-1 only, with CI green (including the mocked Playwright suite), then a quick re-measure of Terminal, Files, the editor
and Chat if the rules move.
