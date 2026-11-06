# TEST-011: Playwright M1 acceptance — sign-up → create workspace (Daneel present) → invite → accept

## Linked requirements
- `19 §M1` acceptance: "Playwright sign-up → create workspace (Daneel present) → invite → accept"; `18 §Layers` Dashboard (Playwright flows).

## Context
- Dashboard `web/` (Playwright 1.65 config in `web/playwright.config.ts`, baseURL `http://127.0.0.1:4100`, smoke spec `web/e2e/smoke.spec.ts`). API runs as a host process (compose image builds are not needed): `. deploy/dev/env.sh` then `target/debug/aurora-api --config <config> serve`; web via `pnpm dev` (or `pnpm build && pnpm start` — prefer production build for stability).
- Email is log-only: invitation and verification links are written to the API's JSON log (INFO, message contains the link). The test must read them from the API log file.
- Invitation accept requires a **verified** email equal to the invitation email. With C-01, signing up via `/auth/signup?invitation=<token>` marks the email verified.
- Use a dedicated database: create `aurora_e2e` (psql on the dev server from `.env`), migrate it with `target/debug/aurora-api migrate`, drop it at the end. Config: copy `deploy/dev/aurora.dev.toml` to a temp file (signup_mode open, org_creation anyone, external_url `http://127.0.0.1:4100`, `auth.trusted_origins` as needed).

## Scope (`web/e2e/**`, `web/playwright.config.ts`, `web/package.json` scripts, `scripts/e2e/m1.sh`)
- `scripts/e2e/m1.sh`: builds (`cargo build -p aurora-api`, `pnpm build`), prepares the DB, starts API (log to a temp file) and web, exports `AURORA_E2E_API_LOG`, runs `pnpm exec playwright test e2e/m1.spec.ts`, tears everything down (kill by PID, drop DB) even on failure.
- `web/e2e/m1.spec.ts`: user A signs up (open mode) → lands in default workspace → creates workspace "Research" → sees agent "Daneel" on its overview → goes to org Members → invites b@example.test as member with the Research binding → reads the invitation link from the API log → in a fresh browser context user B opens `/join?token=…` → "Create account" → signs up with the invitation → accepts → lands in the org and sees "Research" with Daneel. Assert on visible UI text, not API calls.
- Install Chromium for Playwright (`pnpm exec playwright install chromium`; system deps are already present on Arch or report what's missing).

## Acceptance
- [ ] `bash scripts/e2e/m1.sh` passes locally (paste the Playwright summary); a deliberately broken assertion makes it fail and still tears down.
- [ ] No changes to application code — report UI bugs found instead (with selectors/screens), unless trivially a test-id addition (`data-testid`) in a component, which is allowed.

## Dependencies: FE-010..013 (done), BE-011/013 (done). Agent: nextjs-developer (Sonnet).
