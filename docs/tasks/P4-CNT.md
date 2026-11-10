# P4-CNT: Shared rate-limit counters (`CounterStore`)

## Goal
Implement D-119 (SPEC-P4a): rate limits stop being per-replica approximations (D-05's N×) when the operator opts into a shared backend. Default behaviour (in-memory, per replica) is unchanged.

## Spec (source of truth; read, don't re-decide)
- D-119 in `docs/90-decisions.md`; SEC-62 in `docs/17-security.md`.
- `docs/05` (API limiter classes, the counters field of `GET /v1/admin/overview`), `docs/06` §Rate limiting, `docs/04`/`07` where they reference limits, `docs/08` (`ratelimit_key` HKDF purpose), `docs/15` (config `counters.*`, Helm, NetworkPolicy for Redis), `docs/03 §Planned changes` (`rate_limit_windows`, `rate_limit_buckets`, pruning), `docs/02 §Limits`.
- `docs/18` §Shared rate-limit counters (the tests).

## Scope
1. New crate `aurora-counters`: the `CounterStore` trait (`fixed_window_peek`, all-or-nothing `fixed_window_consume`, `bucket_take`, `health`) with backends `memory` (today's behaviour), `postgres` (the two `UNLOGGED` tables, dedicated small pool, store clock), `redis` (optional, TLS, credentials from env, no Cluster); a conformance suite run against all three (Redis in a container, gated like the other container tests).
2. Wire the gateway rule quotas and the API limiter (incl. unauthenticated, rejected-bearer and webhook limits) through it; the evaluator stays pure (counts passed in; consumption a separate atomic call).
3. Unavailable backend per class (`security`, `management`, `gateway`): `local` (default) or `closed` (503 `counter_unavailable`); `open` or any unknown value refuses startup (SEC-62). Keys hold ids and keyed hashes only.
4. Migration for the two tables (next timestamp after the newest file; `aurora-db` has a `build.rs`) and the `counters_prune` due-work arm; metrics `aurora_counters_*`; Helm `counters.*` and the NetworkPolicy egress rule; the overview counters field.
5. Contracts: write `docs/API_CONTRACTS.md` entries **C-400..C-409** only (section "Phase 4 — P4-CNT"). Operator doc in `docs/operator/limits-and-approvals.md`.

## Acceptance
- Every test listed in `18` for this item (conformance over three backends, two-replica tests for API and gateway, fault injection, startup refusals, key privacy); SEC-62 row `proven`.
- fmt, clippy `-D warnings`, `cargo test -p aurora-counters -p aurora-gateway`, full `aurora-api` (`--test-threads=8`), `bash scripts/e2e/gateway.sh` (ask the coordinator first), helm lint/template with `counters.backend=postgres` and `=redis`, guards, OpenAPI drift test.

## Constraints
- Never fail open; the default stays in-memory with no new infrastructure.
- Change files only with the Edit/Write tools; never python, sed, awk, perl, `cat >>` or heredocs. No `scripts/perf/*` (the coordinator runs perf on an idle machine; the gateway hot path changes, so say what you changed there).
