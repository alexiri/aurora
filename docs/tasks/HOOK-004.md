# HOOK-004 (part 1): Gateway ↔ hook host integration — `approval.summarize`, `token.mint` end to end with `fixture-hooks`

## Linked requirements
- M3/09 `§8.2` (summarize: gateway → hook host when a request is held and the tool's approval template says `summarize: hook`; no egress; 1 s wall; order hook → template → generic), `§8.3`, `06 §Approval holds` step 1, C-19 (W9 JSON), I-20, I-22; SEC-13 (plugin code never in gateway/API), SEC-14; `18 §Plugins` "Hooks: each hook point via `fixture-hooks`; egress denial; summarize with no egress; memory/time/output limits enforced".

## Context
- Gateway: `hold.rs::build_summary` has a hook seam (SUM-001); `mint.rs::HttpMinter` (GW-011) calls the hook host. Hook host: `services/aurora-hookhost` (HOOK-002; pulls snapshot + blobs from the API internal routes, C-18). Fixture: `plugins/fixtures/fixture-hooks` (+ `scripts/fixtures/build.sh` builds the guest and signed package).
- Config: `gateway.hookhost_url`; internal secret bearer.

## Scope (`services/aurora-gateway/src/{hold.rs,summarize_client.rs}` + wiring; `tests/e2e-gateway/tests/hooks.rs` (new) + helpers in `tests/e2e-gateway/tests/common/mod.rs`)
- Gateway summarize client (W9, 1 s budget incl. network; any error/timeout ⇒ fall back to template, then generic; never block the hold longer than the budget).
- E2E (real API + gateway + hookhost binaries, as TEST-012 does): upload + approve `fixture-hooks` (trust its test key via `POST /v1/admin/trusted-keys`), create a connection for the minting app directly in the DB (or via API if BE-043 landed), grant it; (a) a held request gets a hook-produced summary; (b) an expired credential triggers `token.mint` through the hook host and the request is injected with the minted token; (c) hook egress to an undeclared host is denied (hook error surfaces as `credential_unavailable`); (d) a summarize hook that loops falls back within ~1 s.

## Acceptance
- [ ] `bash scripts/e2e/gateway.sh` passes including the new hook tests; `cargo test -p aurora-gateway`; clippy, fmt, guards (gateway must not depend on wasmtime).

## Dependencies: HOOK-002, FIX-001, BE-041 (done). Agent: rust-developer (Sonnet).
