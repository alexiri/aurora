# REVIEW-5 findings — evidence (saved by the coordinator from the reviewer's hand-back)

## EVID-1 (high, CONFIRMED by coordinator) — preview wake never waits; 502 instead of wait page / 429
- relay `state.rs:112` always `resolve(label, hash, 0, navigation)`; `api.rs:191-205` long-polls only when wait_secs > 0; `edge.rs:393-400` reads only `decision == "allow"`, never `route`; `state.wake_wait_secs` stored (state.rs:43/63) and never read.
- api `domains/relay/service.rs:316-325`: over the wake limit → `route.start = "limited"`, still `decision: allow` (`let _ = limited;`).
- api `relay/service.rs:~588`: `streams` → `not_running()` unless running; relay `edge.rs:613-620` → 502 "The sandbox is not running."
- Spec: docs/11 §Wake and start step 2 (lines 452-453): wait ≤ PREVIEW_WAKE_WAIT 45 s, else self-refreshing 503 page; docs/11 ~441: 429 `preview_wake_limited` "try later" page; docs/16 `POST /resolve` wait_secs.
- Note: FIX-073 (in flight) lists "the wake-wait page while the sandbox starts" as a missing test and may hit this.

## EVID-2 (medium) — PREVIEW_LISTEN_WAIT unused; `listening` never enforced
- `limits.rs:260` only definition; `relay/service.rs:571-600` streams checks sandbox + runner_endpoint only; relay `api.rs:76` parses `listening`, unused. Spec docs/11:459, docs/02:309. (FIX-074 names the constant as "specified but unused — reconcile".)

## EVID-3 (medium) — SCIM token expiry job writes only an audit row; spec promises a notice to owners
- `services/aurora-api/src/jobs.rs:95-140`. Spec docs/05:356, C-471.

## EVID-4 (low) — SCIM management routes (C-471, C-472) absent from web/openapi.json
- `scim/management.rs:478-484` plain axum routes; `app.rs:340` merged after OpenApiRouter build. Also C-616 terminal WS absent (WS — say so in the contract).

## EVID-5 (low) — `AURORA_CODEX_BIN` test override read by the release supervisor
- `services/aurora-supervisor/src/adapters/codex/launcher.rs:17-19,44`.

## EVID-6 (low) — vendored `web/public/ghostty-vt-0.5.10.wasm` has no recorded source/checksum
- `terminal-tab.tsx:27`; docs/14:680.

## Not covered by this reviewer (first pass)
- D-119..D-167 as a set; SEC-63..67, SEC-71..79 test-by-test; status/error-code parity C-400..C-635; `.ok()`/`unwrap_or_default()` sweep; FEATURES vs code; logging sweep; e2e scripts.

# Second pass (EVID-7..17)

## EVID-7 (medium, PLAUSIBLE) — full grant allowed on a connection whose app is missing from the active snapshot (restrictions default to none) → bypasses custom_only (SEC-67)
- `services/aurora-api/src/domains/grants/service.rs:579-583`, `:240-246`; `grants/effective.rs:141`. Fix: fail closed (refuse `full` when the app is not in the active snapshot; evaluator treats a missing app as custom_only + block_untooled).

## EVID-8 (low) — invalid SCIM domain returns `invalid_request`, not contracted `scim_domain_invalid` (`scim/management.rs:95-131`, `:147`; C-472)
## EVID-9 (low) — runner `404 not_here` (C-617) is a bare status line, no error code (`services/aurora-runner/src/stream_listener.rs:745-751`, `:883-886`)
## EVID-10 (low) — `aurora_preview_shares` gauge labelled with the org slug (`services/aurora-api/src/jobs.rs:167-175`; docs/15:946 says `{state}`)
## EVID-11 (medium, test-gap) — SEC-72 "audit failure = nothing applied" has no SCIM case in `services/aurora-api/tests/it/audit_atomic.rs:227-333` (static scan `:51-118` only)
## EVID-12 (low, test-gap) — relay smuggling corpus tests a classifier on hand-built headers, not the wire (`services/aurora-relay/src/edge.rs:1107-1135`, `:893`); SEC-85
## EVID-13 (low, test-gap) — no test that a deleted group's rule matches nobody (SEC-71; corpus `p4grp_01..07`; `tests/it/groups.rs:296-305`)
## EVID-14 (medium, test-gap/docs) — P4-GRP-2 / P4-PRV-2 / P4-TERM acceptance requires SEC-72/73/81/82/85/87 `proven`, table says pending (FIX-074 overlap); not located: "older supervisor ignores pty.*" test; "terminal bytes never in any log" (only `terminal.rs:396-398`)
## EVID-15 (low, docs) — docs/18:487 wrong SCIM fixture path (files are in `services/aurora-api/tests/fixtures/scim/`)
## EVID-16 (low, docs) — FEATURES.md:145 overstates SEC-69 ("proven"; coverage says API half); FEATURES.md:156 omits IdP-linked groups (D-156)
## EVID-17 (low, docs) — D-122 header not amended by D-138; D-160(a) not marked amended by D-164(b)

## Still not covered
- SEC-63..67, 81/82/85/87 clause by clause; D-142..D-167 in full; reverse-direction error-code parity; full `.ok()`/`let _` sweep; FEATURES beyond 65 sampled rows.
