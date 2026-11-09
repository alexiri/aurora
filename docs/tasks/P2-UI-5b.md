# P2-UI-5b: Chat v2 fits a 1280×800 window; person messages are plain text by design

## Decisions (owner, 2026-11-09, "go on": the coordinator's recommendations)
1. **Compact header on the Chat tab.** At 1280×800 the agent page scrolls slightly: header + tabs ≈ 280 px plus the chat panel's 560 px minimum. On the Chat tab only, compact the agent header so that the page does not scroll at **1280×800** and the composer stays put. For example: handle and badges on the title line, avatar 32 px, the "Freeze agent" action moved into the header's overflow menu, and less vertical padding. The other tabs keep today's header. Use the same components and tokens, no one-off styles.
2. **Person messages render as plain text** (whitespace kept); agent messages stay Markdown in safe mode. Amend `docs/14 §Visual design` › Agent chat to say so, with the reason: what a person typed is shown exactly as typed.

## Scope
- `web/src/components/agents/agent-page.tsx` (header variant for the Chat tab) and whatever shared header pieces it uses; the overflow-menu entry for Freeze must keep the existing confirm dialog and permissions.
- `docs/14` (the two sentences).
- Tests:
  - in `web/e2e/chat-v2.spec.ts`, change the no-scroll assertion's viewport from 1280×900 to **1280×800**; the composer bounding box is still unchanged after 30 messages;
  - a vitest that the Chat tab uses the compact header and other tabs don't;
  - the Freeze action is still reachable from the overflow menu, with its confirm;
  - every existing assertion stays.
- P2-UI-4's guards (token, `style=`, axe both themes, theme, narrow shell) pass.

## Constraints
- **Owns:** the agent page header, `docs/14` (Agent chat paragraph), chat tests.
- Web only; no API change.
- Real built-in package names in mocks.

## Acceptance criteria
- [ ] `pnpm typecheck`, `lint`, `test`, `build`.
- [ ] Playwright `chat-v2`, `stage4-routes`, `stage4-axe`, `shell-theme`, `stage2-narrow`.
- [ ] `bash scripts/e2e/m1.sh`.
- [ ] `bash scripts/guards/run-all.sh`.
- [ ] Screenshots at 1280×800 (light and dark) of the Chat tab with a long transcript, plus one of another tab showing the unchanged header, in `/tmp/aurora-screens/p2-ui-5b/`.
