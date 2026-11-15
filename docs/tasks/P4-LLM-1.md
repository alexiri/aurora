# P4-LLM-1: Provider framework, `openai`, `openai_compatible` and the `anthropic` path set — **large, phase 5**

## Goal
Grow `aurora-llm` from two providers to the shared framework every additional provider plugs into,
and ship the two key-only providers on it: **`openai`** (fixed `api.openai.com`) and
**`openai_compatible`** (any OpenAI-style server). Land, once and for all five new providers, the
shared vocabulary (auth modes, dialects, recipes), the endpoint-entry validation (C-800), the error
classification table (C-808), the inference path sets, and the test-only upstream overrides, so the
provider lanes P4-LLM-2..4 only add their own module and gateway mechanics. Give `anthropic` the
inference path set SEC-65 promised for phase 5.

## Linked requirements
- D-121, **D-260** (a)–(f); contracts C-800, C-802, C-805, C-806 (header rows), C-808, C-809
  (`openai`, `openai_compatible` rows), C-810 (`openai`, `openai_compatible`), C-812 —
  `docs/API_CONTRACTS.md` §Phase 5 — P4-LLM.
- `08 §Phase 4 providers` (table rows `openai`, `openai_compatible`; *Shared mechanics*:
  inference-only paths incl. the `anthropic` set and the one meaning of `allowed_paths`,
  `model_from_request`, streaming, error classification), `08 §LLM secrets`, `06` step 6a and
  step 11 (`header` recipe), `15 §Configuration reference` (`llm.endpoints[]` provider keys,
  `llm.enabled_providers` additions, `llm.dev_upstream_overrides`), `16` (`ModelChoice` vocabularies).
- Security: SEC-64, SEC-65 (`17`). Tests: `18 §Additional LLM providers`.
- Migrations: none (D-260; block `20261201000210..219` stays unused).

## Context
- `crates/aurora-llm/src/`: `types.rs` (`Dialect`, `AuthMode` = `ApiKey` only, `InjectionHeader`,
  `LlmInjection`, `LlmProvider` trait), `registry.rs` (`builtin()`, `path_permitted` restricted to
  `litellm|openai_compatible`, `accepts_injection`, `resolve()`), `model.rs`
  (`inference_path_permitted`, `model_from_request`), `anthropic.rs`, `litellm.rs` (the patterns to
  copy), `error.rs`, `governance/validate.rs`.
- Config: `crates/aurora-config/src/raw.rs` (`LlmEndpoint` with `deny_unknown_fields`),
  `validate.rs` (endpoint rules, `nearest_origin_method`, `known_llm_hosts`).
- Gateway: `services/aurora-gateway/src/governance.rs` (step 6a, `llm_path_not_permitted`),
  `inject.rs` (`Injection::Llm` strips `authorization`/`x-api-key` then sets Bearer or `x-api-key`),
  `reqlog.rs` (request-log `error_code`), metrics module.
- API: `services/aurora-api/src/domains/secrets/**` (LLM secret create: validation, origin),
  `domains/hosted/model.rs` (`client_env(ep, dialect, &AuthMode::ApiKey)` — must pass the key's
  real mode), the `/v1/llm/providers` handler.
- Tests: `crates/aurora-testkit/src/responses.rs` (mock Responses upstream, the pattern for a mock
  OpenAI), `tests/e2e-gateway/tests/gateway.rs` (`litellm_server()` pattern), guard
  `scripts/guards/no-deployment-defaults.sh`.

## Scope
### In scope
1. **Vocabulary (all providers, no behaviour yet for the cloud ones)**: `AuthMode` gains
   `aws_sigv4`, `aws_assume_role`, `gcp_service_account`, `azure_entra` (parse/as_str, serde);
   `Dialect` gains `anthropic_bedrock`, `anthropic_vertex`; `LlmInjection` grows into the recipe set
   of C-806 (`Header { name }`, plus enum variants `AwsSigV4`, `AwsAssumeRole`, `GcpToken`,
   `EntraToken` that later lanes implement — the gateway refuses an unimplemented variant with 502
   `credential_unavailable` and a `debug_assert`). Trait additions of `08 §6`: `inference_paths`,
   `classify_error`, and `auth_modes()` returning the modes per provider.
2. **Endpoint entries (C-800) for all five providers** in `aurora-config`: the provider keys, every
   refusal code, `allowed_paths` adds-only and refused on `bedrock`/`vertex`, the http rule for
   `openai_compatible` (Helm entries: `http` allowed like `litellm`; the dashboard-only part of D-262
   is P4-LLM-6), credential-named keys refused (`client_secret` added). One function, so SET-3's
   settings writes reuse it unchanged. `enabled_providers` accepts the five ids; the default is
   unchanged; providers enabled but missing from `provider_preference` are appended.
