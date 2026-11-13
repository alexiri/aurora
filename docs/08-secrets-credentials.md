# 08 — Secrets, Secret Stores, LLM Providers, Governance and Gateway Credentials

Phase: **0** (inline store, LiteLLM, Anthropic) · **3** (HashiCorp Vault store,
`21 §HashiCorp Vault`). Crates: `aurora-crypto`, `aurora-secrets`, `aurora-llm`.

## 1. Where credential material lives

| Material | Column | Purpose string (AAD) |
|----------|--------|----------------------|
| Secret value (generic, LLM) | `secrets.value_ref` | `secret.value` |
| App connection credentials (JSON) | `app_connections.credentials_ref` | `connection.credentials` |
| OAuth client secret fields (JSON) | `app_oauth_clients.secret_ref` | `oauth_client.secret` |
| Channel integration credentials | `channel_integrations.credentials_ref` | `integration.credentials` |
| Dedicated presence credentials | `channel_presences.credentials_ref` | `presence.credentials` |
| Webhook verification secret | `webhooks.verification.secret_ref` | `webhook.verification` |
| Credential copy of a self-contained deprovision op (RFE-024, owner = op id) | `channel_ops.payload.credentials_ref` | `channel_op.deprovision` |
| Managed connector secret settings (phase 4, P4-MCON; owner = `connectors.secret_owner`, org = nil UUID) | `connectors.secret_refs` (one per field) | `connector.settings` |
| Web Push VAPID private key (phase 5 (D-138, moved from phase 4), RFE-011b; owner = the key id, org = nil UUID) | `webpush_vapid_keys.private_ref` | `webpush.vapid` |
| Web Push subscription endpoint and keys (phase 5 (D-138, moved from phase 4), RFE-011b; owner = the subscription id, org = nil UUID) | `push_subscriptions.secret_ref` | `webpush.subscription` |

Every such column holds a **SecretRef**. Code never interprets a SecretRef
outside `aurora-secrets`; it calls the **SecretVault** service:

```rust
pub struct SealContext { pub purpose: Purpose, pub owner_id: Uuid, pub org_id: Uuid, pub workspace_id: Option<Uuid> }

impl SecretVault {
    /// Write a new value into the deployment's default store for this purpose.
    async fn seal(&self, ctx: &SealContext, plaintext: Secret<Vec<u8>>) -> Result<SecretRef>;
    /// Resolve a ref (managed or reference) to plaintext.
    async fn open(&self, ctx: &SealContext, r: &SecretRef) -> Result<Secret<Vec<u8>>>;
    /// Best-effort removal of managed material (no-op for inline and reference refs).
    async fn discard(&self, ctx: &SealContext, r: &SecretRef) -> Result<()>;
    /// Validate a user-supplied reference: store exists, supports references,
    /// path permitted for this org/workspace, value readable.
    async fn check_reference(&self, ctx: &SealContext, store_id: &str, path: &str, field: Option<&str>) -> Result<SecretRef>;
}
```

The vault holds a **store registry** built at startup from configuration. Both
the API and the gateway build it from the same `[secrets]` configuration.

## 2. The SecretStore seam

```rust
#[async_trait]
pub trait SecretStore: Send + Sync {
    fn id(&self) -> &str;                       // config id, e.g. "inline", "vault-prod"
    fn kind(&self) -> &'static str;             // "inline", "hashicorp-vault", …
    fn capabilities(&self) -> StoreCapabilities; // { managed: bool, references: bool }
    async fn put(&self, ctx: &SealContext, plaintext: &Secret<Vec<u8>>) -> Result<SecretRef>; // always a NEW immutable version
    async fn get(&self, ctx: &SealContext, r: &SecretRef) -> Result<Secret<Vec<u8>>>;
    async fn delete(&self, ctx: &SealContext, r: &SecretRef) -> Result<()>;
    async fn resolve_reference(&self, ctx: &SealContext, path: &str, field: Option<&str>) -> Result<SecretRef>;
    async fn health(&self) -> StoreHealth;
}
```

Rules every store MUST follow:

1. **`put` never overwrites.** It writes a new immutable version and returns a
   ref naming it. The **database row is the compare-and-set authority**: callers
   swap refs with `UPDATE … WHERE <ref/version column> = <old>`. Stores
   therefore need no CAS of their own, and a lost race leaves only an orphan
   version (cleaned by `discard`).
2. **Purpose binding.** A ref resolved with a different purpose or owner than it
   was sealed with MUST fail (`SecretRefMismatch`). Inline achieves this with
   AEAD associated data; external stores by path layout and by storing the
   context alongside the value.
3. **No plaintext in errors, logs or metrics.**
4. **Fail closed**: an unreachable store yields `StoreUnavailable`; the gateway
   maps it to `secret_store_unavailable` (503) and never injects a stale or
   partial value.

### SecretRef format

```
inline:v1:<key_id>:<base64url(nonce‖ciphertext‖tag)>
ext:<store_id>:<percent-encoded path>[#<field>]@<version>   (managed or reference)
```

(phase 3, D-107) `<version>` may be the literal `latest` for a reference ref: the
store reads the current version at every open and never pins one (HashiCorp
Vault reference refs are always stored this way, `21`).

- `inline` refs carry their ciphertext (the "store" is the column itself).
- `ext` refs name an external store id from configuration. A row with an `ext`
  ref whose store is not configured fails with `StoreNotConfigured` (never
  silently ignored).
- `secrets.value_source = reference` marks values Aurora did not write (a user
  pointed at an existing path); `discard` is a no-op for them and the dashboard
  shows the reference (store, path, field — never the value).

### Store modes

| Mode | Who writes the value | Typical use |
|------|----------------------|-------------|
| **Managed** | Aurora, through `seal` into `secrets.default_store` | every secret, connection credential and refreshed token |
| **Reference** | an external system; Aurora only reads | "use the team's existing key at `kv/team-ai/openai#key`" |

Configuration (`[secrets]`, see `15`):

```toml
[secrets]
default_store = "inline"                 # where managed values are written

[[secrets.stores]]
id = "inline"
kind = "inline"                          # always present; implicit if omitted

# phase 3 example
[[secrets.stores]]
id = "vault-prod"
kind = "hashicorp-vault"
address = "https://vault.example.org:8200"
auth = { method = "kubernetes", role = "aurora", mount = "kubernetes" }
kv_mount = "secret"
managed_prefix = "aurora"                # managed values at <kv_mount>/data/<prefix>/<org>/<purpose>/<owner>
[[secrets.stores.reference_scopes]]     # which tenants may reference which paths
org_id = "0190f6a2-7c1e-7b3a-9d4e-5f6a7b8c9d0e"   # (phase 3, D-107) the org's immutable id, never its slug
paths = ["teams/physics/*"]
```

Phase 0 MUST implement: `SecretRef` columns everywhere, the store registry with
configuration validation (unknown `kind` → startup error naming the phase),
the `inline` store, reference-mode plumbing (`value_source`, `check_reference`,
the dashboard form hidden unless some configured store has `references =
true`), and the migration command below. The fake store `memory` (tests only,
`references = true`) exercises reference mode in phase 0 tests.

### Moving values between stores

`aurora-api secrets migrate --to <store_id> [--purpose …] [--dry-run]` re-seals
every managed value into the target store row by row (read → put → CAS swap →
discard old), idempotent and resumable. `aurora-api secrets rekey` is the same
operation targeting `inline` with the active key (used for key rotation).

### Gateway use

The gateway opens values lazily at injection time; plaintext is cached in a
zeroizing in-memory map for ≤60 s keyed by `(SecretRef)`. Refs are cached with
the ResolvedHost. Store latency is therefore paid at most once per minute per
value on the connect path, never per request.

## 3. The inline store (AES-256-GCM)

- **Keyring**: `AURORA_ENCRYPTION_KEYS` = comma-separated `<key_id>:<base64 32
  bytes>`; `AURORA_ENCRYPTION_ACTIVE_KEY` = key id used for new seals. Key ids
  match `[a-z0-9]{1,16}`. All keys remain usable for `open` until removed.
- **Format**: `inline:v1:<key_id>:<b64url(nonce[12] ‖ ciphertext ‖ tag[16])>`;
  nonce random per seal.
- **AAD**: `aurora:v1:<purpose>:<org_id>:<owner_id>` — so ciphertext copied to
  another row, org or purpose fails to decrypt.
- Both the API and the gateway read and write this format (one crate); a
  conformance fixture in `aurora-crypto` pins it.
- Losing the keyring loses every inline value. Operators back it up separately
  from the database.

## 4. Other key material (HKDF purposes)

`AURORA_SIGNING_KEY` (32 bytes, base64) is the root for derived keys:
`HKDF-SHA256(root, info = "aurora:<purpose>:v1")`.

(phase 5 (D-138, moved from phase 4), P4-S3) **Blob-store credentials** are not Aurora secrets and are not in
the `secrets` table or any SecretStore: the S3 backend (`15 §S3-compatible blob
store`) uses the workload's provider chain (preferred) or environment variables named
by the configuration (`blobs.s3.credentials.*_env`), filled from a Kubernetes Secret.
They are held only by the API process, are never sealed into a row, never appear in
a response, log or metric, and never reach the gateway, a runner or a sandbox.

