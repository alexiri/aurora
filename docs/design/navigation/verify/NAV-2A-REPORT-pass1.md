# Agent pages (nav-2a) verification, pass 1 (P5-NAV-4)

Date: 2026-11-28. Reviewer lane, no product code changed (no direct fixes made).

## Result

**Not clean.** 12 rows: **9 Match, 1 Minor difference, 2 Needs changes.** The new header, tabs and pill rows match the
mockups on a real stack, and the emergency reader's view is right. The one defect P5-NAV-1 introduced: **the Terminal
page now scrolls**, and **CI is red on it**.

## Environment

- Commit checked: `99c41755` (master with P5-NAV-1 and FIX-280; not FIX-278).
- Own throwaway kind cluster `aurora-mem5` (:4530), redeployed in place with `kind-dev.sh up --llm-mock --ssh` through
  `heavy.sh` (terminal and SSH on; previews and chat platforms off). **Still up** for P5-SKILLS-5 pass 2. `aurora-dev`
  and `aurora-skm` untouched; no volume pruned.
- Data: `seed.py` memories and skills on Daneel, real agent runs, one schedule and two webhooks (no chat channel:
  chat platforms are off), a bring-your-own agent (Hari), a hosted agent with no AI key (Giskard), Carol co-owner, Dave
  member, Alice for emergency access.
- Playwright, 1256 and 390 px, dark and light on the main pages; some probes at one width or theme. Header position, tab
  rows and panel width were **measured** on each page. Captures show Bob's "had emergency read access" notice (left over
  from the previous review) unless it was dismissed.
- Bob walked the pages with **Show more controls** on (as the mockups draw them) and off.

### Console, `/v1` 4xx and 5xx, overflow, axe

- No CSP violation. **One axe finding** (serious, on the BYO Connect page: row Rules).
- The only sideways scroll outside code blocks is the pill row itself, by design. No page scrolled sideways.
- Requests that failed, none from the new frame: `503` on `…/files` while a cold computer wakes; `409` on `…/files` for
  the agent with no AI key; two failed terminal WebSocket handshakes (202, then 404) while the computer wakes, after
  which it connects; a member's `403` on `…/model-readiness` on Instructions; a burst of `429`s caused by my own test
  opening 22 addresses in a row.
- **Emergency reader: no 403 and no 4xx on any page walked.**

## Registry

Captures for rows that are not Match are in [`nav-2a-pass1/`](nav-2a-pass1/). Match rows were compared with captures
kept in the lane's scratch folder.

| Row | Verdict | What was checked / evidence |
|---|---|---|
| Header | Match (see owner question 1) | Avatar, "Agents /", name, `@handle` only with more controls on, state. Seen: Asleep, Working, Ready (live, during one turn), "Can't answer: no AI key" with **Give Giskard the Mock LLM**, "Bring your own". **It doesn't move:** the name is at y=130 and the tab row at y=170 on all 13 Daneel pages at 1256 px (y=231 and 275 at 390 px), and the tab row held at 170 through Asleep → Working → Ready. With the give-key row the tabs sit lower (218; 323 at 390 px), the same on every tab of that agent. No agent "…" menu |
| Tabs | Match | Underline Chat · Automations · Computer · Settings (gear), the open one marked; no Settings button, no "‹ Chat". Arrows and Home work. Settings opens Instructions first, then the page last open; Computer opens Files, then the last page. BYO: Computer · Settings with more controls, Settings without. Reader: Chat · Automations · Settings |
| Second row | Match | The same pills under Computer and Settings only; one line at both widths; the open pill is brought into view; edge fade; 44 px targets on a phone. Gating: default view lists Instructions, Apps & access, Memory, Skills, Manage and Files, Details; a direct `?tab=evals` still opens Tests and lists it; a member has no Manage; BYO Instructions, Apps & access, Manage; reader Instructions, Memory, Skills. Previews isn't listed (off on this instance) |
| Settings pages | Match | No back link, frame or left column; title (24 px), lead and action; Memory and Skills show list and detail side by side at 1256 px; every panel is 936 px wide |
| Chat | Match | Conversation list, thread and status rail as one framed panel under the tabs; the page doesn't scroll at 1256×900 or 1280×800 |
| Automations | Match | Same width as every section (936 px), page-sized title, **New automation**, filter, list |
| Computer | **Needs changes** (C-1) | Files with the pill row, state line, toolbar and table match; Connect and Details open with the same header. **Terminal makes the page scroll.** Captures `computer-terminal-*` |
| Phone | Match (see owner question 2) | Compact header with back to Agents, one line of tabs, one line of pills, nothing wraps, no page scrolls sideways; Memory's list shows the pills and **New**; a memory opens as its own screen without them; the **Status** sheet opens from Chat |
| Addresses | Match | `?tab=` `chat`, `automations`, `schedules`, `webhooks`, `channels`, `computer` (→ Files), `files`, `terminal`, `ssh`, `details`, `instructions`, `access`, `memory`, `skills`, `evals`, `peers`, `manage` open their page with the right tab and pill; `previews` (off here) and unknown values open Chat; `?c=<id>` opens the conversation. BYO opens Instructions; `?tab=connect` its Connect |
| N-1, N-2, N-3, N-10 | Match | One left menu; Settings is a tab with no second frame; on a phone Settings is header, tabs, one pill line, then content; the header is in one place on every section |
| Tests | **Needs changes** (C-1) | **CI is red** on the P5-NAV-1 push: run 46081931692 for `99c41755`, `web`, 1 failed and 2120 passed: `e2e/terminal.spec.ts:82` (1280 px), "the page scrolls". The same suite passed on `936eca3c`. No `.skip`, `.fixme` or `.only` added and no test file removed since `1e067b16`; no spec still clicks a Settings **button** (the one `button "Settings"` line asserts there is none) or expects "‹ Chat" |
| Rules | **Minor difference** (C-2) | Shared primitives and tokens, both themes, no sideways page scroll; `14` describes D-507. One accessibility violation on an agent page. Captures `rules-byo-connect-*` |