3. **Path sets** as code: `anthropic` (D-260(c)), `openai`, `openai_compatible` (derived from
   dialects, relative to the base path; `["/*"]` opt-out), `litellm` unchanged; `path_permitted`
   uses the provider's set instead of the `litellm|openai_compatible` match. Provide the sets for
   `azure_openai`, `bedrock`, `vertex` as pure functions with their corpus tests (C-805) so the
   provider lanes only wire them.
4. **`openai` provider**: fixed base, `organization`/`project` headers set from the entry and
   removed from the agent request, `GET /v1/models` listing with static fallback, key validation via
   strict listing, `provider_account` origin from the `openai-organization` / `openai-project`
   response headers and the response backstop (`origin_from_response`), `client_env` (C-809).
5. **`openai_compatible` provider**: configured base with optional prefix, `auth_header`,
   `models_path`, dialects (default `["openai_chat"]`), origin `endpoint` only, `client_env` per
   dialect (OpenAI form, or Anthropic form for `anthropic_messages`).
6. **Classification** (C-808): `aurora_llm::classify(provider, status, error_type)` with every row
   of `08` for all five providers; the gateway calls it on injected LLM responses with the status
   and (Bedrock only) `x-amzn-ErrorType`, writes the request-log `error_code`, and counts
   `aurora_gateway_llm_upstream_total{provider,endpoint,class}`. Never alters status or body; never reads
   the response body.
7. **`GET /v1/llm/providers`** (C-802): `auth_modes[]` with `value_shape` and `recommended`,
   per-endpoint `auth_modes`, `recommended_auth_mode`, `harnesses` (from
   `hosted.enabled_harnesses` and the harness dialect table in `aurora-protocol`/supervisor
   registry — read the declared lists, do not duplicate them).
8. **Model readiness and dispatch**: pass the secret's real `llm_auth_mode` to `client_env`; a key
   whose mode the endpoint does not admit is refused at save (422 `llm_key_invalid`, reason
   `auth_mode_unsupported`, C-801); readiness is otherwise unchanged.