(phase 4, P4-CNT) The purpose **`ratelimit_key`** derives the key that hashes e-mail
addresses and client IPs into shared rate-limit counter keys (`05 §Conventions`
*Shared counters*): `HMAC-SHA256(hkdf("ratelimit_key"), value)` truncated to 128 bits,
hex. It is derived from the **active** signing key and rotating the signing key
changes the hashes, which only restarts the affected windows (a counter is not a
record). The Redis backend's credentials are **never in `aurora.toml` or in a URL**:
the configuration names environment variables (`username_env`, `password_env`),
filled from a Secret (Helm `counters.redis.existingSecret`), exactly like the Vault
`approle` variables (`15`).

(phase 5 (D-138, moved from phase 4), RFE-011b) **Web Push material** is opened only by the API: the VAPID private key
(ES256) to sign a push request's JWT, and a subscription's sealed `{ endpoint, p256dh, auth }` to
encrypt and address one push. Both live under the usual store registry (managed in the default
store; an external store works the same), are never returned by a route, never logged and never
sent to the gateway, a runner or a sandbox; an operator-managed VAPID key (`push.vapid.private_key_env`)
is read from the environment at startup and never stored. The endpoint URL is sealed because it is a
capability: with the subscription's keys it lets anyone who holds it push to that browser (VAPID
limits it to this application server's key, but not to Aurora's payloads).

(phase 4, P4-GRP-2) **SCIM directory tokens** (`aud_`, `00 §Token prefixes`) are plain
capability tokens like every other prefix: 32 random bytes, shown once, stored only as a
SHA-256 hash (`scim_tokens.token_hash`) with a display prefix, never sealed (there is
nothing to read back) and never derived from the signing key, so rotating the signing key
does not invalidate them. They authorize `/scim/v2` of their one organization and nothing
else (`04 §SCIM`, SEC-72).

**Keyring-style lists (phase 2, RFE-007, D-51).** Phase 0 held one static value
for each of `AURORA_SIGNING_KEY` and `AURORA_INTERNAL_SECRET`, with no rotation.
From phase 2 both accept the **same shape as the encryption keyring** (§3):

- `AURORA_SIGNING_KEY` = comma-separated `<key_id>:<base64 32 bytes>` (at most
  `KEYRING_MAX` = 4 entries); `AURORA_SIGNING_ACTIVE_KEY` = the id used for
  everything new. Key ids match `[a-z0-9]{1,16}`.
- `AURORA_INTERNAL_SECRET` = comma-separated `<key_id>:<base64 ≥32 bytes>`;
  `AURORA_INTERNAL_ACTIVE_KEY` = the id senders use.
- **Backward compatible**: a value with no `:` is a one-entry ring with id `v0`,
  active (the active variable may then be omitted). A list without its `*_ACTIVE_KEY`
  fails startup naming the variable. Every service validates its ring at startup.
- **Verify with any, sign with active.** Receivers accept anything signed or
  presented with *any* key in their ring; producers use only the active key.
  Nothing else widens: a key id outside the ring is refused.
- **Derived artifacts carry their key id** so verification is one lookup:
  AEAD states (`oauth-state`, `oidc-state`, `link-state`) are prefixed with the id
  (`<key_id>.` before the base64url envelope); **action tokens** become
  `v2.<key_id>.<b64url(payload)>.<b64url(HMAC)>` (`13 §4.4`); artifacts without an
  id (phase 0, `v1.` tokens, unprefixed states) are verified by trying each key
  (≤4, constant-time). Avatar public keys are *stored* values, so rotation does
  not affect them. TTLs (`OAUTH_STATE_TTL` 10 min, `ACTION_TOKEN_TTL` ≤ 24 h) bound
  how long an old key must stay in the ring.
- **Internal secret on the wire**: `Authorization: Bearer <base64 secret>` plus
  `X-Aurora-Key-Id: <key_id>` (not secret); the receiver compares in constant time
  against that key, or — header absent (legacy/rolling) — against every key in
  its ring. `GET /internal/v1/status` on the gateway and hook host, and `aurora-api
  keys status`, report the **ids** (and a short fingerprint, never values) of each
  service's ring and active key so an operator can verify convergence.

**Rotation procedure** (rolling, no downtime; identical for the signing key and the
internal secret):

1. `aurora-api keys generate --kind signing|internal` prints a fresh
   `<key_id>:<base64>` entry. Append it to the ring **on every service that holds
   the ring** (signing key: api; internal secret: api, gateway, hookhost) but do
   **not** change the active id. Roll the services; they now *accept* the new key.
2. Confirm convergence (`keys status` / the status endpoints show the new id on
   every replica). Only then set `*_ACTIVE_KEY` to the new id and roll again:
   producers now use the new key while the old one still verifies in-flight
   artifacts.
3. After the old key's longest artifact lifetime (≥ 25 h for the signing key, the
   action-token TTL plus margin; ≥ 5 min for the internal secret), remove it from
   the ring and roll. If the old key is **compromised**, do this immediately:
   in-flight states fail (users retry), outstanding chat-card tokens fail (the card
   shows "expired", a new card is posted by the next approval event), nothing else.
4. Setup tooling and Helm hooks only ever **append** (never overwrite) ring entries.

Out of scope for phase 2 (owner decision: option a only): `AURORA_RUNNER_SESSION_KEY`
and the gateway CA stay single static values with no orchestrated rotation, and no
KMS-wrapping of the inline keyring is specified (`17 §Residual risks`).

| Purpose | Use |
|---------|-----|
| `oauth-state` | AEAD for app-connect OAuth state |
| `oidc-state` | AEAD for sign-in OIDC state |
| `action-token` | HMAC-SHA256 for channel action tokens (`13`) |
| `avatar-key` | HMAC for rotating public avatar keys |
| `link-state` | AEAD for account-link landing pages |

`AURORA_RUNNER_SESSION_KEY` (shared by runner replicas and the API) is separate
(`11`) and remains a single static value. `AURORA_INTERNAL_SECRET` authenticates
API ↔ gateway-internal ↔ hook host (a ring from phase 2, above).

## 5. Secrets

| Field | Notes |
|-------|-------|
| scope | workspace or organization |
| name | unique per scope |
| kind | `generic` or `llm` |
| host / path pattern | generic: user-supplied (`07 §Matching primitives`), and refused (422 `llm_host_requires_llm_key`) when the pattern can match a governed LLM host (§6, §11); LLM: derived from provider/endpoint, read-only |
| injection | generic: an **injection recipe**; LLM: fixed by the provider |
| value | write-only: never returned by any API after creation |

### Injection recipes (generic secrets)

A recipe is exactly one mechanism invocation from the subset below (the same
engine the plugin mechanisms use, `09 §Mechanisms`), with `{value}` as the only
substitution:

| Recipe | Shape | Effect |
|--------|-------|--------|
| Bearer (default) | `{ "kind": "bearer" }` | `Authorization: Bearer <value>` |
| Header | `{ "kind": "header", "name": "X-Api-Key", "format": "{value}" }` | set header (name validated as a token; `Host`, `Content-Length`, hop-by-hop forbidden) |
| Query | `{ "kind": "query", "name": "api_key", "format": "{value}" }` | set/replace query parameter |
| Basic | `{ "kind": "basic", "username": "x-token" }` | `Authorization: Basic base64(username:value)` |
| Path template | `{ "kind": "path_template", "prefix": "/bot" }` | first path segment starting with `prefix` has its remainder replaced by the value (`/botPLACEHOLDER/send` → `/bot<value>/send`) |
| Path regex | `{ "kind": "path_regex", "pattern": "^/v1/([^/]+)/", "replacement": "/v1/{value}/" }` | regex (Rust `regex`, anchored, ≤256 chars) on the path |

Values are escaped for their context (header value: reject CR/LF/NUL; query:
percent-encode; path: percent-encode per segment).

### Agent-proposed custom secrets

The `propose_custom_connection` platform tool lets an agent propose a generic
secret (name, host, path, recipe — never a value). The API creates a pending
proposal shown as a dashboard card ("Agent Daneel asks for an API key for
`api.example.com`"); the person pastes the value **in the dashboard** (never in
chat). An accepted proposal creates the secret in the agent's workspace and a
grant for the proposing agent only. Proposals are stored as `secrets` rows with
`proposed_by_agent_id` and an empty `value_ref` placeholder `pending:` until
accepted (rows with `pending:` refs are never candidates).

## 6. LLM providers

LLM providers are a **core registry** (`aurora-llm`), not plugins: their hosts
are reserved and their traffic shapes the platform itself.

