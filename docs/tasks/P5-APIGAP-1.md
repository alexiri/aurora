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
