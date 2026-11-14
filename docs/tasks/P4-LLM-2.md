# P4-LLM-2: The gateway's LLM mint service and the `vertex` provider — **large, phase 5**

## Goal
Build the one gateway service that produces short-lived LLM credentials (D-261(e)) and ship its
first user, **Google Vertex AI**: an org stores a GCP service-account key, the gateway mints
OAuth2 access tokens from it and injects them on Vertex prediction routes of the endpoint's own
project and location, and Claude Code reaches Claude on Vertex through the `anthropic_vertex`
dialect. P4-LLM-3 (Entra) and P4-LLM-4 (STS assume-role) add their modes to this service.

## Linked requirements
- D-121, **D-261** (d), (e), (f); contracts C-801 (`gcp_service_account`), C-804 (`vertex`),
  C-805, C-806 (`vertex`), **C-807**, C-809 (`vertex`), C-810 (`vertex`) —
  `docs/API_CONTRACTS.md` §Phase 5 — P4-LLM.
- `08 §Phase 4 providers` (row `vertex`; *Shared mechanics*: credentials, *Vertex tokens*,
  *Minted credentials: one service*, inference-only paths for `vertex`, `model_from_request`),
  `08 §LLM secrets` (validation by mint), `06` step 11 and §Caches (LLM mint row),
  §Guidance responses (`credential_unavailable` `llm_mint`), `02 §Limits` (`LLM_TOKEN_MINT_TIMEOUT`,
  `LLM_TOKEN_SKEW`, `LLM_TOKEN_MINT_FAILURE_HOLD`, `LLM_MINT_CACHE_MAX`).
- Security: SEC-64, SEC-65, **SEC-97**. Tests: `18 §Additional LLM providers` (mint service, Vertex).
- Migrations: none.

## Context
- P4-LLM-1's framework: `AuthMode::GcpServiceAccount`, `Dialect::AnthropicVertex`, the recipe
  variant `GcpToken`, the Vertex path-set function, C-800 validation of `project_id`/`location`,
  `llm.dev_upstream_overrides` resolver.
- Gateway OAuth refresh and hook mint for patterns (single-flight, zeroizing cache, guarded
  client): `services/aurora-gateway/src/{refresh.rs,mint.rs,plaintext.rs,upstream.rs,guard.rs}`;
  invalidation hub registration in `main.rs`.
- API destination guard for outbound calls (FIX-106): `aurora_core::ranges`, the app-instance
  probe client.
- JWT signing: check `Cargo.lock` for an RSA/JWT crate already in the tree (`jsonwebtoken`, `rsa`,
  `ring`); prefer one already present; add a crate only with licence check and `02 §Libraries`.

