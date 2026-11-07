# TEST-010: Permission-matrix test (route × role) — M1 acceptance

## Linked requirements
- `19 §M1` acceptance "permission-matrix test", `18 §Identity` ("Permission matrix test generated from `04 §Permission matrix` (every route × role), suspended members, workspace key confinement, CSRF"); `04 §Permission matrix`, `§The access law`; SEC-21, SEC-29.

## Design
- `services/aurora-api/tests/permission_matrix.rs`: a **table** of (method, path template, required permission) rows covering every management route implemented so far (derive the route list from `web/openapi.json` paths; a test asserts every OpenAPI path+method appears in the table or in an explicit `EXEMPT` list with a reason — so new routes can't silently skip the matrix).
- Principals: org owner, org admin, org member with workspace binding (member role), org member with workspace owner binding, org member without binding, suspended member, non-member, instance admin (non-member of the org), workspace API key (same workspace / other workspace), org API key of an admin and of a member.
- For each row and principal: send a request with minimal valid body (or an obviously invalid body — the assertion is about authorization, so accept 400/422 as "authorized" outcomes and 401/403/404 as "denied"); assert allowed/denied per `04` (invisible ⇒ 404, visible but insufficient ⇒ 403 `forbidden`, key on other workspace ⇒ 403 `workspace_mismatch`, session-only route with key ⇒ 403 `session_required`).
- Use the existing `Harness` helpers (`services/aurora-api/tests/common/mod.rs`).

## Acceptance
- [ ] `cargo test -p aurora-api --test permission_matrix` passes; route-coverage assertion passes; any genuine authorization defect found is **reported** (do not change production code) with the row and observed status.

## Dependencies: M1/M2/M3 API (done). Agent: haiku-tester (escalate to rust-developer if the harness setup or table generation is non-obvious).
