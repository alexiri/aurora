# P5-AGENT-1: `agent_id` filter on the gateway approvals list

## Goal
Let the agent page's status rail list one agent's pending approvals without fetching and filtering the whole workspace (C-700). Additive; no client changes beyond the regenerated schema.

## Linked requirements
- Master spec IDs/sections: D-201 (status rail), C-700 (`docs/API_CONTRACTS.md` §Phase 5 — P5-AGENT), `05 §Approvals` row, `05 §Conversation visibility (phase 1, RFE-009)`.
- Requirement summary: `GET /v1/workspaces/{ws}/approvals?status=&agent_id=` returns only that agent's approvals, with the same semantics as `GET /v1/workspaces/{ws}/action-approvals?agent_id=`.

## Context
- Handler: `services/aurora-api/src/domains/approvals/routes.rs` (list route, `Query<ListQuery>`, `service::validate_status`).
- Query DTO: `services/aurora-api/src/domains/approvals/dto.rs` `ListQuery { status, limit, cursor }`.
- Pattern to copy: `services/aurora-api/src/domains/action_approvals/dto.rs` `ListQuery { status, agent_id, limit, cursor }` and its repository query.
- Visibility: approvals naming a conversation the caller cannot see are omitted from lists (`05 §Conversation visibility`). The new filter is applied in the same SQL query, before pagination, never as a post-filter.
- Tests: `services/aurora-api/tests/it/approvals.rs`, `services/aurora-api/tests/it/direct_visibility.rs`.

## Scope
### In scope
- `agent_id: Option<Uuid>` on the approvals `ListQuery`, documented in the OpenAPI params (utoipa `IntoParams`).
- Repository query filters `agent_id = $n` when present; cursor pagination unchanged.
- An agent of another workspace or an unknown id: empty page (200), never 404. A malformed UUID: the existing query-validation error of this router (same as a malformed `cursor`).
- Regenerate the checked-in OpenAPI document and the web client schema: `UPDATE_OPENAPI=1 cargo test -p aurora-api --test it openapi_matches_checked_in` (writes `web/openapi.json`), then `cd web && pnpm gen:api` (writes `src/lib/api/schema.d.ts`); CI's "API client drift" step must pass.

### Out of scope
- Any web change other than the regenerated `schema.d.ts` (P5-AGENT-2 owns the UI).
- Other approval routes (`pending`, `stream`, decisions), action approvals (already filterable).

## Constraints / file ownership
- Owns: `services/aurora-api/src/domains/approvals/**`, `services/aurora-api/tests/it/approvals.rs` (and a case in `direct_visibility.rs`), `web/openapi.json`, `web/src/lib/api/schema.d.ts`.
- No migration (check `EXPLAIN` on a seeded DB: if the list query does not use an index with the filter, add `(workspace_id, agent_id, created_at)` only after asking the coordinator; a migration takes the next free number at merge).
- Do not weaken the visibility predicate or the `can_approve` check.

## Acceptance criteria
- [ ] `?agent_id=A` returns only approvals of agent A; without it the response is byte-for-byte today's.
- [ ] `?agent_id=` of an agent in another workspace, or an unknown UUID → 200 with an empty page.
- [ ] Malformed `agent_id` → the router's existing validation error.
- [ ] An approval naming another member's direct conversation stays omitted with the filter (IT in `direct_visibility.rs`).
- [ ] Pagination: with more than `limit` matching rows, the cursor walks only the filtered set (IT).
- [ ] `status` and `agent_id` combine (IT).
- [ ] OpenAPI and `schema.d.ts` regenerated; drift guard green.

## Verification
- Commands: `cargo fmt --check`; `cargo clippy -p aurora-api --all-targets -- -D warnings`; `cargo test -p aurora-api --test it approvals` and `… direct_visibility`; `cargo test -p aurora-api --test it openapi_matches_checked_in`; `cd web && pnpm gen:api && git diff --exit-code src/lib/api/schema.d.ts` after committing the regenerated file.
- Expected results: all pass; new ITs fail on the code before the change.

## Dependencies
- Blocked by: none (phase 5 starts on the owner's word; this one is ready).
- Unblocks: P5-AGENT-2 (the Needs you part of the rail).

## Suggested agent/model and reason
`rust-developer` (Sonnet) — small, but touches a permission-filtered SQL list and its ITs.

## Completion report
- Files changed:
- Tests/checks:
- Remaining risks:
