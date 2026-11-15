# P4-LLM-3: The `azure_openai` provider — resource keys and Microsoft Entra applications — **medium, phase 5**

## Goal
Let an operator (or instance admin) configure an **Azure OpenAI** resource as an endpoint and let
organizations store either the resource key or a **Microsoft Entra application** (tenant, client id,
client secret). The gateway injects `api-key` for the first and a gateway-minted Entra token for the
second, only on the OpenAI v1 inference paths of that resource. Codex reaches the deployments
through `openai_responses`.

## Linked requirements
- D-121, **D-261** (c), (e), (f); contracts C-800 (`azure_openai`), C-801 (`api_key`,
  `azure_entra`), C-802 (endpoint `auth_modes`), C-804 (`azure_entra`), C-805, C-806 (`azure_openai`),
  C-807 (Entra spec), C-809, C-810 (static) — `docs/API_CONTRACTS.md` §Phase 5 — P4-LLM.
- `08 §Phase 4 providers` (row `azure_openai`; *Minted credentials*; inference set `azure_openai`;
  *Governance*: `allowed_organization_ids` are tenant ids, `provider_account` admits `azure_entra`
  only), `08 §LLM secrets` (validation), `06` step 11 (`entra_token`), `12 §Adapter codex`
  (base URL `<base>/openai/v1`), `15` (provider keys).
- Security: SEC-64, SEC-65, SEC-97. Tests: `18 §Additional LLM providers`.
- Migrations: none.

## Context
- P4-LLM-1: `AuthMode::AzureEntra`, recipe `Header { name: "api-key" }` and `EntraToken`, the
  `azure_openai` path-set function, C-800 validation (base URL host only, `models` and
  `default_model` required).
- P4-LLM-2: `aurora-llm::mint` (`MintSpec`) and the gateway `LlmMint` service — add one spec.
- Azure v1 API: `https://<resource>.openai.azure.com/openai/v1/{chat/completions,responses,models}`
  (no `api-version`); `model` in the body is the **deployment name**. Entra: client-credentials
  grant at `https://login.microsoftonline.com/<tenant_id>/oauth2/v2.0/token`, scope
  `https://cognitiveservices.azure.com/.default`; the access token is a JWT whose `tid` and
  `appid` (v1 tokens) or `azp` (v2) name tenant and application. The gateway reads these claims
  from the token it just received from the pinned authority over TLS; it does not validate the
  token's signature (it is not the audience) — document that in the module.

## Scope
### In scope
1. **`azure_openai` module**: configured base (https, host only), dialects `openai_chat` and
   `openai_responses`, path set wired (`/openai/v1/…` as `08`), `model_from_request` from body
   `/model`, static `models` (deployment names), `client_env` (`OPENAI_BASE_URL=<base>/openai/v1`,
   placeholder key), origin methods `endpoint` and `provider_account`, registry line.
2. **`api_key` mode**: `api-key` header; agent `Authorization` and `api-key` removed; validation by
   `GET /openai/v1/models`.
3. **`azure_entra` mode**: value shape (GUIDs, specific tenant), Entra `MintSpec` (form body,
   response parsing, `tid`/`appid|azp` extraction into `origin`, `Rejected` on `invalid_client` /
   `unauthorized_client` / 400/401, `Unavailable` otherwise), gateway injection `Authorization:
   Bearer <token>` with agent `api-key` removed; validation = mint then `GET /openai/v1/models` with
   the token; `provider_account` origin = tenant id. An endpoint with a `provider_account` rule
   admits `azure_entra` keys only (C-802 `auth_modes`; an `api_key` there → 422
   `llm_key_unverifiable`).
4. **Mocks** in `aurora-testkit/src/llm/`: `azure.rs` (v1 routes, accepts `api-key` or Bearer,
   records headers, SSE) and `entra.rs` (verifies the form, issues an unsigned-test JWT with
   `tid`/`appid`, can refuse with `invalid_client`).

### Out of scope
Sovereign clouds (`login.microsoftonline.us` etc.), managed identities, workload identity
federation, certificate credentials, the classic `api-version` deployment routes — all refused by
validation or simply absent. Dashboard (P4-LLM-6), Codex adapter changes (none expected; P4-LLM-5
verifies end to end).

## Constraints / file ownership
Owns: `crates/aurora-llm/src/{azure_openai.rs,mint/entra.rs}`, one append-only `builtin()` line in
`registry.rs` and one `mod` line in `lib.rs`, the `EntraToken` and Azure `Header` arms in the
gateway's `inject.rs`, Azure value validation in API `domains/secrets/**`,
`crates/aurora-testkit/src/llm/{azure.rs,entra.rs}`. May run beside P4-LLM-4: the only shared
lines are the append-only registry and module lines (the second lane to merge rebases them).

## Acceptance criteria (each test seen failing first)
1. Unit: Entra spec builds the exact form (no secret in any `Debug`), parses `expires_in`, extracts
   `tid`/`appid`/`azp`, maps errors to `Rejected`/`Unavailable` without the provider's message;
   tenant `common` refused at shape check.
2. Gateway IT (`gw/it/llm_azure.rs`): `api_key` → `api-key` set and agent `Authorization`/`api-key`
   removed; `azure_entra` → Bearer from the mint, agent `api-key` removed; single-flight shared with
   P4-LLM-2's tests; non-v1 or management paths (`/openai/deployments/...?api-version=`,
   `/openai/v1/files`) → `llm_path_not_permitted`; deployment-name allow-list honoured; SSE
   passthrough.
