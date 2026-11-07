# TEST-018: Security invariant coverage (SEC-01..SEC-39)

## Goal
Phase-0 exit requires "every SEC invariant has a passing proving test" (`docs/19`). Map each of SEC-01..SEC-39 to the test(s) that prove it, and add the missing ones.

## Linked requirements
- `docs/17-security.md`: the invariant list, each with its statement and intended proof.
- `docs/19` Phase 0 exit.
- Known proofs so far (verify each before citing it):
  - SEC-01/02/09/24/25/26/34: `tests/e2e-hosted/tests/hosted.rs` (TEST-013), with SEC-02/09 also in `k8s.rs` (OPS-011)
  - SEC-35..39: gateway governance tests (GW-005, TEST-020)
  - SEC-22 (CSRF): API foundation tests
  - SEC-33: `scripts/guards/no-integration-names.sh`
  - SEC-18..20: TEST-016 (running in parallel; read its branch/merge state, and if not merged yet, mark those rows "pending TEST-016" rather than duplicating)
  - the permission matrix (`services/aurora-api/tests/permission_matrix.rs`) for authorization invariants
  - SEC-FIX-01 `spawn_env` for SEC-34

## Scope
### In scope
1. `docs/tests/security-coverage.md`: one row per SEC-nn with statement (short), proving test(s) as `path::test_name` (or guard script), level (unit/service/e2e/guard), and status (proven / added / pending). Each cited test must exist and pass, and must actually assert the invariant (not just touch the code path).
2. Add missing proving tests at the cheapest sufficient level, in the owning crate.
3. If an invariant is **not implemented** (the test cannot pass without production changes), implement a minimal fix if it's local and clear. Otherwise stop and report it precisely: which invariant, what's missing, and a proposed fix. Never weaken an invariant's test to pass.

### Out of scope
- Channel e2e (TEST-016), performance (OPS-014).

## Constraints / file ownership
- You own `docs/tests/security-coverage.md`, new tests (new files, or new functions appended to existing test files), and minimal local production fixes with justification.
- Other lanes (TEST-016 in `tests/e2e-hosted/tests/channels.rs`, OPS-014 in gateway benches/scripts) run in parallel. Don't edit their files.

## Acceptance criteria
- [ ] All 39 rows are "proven" or "added" (or "pending TEST-016" for SEC-18..20 only), each with a passing test.
- [ ] The touched crates' tests pass; fmt, clippy `-D warnings` and guards pass.

## Suggested agent/model
Sonnet, own worktree.