## Change list

### P5-NAV-1

**C-1 · broken (CI red) · rows Computer, Tests · `after/nav-2a-computer.png` (the pane fits the page)**

- File: `web/src/app/globals.css`, `.wterm.term-pane { height: max(18rem, calc(100dvh - 25rem)) }`.
- Differs: the new header puts the tab row on its own line. P5-NAV-1 changed the chat card's offset (7.5rem → 11rem)
  but not the terminal pane's, so the Terminal page scrolls. Measured on the live stack, notice dismissed:

  | Viewport | More controls | The page scrolls by |
  |---|---|---|
  | 1280×800 | on | 122 px |
  | 1280×800 | off | 8 px |
  | 1256×900 | on | 36 px |

  (With more controls on, the "Showing more controls · Hide" line under the pane adds to it.) At 390 px the page may
  scroll to the pane, as the spec allows.
- Should be: at 768 px and wider the Terminal page doesn't scroll at 800 px height, with more controls on or off, as
  FIX-138 requires and the spec asserts. Take the new header into the pane's offset (and count the lines under the
  pane). `e2e/terminal.spec.ts` is the failing test; it already fails on master. Don't loosen it.
- Also look at: Files scrolls 13 px at 1280×800 with more controls on (1 px with them off), which is only the same
  footer line; no spec asserts Files.
- Captures: `computer-terminal-*` (`…-1280x800` and `…-1256x900` are single screens with the notice dismissed).

### Agent Connect page, bring-your-own (older than P5-NAV-1; `connect-tab.tsx`, last changed by P5-LAYOUT-1)

**C-2 · rough · row Rules**

- Differs: axe `scrollable-region-focusable` (serious): the code boxes (`pre`, `max-h-64 overflow-auto`) scroll but
  can't be focused with a keyboard. Two of them at 390 px, one at 1256 px, both themes.
- Should be: the scrolling code boxes are focusable and named (the diff view already does this with `tabIndex={0}` and a
  label).
- Captures: `rules-byo-connect-*`.

## Two questions for the owner (counted as Match, because a brief asks for what is built)

1. **The agent "…" menu.** D-507(a), every nav-2a mockup and my Header row show an agent menu in the header, labelled as
   the agent's. P5-NAV-1 says to keep one only if today's page has one, and not to invent one; there is none (D-408), so
   none was built. On a phone the mockup's "…" place holds **Status** on Chat. If you want the menu, it needs a decision
   on what it holds.
2. **Chat on a phone.** My Phone row and P5-NAV-1 scope 6 say Chat "still opens on the conversation list". It opens the
   person's conversation, with the list one tap back. That is what it did before P5-NAV-1 too (`14`, P5-REFINE-11), so
   the briefs' "still" was mistaken about today; the lane kept today's behaviour.

## Differences the lane kept that I accept

Each follows a brief, a token or the shared components: name at 20 px and pills at 14 px; ink primary buttons; page
contents unchanged (Computer's state line and Files toolbar, Memory's two rows of chips, Instructions' fields and lead,
the Automations rows, the chat's thread and composer); an empty Status slot on a hosted agent's other phone sections;
the "more controls" line under non-chat pages; Memory's title wrapping beside its actions on narrower screens; the rail
and switcher (P5-NAV-2/-3). I did not capture 768 px or 1024 px.

## The emergency reader

Alice with an open grant, Daneel's page, both widths and themes: the header has no state, chip or Status button; its
only controls are the banner's **End now** and the way back to Agents; tabs Chat · Automations · Settings with
Instructions · Memory · Skills; `?tab=files` and `?tab=manage` fall back to Chat; the tab row is at the same height on
every tab. A skill's files are listed with "Emergency read access doesn’t cover a skill’s files." and can't be opened,
and a conversation has no composer and no Reply, button or **R** key (FIX-280). **No write control and no 403.**

## Regressions looked for

- **Try it in chat** opens the Chat tab with the draft (1256 and 390 px).
- The phone **Status** sheet opens with Needs you, Doing now and What Daneel did.
- The full-window terminal (`/agents/daneel/terminal`) and **Open in new window** work.
- Conversation links (`?tab=chat&c=…`), the form notification and activity links use, open the conversation. I did not
  send a notification.
- Found: C-1 only.

## Not checked

- The **Update available** mark and "Starting its computer…" (the computer woke before the header showed it).
- Previews (off on this instance) and a chat-channel automation (chat platforms off).
- A paused agent's banner.

## For pass 2

Re-check C-1 (rows Computer and Tests, with CI) and C-2 (Rules), plus the owner's two answers if they change anything.
