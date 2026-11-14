# P4-LLM-4: The `bedrock` provider — assume-role, IAM keys and Bedrock API keys — **large, phase 5**

## Goal
Let organizations use **AWS Bedrock** through an endpoint of one region. The recommended credential
is **assume-role** (D-261(b)): the org stores a source IAM key and a role ARN, the gateway assumes
the role with the org's own external id and SigV4-signs inference with the session credentials.
Static IAM keys (`aws_sigv4`) and Bedrock API keys (`api_key`) are also accepted. Requests are
signed in the gateway only, limited to the runtime inference routes, and streamed back
(AWS event stream) unchanged. Claude Code reaches Claude on Bedrock through `anthropic_bedrock`.

## Linked requirements
- D-121, **D-261** (a), (b), (e), (f), D-95 (AssumeRole shape, external id); contracts C-800
  (`bedrock`), C-801 (`aws_sigv4`, `aws_assume_role`, `api_key`), C-802, C-804 (`bedrock`), C-805
  (incl. `llm_request_too_large`), C-806 (`bedrock`), C-807 (STS spec), C-809, C-810 (`bedrock`) —
  `docs/API_CONTRACTS.md` §Phase 5 — P4-LLM.
- `08 §Phase 4 providers` (row `bedrock`; credentials incl. `aws_assume_role`; *Minted credentials*;
  inference set `bedrock`; `model_from_request` percent-decoded `{modelId}`; streaming),
  `08 §LLM secrets` (validation by `AssumeRole` / `GetCallerIdentity`), `06` steps 6a, 11, 12,
  `09 §5` (`aws_sigv4` finalizer), `02 §Limits` (`LLM_SIGN_BODY_MAX`, `LLM_ASSUME_ROLE_DURATION`).
- Security: SEC-64, SEC-65, SEC-97. Tests: `18 §Additional LLM providers`.
- Migrations: none.

## Context
- P4-LLM-1: `AuthMode::{AwsSigV4, AwsAssumeRole}`, recipes `AwsSigV4`/`AwsAssumeRole`, the Bedrock
  path-set function, C-800 validation (`region`, optional VPC `base_url`, no `allowed_paths`),
  classification rows (incl. `x-amzn-ErrorType`), `llm.dev_upstream_overrides` resolver.
- P4-LLM-2: `aurora-llm::mint` and the gateway `LlmMint` service — add the STS `AssumeRole` spec.
- SigV4 in the gateway: `services/aurora-gateway/src/mechanisms/finalize.rs` (`sign`,
  `finalize`; checked against AWS vectors, `fixture-sigv4`). Reuse it; do not write a second
  signer. The `aws-keys` package's assume-role hook (`plugins/builtin/aws-keys/**`, D-95) is the
  reference for the STS request (form fields, error-code handling), not a dependency.
- Org external id: `organizations.external_id` (the gateway role already has `SELECT`); the hook
  context variable `org_external_id` reads the same column — read it the same way.

## Scope
### In scope
1. **`bedrock` module**: runtime host `bedrock-runtime.<region>.amazonaws.com` (or the VPC
   `base_url`), control-plane host `bedrock.<region>.amazonaws.com` used by the API only and never
   an injection target, dialects `anthropic_bedrock` (default), `openai_chat`/`openai_responses`
   when declared, path set wired, `model_from_request` from the percent-decoded `{modelId}` (profile
   id or ARN, exact compare), `client_env` (C-809), origin methods, registry line.
2. **Signing in the gateway** (`aws_sigv4`, `aws_assume_role`): remove every agent `Authorization`
   and `X-Amz-*` header, buffer the body up to `LLM_SIGN_BODY_MAX` (larger → 413
   `llm_request_too_large`, decided in step 6a before any value is opened), sign service `bedrock`
   with the endpoint region, set `X-Amz-Security-Token` when a session token exists. Response
   bodies (`application/vnd.amazon.eventstream`) stream through unbuffered.
