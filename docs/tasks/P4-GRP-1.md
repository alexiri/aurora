# P4-GRP-1: Directory groups (manual)

## Goal
Implement D-130/D-131 (SPEC-P4b): per-org groups, membership, group → workspace access bindings, group identities in policy, group → org-role mappings.

## Spec (source of truth; read, don't re-decide)
- D-130, D-131; SEC-71 (and its residual risk) in `docs/17`.
- `docs/04` §Groups (roles, access law, effective role = strongest of direct, group and IdP-claim sources, permission matrix, audit set, account deletion); `docs/03 §Planned changes` (`groups`, `group_members`, `group_workspace_access`, `effective_access`, gateway role SELECT); `docs/05` (Groups routes, `workspaces/{ws}/access?expand=groups`, bus kind `access`, gateway scope `user`, invalidation table); `docs/06` (principals, cache); `docs/07` (group identity, needs a speaker; warnings; effective/simulate); `docs/10` (visibility); `docs/14` (Groups page, workspace Access with group-derived members, policy picker, account); `docs/02` limits.
- `docs/18` §Groups (the tests).
- P4-IDP-1 (merged before this starts) adds `idp claim <value>` as an access source: keep the effective-access sources list consistent with it.

## Scope
Migration (next timestamp; `aurora-db` has a `build.rs`; move the rows out of `03 §Planned changes`), repos and `effective_access`; API routes, permission-matrix rows, audit (security-critical); gateway principal set + `user`-scope invalidation and the gateway role grants (keep `grants.rs` minimal, FIX-016 test must pass); policy evaluator (`{ "kind": "group" }`, never matches speakerless traffic, a deleted group matches nobody); web (org Groups page, workspace Access, policy picker); OpenAPI + `schema.d.ts`. Contracts: `docs/API_CONTRACTS.md` **C-460..C-469** only.

## Acceptance
Every test in `18 §Groups`; SEC-71 `proven`; permission matrix and OpenAPI drift tests; full `aurora-api` (`--test-threads=8`), `aurora-db` (incl. `gateway_grants`), `aurora-policy` corpus, gateway tests, `scripts/e2e/gateway.sh` (ask the coordinator first); web typecheck/lint/test/build + Playwright `stage4-routes`/`stage4-axe`/`stage2-narrow` for new screens; fmt, clippy `-D warnings`, guards.

## Constraints
Groups only add deliberately granted access (never create membership, never grant `owner`). Change files only with the Edit/Write tools; no python/sed/awk/perl/`cat >>`/heredocs. No `scripts/perf/*`.