9. **`llm.dev_upstream_overrides`** (C-812): parsed in `aurora-config`, refused unless every target
   is loopback and `gateway.insecure_loopback_injection` is true, refused inside Kubernetes; the
   registry applies it to fixed bases (`openai`, `anthropic`) and exposes a resolver the mint and
   Bedrock lanes reuse. Add it to `no-deployment-defaults.sh`; it is not a settings key (if SET-1 has
   landed, add it to the registry's excluded list with a test).
10. Docs: move nothing out of "phase 5" wording except what this lane makes true; note in
   `docs/operator/llm-endpoints.md` (if it exists, else in `15`) the two providers.

### Out of scope
Azure, Bedrock, Vertex modules and every minted or signed mode (P4-LLM-2..4); supervisor adapters
(P4-LLM-5); dashboard and D-262 destination rules (P4-LLM-6).

## Constraints / file ownership
Owns: `crates/aurora-llm/**` except the future `azure_openai.rs`, `bedrock.rs`, `vertex.rs`,
`mint/**`; `crates/aurora-config/src/{raw.rs,validate.rs}` (LLM parts); gateway `governance.rs`
(path step), `inject.rs` (LLM header recipes), `reqlog.rs`/metrics (LLM class); API
`domains/secrets/**` (auth-mode dispatch, `api_key` shape), `domains/hosted/model.rs`
(auth-mode pass-through), the providers route; `crates/aurora-testkit/src/llm/{mod.rs,openai.rs}`;
`scripts/guards/no-deployment-defaults.sh`; `web/openapi.json` and `schema.d.ts` regenerated.
Nothing changes for a default configuration. No `unsafe`.

## Acceptance criteria (each test seen failing first)
1. Unit (`aurora-config`): every C-800 refusal code for every provider, with the key path in the
   message; `allowed_paths` on `bedrock`/`vertex` refused; `http://` accepted for
   `openai_compatible` (Helm) and refused for `openai`/`azure_openai`; `client_secret` key refused;
   default config parses and validates unchanged.
2. Unit (`aurora-llm`): path corpus per provider (allowed and refused routes of `18`, including
   `anthropic` `/v1/files` and `/v1/messages/batches` refused, `openai` files/fine-tuning refused,
   compatible derived sets and `/*` opt-out, Azure/Bedrock/Vertex pure sets incl. other
   project/location); `classify` table for every documented error; `client_env` contains no
   key-shaped value.
3. Gateway IT (`gw/it/llm_openai.rs`, `llm_compatible.rs` against `aurora-testkit` mocks): header
   replacement (agent `Authorization`, `x-api-key`, `api-key`, `OpenAI-*` removed; configured org
   and project set), SSE passthrough byte-for-byte, `llm_path_not_permitted` before policy and
   before the value is opened, `model_from_request` with allow-list (truncated body refused),
   request-log class for 401/429/500 and the metric, upstream status/body unchanged.
4. Gateway IT: `anthropic` endpoint now refuses `/v1/files` with `llm_path_not_permitted`; an
   `allowed_paths = ["/v1/files"]` entry admits it.
5. API IT: LLM secret on `openai` validates via listing; `provider_account` rule with a foreign
   `openai-organization` → 422 `llm_key_wrong_organization`; response backstop marks the key; a
   compatible key validates via `models_path`; `GET /v1/llm/providers` shape (C-802) incl.
   `harnesses` for both shipped harnesses; the turn's `ModelChoice` for a Codex agent on `openai`
   carries C-809's env.
6. e2e-gateway (`tests/e2e-gateway`, real binaries): `openai` reached through
   `llm.dev_upstream_overrides`; a config with the override and `insecure_loopback_injection =
   false` refuses to start.
7. Default unchanged: existing `aurora-llm`, governance and gateway suites pass untouched.
8. fmt, clippy `--workspace`, targeted tests, guards exit 0, OpenAPI drift test.

## Dependencies
None (SET-3 independent: if SET-3 merged first, its settings writes pick up the new validation
through the shared function; if not, SET-3 does). Blocks P4-LLM-2..6.

## Suggested agent/model and reason
Sonnet (`rust-developer`): multi-crate, security-relevant but well specified; Opus review of the
path sets and header strip lists.

## Completion report
Commits: `c1a36c0b`, `8acb4f24`, `a16fc0c6`, `1052f85e` and the follow-up `P4-LLM-1: governance IT mock serves SSE only for the openai endpoint` (branch `alexiri/p5-agent`). Gate level: **batch gate 1**.

- **What changed.** Provider framework in `aurora-llm` (auth modes, cloud dialects, injection recipes, `classify`, inference path sets incl. `anthropic`, `openai`, `openai_compatible`), C-800 endpoint rules in `aurora-config/src/llm_endpoint.rs` (one function for Helm and later settings writes), `llm.dev_upstream_overrides` (C-812), gateway per-mode recipe with prefix strips and fixed headers, request-log `error_code` and `aurora_gateway_llm_upstream_total` (D-248(b), renamed from the unprefixed name), API auth-mode checks at save, per-key auth mode into `client_env`, `GET /v1/llm/providers` (`auth_modes` stays `string[]` plus `auth_mode_details[]`, D-248(a)), `value_invalid` (D-248(c)). OpenAPI and `schema.d.ts` regenerated.
- **Tests added.** `aurora-config` C-800 refusals; `aurora-llm` path corpus, `classify` table, `openai`/`openai_compatible` ITs; gateway `governance::anthropic_key_is_injected_only_on_inference_paths`, `governance::openai_key_pipeline_headers_paths_models_sse_and_classification` (header replacement, SSE byte for byte, path refusal with the store down, model allow-list incl. truncated body, 401/429/500 request-log class with status and body unchanged); API `secrets.rs` ITs (listing validation, `provider_account` rule, `auth_mode_unsupported`, providers shape); e2e-gateway `s20_openai_endpoint_reached_through_the_dev_upstream_override` (real binaries, key listing at the override target, header replacement, 429 unchanged, `aurora_gateway_llm_upstream_total` scraped with `class="ok"` and `"rate_limited"`) and `s21_dev_upstream_override_without_insecure_loopback_refuses_to_start`.
- **Notes.** The override replaces the fixed origin for the API (listing) and for the gateway's host match and injection, so an e2e agent addresses the loopback target directly. The gateway IT recognises `openai` upstream traffic by the project header the gateway sets. Docs: `docs/operator/llm-endpoints.md` (providers, keys, `allowed_paths`); `08`, `12`, `15` and `18` already carried the spec batch and D-248's renames, so no further edits were needed.
- **Gate 1 results.** `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `scripts/guards/run-all.sh`, `extract-schema.py --check`, `openapi_matches_checked_in` (1 passed), `aurora-llm` (59 lib + its) , `aurora-config` (67), `aurora-db` (38), `aurora-gateway` full (273 `it` tests), `aurora-api` full after `scripts/fixtures/build.sh` (`--test-threads=4`), `FRONTDOOR=1 scripts/e2e/gateway.sh` (19 gateway tests incl. s20/s21, frontdoor, hooks), `scripts/e2e/llm.sh` (LiteLLM → VidaiMock, **mocked**; hosted leg skipped: `AURORA_TEST_PODMAN`/`AURORA_TEST_LLM` unset), web `typecheck`. The first gateway run found my own SSE mock change breaking the LiteLLM path test; fixed in the follow-up commit.
- **Not done / open.** The metric is asserted only in the e2e (real binaries); the governance IT asserts the request-log class. `aurora-supervisor` untouched (P4-LLM-5). The fail-before check for `s21` was not run separately (the config unit tests cover the refusal rule).