3. **STS `AssumeRole` mint spec**: SigV4 (service `sts`, endpoint region) with the **source** key,
   form `Action=AssumeRole&Version=2011-06-15&RoleArn&RoleSessionName=aurora-<org_id>&ExternalId=<org external_id>&DurationSeconds`;
   XML parse of `Credentials` and `AssumedRoleUser.Arn`; `Expiration` caps the cache; `AccessDenied`,
   `InvalidClientTokenId`, `SignatureDoesNotMatch`, `ExpiredToken` → `Rejected`, else
   `Unavailable`. A missing external id fails closed with no call.
4. **`api_key` mode**: `Authorization: Bearer <key>` (agent auth headers removed); origin `endpoint`.
5. **Secret values** (C-801): shapes; `role_arn` partition must match the region's partition;
   duration bounds (`LLM_ASSUME_ROLE_DURATION`); validation = `AssumeRole` (assume-role; reason
   `assume_role_denied` on `AccessDenied` with the trust-policy hint), signed `GetCallerIdentity`
   (`aws_sigv4`), listing (API key). Origin: role account and ARN, or caller account.
6. **Model listing** (C-810): `ListFoundationModels` + `ListInferenceProfiles` signed by the API
   (with the minted session for assume-role, computed in the API, never cached there), merged
   profiles first; static fallback.
7. Limits `LLM_SIGN_BODY_MAX`, `LLM_ASSUME_ROLE_DURATION` with readers; remove their lines from
   `scripts/guards/limits-gaps.allow`.
8. **Mocks** in `aurora-testkit/src/llm/`: `bedrock.rs` (runtime: **verifying** SigV4 from a known
   secret and session token, `invoke`/`invoke-with-response-stream` with a real event-stream frame
   sequence, error headers `x-amzn-ErrorType`; control plane list routes), `sts.rs` (`AssumeRole`
   and `GetCallerIdentity`, verifying the source-key signature and recording `ExternalId`,
   `RoleSessionName`, `DurationSeconds`; can answer `AccessDenied`).

### Out of scope
Converse-native and guardrail features as dialects, async invoke, cross-account role chaining,
MFA, session policies, the global STS endpoint, AWS China/ISO partitions, gateway or IRSA identity.

## Constraints / file ownership
Owns: `crates/aurora-llm/src/{bedrock.rs,mint/sts.rs}`, append-only `builtin()` and `mod` lines,
the `AwsSigV4`/`AwsAssumeRole` arms in the gateway's `inject.rs` and the body-buffer hook in the
pipeline (step 6a/12 only), Bedrock value validation and listing in API `domains/secrets/**`,
`crates/aurora-testkit/src/llm/{bedrock.rs,sts.rs}`, the two `limits.rs` entries. May run beside
P4-LLM-3 (only the append-only lines are shared). Never log a signature, access key id, session
token or the source key.

## Acceptance criteria (each test seen failing first)
1. Unit: STS request matches a recorded vector (canonical request and signature for fixed time and
   keys); XML parser incl. error documents; partition/region mismatch refused.
2. Gateway IT (`gw/it/llm_bedrock.rs`, verifying mocks): each mode reaches the runtime mock with a
   valid signature; with assume-role the signing key id is the **session** key and the source key
   id never appears; `ExternalId` equals the seeded org's id and differs for a second org; agent
   `Authorization`/`X-Amz-*` never reach the mock; body over the limit → 413 before the secret is
   opened; guardrail, async-invoke, agent routes and the control-plane host → `llm_path_not_permitted`;
   event-stream bytes identical and the first frame not delayed by the whole stream.
3. Gateway IT: an S3 request under Default Rule `block` is still blocked while Bedrock works
   (exact-host carve-out).
4. API IT: assume-role `AccessDenied` → 422 `assume_role_denied`; a valid role → origin with the
   role's account; `provider_account` rule naming another account → `llm_key_wrong_organization`;
   an `api_key` on a `provider_account` endpoint → `llm_key_unverifiable`; listing merges profiles
   and models, falls back to static on 403.
5. e2e-gateway: an assume-role Bedrock request through real binaries (STS and runtime via
   `llm.dev_upstream_overrides`); grep of logs and request log finds neither key nor session token.
6. fmt, clippy, targeted tests, guards exit 0.

## Dependencies
P4-LLM-1, P4-LLM-2.

## Suggested agent/model and reason
Sonnet (`rust-developer`); Opus review of the signing path and the external-id handling (a
confused-deputy mistake here crosses tenants).
