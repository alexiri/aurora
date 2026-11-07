# SEC-FIX-03: Enforce the permission matrix on reads (TEST-010 findings)

## Goal
Make every read route of matrix-gated resources apply the same role check as its writes (decision I-27), and turn TEST-010's `EXPECTED_DEFECT` list into hard assertions.

## Linked requirements
- `docs/04-identity-tenancy.md` §Permission matrix, §The access law; `docs/05-control-plane-api.md` §Request scoping (404 vs 403); decision **I-27** in `docs/90-decisions.md`.
- TEST-010 (`services/aurora-api/tests/permission_matrix.rs`), which found the defects.

## Findings to fix (production code)
Currently readable by principals who must be denied:
- Org owner/admin only: `GET /v1/orgs/{org}/policy`, `…/policy/generations`, `…/policy/warnings`, `GET /v1/orgs/{org}/secrets`, `GET /v1/orgs/{org}/connections`, `GET /v1/orgs/{org}/oauth-clients`, `GET /v1/orgs/{org}/app-availability`. Plain members, members with only a workspace binding, and org API keys of plain members currently get 200.
- `can_own` only: `GET /v1/workspaces/{ws}/policy`, `…/policy/generations`, `…/policy/warnings`, `GET /v1/workspaces/{ws}/secrets`, `…/connections`, `…/oauth-clients`, `…/access`, `GET /v1/agents/{agent}/grants`. Workspace members (non-owner) and their keys currently get 200.
- Also check the single-resource reads of the same kinds against the owning scope (`GET /v1/secrets/{id}`, `GET /v1/connections/{id}`, `GET /v1/connections/{id}/grants`, `GET /v1/oauth-clients/{id}` if present, `GET /v1/secrets/{id}/models`). Apply the same rule. Exception: a resource a `can_use` member legitimately needs for an agent flow named in the spec (e.g. LLM model catalogue for the agent model picker). If you find such a case, keep it, document why in the completion report, and add a matrix row with the intended expectation.
- Status code: follow the existing pattern of each domain and `05 §Request scoping`. When the caller can see the parent (an org member reading an org sub-resource), answer **403** `forbidden`; otherwise 404. Be consistent with the corresponding write routes.

## Test changes (`services/aurora-api/tests/permission_matrix.rs`)
- Remove every `EXPECTED_DEFECT` entry for the routes above; they must now fail the test if the defect returns.
- `GET /v1/governance/instance`: change the row expectation to "any authenticated principal" (I-27) and drop it from `EXPECTED_DEFECT`.
- `GET /v1/workspaces/{ws}/audit`: expectation `can_own` (I-27) and drop it from `EXPECTED_DEFECT`.
- `GET /v1/admin/registry/snapshot`: seed an active snapshot (startup activates an empty one in the API; reuse whatever the plugins tests do), or treat 404-without-snapshot as allowed for the instance admin only. Remove it from `EXPECTED_DEFECT`. `EXPECTED_DEFECT` must end up empty, and then you delete it.
- Reduce `INCONCLUSIVE (validation before authz)` skips: for write rows, send a minimal **valid** body per row (add a `body` field to `Row` or a per-path body table) so the denial side is actually exercised. Report how many rows remain inconclusive and why. Do not reorder production extractors to run authorization before body validation unless a route leaks existence through that order. If one does, fix it and report it.
- Update the existing domain tests (`tests/policy.rs`, `secrets.rs`, `grants_connections.rs`, `apps_connections.rs`, …) that relied on member-level reads.

## Frontend check
- `web/`: pages that load these lists for non-owners (credentials, policy, access tab, org pages) must handle 403/404 without crashing. Grep the hooks/pages that call these operations. If a page would break, show the existing "no access" / empty state pattern and don't add new UI. Run `pnpm -C web test` and `pnpm -C web typecheck` (or the repo's equivalent scripts in `web/package.json`).

## Constraints / file ownership
- You own: `services/aurora-api/src/domains/{policy,secrets,connections,oauth_clients,availability,tenancy,grants,audit}/**`, `services/aurora-api/tests/{permission_matrix,policy,secrets,grants_connections,apps_connections,tenancy,audit}.rs`, `web/src/**` only where a page breaks.
- Use `aurora-db::authz` (the single access-law implementation). Don't hand-roll role checks.
- Never loosen a test expectation to make it pass; if the spec seems to want member reads somewhere, stop and ask.

## Acceptance criteria
- [ ] All findings above deny the unauthorized principals and keep allowing the authorized ones (matrix proves it).
- [ ] `EXPECTED_DEFECT` is gone; the matrix passes; the inconclusive count is reported and below 10, or each remaining one is justified.
- [ ] `cargo fmt --check`, `cargo clippy -p aurora-api --all-targets -- -D warnings`, `cargo test -p aurora-api` all pass, and web tests and typecheck pass.

## Dependencies
- Blocked by: TEST-010 (committed with the defect list), BE-062/063 (committed)
- Unblocks: M1 acceptance

## Suggested agent/model
Sonnet, in its own worktree (touches many aurora-api domains).