```rust
pub trait LlmProvider: Send + Sync {
    fn id(&self) -> &'static str;                      // "litellm", "anthropic"
    fn display_name(&self) -> &'static str;
    fn endpoint_kind(&self) -> EndpointKind;           // Fixed(url) | Configured
    fn auth_modes(&self) -> &'static [AuthMode];       // e.g. api_key
    fn dialects(&self, ep: &Endpoint) -> Vec<Dialect>; // anthropic_messages, openai_chat, openai_responses
    fn hosts(&self, ep: &Endpoint) -> Vec<HostPattern>;
    fn injection(&self, mode: &AuthMode) -> InjectionRecipe;
    async fn list_models(&self, ep: &Endpoint, key: &Secret<String>, http: &Client) -> Result<Vec<ModelInfo>>;
    fn default_model(&self, ep: &Endpoint) -> Option<String>;
    /// Environment a client inside the sandbox needs to talk this dialect
    /// to this endpoint through the gateway, with placeholder credentials.
    fn client_env(&self, ep: &Endpoint, dialect: Dialect, mode: &AuthMode) -> Vec<(String, String)>;

    // --- governance (§11) ---
    /// Which key-origin methods this provider can perform (empty = none).
    fn origin_methods(&self) -> &'static [OriginMethod];
    /// Ask the provider which account a key belongs to, without sending any
    /// prompt (a free metadata request). Errors: Unreachable, InvalidKey, Unverifiable.
    async fn verify_key_origin(&self, ep: &Endpoint, method: OriginMethod, key: &Secret<String>, http: &Client) -> Result<KeyOrigin>;
    /// Account identifiers the provider reports on an ordinary response, if any.
    fn origin_from_response(&self, ep: &Endpoint, headers: &HeaderMap) -> Option<KeyOrigin>;
    /// Which model a request asks for: Some(id), None (request uses no model,
    /// e.g. listing models), or Unknown (body truncated/unparsable — fail closed).
    fn model_from_request(&self, ep: &Endpoint, method: &Method, path: &str, body_prefix: &[u8], truncated: bool) -> ModelRef;

    // --- (phase 5 (D-138, moved from phase 4), P4-LLM, D-121) additions; phase-0 providers return the defaults ---
    /// Paths on which Aurora may inject this provider's credential (SEC-65); `None` = unrestricted (litellm, anthropic).
    fn inference_paths(&self, ep: &Endpoint) -> Option<PathSet> { None }
    /// Provider error → class, shared by the gateway (request log, metrics) and the harness adapters.
    fn classify_error(&self, status: u16, error_type: Option<&str>) -> LlmErrorClass { LlmErrorClass::default_for(status) }
}
// InjectionRecipe gains (LLM secrets only): Header { name } (e.g. Azure `api-key`), AwsSigV4 { region, service },
// GcpServiceAccountToken { scope }; AuthMode gains `aws_sigv4`, `gcp_service_account`.

pub struct KeyOrigin {             // every field optional; which ones are filled depends on the method
    pub organization_id: Option<String>,
    pub workspace_id: Option<String>,
    pub team_id: Option<String>,
    pub user_id: Option<String>,
}
```

### Phase 0 providers

| Provider | Endpoint | Auth | Dialects | Injection | Models | Key origin (§11) |
|----------|----------|------|----------|-----------|--------|------------------|
| **`litellm`** (first) | **Configured** by the operator: one or more `[[llm.endpoints]]` with `provider = "litellm"` and a `base_url` (e.g. a central LiteLLM proxy). | `api_key` (LiteLLM virtual key `sk-…`) | as declared per endpoint; default `["openai_chat", "openai_responses", "anthropic_messages"]` | `Authorization: Bearer <key>`; any agent-supplied `x-api-key` header is removed | `GET <base>/v1/models` (OpenAI list format); fallback to the endpoint's static `models` list | `endpoint` (a key that works on the operator's endpoint is that operator's key); optionally `litellm_team` (LiteLLM's key-info endpoint called with the key itself returns its team and user ids; the exact call is pinned in the provider and covered by the live suite against the deployed LiteLLM version) |
| **`anthropic`** (second) | Fixed `https://api.anthropic.com` | `api_key` | `anthropic_messages` | `x-api-key: <key>` (agent-supplied `Authorization` removed) | `GET /v1/models` (paged); fallback static list | `provider_account`: `GET /v1/models?limit=1` with the key; the `anthropic-organization-id` and `anthropic-workspace-id` response headers name the key's organization and workspace. The same headers on every ordinary response feed the response check. |

Model location in requests (`model_from_request`): for both providers, `POST`
requests to model-invoking paths (`/v1/messages`, `/v1/messages/count_tokens`,
`/v1/chat/completions`, `/v1/completions`, `/v1/responses`, `/v1/embeddings`,
and the same paths under LiteLLM route prefixes) carry the model in the JSON
body at `/model`; other requests use no model.

Endpoint configuration (shared by API and gateway):

```toml
[llm]
enabled_providers = ["litellm", "anthropic"]
provider_preference = ["litellm", "anthropic"]      # auto-selection order (§Model selection)

[[llm.endpoints]]
id = "central"
provider = "litellm"
display_name = "Central LiteLLM"
base_url = "https://llm.example.org"
dialects = ["openai_chat", "anthropic_messages"]
default_model = "claude-sonnet-5-5"
models = []                                        # optional static fallback
allowed_models = []                                # optional instance-wide model allow-list (§11); empty = any
key_origin = { method = "endpoint" }               # §11

[[llm.endpoints]]
id = "anthropic"
provider = "anthropic"
key_origin = { method = "provider_account", allowed_organization_ids = ["<corporate org uuid>"], allowed_workspace_ids = [] }
```

The `anthropic` endpoint is implicit (`id = "anthropic"`, no key-origin rule)
when the provider is enabled and no endpoint of that provider is declared;
declare it explicitly to attach origin rules or a model allow-list. Adding a provider later (OpenAI, Azure OpenAI, Bedrock, Vertex, a
generic OpenAI-compatible endpoint) is a module + one registry line; adding an
endpoint of an existing provider is configuration only. (phase 5 (D-138, moved from phase 4), P4-LLM) These five are
specified below (*Phase 4 providers*).

### Phase 4 providers (P4-LLM, D-121, SEC-64, SEC-65)

From phase 4 the registry also holds `openai`, `azure_openai`, `bedrock`, `vertex` and
`openai_compatible`. **Nothing changes for an existing deployment**: `llm.enabled_providers`
keeps its default `["litellm", "anthropic"]`, so the new providers exist only where an
operator enables them and declares an endpoint. Providers enabled but missing from
`provider_preference` are appended to it in `enabled_providers` order. The trait of §6 is
unchanged except for the additions listed under *Shared mechanics* below; every provider is
a module of `aurora-llm` plus one registry line.