## Scope
### In scope
1. **`aurora-llm::mint`** (C-807): `MintSpec` per minted mode with the pinned endpoint, a pure
   request builder and a pure response parser returning `Minted { material, expires_at, origin }`
   or `MintError { class: Rejected | Unavailable, provider_code }` (never the provider's message).
   Implement the `gcp_service_account` spec: RS256 assertion (`iss` = `client_email`, `scope` =
   `https://www.googleapis.com/auth/cloud-platform`, `aud` = the pinned token URI, `iat`, `exp` ≤
   10 min), JWT-bearer grant form, `access_token`/`expires_in` parsing.
2. **Gateway `LlmMint` service**: cache key `(secret_id, value_version, mode, region)`,
   single-flight, zeroizing entries, reuse until `LLM_TOKEN_SKEW` before expiry (≤ 55 min),
   `LLM_TOKEN_MINT_TIMEOUT`, failure hold `LLM_TOKEN_MINT_FAILURE_HOLD`, LRU bound
   `LLM_MINT_CACHE_MAX`; dials through the destination guard with `tls.extra_ca_bundle`;
   registered with the invalidation hub (`connection`, `workspace`, `org`, `all`, `llm` reload);
   502 `credential_unavailable` `{ kind: "llm_mint", reason }`; request-log codes and the
   `gateway_llm_mint_total` metric. Generic over modes so P4-LLM-3/4 add one `MintSpec` each.
3. **`vertex` provider module** (`aurora-llm/src/vertex.rs`): hosts `<location>-aiplatform.googleapis.com`
   (`aiplatform.googleapis.com` for `global`), dialects `anthropic_vertex` (default) and
   `openai_chat`, path set wired with project/location pinning, `model_from_request` from the path
   (`<model>` before `:`; body `/model` on the OpenAI route), static `models` (required),
   `provider_account` origin from `project_id`/`client_email` proven by the mint, `client_env`
   (C-809), registry line.
4. **Secret value** (C-801): shape checks (`type`, `private_key` PEM parses, `client_email`,
   `project_id`, `token_uri` pinned) before any call; validation = one mint through the API's
   guarded client (same builder); nothing minted is stored.
5. Limits in `limits.rs` with readers; remove `LLM_TOKEN_MINT_TIMEOUT`, `LLM_TOKEN_SKEW`,
   `LLM_TOKEN_MINT_FAILURE_HOLD`, `LLM_MINT_CACHE_MAX` from `scripts/guards/limits-gaps.allow`.
6. **Mocks** in `aurora-testkit/src/llm/`: `google_token.rs` (verifies the RS256 assertion against
   the test key, checks `iss`, `scope`, `aud`, `exp`; can answer 400 `invalid_grant`, 503, or
   delay) and `vertex.rs` (prediction routes incl. `streamRawPredict` SSE; records received headers).

### Out of scope
Entra and STS specs (P4-LLM-3/4); claude-code adapter env (P4-LLM-5); dashboard (P4-LLM-6).

## Constraints / file ownership
Owns: `crates/aurora-llm/src/{vertex.rs,mint/**}`, one `builtin()` line in `registry.rs` (append-only;
P4-LLM-3/4 add theirs), `services/aurora-gateway/src/llm_mint.rs` (new) and its wiring in
`main.rs`/`inject.rs` (the `GcpToken` arm only), API `domains/secrets/**` `gcp_service_account`
validation, `crates/aurora-testkit/src/llm/{google_token.rs,vertex.rs}`, `limits.rs` entries above.
No minted value in any log, metric label, error or test snapshot.

## Acceptance criteria (each test seen failing first)
1. Unit: assertion claims and signature verify with the test public key; parser maps 400/401/403
   to `Rejected`, 5xx/timeout to `Unavailable`, never keeps the provider's message.
2. Gateway IT (`gw/it/llm_mint.rs`): 50 concurrent requests → one mint; reuse until skew, re-mint
   after; a failed mint → 502 `credential_unavailable` with `reason`, nothing forwarded, and one
   upstream call within the hold; eviction at the bound; a secret value change drops the entry
   (next request mints with the new key); `mint_rejected` logs request-log `llm_auth`.
3. Gateway IT (`gw/it/llm_vertex.rs`): injected `Authorization: Bearer <minted>`, agent
   `Authorization` and `x-goog-*` removed; other project or location in the path →
   `llm_path_not_permitted` before policy and before the key is opened; non-prediction route
   refused; SSE passthrough byte-for-byte; allow-list on the path model.
4. API IT: save with a wrong `token_uri`, a non-service-account JSON, an unparsable key → 422 with
   the C-801 reason, no outbound call; a valid key → `verified` origin with project and e-mail; a
   `provider_account` rule with another project → `llm_key_wrong_organization`.
5. e2e-gateway: a Vertex request through real binaries with the token mock and `vertex` mock
   reached via `llm.dev_upstream_overrides`; grep of logs and request log finds neither the private
   key nor the minted token.
6. fmt, clippy, targeted tests, guards exit 0 (limits ratchet updated).

## Dependencies
P4-LLM-1. Blocks P4-LLM-3 and P4-LLM-4 (they add mint specs) and the Vertex half of P4-LLM-5.

## Suggested agent/model and reason
Sonnet (`rust-developer`); Opus review of the mint service (concurrency, invalidation, SEC-97).