3. API IT: `api_key` validated via models; `azure_entra` with `invalid_client` → 422 `llm_key_invalid`
   reason `mint_rejected`; `provider_account` rule with another tenant → `llm_key_wrong_organization`;
   an `api_key` on that endpoint → `llm_key_unverifiable`; `GET /v1/llm/providers` lists the
   endpoint's admitted modes.
4. e2e-gateway: an Entra-authenticated request through real binaries (authority via
   `llm.dev_upstream_overrides`); the client secret and token appear in no log or request-log row.
5. fmt, clippy, targeted tests, guards exit 0.

## Dependencies
P4-LLM-1, P4-LLM-2.

## Suggested agent/model and reason
Sonnet (`rust-developer`): a bounded provider module on an existing service; Haiku is too light for
the mint and governance interplay.

## Completion report
Commit on `alexiri/p5-agent`, prefix `P4-LLM-3:`. Gate level: **batch gate for P4-LLM-3 alone** (run so 3 can merge before 4): `cargo test -p aurora-llm` (24 unit + 68 it), `-p aurora-config` (18+67), `-p aurora-testkit`, `-p aurora-gateway` (291 it, 12 lib), `-p aurora-api -- --test-threads=4` after `scripts/fixtures/build.sh` (97 lib + 1118 it passed; `secret_stores::a_running_migration_excludes_a_second_one_and_the_cli_run` failed once under load with "another secrets migration is running" — a shared-lock flake of FIX-124 code, passes alone), `cargo clippy --workspace --all-targets --all-features -- -D warnings` exit 0, `bash scripts/guards/run-all.sh` exit 0, `python3 scripts/extract-schema.py --check` exit 0, OpenAPI drift test ok, `FRONTDOOR=1 bash scripts/e2e/gateway.sh` (20 of 21 passed; s23 then failed on config validation, fixed below and re-run alone: ok), `bash scripts/e2e/llm.sh` (4 passed; codex binary and live legs skipped), `AURORA_TEST_PODMAN=1 bash scripts/e2e/hosted.sh` 17 passed (mocked upstreams; the real-LLM leg needs `AURORA_TEST_LLM=1` and did not run).
- **Config fix found by the e2e.** `azure_openai` `base_url` accepted `https` only, so the e2e stack (a loopback mock resource) could not start. `08 §Configuration` exempts loopback from the https rule: `crates/aurora-config/src/llm_endpoint.rs` now also accepts `http://` to a loopback host for `azure_openai` (public API unchanged; test added). `bedrock` will need the same in P4-LLM-4.

- **What changed.** `aurora-llm`: `azure_openai.rs` (configured base, `/openai/v1` path set, `api-key` and `EntraToken` recipes by mode, static deployment models, C-809 env, validation and origin per mode); `mint/entra.rs` (`EntraSpec`: pinned authority, form body, `expires_in` number or string, `tid`/`appid|azp` claims read without a signature check, `parse_value` with the C-801 reasons); trait methods `list_models_strict_as` and `verify_key_origin_as` (the auth mode reaches providers with several value shapes; default delegates), `LlmError::MintRejected`; registry line and the override in `with_overrides`. `aurora-config`: the Entra authority is a legal `llm.dev_upstream_overrides` origin. API: `check_key` takes the mode, `azure_entra` shape refusals, `mint_rejected` details, admission by the provider's modes (D-311). Gateway: no code change — the `entra_token` arm and the `api-key` header arm were already wired by P4-LLM-1/2. Testkit `llm::entra` (verifying authority mock) and `llm::azure` (v1 routes, `api-key` or Entra JWT, SSE, request recording).
- **Tests.** `aurora-llm` unit (form bytes, claims, error classes, value shape, Debug never shows the secret) and `tests/it/azure.rs` (mint against the verifying mock incl. wrong secret, other-tenant token; provider validation and origin for key and Entra); gateway `governance::azure_*` (key → `api-key`, agent credentials removed, SSE byte for byte, allow-list on deployment names, `api-version`/files/fine-tuning/images/other-version routes refused before policy with the store down and the authority untouched; Entra: 12 concurrent requests → one mint, Bearer injected, no client secret upstream; failed mints → 502 `llm_mint` `mint_rejected`/`mint_unavailable`, hold, request-log codes, no secret in answer or log); API `secrets::azure_keys_and_entra_applications_are_validated_and_attributed` (all C-801 shape reasons without an outbound call, key validated by models, Entra origin tenant/application, wrong tenant → `llm_key_wrong_organization`, `api_key` under `provider_account` → `llm_key_unverifiable`, `mint_rejected`, unreachable authority saved `pending`/refused with a rule, providers listing); e2e-gateway `s23_azure_entra_token_is_minted_by_the_gateway_and_never_leaks`. The gateway and provider tests cannot pass without this task (the provider did not exist); the unit/IT tests for the refusal of another tenant's token and the single flight are the ones that would fail on a regression.
- **Notes.** Gateway ITs live in `governance.rs` (the Azure rig beside Vertex's) rather than `llm_azure.rs`. Brief deviation recorded as D-311(a): admission is by the provider's modes, so an `api_key` on an endpoint with a `provider_account` rule reaches the origin check and ends as `llm_key_unverifiable` (C-802's per-endpoint `auth_modes` stays a hint). Provider-level `provider_rejected` is not emitted (a 401/403 from models stays reason-less `llm_key_invalid`). The `provider_account` origin of an Entra secret costs a second mint at save (validation and proof are separate trait calls).
