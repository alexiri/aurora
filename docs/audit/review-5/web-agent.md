# REVIEW-5 findings — web-agent (saved by the coordinator from the reviewer's hand-back)

## WEBA-1 (high; code path confirmed, not observed in a browser) — wterm DOM renderer writes inline `style=` attributes; production CSP `style-src 'self' 'nonce-…'` blocks them → no colours / wide-cell spacing in prod
- `web/src/proxy.ts:23`; `web/node_modules/.pnpm/@wterm+dom@0.5.10/.../dist/renderer.js:170`, `:898`, `:912` (`appendRun` at :130 uses `style.cssText`); `web/src/components/agents/terminal-tab.tsx:297-311`. Dev adds 'unsafe-inline'; Playwright runs `next dev` (`web/playwright.config.ts:55`).
- Spec contradiction: C-622 (API_CONTRACTS:1047) "nothing else loosens" vs docs/14:680-690 "DOM rendering … 24-bit color". Owner decision: (a) loosen style-src on the terminal route (SEC decision), (b) CSSOM render path, (c) accept colourless.

## WEBA-2 (high, test-gap/docs) — Playwright suites not run by any CI workflow; docs/18:18 says every PR
- `.github/workflows/ci.yml:242-260`; `nightly.yml:50-56` (only first-run); `web/playwright.config.ts:36-50`; docs/18:18, :905. `e2e/terminal.spec.ts:44-53` CSP test reads `next dev` header only.

## WEBA-3 (medium, CONFIRMED) — ANSI palette tokens are self-referential custom properties (`--term-color-N: var(--term-color-N)`) → every ANSI colour invalid in both themes
- `web/src/app/globals.css:223-242`; root values `:183-198`, `:205-220`; wterm `renderer.js:25-29`.
- Fix: root `--aurora-term-color-N`, map on `.wterm.term-pane`.

## WEBA-4 (medium) — during a wake the pane retries on every close code; 4403/4404/4409 never end it
- `terminal-tab.tsx:177-182` before classification `:184-196`.

## WEBA-5 (medium) — RETRYABLE 4502/4503 never retried: `wasClean` check returns first (API sends real close frames)
- `terminal-tab.tsx:68`, `:184`, `:190`; api `terminal/service.rs:186`, `:557`, `:581`; test uses `wasClean: false` everywhere (`terminal-tab-reconnect.test.tsx:31`).

## WEBA-6 (medium) — failed readiness query wedges the getting-started guide on "Checking what … can use…"; `readinessError` never read
- `web/src/lib/onboarding/use-guide.ts:81`, `:61`, `:89`; `guide.ts:75-82`; `getting-started.tsx:96-97`, `:140-143`.

## WEBA-7 (medium) — "Retry now" shown on a frozen agent's chat error block (C-615 "hidden on frozen agents")
- `web/src/components/chat/status-rows.tsx:151`, `:124`; `message-group.tsx:230-240`; `agents/agent-page.tsx:157`; cf. `computer-tab.tsx:226`.

## WEBA-8 (low) — failed secrets query makes the key step look missing (`use-guide.ts:77-80`; `guide.ts:86-97`)
## WEBA-9 (low) — `at_capacity`, `agent_start_failed` have no error copy (`lib/chat/transcript.ts:184-251`); C-581 omits both codes
## WEBA-10 (low) — share "Waking" status never refreshes after Start now (`lib/api/previews.ts:21-27`, `:57-63`)
## WEBA-11 (low) — admin nav renders for any signed-in user on an /admin path (`app-shell.tsx:437`; page guard is correct)
## WEBA-12 (low, PLAUSIBLE) — Terminal/Previews tabs shown without a `can_use` check (`agent-page.tsx:71-80`, `:167-173`; docs/14:677-678)
## WEBA-13 (low) — frozen agent terminal says "paused" (`terminal-tab.tsx:236-241`; tests assert /paused/)
## WEBA-14 (low, PLAUSIBLE) — wake retry loop has no cap (`terminal-tab.tsx:177-181`) vs wake limiter 12/h
## WEBA-15 (low) — Overview shows Loading forever when the workspace query fails (`overview-view.tsx:150-153`)
## WEBA-16 (low, owner question; same as LIFE-7) — pod Warning events + supervisor endpoint reach every can_use member (`hosted/routes.rs:300-315`, `:274-291`, `:186-190`; `computer-tab.tsx:36-76`; C-586 admin-only vs C-613)
## WEBA-17 (low) — keystrokes typed while disconnected dropped silently (`terminal-tab.tsx:304-308`)
## WEBA-18 (low) — light-theme ANSI white ~1.2:1 contrast (`globals.css:189-198`)
## WEBA-19 (low, docs) — WASM "~400 KB" vs 599,368 bytes; "hashed" vs version-named; sha256 matches the pinned npm package (docs/14:680, D-161)
## WEBA-20 (low) — `?draft=` refills the composer on every remount (`chat/composer.tsx:48-52`)

## Not covered
- visual identity at 390 px (no browser); full read of onboarding/context-switcher/harness-picker/chat-v2 tests and onboarding e2e specs; schedules-tab, instructions-tab; a11y with a screen reader.
