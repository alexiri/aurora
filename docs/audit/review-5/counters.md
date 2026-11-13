# REVIEW-5 findings — counters (saved by the coordinator from the reviewer's hand-back)

## CNT-1 (high, CONFIRMED) — SCIM limiter refunds a token on every refusal; a flooding SCIM token passes ~half its requests
- `services/aurora-api/src/domains/scim/rate_limit.rs:55-58`, `:133-137`, `:150-156`; `services/aurora-api/src/http/counters.rs:149`, `:173` (take_shared already refunds); `http/apilimit.rs:408-410`, `:717-720` (/v1 path doesn't refund on local refusal).
- Spec: docs/05 §Shared counters "Local pre-filter: a refusal there is final"; "A store refusal refunds the local token".
- Test gap: `tests/it/scim_scope.rs:306-345` stops at the 6th request.

## CNT-2 (medium, docs) — documented Redis ACL too narrow for the Lua scripts (needs TIME, INCRBY, PEXPIRE, SET)
- `docs/15-deployment-operations.md:628`; `crates/aurora-counters/src/redis_store.rs:20,28,34,46,57-58,65,75,88`.

## CNT-3 (medium) — gateway relays any `Upgrade` protocol as a byte pipe after 101; spec describes WebSocket only
- `services/aurora-gateway/src/hygiene.rs:47-52`; `proxy/mod.rs:267-271`, `:407-421`; `upstream.rs:320-326`. docs/06:554-556.
- Scenario: `Upgrade: h2c` on plain http upstream → post-101 bytes bypass the pipeline.

## CNT-4 (medium, PLAUSIBLE) — one breaker + one Postgres pool for `security` and `management` classes; anonymous flood can open the breaker → `management=closed` becomes API-wide 503
- `crates/aurora-db/src/counters.rs:236-293`; `services/aurora-api/src/http/counters.rs:50-58`; `apilimit.rs:650-654`; `crates/aurora-counters/src/guard.rs:105-156`.

## CNT-5 (medium, test-gap/docs) — Redis conformance suite never runs in CI and passes vacuously; docs/18:422 says CI runs it
- `crates/aurora-counters/tests/it/conformance.rs:658-662`, `:678-687`, `:689-694`; `.github/workflows/*.yml`.

## CNT-6 (low) — webhook refusal `Retry-After` clamped to 10 s (`http/counters.rs:229-234`)

## CNT-7 (low) — non-numeric Redis counter value read as 0 / full bucket (fails open) — `redis_store.rs:28`, `:46`, `:75`; SEC-62

## CNT-8 (low) — every refused CONNECT auth logs at warn with no limit — `services/aurora-gateway/src/proxy/mod.rs:459-467`, `:217-245`; compare `:145-168`

## CNT-9 (low, PLAUSIBLE) — caller disconnect during a breaker probe counts as a store failure and re-arms cooldown — `guard.rs:116-128`, `:159-167`

## CNT-10 (low) — gateway builds/warms the Postgres counters pool even when `gateway` class is memory — `services/aurora-gateway/src/main.rs:246-251`; `crates/aurora-db/src/counters.rs:244-257`

## Not covered
- sqlx rollback timing under timeout; Expect/trailers/Connection-nominated on the wire; database proxy lane; h2 pooled path; Helm counters cases; rule-quota tests; Redis scripts not executed.
