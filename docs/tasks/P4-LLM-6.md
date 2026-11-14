# P4-LLM-6: Endpoint destinations and the dashboard for the new providers — **medium–large, phase 5**

## Goal
Finish the provider work where people meet it: (A) apply D-262 so an LLM endpoint added in the
dashboard opens no private destination, plain-HTTP path or app host the instance has not opened,
with the endpoint test probing each provider correctly; and (B) give Admin › LLM endpoints and the
Credentials › LLM keys form the provider- and auth-mode-specific fields, the harness badges and the
assume-role external-id help.

## Linked requirements
- **D-262**, D-260 (a), (f), D-261 (b), (c); contracts C-800 (dashboard-only refusals), C-803,
  **C-811**, **C-813**, C-802 (consumed) — `docs/API_CONTRACTS.md` §Phase 5 — P4-LLM; C-772..C-774,
  C-779 (SET-3).
- `08 §Phase 4 providers` (*Endpoint destinations*, *Wire formats and harness reach*), `08 §LLM
  hosts`, `06` step 2 (implicit allowance only for Helm endpoints, never metadata/link-local),
  `15 §Instance settings` row `llm`, `23` (handing a private endpoint from Helm to the dashboard),
  `14` (Credentials › LLM keys P4-LLM paragraph; Settings › LLM endpoints bullet).
- Security: **SEC-98**, SEC-89/SEC-90 (settings). Tests: `18 §Additional LLM providers`
  (endpoint destinations), `18 §Dashboard visual design`.
- Migrations: none.

## Context
- SET-3's code: settings registry entry for `llm`, gateway endpoint-table rebuild on reload,
  `POST /v1/admin/settings/llm/endpoint-test`, `web/src/app/admin/settings/llm/**`,
  `web/src/components/admin/settings/llm-*.tsx`.
- Gateway guard: `services/aurora-gateway/src/guard.rs`; today `main.rs` pushes every endpoint host
  into `allowed_private` at start (the code this lane replaces).
- API destination guard (FIX-106) and settings preview/warnings (SET-1 `preview` service).
- App claims and instance registrations: registry snapshot host index, `app_instances`.
- Web: the LLM key form under `web/src/app/**/credentials/**` (LLM tab), `GET /v1/llm/providers`
  (C-802 from P4-LLM-1), `Org.external_id`; primitives per `14 §Visual design` (`RadioGroup`,
  `CodeBlock`, `Badge`, `Notice`, `FormDialog`, `SettingField`).

## Scope
### In scope
**Part A — destinations (Rust)**
1. Guard allowance by source: the gateway builds the implicit endpoint allowance from **Helm**
   entries only (the settings snapshot knows each entry's source), excluding metadata and
   link-local addresses (judge resolved addresses, so a name pointing at `169.254.169.254` is
   refused), and rebuilds it with the endpoint table on every `llm` or `limits` reload; dashboard
   entries are judged by `gateway.allowed_private` only.
2. Write validation (dashboard entries only, C-800): `http_needs_private_host`,
   `endpoint_host_claimed` (static claims via the snapshot host index, app-instance hosts),
   field paths per C-773.
3. C-813: GET marks `needs_allowed_private` (API resolver, 2 s timeout, 60 s cache), preview warns
   `endpoint_resolves_private`.
4. C-803: the endpoint test probes each provider's path; status < 500 is reachable; the D-262 rules
   apply to the probe.

**Part B — dashboard (Next.js)**
5. Admin › LLM endpoints dialog: provider field sets (OpenAI org/project; Azure resource URL and
   deployment names; Bedrock region and VPC URL; Vertex project and location; compatible base URL,
   auth header, models path), `allowed_paths` hidden for Bedrock/Vertex, harness `Badge`s per row
   and the "no enabled harness" `Notice`, `needs_allowed_private` badge linking to Limits, field
   errors on the base URL, preview warning in the save confirmation.
6. LLM key form: auth-mode `RadioGroup` from C-802 (`recommended` first); Bedrock assume-role
   fields with the org external id and a trust-policy snippet in a `CodeBlock` with Copy; Azure Entra
   fields; GCP key file import (exists per `14`; keep); verified account after save; per-endpoint
   harness hint; C-801 `details.reason` messages in plain language.
7. `docs/operator/llm-endpoints.md`: providers, auth modes per provider, the private-address rule
   and the Helm hand-over step (D-262), the LiteLLM route for cross-format use (D-260(a)).

### Out of scope
Provider modules and mint (P4-LLM-1..4), adapters (P4-LLM-5), any change to SET-3's generic settings
mechanics.

## Constraints / file ownership
Owns: gateway guard-allowance construction (`main.rs` part and its reload hook), the `llm` settings
validation hook for dashboard-only rules and GET/preview warnings in the API settings domain,
`web/src/app/admin/settings/llm/**`, `web/src/components/admin/settings/llm-*.tsx`, the LLM key
form components, `docs/operator/llm-endpoints.md`, `web/openapi.json`/`schema.d.ts`. Shared
primitives only; add a missing one to the shared set, never a one-off. Both themes, no horizontal
scroll at 390 px.

## Acceptance criteria (each test seen failing first)
1. Gateway IT: a Helm endpoint on `10.0.0.x` passes the guard; the same entry as a dashboard
   endpoint → `destination_refused` until `gateway.allowed_private` covers it (live, no restart);
   an endpoint resolving to `169.254.169.254` or an `fe80::` address is refused for both sources;
   removing an endpoint revokes its pass on the next request.
2. API settings IT: `http_needs_private_host`, `endpoint_host_claimed` (with `app_id`) on the right
   field paths; Helm entries are not re-validated by a PUT; GET `needs_allowed_private` and the
   preview warning for a dashboard endpoint on a private name; mirroring a private Helm endpoint to
   the dashboard yields the mark.
3. API IT: endpoint test per provider against the mocks (401 counts as reachable), and
   `destination_refused` for an uncovered private dashboard entry.
4. Web tests: provider field sets, hidden `allowed_paths`, harness badges and notice, field errors,
   preview warning; key form per auth mode (assume-role shows the external id and snippet; Entra
   fields; reasons rendered); axe at 390 px in both themes.
5. fmt, clippy, targeted api + gateway tests, guards exit 0, web lint/typecheck/tests and production
   build, no OpenAPI drift.

## Dependencies
SET-3 (settings page, endpoint test, reload) and P4-LLM-1 (C-800, C-802). The key-form modes for
assume-role and Entra are exercised end to end once P4-LLM-3/4 are merged (web tests use the API
schema and mocks before that).

## Suggested agent/model and reason
Sonnet (`rust-developer` for Part A, then `nextjs-developer` for Part B); Opus review of Part A
(a mistake opens internal addresses to every agent).