| Provider | Endpoint (hosts) | Auth modes | Dialects | Injection (gateway) | Models | Key origin (§11) |
|----------|------------------|-----------|----------|---------------------|--------|------------------|
| **`openai`** | Fixed `https://api.openai.com`; optional `organization`, `project` set as `OpenAI-Organization` / `OpenAI-Project` | `api_key` | `openai_chat`, `openai_responses` | `Authorization: Bearer <key>`; agent-supplied `Authorization`, `OpenAI-Organization`, `OpenAI-Project` removed, the configured ones set | `GET /v1/models` (paged); fallback static | `provider_account`: the organization and project the key reports in the `openai-organization` / `openai-project` response headers of `GET /v1/models` (and of every ordinary response, for the backstop); the exact fields are pinned in the provider and covered by the live suite, like `litellm_team`. `organization_id` ← organization, `workspace_id` ← project |
| **`azure_openai`** | **Configured**: `base_url = https://<resource>.<azure-domain>` (https, host only; the OpenAI **v1 API** `/openai/v1/…` is the only surface supported in phase 4, so no `api-version` is involved) | `api_key` (the resource key) | `openai_chat`, `openai_responses` | `api-key: <key>`; agent-supplied `Authorization` and `api-key` removed | **static `models` = deployment names (required, with `default_model`)**: Azure's data plane lists base models, not deployments; `GET /openai/v1/models` is used only to validate the key | `endpoint` only (a key that works on the operator's resource is that operator's) |
| **`bedrock`** | **Configured**: `region`; hosts `bedrock-runtime.<region>.amazonaws.com` (optional `base_url` override for a VPC endpoint). The control-plane host `bedrock.<region>.amazonaws.com` is used only by the API for listing and is **never** an agent-reachable injection target | `aws_sigv4` (IAM access key id + secret [+ session token], JSON value) and `api_key` (a Bedrock API key) | `anthropic_bedrock` (default); `openai_chat` / `openai_responses` only when the endpoint declares them (Bedrock's OpenAI-compatible surface) | `aws_sigv4`: the gateway **removes every agent-supplied `Authorization` and `X-Amz-*` header and SigV4-signs the final request** (service `bedrock`, the endpoint's region; the mechanism of `09 §5`, body buffered up to `LLM_SIGN_BODY_MAX` 8 MiB, larger → 413 `llm_request_too_large`); `api_key`: `Authorization: Bearer <key>` | `GET /foundation-models` and `GET /inference-profiles` on the control-plane host, signed by the API (needs `bedrock:ListFoundationModels`, `bedrock:ListInferenceProfiles`); on failure, the static `models` list | `aws_sigv4`: `provider_account` = the AWS account id from a signed `sts:GetCallerIdentity` (needs no permission); `organization_id` ← account id. `api_key`: `endpoint` only |
| **`vertex`** | **Configured**: `project_id`, `location` (`global` or a region); host `<location>-aiplatform.googleapis.com` (`aiplatform.googleapis.com` for `global`) | `gcp_service_account` (the service-account key file, JSON value) | `anthropic_vertex` (default); `openai_chat` (Vertex's OpenAI-compatible `…/endpoints/openapi/chat/completions`) when declared | the gateway **mints an OAuth2 access token** from the key (JWT bearer grant, scope `https://www.googleapis.com/auth/cloud-platform`) and sets `Authorization: Bearer <token>`; agent-supplied `Authorization` and `x-goog-*` removed | **static `models` (required, with `default_model`)**: no listing call that needs no extra IAM; the key is validated by minting a token | `provider_account`: the key's `project_id` and `client_email` (proven live by the successful mint); `workspace_id` ← project id, `user_id` ← service-account e-mail |
| **`openai_compatible`** | **Configured**: `base_url` (https, optional path prefix); any host, allowed by the guard even when private (like `litellm`) | `api_key` | as declared per endpoint (default `["openai_chat"]`; may add `openai_responses`, `anthropic_messages`) | `Authorization: Bearer <key>` or, per endpoint `auth_header = "api-key"` / `"x-api-key"`; the others removed | `GET <base>/v1/models` (or `models_path`); fallback static | `endpoint` only |

**Shared mechanics**

- **New dialects** (open vocabulary, `16`): `anthropic_bedrock` (the Anthropic Messages body over Bedrock's
  `invoke` and `invoke-with-response-stream`) and `anthropic_vertex` (the Anthropic body over
  `:rawPredict` / `:streamRawPredict`). Provider-native dialects with no harness that speaks them
  (Bedrock Converse, Gemini `generateContent`) are **not** declared in phase 4. A harness
  declares the dialects it speaks (`12`); model selection is unchanged (§Model selection).
- **Credentials are Aurora secrets** (`kind = llm`, `llm_provider`, `llm_auth_mode`): `api_key` values are a string;
  `aws_sigv4` is the JSON `{ "access_key_id", "secret_access_key", "session_token"? }`;
  `gcp_service_account` is the service-account key JSON (`"type": "service_account"`, `private_key`,
  `client_email`, `project_id`, `token_uri`). They are sealed like every secret (managed or reference
  mode), validated for shape at save (422 `llm_key_invalid`), write-only, and **never reach a sandbox, a
  BYO client, the hook host, a response, a log or a metric**; only the gateway (injection) and the API
  (validation, listing, origin) open them (SEC-64). The endpoint configuration holds **no credentials**:
  a key named `api_key`, `access_key*`, `secret*`, `private_key`, `password` or `token` in an
  `[[llm.endpoints]]` entry refuses startup. Temporary AWS credentials (`session_token`) are not
  refreshed by Aurora: they stop working when they expire and the key must be replaced; use a
  least-privilege IAM user (`bedrock:InvokeModel*` only) or a Bedrock API key.
- **Vertex tokens.** The mint is stateless (no single-use refresh token), so unlike OAuth refresh
  (§8) it needs no database lock: each gateway replica mints for itself, single-flight per
  `(secret_id, value_version)`, and caches the token in the zeroizing in-memory cache for
  `min(expires_in − 300 s, 55 min)`; the signed assertion is valid ≤ 10 min and never logged. The key's
  `token_uri` must equal `https://oauth2.googleapis.com/token` (anything else is refused at save, so a
  crafted key cannot redirect the assertion); the mint goes out through the guarded dial with a
  `LLM_TOKEN_MINT_TIMEOUT` of 10 s. A failed mint answers 503 `credential_unavailable` and nothing is
  forwarded.
- **Inference-only paths** (SEC-65). Where a provider's credential could do more than call a model (an IAM key,
  a GCP service account), Aurora injects it only on the provider's **inference path set**
  and only for the endpoint's own account, region and project; anything else on the endpoint host
  is refused by the gateway with 403 `llm_path_not_permitted` (`details: { provider,
  endpoint_id }`), checked in step *LLM governance* before policy, and applied only where Aurora
  would inject (an agent's own key under `open` posture is unchanged). The sets are code, pinned per
  provider and covered by tests; the operator can only narrow them (`allowed_paths`):
  - `openai`: `POST /v1/chat/completions`, `/v1/completions`, `/v1/responses`, `/v1/embeddings`; `GET /v1/models[/{id}]`; `GET|DELETE /v1/responses/{id}` (and `/input_items`, `/cancel`).
  - `azure_openai`: the same paths under `/openai/v1/`.
  - `bedrock` (runtime host): `POST /model/{modelId}/{invoke|invoke-with-response-stream|converse|converse-stream|count-tokens}` and, only for a declared OpenAI dialect, `POST /openai/v1/{chat/completions|responses}`. Guardrail, async-invoke, agent and every other runtime route are refused.
  - `vertex`: `POST /v1/projects/<project_id>/locations/<location>/publishers/{anthropic|google}/models/<model>:{rawPredict|streamRawPredict|generateContent|streamGenerateContent|countTokens}` and, when declared, `POST …/endpoints/openapi/chat/completions`; the **project and location in the path must equal the endpoint's**, otherwise `llm_path_not_permitted`.
  - `openai_compatible`: derived from the declared dialects relative to the base path (`openai_chat` → `/v1/chat/completions`, `/v1/completions`; `openai_responses` → `/v1/responses` family; `anthropic_messages` → `/v1/messages`, `/v1/messages/count_tokens`; always `/v1/models`, `/v1/embeddings`); `allowed_paths = ["/**"]` is the explicit opt-out for servers with other routes.
  - `litellm` (**phase 4, live**, D-177, FIX-105): `POST /v1/messages`, `/v1/messages/count_tokens`, `/v1/chat/completions`, `/v1/completions`, `/v1/responses`, `/v1/embeddings` (also under a route prefix, as `model_from_request` reads them) and `GET /v1/models[/{id}]`. The LiteLLM management API (`/key/*`, `/user/*`, `/team/*`, …) and every other route get no key: 403 `llm_path_not_permitted`. For `litellm` and `openai_compatible` the per-endpoint `allowed_paths` **adds** paths to the set (absolute path, or a prefix ending in `/*`, at most 32 entries): an operator who needs `GET /health/liveliness` lists it; `/*` is the explicit opt-out.
  (`anthropic` keeps its phase-0 behaviour: no path restriction until phase 5.)
- **`model_from_request`** per provider: `openai`, `azure_openai`, `openai_compatible`: JSON body `/model` as today (`Unknown` when the body prefix is truncated or unparsable → refused when an allow-list applies). `bedrock`: the percent-decoded `{modelId}` path segment (an inference-profile ARN or id, compared exactly with the allow-list; no body needed). `vertex`: the `<model>` before `:` in the path (`claude-sonnet-4-5@20250929`, no publisher); for the OpenAI-compatible route, body `/model`. Allow-list entries are therefore the ids the client really sends (for Azure, the deployment name; for Bedrock, the profile id such as `eu.anthropic.claude-…`).
- **Streaming.** Responses are streamed through unbuffered: SSE (OpenAI, Azure, Vertex, compatible) and the AWS binary `application/vnd.amazon.eventstream` (Bedrock). The gateway never parses or rewrites a response body, sets no response timeout beyond `UPSTREAM_HEADER_TIMEOUT` (time to response headers), and only Bedrock's `aws_sigv4` needs the *request* body buffered (to sign it).
- **Error classification and usage.** One table in `aurora-llm` (`classify(provider, status, error_type)`), used by the gateway (request-log `error_code` `llm_auth`, `llm_rate_limited`, `llm_quota`, `llm_overloaded`, `llm_refusal`; metric `gateway_llm_upstream_total{provider,endpoint,class}`) and by the harness adapters for the turn error codes of `12`: 401/403 with the provider's credential errors (OpenAI `invalid_api_key`; Azure 401; Bedrock `UnrecognizedClientException`, `InvalidSignatureException`, `ExpiredTokenException`, `AccessDeniedException`; Vertex 401/403 `UNAUTHENTICATED`/`PERMISSION_DENIED`) → `auth`; 429 and `ThrottlingException` / `RESOURCE_EXHAUSTED` → `rate_limited`; OpenAI `insufficient_quota`, Bedrock `ServiceQuotaExceededException` → `quota`; 5xx, 529, `ServiceUnavailableException`, `ModelNotReadyException` → `overloaded`; Azure `content_filter`, Bedrock guardrail intervention, Vertex `SAFETY` → `refusal`. The gateway **never alters** an upstream status or body (clients keep their own retry logic); a spike of `llm_auth` marks nothing automatically. **Usage** stays harness-reported (`turns.usage`, `10 §Usage and metering`); the API stamps `provider`, `endpoint_id` and `model` from the turn's `ModelChoice`, never from what the harness claims, so usage aggregates are correct for every provider. The gateway does not count tokens.
- **Governance** (§11) applies to every new endpoint unchanged: instance endpoint list, org `endpoints.<id>`, key scopes, `sanctioned_only`, origin rules (the id meanings are in the table: for `bedrock` `allowed_organization_ids` are AWS account ids, for `vertex` `allowed_workspace_ids` are GCP project ids, for `openai` organization and project ids), model allow-lists, the origin preflight and the response backstop (`origin_from_response` exists for `openai` only; the others have none). The LLM hosts of a configured endpoint are **reserved** (§LLM hosts) and leave the known-hosts list automatically; the hosts of a new provider that has **no** configured endpoint stay *known* LLM hosts (refused under `sanctioned_only`), as today. The credentialed Default Rule skips these hosts exactly as it skips the phase-0 ones (`07 §Why the Default Rule skips LLM hosts`); the carve-out is by **exact configured host** (never `*.amazonaws.com` or `*.googleapis.com`), so enabling Bedrock does not exempt any other AWS or Google API from the Default Rule, and an explicit block rule naming an endpoint host still wins.
- **Configuration** (`15`): endpoint keys per provider are in the table of `15`; unknown keys, a non-https `base_url` (except loopback), a `base_url` with userinfo or query, an empty `models`/`default_model` for `azure_openai` and `vertex`, an invalid region/location/project id, or a duplicate endpoint id refuse startup naming the key. API-side calls (validation, listing, origin) and the gateway's token mint honour `tls.extra_ca_bundle`.

### LLM hosts

Two host sets matter:

- **Endpoint hosts**: every host of every configured provider endpoint.
  `is_llm_host(host)` is true for these. They are **reserved**: plugin
  packages cannot claim them (`09`), and the credentialed default-deny never
  applies to them (`07`). Because endpoints are operator-configured, their
  hosts are implicitly allowed by the gateway's destination guard even when
  they resolve to private addresses (e.g. an in-cluster LiteLLM); upstream TLS
  to them uses `tls.extra_ca_bundle` when set.
- **Known LLM hosts**: hosts of well-known model APIs that Aurora does *not*
  have a configured endpoint for (`governance.llm.known_llm_hosts`, §11).
  They are not reserved for plugins, but governance can make them unreachable.

Together they are the **governed LLM hosts**. A generic secret can never
target a governed LLM host (§11), and app connections can never inject on one.

### LLM secrets

Created with `kind = llm`, `llm_provider`, `llm_endpoint_id`,
`llm_auth_mode`; host pattern and recipe are derived and read-only. Creation and
every value change run, in order:

1. **Governance admission** (§11): the endpoint must be permitted for the org
   and the secret's scope (organization / workspace) must be permitted for that
   endpoint, else 403 `llm_key_not_permitted`.
2. **Validation**: list models with the key (failure → 422 `llm_key_invalid`).
   (phase 5 (D-138, moved from phase 4), P4-LLM) For providers without a usable listing call validation is the
   provider's own cheapest proof of a live key: `azure_openai` `GET /openai/v1/models`,
   `bedrock` with `aws_sigv4` a signed `sts:GetCallerIdentity`, `vertex` the token mint;
   the value's shape (JSON fields, `token_uri`, key type) is checked first.
3. **Origin verification** when the endpoint has a key-origin rule: call
   `verify_key_origin`, compare with the effective rule, store the result in
   `secrets.origin`. A key from another account → 422
   `llm_key_wrong_organization` and nothing is saved; a provider that cannot
   report the account → 422 `llm_key_unverifiable`.

The check uses the provider's own `GET /v1/models` **without** the static-list fallback that
feeds the model picker (FIX-105): an endpoint that answers an error or nothing never makes a key
`verified`, whatever `models` is configured.

If the endpoint is unreachable: without a key-origin rule the key may be saved
anyway (`origin.status = pending`, verified by the gateway before first use);
with a key-origin rule → 422 `llm_endpoint_unreachable`, nothing saved.

Org-level LLM secrets are common (one central LiteLLM key per org). Personal
keys at workspace scope are allowed where governance permits workspace-scoped
keys — the rules decide *which account* a key must belong to, not who may hold
one. Like all credentials, LLM secrets are used only when granted.

### Model selection for hosted agents

`agents.model_provider`, `agents.model_endpoint_id`, `agents.model_id`:

- Only **usable** keys count: granted, `active`, admitted by governance, and
  (where the endpoint has a key-origin rule) with a verified origin that still
  matches the effective rule (§11).
- When all are null, the dispatcher picks the first usable LLM secret, in
  `provider_preference` order, whose endpoint serves a dialect the agent's
  harness speaks (`12 §Harness interface`; phase 4, P4-HAR: the dialect used is the
  first of the harness's own preference-ordered list that the endpoint also serves),
  and uses the endpoint's
  `default_model` (or the provider's) — or, when a model allow-list applies
  and the default is not on it, the first allowed model.
- An explicit model is stamped with its provider and endpoint. If that
  provider/endpoint is no longer usable, or the model is no longer allowed, the
  override is **dropped** (with a notice in the next turn), never re-pointed;
  provider is never inferred from a model id.
- Model pickers (`GET /v1/agents/{a}/models`, `GET /v1/secrets/{id}/models`)
  list only allowed models.
- No compatible granted key → the turn fails with error code `no_model_key`
  and the dashboard deep-links to "store key → grant to agent".
- (phase 4, FIX-019, D-136) A usable key but **no model** (no agent
  override, and the chain above ends empty: no endpoint `default_model`, no
  provider default, no allow-list, no static `models` — the normal case for a
  LiteLLM endpoint configured with only `base_url`) → the turn fails with
  `no_model_key`'s sibling **`no_default_model`** and the dashboard links to the
  agent's model setting (Instructions). The live model listing is **never**
  used to pick a model automatically: its order is not meaningful and it mixes
  chat, embedding and image models.

#### Model readiness (phase 4, FIX-019, D-136)

One function computes, for an agent, every granted LLM key's readiness and the
default selection; the dispatcher's `no_model_key` / `no_default_model`
decision and `GET /v1/agents/{agent}/model-readiness` both use it, so the
dashboard always explains exactly what the dispatcher decided. Per key, the
first failing check in this order is its `reason`:

| `reason` | Meaning | What fixes it |
|---|---|---|
| `not_active` | the secret is not `active` (pending proposal, rejected) | accept or replace the value |
| `endpoint_unknown` | the key's endpoint is no longer in `llm.endpoints` | operator; or a new key on a configured endpoint |
| `provider_disabled` | the endpoint's provider is not in `llm.enabled_providers` | operator |
| `scope_not_permitted` | governance does not permit this key scope on the endpoint | org admin (Governance) |
| `origin_unverified` | the endpoint has a key-origin rule and the key's origin is not `verified` for its current value version | Re-verify |
| `origin_not_compliant` | verified, but the account is not in the effective allow-list | a key from an allowed account |
| `no_shared_dialect` | the endpoint serves no dialect the agent's harness speaks | operator (`dialects`) or another harness |

A key with no failing check is `usable`. `default` is what the next turn would
use without an override (`{ endpoint_id, model_id }`), or `null` when no model
resolves (`no_default_model`). An endpoint whose provider lists models live and
that has no `default_model` logs a **warning at start** (`15`); it still starts.
- The supervisor receives `{ provider, endpoint base URL, dialect, model,
  effort, client_env }`. `client_env` carries **placeholders only**; the
  gateway injects the real key.

### Platform-internal LLM calls

Phase 0 has exactly one: model catalogue listing (API → endpoint, with the
granted key, cached 10 min). Future internal calls (reaction picking, memory
review) MUST go through a single `PlatformLlm` service that names the
workspace secret it uses and records usage.

## 7. Gateway credentials

| Kind | Prefix | Minted | Expires / revoked |
|------|--------|--------|-------------------|
| `agent` | `aug_` | BYO agent creation; `POST /v1/agents/{a}/token` (regenerate revokes the previous one and all of the agent's `context` credentials) | phase 0–2: never expires; **from phase 3 (RFE-013)** expires at `created + effective maximum age` when one is configured (below), otherwise still never; revoked on regenerate / agent delete. (phase 2, RFE-025) refused while the effective BYO posture is `disabled` and, when the agent has an effective source list, outside it (§11.9) |
| `boot` | `aub_` | each `sandbox.start` claim, bound to `boot_id` | revoked when the boot ends (stop report, failure, reconcile) |
| `context` (hosted) | `auc_` | each `turn.deliver`, bound to agent, conversation, turn | `expires_at = now + TURN_CEILING + grace` at mint; set to `now + CONTEXT_CREDENTIAL_GRACE` when the turn ends |
| `context` (BYO) | `auc_` | `POST /v1/agent/context-credentials` authenticated with the agent token: `{ "label": string ≤128, "conversation_id"?: uuid, "ttl_secs"?: 60..86400 (default 3600) }` | at `expires_at`; revoked with the agent token |

Context credentials are for **attribution**, not isolation: conversations of
one agent share a sandbox. The security boundary is the agent.

(phase 2, RFE-025) **Governed BYO.** Creating a BYO agent or regenerating its
token is subject to the BYO posture (`§11.9`); a BYO `agent`/`context` credential
is admitted only from the effective source CIDR list of its agent; and an agent
with `byo_require_context` may use its `aug_` token only to mint `auc_`
credentials and for uncredentialed traffic.

(phase 3, RFE-013, D-60) **Agent token maximum age.** Phase 0–2 `aug_` tokens
never expired. From phase 3 an operator and each org may set an **optional
maximum age** for BYO agent tokens; the default is **none** (tokens still never
expire, so nothing changes unless someone opts in).

- **Setting.** Instance: `governance.agent_tokens.max_age_days` (`15`; absent =
  none). Org: `governance.agent_tokens.max_age_days` in the org governance
  document (`§11.2`; absent = none). `AGENT_TOKEN_MAX_AGE_MIN`…`MAX` = 1…3 650 days
  (422 `governance_invalid` outside). **Tighten-only**: the effective value is the
  smaller of the values set; an org value above the instance value is refused (422
  `governance_below_instance_minimum`).
- **At mint** (BYO creation and `POST /v1/agents/{a}/token`; the regenerate flow
  is unchanged otherwise): `gateway_credentials.expires_at = now + effective
  maximum age`, null when none. BYO `context` credentials never outlive the token
  that minted them: `expires_at = least(requested expiry, the agent token's
  expires_at)`.
- **When the setting is introduced or tightened** the API, in the same
  transaction as the governance change (instance: at API start, as a system
  action), sets for every live `agent` credential of the scope
  `expires_at = least(expires_at, greatest(created_at + max_age, now +
  AGENT_TOKEN_RETROFIT_GRACE))` (grace 7 days), so existing integrations get a week
  to rotate rather than breaking at once, and invalidates the `org` scope.
  Loosening or removing a limit **never extends** an existing expiry: regenerate
  to obtain a fresh one. The change is part of the audited `governance.update`
  (metadata `agent_tokens_retrofitted: n`).
- **Expired token.** The gateway rejects it like a revoked one: 407
  `proxy_auth_required` with `details.reason = "agent_token_expired"`; the API's
  agent-token routes answer 401 `agent_token_expired`. The caller regenerates in
  the dashboard (any actor allowed to regenerate under the BYO posture, §11.9).
  The expiry is checked on every request (the cached identity carries `expires_at`).
- **Visibility and staleness.** `gateway_credentials.last_used_at` (new churn
  column: written by the gateway on authentication and by the API on agent-token
  routes, at most once per minute per credential) and `expires_at` are shown
  on the agent's Connect tab and in agent lists. A token is **stale** when it has
  not been used for `AGENT_TOKEN_STALE_DAYS` (30) days (never used counts from
  `created_at`) and **expiring** within `AGENT_TOKEN_EXPIRING_DAYS` (14) days;
  `GET /v1/workspaces/{ws}/agent-tokens` (`can_own`) and `GET
  /v1/orgs/{org}/agent-tokens` (org owner/admin) list them with `?stale=true`
  and `?expiring_within_days=n` filters, so owners can regenerate or delete
  unused identities. Listing never returns token material.

(phase 2, RFE-005) A hosted `context` credential also carries the turn's
**speaker** (`gateway_credentials.speaker_user_id`, copied from
`turns.speaker_user_id` at mint when the speaker is a person; null otherwise).
Policy `user` identities match only this value (`07`). `boot`, `agent` and BYO
`context` credentials never carry a speaker. Because conversations of one agent
share a sandbox, a compromised agent process could borrow another concurrent
turn's credential; speaker-bound rules are therefore *attribution*, not
isolation (`17 §Residual risks`) — pair sensitive grants with agent-level rules and
approvals.

(phase 1, RFE-003) **Capabilities.** `gateway_credentials.capabilities` (text
array, default empty; open vocabulary, unknown values ignored) records what a
credential may do beyond ordinary MITM traffic. Phase 1 defines one: `browser`
— permission to use the **open lane** (`06 §Open-lane admission and posture`).
It can be set only on `boot` credentials, by the API when it mints the boot
token for a `sandbox.start` whose boot document carries the capability
(`11 §The boot document`); `agent` and `context` credentials always have an
empty set. It is fixed at mint time (a new boot gets a new credential).

## 8. OAuth token lifecycle (gateway)

Connection credentials JSON contains the fields declared by the manifest
(`access_token`, `refresh_token`, …); `credentials_expires_at` mirrors the
access-token expiry. The **gateway** refreshes (it is on the request path).

1. **Trigger**: `credentials_expires_at < now + REFRESH_MARGIN`, or an upstream
   401 matching the app's `token.stale_codes` (once per request).
2. **Serialize per connection**: an in-process striped mutex, then a
   **session-level** Postgres advisory lock
   `pg_try_advisory_lock(hashtextextended('aurora:refresh:'||id, 0))` retried
   up to `REFRESH_LOCK_WAIT` on a dedicated pooled connection (no transaction is
   held open across the provider call). Rationale: refresh tokens are often
   single-use; a duplicate exchange may revoke the grant.
3. **Re-read** the row after acquiring the lock. If `credentials_version`
   changed (another instance refreshed), use the new value. If `status =
   reauth_required`, stop.
4. **Exchange** with the provider (token endpoint from the snapshot, client
   credentials from the minting `app_oauth_clients` row).
5. **Persist** in a spawned task that is not cancelled by client hang-up:
   `seal` the new credentials, then
   `UPDATE app_connections SET credentials_ref=$new, credentials_version=credentials_version+1, credentials_expires_at=$exp WHERE id=$id AND credentials_version=$read`.
   CAS failure → discard the new ref and re-read (someone else won).
6. `invalid_grant` (or the manifest's `token.reauth_codes`) → `status =
   reauth_required`, `reauth_required_at = now()`; only a user reconnect clears
   it. (phase 3, RFE-022: `reauth_required` is also set, with a `plugin_major_upgrade`
   reason, when a plugin major upgrade activates and the connection's stored
   credential keys no longer satisfy the new manifest's required fields,
   `09 §10`.) Transient errors (5xx, timeouts) → `credential_unavailable` for this
   request, no state change.
7. Release the lock.

`token: hook` (`token.mint`) follows the identical discipline with the hook
host as the "provider" (`09 §Hooks`).

### OAuth clients

Clients come from `app_oauth_clients` at three scopes, most specific first:
workspace → organization → **instance** (rows with `org_id IS NULL`, managed by
instance admins). A connection records the client that minted it
(`oauth_client_id`) because refresh tokens are bound to their client; deleting a
client that has connections is refused (409 `client_in_use`).

## 9. Credential stubs

Apps may declare **stubs** (`09 §agent`): destination path under the home or
`/run/aurora/stubs`, content in which every secret field holds the sentinel
`aurora-managed`, expiry fields in the past. Written into hosted sandboxes at
boot (as files in the start payload) and returned by the BYO container-config.
Rules: never overwrite a file that lacks the sentinel; `mkdir -p`; mode 0600.

## 10. BYO bootstrap ("container config")

`GET /v1/agents/{agent}/container-config` (session/key) and
`GET /v1/agent/container-config` (agent token) return:

```json
{
  "proxy_url": "https://x:aug_…@gateway.example.org:4102",
  "env": { "HTTPS_PROXY": "…", "HTTP_PROXY": "…", "https_proxy": "…", "http_proxy": "…",
           "NO_PROXY": "localhost,127.0.0.1", "SSL_CERT_FILE": "/aurora/ca.pem",
           "NODE_EXTRA_CA_CERTS": "/aurora/ca.pem", "REQUESTS_CA_BUNDLE": "/aurora/ca.pem",
           "CURL_CA_BUNDLE": "/aurora/ca.pem", "GIT_SSL_CAINFO": "/aurora/ca.pem" },
  "ca_pem": "-----BEGIN CERTIFICATE-----…",
  "ca_path": "/aurora/ca.pem",
  "proxy_ca_path": "/aurora/ca.pem",
  "llm_env": { "ANTHROPIC_BASE_URL": "…", "ANTHROPIC_AUTH_TOKEN": "aurora-placeholder" },
  "stubs": [ { "path": "…", "content": "…", "mode": "0600" } ]
}
```

The token is embedded only when called with the agent token or right after
creation/regeneration (otherwise `proxy_url` contains `<AGENT_TOKEN>` and the
CLI substitutes its stored token). Hosted agents are refused (403
`hosted_agent_token_hidden`). `aurora run -- <cmd>` applies this to a child
process (writes the CA to a temp file, sets env, builds a combined CA bundle).

(phase 1, RFE-002) Phase 0 emitted `http://…:4102` because the gateway listener
was plaintext. From phase 1 the gateway proxy listener is **TLS-only**, so
`proxy_url` and every `*_PROXY` variable are `https://…` (the CLI refuses to
emit or accept an `http://` gateway URL for a non-loopback host). `ca_pem` is
the CA that signs both the proxy listener's certificate (outer leg) and the
MITM leaves (inner leg); `proxy_ca_path` names the file for clients that take a
separate trust anchor for the proxy leg (curl `--proxy-cacert`, git
`http.proxySSLCAInfo`). If the operator configured
`gateway.proxy_tls.cert_file`, `ca_pem` is still the MITM CA and the response
adds `proxy_tls: "operator_certificate"` so the CLI uses the system roots for the
outer leg. Client caveats are in `06 §Proxy listener TLS`.

(phase 2, RFE-025) For a BYO agent the response adds `byo: { posture, source_cidrs: [..] | null, require_context: bool }` (effective values), so the CLI can warn "this host's address is not in the agent's source list" before the first request, and `notes: [..]` when a context credential is required (`§11.9`). Both routes answer 403 `byo_disabled` while the effective posture is `disabled`.

## 11. LLM and egress governance

Phase: **0**. This section owns governance. It lets operators and org admins
control *which model accounts* agents may use and *where agents may send
traffic at all*, at several levels that can each only **tighten** the level
above, enforced at several points.

### 11.1 Levels

| Level | Who | Where stored | Can |
|-------|-----|--------------|-----|
| **Instance ceiling** | operator | configuration `[governance]`, `[byo]` (phase 2, RFE-025) and `[[llm.endpoints]]` (`15`) | enable providers and endpoints; set key-origin rules and model allow-lists per endpoint; set permitted key scopes; set **minimum** postures for every org (including, from phase 1, the **open-lane** minimum, RFE-003); set the known-LLM-hosts list and an egress baseline allow-list; (phase 3, RFE-013) set a maximum age for BYO agent tokens; (phase 2, RFE-025) switch BYO agents off (`byo.enabled`) and set the BYO posture floor and default source CIDRs |
| **Organization governance** | org owners/admins | `organizations.governance` (`03`) | permit a subset of endpoints; restrict key scopes per endpoint; narrow origin rules and model allow-lists; choose the LLM posture and the egress posture (never looser than the instance minimum); maintain the org egress allow-list; (phase 3, RFE-013) set a stricter maximum age for BYO agent tokens; (phase 2, RFE-025) choose the org **BYO posture** and default BYO source CIDRs |
| **Workspace policy** | workspace owners | the workspace's policy generation (`07`) | block destinations with rules; set the workspace's own uncredentialed default to `block` with its own allow-list (it cannot loosen the org); (phase 1, RFE-003) set the workspace's **open-lane posture** (`enabled` \| `allowlist_only` \| `disabled`; it can only tighten); (phase 2, RFE-025) set the workspace's **BYO posture** (`enabled` \| `owners_only` \| `disabled`; it can only tighten) |
| **Agent grants and rules** | workspace owners | grants, rules (`07`) | decide which permitted keys each agent may use |

Effective settings are computed, never stored: `effective = instance ∩ org`
for permissions and lists, `max(instance minimum, org)` for postures. When the
instance tightens later, orgs are tightened automatically and the dashboard
shows "enforced by instance".

### 11.2 Organization governance document

`organizations.governance` (jsonb, validated on write; `governance_revision`
for optimistic concurrency):

```json
{
  "llm": {
    "posture": "open | sanctioned_only",
    "endpoints": {
      "<endpoint_id>": {
        "allowed": true,
        "key_scopes": ["organization", "workspace"],
        "allowed_models": ["claude-sonnet-5-5", "..."],
        "origin": { "allowed_organization_ids": [], "allowed_workspace_ids": [], "allowed_team_ids": [] }
      }
    }
  },
  "egress": {
    "posture": "open | credentialed_only",
    "allowlist": [ { "host": "pypi.org" }, { "host": "*.githubusercontent.com", "path": "/**" } ],
    "open_lane": "enabled | allowlist_only | disabled"
  },
  "byo": {
    "posture": "enabled | owners_only | disabled",
    "source_cidrs": ["203.0.113.0/24", "2001:db8::/32"],
    "require_context_credentials": false
  },
  "agent_tokens": {
    "max_age_days": 90
  }
}
```

Rules (422 `governance_invalid` / `governance_below_instance_minimum`):

- Endpoints absent from `llm.endpoints` are **not permitted** (default for a
  new org: every instance endpoint present with `allowed: true`, the instance's
  scopes, and no narrowing).
- `key_scopes` ⊆ instance `allowed_key_scopes`.
- `allowed_models`: empty = the instance list for that endpoint (empty there =
  any); otherwise must be a subset of a non-empty instance list.
- `origin` lists: if the instance rule lists ids, the org lists must be subsets;
  if the instance rule lists none, the org may set its own (e.g. each org on a
  shared instance pins its own Anthropic organization). An `origin` narrowing on
  an endpoint whose instance method is `endpoint` is refused, except
  `allowed_team_ids` which upgrades the check to `litellm_team`.
- Postures cannot be below the instance minimums. (phase 1, RFE-003)
  `egress.open_lane` (default `enabled`; absent = `enabled`) follows the same
  rule with the order `enabled` < `allowlist_only` < `disabled`: it cannot be
  looser than `governance.egress.open_lane_minimum`. Under `allowlist_only` the
  org's `egress.allowlist` (∪ the instance baseline) is the list tunnels are
  checked against (`06 §Open-lane admission and posture`).
- Allow-list entries use the policy host/path patterns (`07`); ≤500 entries.
- (phase 3, RFE-013) `agent_tokens.max_age_days` (absent = none) follows §7: 1…3 650, never above the instance value; changing it applies the retrofit rule of §7 to the org's live agent tokens.
- (phase 2, RFE-025) `byo` (absent = all defaults) follows §11.9: `posture`
  (default absent = the instance floor) cannot be looser than the instance floor
  (order `enabled` < `owners_only` < `disabled`); `source_cidrs` is a list of ≤32
  CIDRs (absent/empty = no org default; each entry must lie inside the instance
  `byo.default_source_cidrs` when that list is non-empty, else 422
  `byo_cidr_outside_default`); `require_context_credentials` (default `false`)
  can only be switched on when the instance leaves it off. Invalid CIDRs, `/0`
  prefixes or more than 32 entries → 422 `governance_invalid`.

Changing governance re-evaluates every LLM secret of the org against the new
effective rules **using the stored origin** (no provider calls): secrets that
no longer comply get `origin.status = mismatch` or become non-permitted, are
skipped by the gateway, and their owners are notified. Agents' model overrides
that are no longer allowed are dropped. Gateways are invalidated (`org` scope).
Every change is audited (`governance.update`).

### 11.3 Key origin

`secrets.origin` (jsonb, LLM secrets only):
`{ "status": "verified" | "mismatch" | "unverifiable" | "pending", "method", "organization_id"?, "workspace_id"?, "team_id"?, "user_id"?, "value_version", "checked_at" }`.

A key **complies** when `status = verified`, `value_version` equals the
secret's current `value_version`, and the stored ids satisfy the *current*
effective rule: organization id ∈ allowed organizations (if listed), workspace
id ∈ allowed workspaces (if listed), team id ∈ allowed teams (if listed).
Method `endpoint` complies by construction once the key is validated.

(phase 4, FIX-019, D-136) Method `litellm_team`: `GET <base>/key/info` with
the key as bearer. A key whose info reports **no team** verifies with
`team_id = null` when no `allowed_team_ids` applies (instance rule and org
narrowing both empty), and is refused with reason `key_reports_no_team` when
team ids are required. A refused or failed verification records **why**
(`origin.detail`, ≤ 200 chars, no key material: `key_reports_no_team`, the
HTTP status of `/key/info` such as `key_info_http_401`, `endpoint_unreachable`,
`bad_response`), and the save / Re-verify error and the LLM key row show it.
The call is covered against the LiteLLM version deployed by the kind mock with
a **team key** and a **teamless key**, not only the master key.

Verification happens at three points:

1. **At save** (API, §6 LLM secrets) — the primary control: a non-compliant key
   is never stored, so no prompt is ever sent with it.
2. **Before first use of a value** (gateway): just before injecting an LLM key,
   the gateway checks an in-memory map `(secret_id, sha256(plaintext)) →
   KeyOrigin` with TTL `LLM_ORIGIN_RECHECK`. On a miss it calls
   `verify_key_origin` (timeout `LLM_ORIGIN_CHECK_TIMEOUT`) and only then
   injects. This catches values that change outside Aurora (reference-mode
   secrets in an external store) and keys verified under older rules. A
   non-compliant result → 403 `llm_key_wrong_organization` (or
   `llm_key_unverifiable`), `secrets.origin` updated, `NOTIFY
   aurora_secret_origin`. The provider being unreachable → 503
   `llm_origin_check_unavailable` (fail closed; already-verified values keep
   working until their TTL).
3. **On every response** (gateway backstop): for endpoints whose provider
   implements `origin_from_response`, the reported ids are compared with the
   rule. A mismatch replaces the response with 403
   `llm_key_wrong_organization` and marks the secret as in 2. Because the
   request has already been sent, this is a backstop, not the control. A
   response without the headers is counted (`gateway_llm_origin_header_missing_total`)
   but not refused.

Keys whose endpoint has no key-origin rule are treated as compliant (the
operator chose not to restrict that endpoint).

### 11.4 LLM posture

- **`open`**: governed LLM hosts may also be reached with credentials the agent
  supplies itself (Aurora injects nothing). Aurora-held keys must still comply.
- **`sanctioned_only`**: every request to a governed LLM host must carry a
  **sanctioned key injected by Aurora**: an `active` LLM secret of that host's
  endpoint, permitted by governance for the endpoint and scope, and compliant
  (§11.3). Requests to **known LLM hosts** (providers Aurora has no configured
  endpoint for) are refused outright.

(phase 3, RFE-023) **Relation to the policy Default Rule.** The credentialed Default
Rule (`07 §Evaluation`) deliberately does **not** apply to governed LLM hosts: the
model loop is credentialed traffic and must not be bricked by a default-deny of
tool credentials. This section — posture, endpoint and scope permissions, key
origin, model allow-lists — *is* the control for LLM traffic. Explicit policy
rules naming an LLM host still win (`07 §Why the Default Rule skips LLM hosts`).

### 11.5 Model allow-lists

When the effective `allowed_models` for an endpoint is non-empty, every request
on which Aurora injects a key of that endpoint is checked with
`model_from_request`: `Some(model)` must be on the list (exact id match),
`None` passes, `Unknown` (truncated or unparsable body) is refused. Refusal:
403 `llm_model_not_allowed` with the allowed list in `details`. Operators
SHOULD additionally restrict models on the provider side (LiteLLM per-key or
per-team model lists; Anthropic workspace settings), which is stronger than
any proxy check.

### 11.6 Egress posture (uncredentialed default-deny)

- **`open`** (default): requests on which Aurora injects nothing are allowed
  unless a rule blocks them.
- **`credentialed_only`**: requests on which Aurora injects nothing are
  **blocked** unless an explicit allow rule matches them or their host/path is
  on the egress allow-list (org list ∪ instance baseline). This applies to the
  MITM lane **and the open lane** (browser tunnels are uncredentialed by
  definition; only allow-listed hosts are reachable). Credentialed requests
  are unaffected (they are governed by grants, rules and the credentialed
  default). Refusal: 403 `blocked_by_egress_policy`.

(phase 1, RFE-003) The **open-lane posture** is a separate, orthogonal control
for the tunnel lane only: `enabled` (default) / `allowlist_only` (tunnels reach
only allow-listed hosts, even when the egress posture is `open`) / `disabled`
(no tunnels). It is set per level (instance `governance.egress.open_lane_minimum`,
org `egress.open_lane`, workspace policy `open_lane_posture`), tightens
downwards only, and the effective value is the strictest. Admission order and
errors: `06 §Open-lane admission and posture`; CONNECT-time rule semantics:
`07 §CONNECT-time evaluation`.

The org posture is evaluated as the org scope's *uncredentialed default*; a
workspace may set its own uncredentialed default to `block` in its policy
generation (`07 §Evaluation`). Typical allow-list contents: package registries,
public source hosting, documentation sites. Operators SHOULD provide a sensible
`baseline_allowlist` when they set `credentialed_only` as the instance minimum,
otherwise hosted agents cannot install packages.

### 11.7 Enforcement points (summary)

| Point | What is enforced |
|-------|------------------|
| API: secret create/update | endpoint permitted; key scope permitted; generic secrets refused on governed LLM hosts; key origin verified |
| API: governance change | re-evaluate stored origins; drop disallowed model overrides; invalidate gateways |
| API: model selection and pickers | only usable keys; only allowed models |
| Gateway step *governance* (`06`) | endpoint/scope permitted and stored origin compliant for the selected LLM key; `sanctioned_only` (no agent-supplied LLM credentials, no unconfigured providers); model allow-list |
| Gateway policy evaluation (`07`) | uncredentialed default per scope (`credentialed_only`) with allow-lists |
| Gateway open-lane admission (`06`, phase 1) | `browser` capability on the credential; effective open-lane posture (`disabled` / `allowlist_only`) before policy; CONNECT-time rule semantics |
| API: BYO agent create / token regenerate (phase 2, RFE-025) | effective BYO posture (`disabled` refuses; `owners_only` needs `can_own`) |
| Gateway credential admission (phase 2, RFE-025) | BYO posture `disabled` refuses `aug_`/BYO `auc_`; the source CIDR check; context credential required for credentialed requests |
| Gateway before injection | origin preflight for unseen key values |
| Gateway after response | origin backstop |

### 11.8 Limits

Governance covers traffic that passes through the gateway: all hosted-agent
traffic, and BYO traffic routed through the proxy. A BYO agent run outside
Aurora's network control can reach providers directly; that is a matter for
endpoint and network policy. A model reachable through some other API (a
self-hosted proxy, a SaaS that embeds a model) is not recognizable by host;
only `credentialed_only` egress closes that route.

### 11.9 BYO posture and source restriction (phase 2, RFE-025, D-59)

Phase 0/1: any member with `can_use` could create a BYO agent and receive an
`aug_` token that works from anywhere that can reach the proxy port. From phase 2
BYO is governed like the rest of this section. **Hosted agents are unaffected**
(they are controlled by `hosted.enabled`).

**(a) BYO posture.** Three values, ordered `enabled` < `owners_only` < `disabled`:

| Posture | Create a BYO agent / regenerate its token | Existing BYO credentials |
|---|---|---|
| `enabled` | `can_use` (phase 0 behaviour) | work |
| `owners_only` (default) | `can_own` of the workspace (org owners/admins included) | work |
| `disabled` | nobody (403 `byo_disabled`) | refused by the gateway (403 `byo_disabled`), live tunnels closed |

- **Layers**: instance master switch `byo.enabled` (boolean, default `true`;
  `false` ⇒ effective posture `disabled` everywhere, whatever org or workspace
  say) → instance floor `byo.minimum_posture` (default `owners_only`) → org
  `governance.byo.posture` → workspace `workspaces.byo_posture` (`03 §Planned
  changes`). **Effective = the strictest** of the layers (absent/null = inherit).
  Orgs and workspaces can only tighten; a value below the floor is refused
  (422 `governance_below_instance_minimum`).
- Because the default floor is `owners_only`, a deployment that wants phase 0
  behaviour sets `byo.minimum_posture = "enabled"`. Existing BYO agents and
  tokens are untouched by the upgrade.
- Refusals at creation/regeneration: 403 `byo_not_permitted` (posture
  `owners_only`, caller lacks `can_own`; `details.posture`, `details.level`) and 403
  `byo_disabled` (`details.level` = `instance`\|`org`\|`workspace`, `details.manage_url`).
  `POST /v1/agent/context-credentials` and `GET /v1/agent/container-config` with
  an agent token also answer 403 `byo_disabled` when disabled. Container-config
  viewing for an existing agent (token placeholder) stays `can_use`.
- Tightening to `disabled` **never deletes or revokes** anything: agents, tokens
  and homes stay; the gateway simply refuses them, so loosening again restores
  service instantly (invalidation scopes `agent`/`workspace`/`org`; the instance
  switch is read from configuration at start, so changing `byo.enabled` is a
  rolling restart). The dashboard marks affected agents "BYO disabled by
  <level>".

**(b) Source restriction.** `agents.byo_source_cidrs` (≤32 IPv4/IPv6 CIDRs; null =
no agent list) restricts where the credentials of a BYO agent may be used from.
The **effective list** is the agent's list if set, else the org default
(`governance.byo.source_cidrs`), else the instance default
(`byo.default_source_cidrs`), else *unrestricted*. Lists are tighten-only along
that chain: when an outer list is non-empty, every entry of an inner list must
lie inside it (422 `byo_cidr_outside_default`). `/0` prefixes (`0.0.0.0/0`,
`::/0`) and an empty array are refused (422 `byo_cidr_invalid`); clearing the
agent list means `null` (inherit). Editing an agent's list needs `can_own`
(security-critical audit action `agent.byo_settings.update`). The check
applies to `aug_` and BYO `auc_` credentials only, at every connect-time
admission (`06 §BYO admission`), and refuses with 403 `source_not_permitted`.
Plain `aub_`/hosted `auc_` are not subject to it.

**(d) Context credentials required.** `agents.byo_require_context` (default
`false`) makes the gateway refuse, for that agent, any request that would
**inject a credential** (the request has at least one selected candidate,
`07 §Injection selection`) when it is authenticated with the agent token
(`aug_`) instead of a context credential (`auc_`) — 403 `context_credential_required`
(`details.mint = "POST /v1/agent/context-credentials"`). Uncredentialed traffic
is unaffected. The effective value is `instance byo.require_context_credentials
∨ org governance.byo.require_context_credentials ∨ agent`; it can only be
switched **on** by outer layers. Every injected request of such an agent therefore
carries a labelled run (`context_label`, ≤128, sanitized, in the request log and
approval cards). `aurora run -- <cmd>` mints one context credential per run
(label `aurora run: <command line, truncated>`, `ttl_secs` default 3600, flag
`--ttl 60..86400`) and uses it as the proxy credential; the container config's
`proxy_url` for such an agent carries the placeholder `<CONTEXT_TOKEN>` and says
so in `notes`.

**(c)** The policy editor and `…/policy/warnings` flag allow rules with empty identities that select credentials (`07 §Injection selection`, `rule_applies_to_all_agents`).

**Residual**: a token is a bearer secret and can still be shared; the CIDR list
restricts network position, not identity, and users behind a shared NAT satisfy
a list that contains their egress address (`17 §Residual risks`).
