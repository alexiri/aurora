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
org_slug = "physics"
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

| Purpose | Use |
|---------|-----|
| `oauth-state` | AEAD for app-connect OAuth state |
| `oidc-state` | AEAD for sign-in OIDC state |
| `action-token` | HMAC-SHA256 for channel action tokens (`13`) |
| `avatar-key` | HMAC for rotating public avatar keys |
| `link-state` | AEAD for account-link landing pages |

`AURORA_RUNNER_SESSION_KEY` (shared by runner replicas and the API) is separate
(`11`). `AURORA_INTERNAL_SECRET` authenticates API ↔ gateway-internal ↔ hook host.

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
}

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
endpoint of an existing provider is configuration only.

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
3. **Origin verification** when the endpoint has a key-origin rule: call
   `verify_key_origin`, compare with the effective rule, store the result in
   `secrets.origin`. A key from another account → 422
   `llm_key_wrong_organization` and nothing is saved; a provider that cannot
   report the account → 422 `llm_key_unverifiable`.

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
  harness speaks (`12 §Harness interface`), and uses the endpoint's
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
| `agent` | `aug_` | BYO agent creation; `POST /v1/agents/{a}/token` (regenerate revokes the previous one and all of the agent's `context` credentials) | never expires; revoked on regenerate / agent delete |
| `boot` | `aub_` | each `sandbox.start` claim, bound to `boot_id` | revoked when the boot ends (stop report, failure, reconcile) |
| `context` (hosted) | `auc_` | each `turn.deliver`, bound to agent, conversation, turn | `expires_at = now + TURN_CEILING + grace` at mint; set to `now + CONTEXT_CREDENTIAL_GRACE` when the turn ends |
| `context` (BYO) | `auc_` | `POST /v1/agent/context-credentials` authenticated with the agent token: `{ "label": string ≤128, "conversation_id"?: uuid, "ttl_secs"?: 60..86400 (default 3600) }` | at `expires_at`; revoked with the agent token |

Context credentials are for **attribution**, not isolation: conversations of
one agent share a sandbox. The security boundary is the agent.

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
   it. Transient errors (5xx, timeouts) → `credential_unavailable` for this
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

## 11. LLM and egress governance

Phase: **0**. This section owns governance. It lets operators and org admins
control *which model accounts* agents may use and *where agents may send
traffic at all*, at several levels that can each only **tighten** the level
above, enforced at several points.

### 11.1 Levels

| Level | Who | Where stored | Can |
|-------|-----|--------------|-----|
| **Instance ceiling** | operator | configuration `[governance]` and `[[llm.endpoints]]` (`15`) | enable providers and endpoints; set key-origin rules and model allow-lists per endpoint; set permitted key scopes; set **minimum** postures for every org (including, from phase 1, the **open-lane** minimum, RFE-003); set the known-LLM-hosts list and an egress baseline allow-list |
| **Organization governance** | org owners/admins | `organizations.governance` (`03`) | permit a subset of endpoints; restrict key scopes per endpoint; narrow origin rules and model allow-lists; choose the LLM posture and the egress posture (never looser than the instance minimum); maintain the org egress allow-list |
| **Workspace policy** | workspace owners | the workspace's policy generation (`07`) | block destinations with rules; set the workspace's own uncredentialed default to `block` with its own allow-list (it cannot loosen the org); (phase 1, RFE-003) set the workspace's **open-lane posture** (`enabled` \| `allowlist_only` \| `disabled`; it can only tighten) |
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
| Gateway before injection | origin preflight for unseen key values |
| Gateway after response | origin backstop |

### 11.8 Limits

Governance covers traffic that passes through the gateway: all hosted-agent
traffic, and BYO traffic routed through the proxy. A BYO agent run outside
Aurora's network control can reach providers directly; that is a matter for
endpoint and network policy. A model reachable through some other API (a
self-hosted proxy, a SaaS that embeds a model) is not recognizable by host;
only `credentialed_only` egress closes that route.
