# P5-APIGAP-1: Wake a sleeping computer, and say when it sleeps

## Goal
Owner, 2026-11-19 (D-399). The agent page's Computer tab (P5-POLISH-6, D-382) was designed with **Wake it now**
and "Sleeps after n minutes of quiet", but the API has neither (confirmed by P5-POLISH-11 pass 2):
`POST /v1/agents/{agent}/sandbox/retry` and `restart` only leave `failed` (409 `sandbox_not_failed` otherwise,
`hosted/routes.rs`), and `SandboxView` has no idle field; the idle timeout is Helm config shown only on
Admin › Agents › Deadlines.

## Change
1. **Wake (C-1045):** `POST /v1/agents/{agent}/sandbox/wake` starts the agent's computer when it is asleep
   (stopped for idleness). Idempotent: already running or starting → 200 with the current `SandboxView`; started
   → 202 with it. `failed` → 409 `sandbox_failed` (the existing retry is the way out); frozen → 423 `frozen`
   (D-396); BYO agent → 409 `not_hosted`. Who: anyone allowed to start a turn with the agent (a turn wakes the
   computer today; this is the same start without a message). Reuse the start path a turn uses; no second way to
   start a sandbox. Rate-limit like the other sandbox actions. Audit as the other sandbox actions are.
   P6-WARMPOOL (D-379, warm on the first keystroke) will reuse this route; keep it cheap and side-effect free beyond
   the start.
2. **Idle (C-1046):** `SandboxView` gains `idle_stop_after_secs: int | null` (the effective idle timeout that will
   stop this computer; null when idle stop is off). Read it from the same config the runner applies.
3. **Web:** Computer tab: when asleep, **Wake it now** (one button, pending state until the status says starting /
   running, errors through the shared notice); the status line says "Sleeps after n minutes of quiet" from the new
   field (in words, `settings-words`), nothing when null. Replace the placeholders P5-POLISH-6 left (its Completion
   report lists them). Regenerate the API types.
4. Docs: `05` (route, view), `API_CONTRACTS` C-1045/C-1046, `14` (Computer tab), `15` if the idle setting's text
   changes.

## Tests
API ITs: wake from asleep → 202 and a start is requested from the runner (mocked as the restart ITs do); running →
200 no second start; failed → 409; frozen → 423; a member without turn rights → 403/404 as the sibling routes;
`idle_stop_after_secs` reflects the config and null when off. Vitest: the button and the line. Targeted only (CI is
the gate); `extract-schema --check`.

## Ownership
`services/aurora-api` hosted sandbox routes and view, the runner start path only if needed, `web` Computer tab.
Size M. D-399, C-1045, C-1046.

## Completion report
Done (2026-11-19, lane `p5-platform`).
- **Wake (C-1045).** `POST /v1/agents/{agent}/sandbox/wake` (`wakeAgentSandbox`, `hosted/routes.rs`): `can_use` through `load_agent` (404 for an agent the caller cannot see), BYO → 409 `not_hosted`, frozen → 423, `failed` → 409 `sandbox_failed`, `starting`/`running` → 200 with the view and no demand, otherwise 202. It calls `terminal::service::wake_demand`, the helper the terminal, Files and SSH already use (sets `sandboxes.terminal_wake_at`, notifies due work; the due-work start arm does the start), so there is no second start path; the same per-agent hourly budget applies and past it the answer is 429 `rate_limited`. A hosted agent without a sandbox row gets one (`unprovisioned`, `ON CONFLICT DO NOTHING`) so it can be woken. Audit `agent.sandbox_wake` (metadata `previous_state`), best-effort. No runner-plane change.
- **Idle (C-1046).** `SandboxView.idle_stop_after_secs` = `hosted.idle_stop_secs` (the value `claim_stops` receives), null when 0; set in both view constructors.
- **Web.** `useWakeSandbox`/`chatApi.wake`; Computer tab: **Wake it now** (hidden when awake, starting, failed or frozen; "Waking…" and disabled while the POST is in flight and, after the 202, until the status says starting/running, freed after 60 s if it never does; errors through `ErrorNotice`), the status line "Sleeps after 30 minutes of quiet." from `settings-words` `durationWords`, nothing when null. The static "sleeps when there is no work" copy P5-POLISH-6 left is replaced by it (the generic sentence stays only when the field is absent on a running computer).
- **Files.** `services/aurora-api/src/domains/hosted/{routes.rs,dto.rs}`; ITs `services/aurora-api/tests/it/sandbox_wake.rs` (new, registered in `main.rs`) and the route in `permission_matrix.rs` (`WsUse`); `web/openapi.json`, `web/src/lib/api/schema.d.ts` (regenerated); `web/src/lib/api/conversations.ts`, `web/src/components/agents/computer-tab.tsx`, `web/tests/agent-sandbox-failure.test.tsx`; docs `04` (audit action), `05`, `14`, `API_CONTRACTS` C-1045/C-1046, PROGRESS. `15` unchanged (the idle setting's text did not change).
- **Tests run.** See the gates line below.
- **Gates run (all passed).** ITs `sandbox_wake::*` (5: asleep → 202 and the demand set plus the audit row polled; running/starting → 200 and no demand; failed 409, frozen 423, BYO 409, foreign workspace 404; the 13th wake in an hour 429; `idle_stop_after_secs` 1200 and null for 0 and present on an agent without a sandbox row), `permission_matrix_routes_by_principal` and `coverage_every_openapi_route_is_listed` (the new route is `WsUse`), `groups::permission_matrix_per_route`, `openapi_matches_checked_in` (11 passed); vitest `agent-sandbox-failure` + `terminal-tab` (17 passed, incl. the two new tests); `cargo fmt --all --check`; `cargo clippy -p aurora-api --all-targets -- -D warnings`; `extract-schema.py --check`; `scripts/guards/run-all.sh`; web `pnpm typecheck`; eslint on the touched web files.
- **Not done.** Full suites and Playwright (CI is the gate; no Playwright spec asserted the replaced Computer-tab copy). The IT checks that the start is *requested* (the wake demand the due-work arm reads), not a runner start end to end: the demand and the arm are the terminal's, covered by the terminal and runner-plane ITs.
