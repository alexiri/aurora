# P5-NAV-1: Agent pages: one fixed header with Chat · Automations · Computer · Settings, and no second left menu (owner, 2026-11-27)

## Goal

Today an agent's Settings adds a 208 px left menu beside the 256 px rail (on Memory and Skills a list follows: four
columns, the content gets ~450 px), Settings is a button beside the section tabs that opens a different frame with its
own "‹ Chat" back button and h2, the header moves between sections (Automations renders narrower, so its tabs drop under
the name), and on a phone seven section pills wrap onto two rows (N-1, N-2, N-3, N-10). The owner chose **nav-2a** for
**every agent page** (D-507):

- one **fixed header** on every section: avatar, name, `@handle` (Expert), status, attention chip / "Update available"
  when they apply, and the agent's actions menu if one exists today;
- one row of **underline tabs: Chat · Automations · Computer · Settings** (Settings is a tab, not a button);
- a quieter **second row of pills** only under **Computer** (Files · Terminal · Connect · Previews · Details) and
  **Settings** (Instructions · Apps & access · Memory · Skills · Tests · Other agents · Manage);
- every section at **full content width**; no "‹ Chat" button, no Settings frame, no left sub-nav;
- on a phone both rows are **single lines that scroll sideways**, never wrapping.

Decision: **D-507** (amends D-408's labelled Settings button: Settings becomes a tab). Addresses do not change.

## Linked requirements (look at every image before coding)

- `docs/design/navigation/README.md` (binding: precedence, how to read a mockup), `review.md` N-1, N-2, N-3, N-10,
  nav-2a and "nav-2a, page by page".
- **After:** `after/nav-2a.png` / `-light.png` / `.html` (Settings › Memory), `after/nav-2a-chat.*`,
  `after/nav-2a-automations.*`, `after/nav-2a-computer.*` (Computer › Files with the second row), `after/nav-2a-phone.*`
  (Chat, Computer › Files, Settings › Instructions at 390 px); also the middle frame of `after/nav-5.*`.
- **Before:** `before/N03-agent-chat.png`, `N04-agent-settings-instructions.png`, `N05-agent-settings-memory.png`,
  `N06-agent-settings-skills.png`, `N07-agent-computer.png`, `N21-agent-automations.png`, `P02-phone-agent-settings.png`,
  `P04-phone-agent-instructions.png`.
- `docs/14-web-dashboard.md`: agent header (~581–607), the Settings button and page (~617–631), Computer sub-nav
  (~689–692), tabs (~406–409, ~1311), `ListDetail` (~498–510).

## Wave and order

Independent of P5-NAV-2/-3 (they change the rail and the switcher; this changes the agent page). Touches the frame
around the agent's Settings pages: if P5-SKILLS-3 (the Skills list and panel) is running, merge in either order and
rebase; the content area only gets wider. Reviewed by **P5-NAV-4**.

## Context (what exists today)

- `web/src/components/agents/agent-page.tsx`: header (~164–254: phone "Back to agents", avatar, "Agents /" breadcrumb,
  h1, `@handle` (Expert), state/attention/"Update available", phone **Status** button, the **Settings** outline button
  ~224–233 with `aria-current`), section tabs ~243–253 (`Tabs`, segmented, `SECTIONS` without "settings", ~152; BYO →
  Computer only; emergency → Chat · Automations), Computer sub-tabs ~284–294 (`Tabs overflow="menu"`; Terminal, Connect,
  Previews are Expert and feature-gated; Details reads "Overview" when the terminal is off).
- `web/src/components/agents/settings-shell.tsx` (~24–80): "‹ Chat" **button** (~50–53; "Computer" for BYO), h2 + lead
  (~54–55, `sections.ts:100-109`), action slot (~57), the sub-nav `Tabs variant="segmented" columnFromMd` (~60–72);
  on a phone the item screen of a `ListDetail` hides the heading and sub-nav (~48, ~69).
- `web/src/components/agents/sections.ts`: entries (~96), Manage for owners (~145), altitude gating (~162–174, ~193–201;
  a direct `?tab=` still opens a hidden page).
- Chat: `web/src/components/chat/chat-panel.tsx` (conversation list 232 px from 1024 px, status rail 56/288 px),
  `agents/chat-tab.tsx`. Automations: `automations/automations-tab.tsx` (find why it renders narrower than the others).
- Primitives: `web/src/components/ui/tabs.tsx` (segmented | underline, `overflow: scroll | menu`, `columnFromMd`),
  `ui/list-detail.tsx` (two screens below 1100 px).

## Scope

1. **One header component** for every section (Chat, Automations, Computer, every Settings page, BYO and emergency
   variants): the elements listed in the Goal, laid out as `after/nav-2a.png` (name and status on the left; the actions
   menu, if today's page has one, on the right, labelled for the agent — "Actions for <Agent>" — so it can't be confused
   with a conversation's "…"; if there is none today, don't invent one). Same position and height on every section.
2. **The tab row**: underline tabs Chat · Automations · Computer · Settings (BYO: Computer · Settings; emergency: Chat ·
   Automations · Settings with its emergency pages only), the active one marked, `aria-current`. Opening Settings goes to
   the last Settings page used, or Instructions (today's behaviour of the button).
3. **The second row**: a new shared quiet pill row (`ui/sub-tabs.tsx` or a `Tabs` variant, documented in `14`
   §Components) used for **both** Computer's sub-pages and Settings' sections, with the same gating as today (altitude,
   features, owner-only Manage, BYO, emergency). It scrolls sideways when it doesn't fit, with a fade at the edge; it
   never wraps.
4. **Settings pages**: remove the "‹ Chat" button, the h2 frame and the left sub-nav column; each page keeps a content
   title row (title, lead, its action such as **New memory**) as drawn in `after/nav-2a.png`. Memory and Skills keep their
   list + detail inside the now-wider content; re-check `ListDetail`'s two-screen cut-off (1100 px) against the new width
   and keep two columns wherever they fit.
5. **Full width everywhere**: fix whatever makes Automations narrower; every section uses the same content width.
6. **Phone** (`after/nav-2a-phone.png`): compact header (back to Agents, avatar, name, status, actions), the tab row and
   the second row as single scrolling lines; Chat still opens on the conversation list; the Status sheet stays.
7. **Docs**: rewrite `14`'s agent header, Settings and Computer navigation paragraphs to this model (D-507), and the tabs
   paragraph where it says the agent strip is segmented.

## Out of scope

The rail, the switcher, Manage, Administration, Your account (P5-NAV-2/-3); the content of Memory, Skills and other
Settings pages beyond their frame; any API change.

## Constraints / file ownership

Owns `agent-page.tsx` (header, tabs, Computer strip), `settings-shell.tsx`, `sections.ts`, the new sub-tabs primitive,
`automations-tab.tsx` (width only), `chat-panel.tsx`/`chat-tab.tsx` only as far as the header needs. Tokens and shared
primitives only; both themes; no horizontal page scroll at 390 px.

## Acceptance criteria

- At 1256 px, dark and light: Chat, Automations, Computer › Files and Settings › Memory match `after/nav-2a-chat`,
  `nav-2a-automations`, `nav-2a-computer`, `nav-2a`; the header does not move when switching tabs; no second left menu
  anywhere on an agent page; Memory and Skills show list + detail side by side.
- At 390 px: matches `after/nav-2a-phone`; neither row wraps.
- Every old `?tab=` address still opens its page; BYO and emergency views show only their allowed tabs and pages.

## Tests move with the UI (owner, 2026-11-20 — binding) and the lane gate (owner, 2026-11-27)

1. Before each commit that changes visible text, a label, a role, a heading, a button that became a tab, or a page's
   structure: `rg -n "Settings\"|‹ Chat|Back to chat|columnFromMd|<old selector>" web/tests web/e2e scripts/e2e` and
   update **every** hit in the same commit: e.g. `web/tests/agent-page-header.test.tsx`, `agent-sections.test.ts`,
   `dashboard-links.test.tsx`, `polish-2-primitives.test.tsx`; `web/e2e/agent-tabs.spec.ts`, `agent-content.spec.ts`,
   `altitude.spec.ts`, `ia-navigation.spec.ts`, `try-it-in-chat.spec.ts`, `stage4-routes.spec.ts`, `stage4-axe.spec.ts`,
   any spec that clicks the Settings **button** (it is now a tab: `getByRole("tab", { name: "Settings" })` or the link
   role the component uses), production specs. Never loosen, skip, `fixme` or delete a test to get green.
2. **Run only what this lane changed** (the owner's gate, 2026-11-27): new tests, the existing vitest files of the
   components you changed (`scripts/dev/test.sh <lane> web vitest <files>`), and the cheap drift guards
   `scripts/dev/test.sh <lane> web vitest tests/audit-actions.test.ts tests/arbitrary-values.test.ts tests/ui-text.test.ts tests/labels.test.ts`;
   `pnpm lint` and `pnpm typecheck` through `scripts/dev/heavy.sh <lane> …`. Playwright specs are updated by reading
   (run one only if the header can only be seen there); GitHub CI runs everything.
3. The completion report lists every test file changed and why; a merge that turns CI red because a test still expects
   the old button or column comes back to this lane.

## Docs to update

`14` (as in Scope 7; the new sub-tabs primitive in §Components), PROGRESS and FEATURES rows at every commit.

## Suggested agent/model and reason

Sonnet `nextjs-developer` (one shared header across many pages, a new primitive, many tests to move).

## Completion report

Commits; screenshots at 1256 px (dark, light) and 390 px beside each `after/nav-2a*` mockup; every remaining difference
with its reason; the cause of Automations' narrow width; tests changed; commands and results.
