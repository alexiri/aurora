# SET-3: Admin › LLM endpoints — providers, endpoints and model allow-lists as live settings — **medium, phase 5**

## Goal
An instance admin configures the LLM providers and endpoints (`llm.enabled_providers`,
`llm.provider_preference`, `llm.endpoints`) on **Admin › LLM endpoints** instead of in Helm values,
with a reachability test, and the API and every gateway apply the change without a restart. A list
set in Helm stays locked as a whole.

## Linked requirements
- **D-236**, **D-237** (section `llm`, all three keys whole lists), D-238.
- Contract **C-779** (entry shape, endpoint test route, `secrets_become_non_permitted` warning);
  C-772..C-774 (generic routes).
- Spec: `15 §Instance settings` row `llm`; `15 §Configuration reference` rows `llm.*` (the entry
  rules, including P4-LLM provider keys when P4-LLM lands), `08 §6` (providers, LLM hosts, model
  selection, model readiness), `08 §11.2`/`§11.3` (re-evaluation, key origin), `06 §Caches`;
  `14 §Instance administration` (Settings); setup wizard `14 §Onboarding` (its "LLM endpoints" step
  links here).
- Security: SEC-89/SEC-90; credentials never in endpoint entries (as at startup).
- Migration numbers if needed: `20261201000194..000195` (none expected).
- Depends on **SET-1** and **SET-2** (gateway governance rebuild and the per-org re-evaluation job
  are reused).

## Context
- Config: `crates/aurora-config/src/{raw.rs,validate.rs}` (`LlmSection`, `LlmEndpoint`,
  `KeyOrigin`, `llm_endpoint_warnings`, `nearest_origin_method`, `known_llm_hosts`).
- API readers: `services/aurora-api/src/{app.rs,main.rs}`, `domains/hosted/model.rs` (model
  readiness, default model), `domains/onboarding/service.rs` (setup step), `domains/secrets/routes.rs`
  (LLM secret create: endpoint permitted, host derived).
- Gateway: `services/aurora-gateway/src/main.rs` (endpoint table at start), LLM host reservation and
  governance step (`governance.rs`, `resolve.rs`).
- App-instance probes' destination guard (pattern for the endpoint test):
  `domains/app_instances/**` (C-730 reasons).

## Scope
### In scope
1. **Registry entries** for section `llm` with the existing validation (credential-like keys
   refused, https only except loopback, `allowed_paths` rules, unknown `key_origin.method` with the
   nearest suggestion, provider-specific keys) — reuse the functions, never re-implement them. Each
   entry's `id` is unique; `default_model` warning becomes a preview warning. Update
   `movable-keys.txt`.
2. **Cross-section rules** run on the merged view: `governance.llm.require_key_origin` needs a
   `key_origin` on every endpoint (422 naming `/llm.endpoints/<id>/key_origin` or the governance
   key); `llm.provider_preference` ⊆ `enabled_providers`.
3. **API readers live**: all reads of `llm.*` through `state.settings()`; LLM secret creation, model
   pickers, model readiness and the setup wizard reflect a change without restart.
4. **Gateway readers live**: rebuild the endpoint table, reserved LLM hosts and the effective known
   LLM hosts list (built-in + `known_llm_hosts_extra` − endpoint hosts) on every `llm` swap, together
   with SET-2's governance rebuild, then flush as `all`.
5. **Effects on existing data**: removing an endpoint or narrowing its models/origin runs SET-2's
   per-org re-evaluation (secrets on a removed endpoint become non-permitted and are skipped by the
   gateway; owners notified; disallowed model overrides dropped). Preview warning
   `secrets_become_non_permitted` with counts per org, and `affected_orgs` listing those orgs.
6. **Endpoint test** `POST /v1/admin/settings/llm/endpoint-test` (C-779): one unauthenticated GET of
   the models path through the API's destination guard (`gateway.allowed_private` honoured), 10 s,
   reasons as C-779; never sends a credential or a stored key.
7. **Dashboard** `/admin/settings/llm`: enabled providers (checkbox group) and preference order
   (reorderable list with up/down buttons, keyboard accessible); endpoints as an `ItemList`, each
   opening a `FormDialog` (`xl`) with the provider-specific fields shown for the chosen provider
   (P4-LLM fields hidden until that provider is enabled in the build), allowed models, key origin,
   allowed paths, and **Test connection**; a locked list shows every endpoint read-only with
   "Configured in Helm (`config.llm.endpoints`)". The setup wizard's LLM step links here.
8. Docs: `docs/operator/llm-endpoints.md` (dashboard first; Helm as the locked alternative; what
   removing an endpoint does to existing keys).

### Out of scope
- New providers (P4-LLM), credential handling, org governance of endpoints (unchanged).
- Merging Helm and dashboard endpoints in one list (whole-list lock, D-237(e)).

## Constraints / file ownership
Owns: registry entries for `llm`, the LLM readers listed in Context, gateway endpoint-table rebuild,
`web/src/app/admin/settings/llm/**`, `web/src/components/admin/settings/llm-*.tsx`, the setup-step
link, `docs/operator/llm-endpoints.md`, `web/openapi.json`, `schema.d.ts`, `movable-keys.txt`.
After SET-2 (same gateway modules). Shared primitives, tokens, both themes, 390 px.

## Acceptance criteria
1. Unit: every startup validation case of `llm.*` (existing tests) also fails a PUT with 422 and the
   same key path; credential-like keys refused.
2. IT: add an endpoint live → an org can create an LLM secret on it and the gateway injects for it
   without restart (gateway IT); remove it → the secret is non-permitted, skipped by the gateway,
   owner notified; preview lists the org and the count.
3. IT: `require_key_origin = true` (test config) + an endpoint without `key_origin` → 422 naming it.
4. IT: a list set in the test config → GET shows it locked, PUT with `llm.endpoints` → 409.
5. IT: endpoint test — reachable fixture, `destination_refused` for a private address without
   `allowed_private`, `timeout`; no `authorization` header sent (fixture asserts).
6. IT: known LLM hosts: adding an endpoint on `api.openai.com` removes it from the known list in the
   gateway (a sanctioned-only org can then use the endpoint, not a generic secret on that host).
7. Web tests: provider checkboxes and order, endpoint dialog per provider, Test connection result,
   locked list rendering, preview warning; 390 px, both themes.
8. fmt, clippy, targeted api + gateway tests, full suites at the end, guards exit 0, web checks and
   production build.

## Suggested agent/model and reason
Sonnet (`rust-developer`, then `nextjs-developer`); Opus review of the gateway rebuild (host
reservation changes affect which requests can carry prompts).
