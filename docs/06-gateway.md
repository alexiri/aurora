# 06 — The Gateway

Phase: **0** (database lane: phase 2). Binary: `aurora-gateway`.

The gateway turns "an agent with no secrets" into "an agent that can use
exactly what it was granted". It is a tokio/hyper/rustls service, PID 1 in its
container, horizontally scalable.

## Listeners

| Port | Name | Reachable from | Serves |
|------|------|----------------|--------|
| 4102 | **proxy** | sandboxes, BYO agents | `CONNECT host:port`; absolute-form `http://` requests; (phase 2) database wire protocols sniffed by first bytes. **Nothing else**: origin-form requests get 400 `not_a_proxy_request`. |
| 4103 | **internal** | API replicas and monitoring only | `GET /healthz`, `GET /readyz` (snapshot loaded + DB reachable), `GET /metrics`, `GET /internal/v1/status` (instance id, snapshot hash, cache sizes, pending holds). Auth: `Authorization: Bearer <internal secret>` except health/metrics. |

The proxy port is raw TCP and cannot sit behind an HTTP path router. External
BYO agents reach it directly (LoadBalancer/NodePort; VPN recommended). The
advertised address is configured separately (`gateway.advertised_proxy_url`).

### Subcommands

- `aurora-gateway serve` — normal operation.
- `aurora-gateway generate-ca --out <dir>` — mints the CA (ECDSA P-256,
  10-year validity, `CN=Aurora Gateway CA <instance>`, key usage certSign,
  pathlen 0) and writes `ca.pem` + `ca.key` (0600). Refuses to overwrite.
  The CA MUST be minted by this command (consistent key-identifier scheme); an
  operator-supplied CA is accepted if it is a CA cert with an ECDSA P-256 or
  RSA-2048+ key.
- `aurora-gateway check-snapshot --snapshot <file> --fixtures <dir>` — offline
  mode used by `aurora plugin test`: loads a snapshot, runs fixture requests
  through the real matcher, injection and summary code against recorded
  upstreams; prints results as JSON.

## Credential authentication

- Every `CONNECT` and absolute-form request MUST carry
  `Proxy-Authorization: Basic base64(<user>:<token>)`. Missing or invalid →
  `407` with `Proxy-Authenticate: Basic realm="aurora"` and error
  `proxy_auth_required`.
- `<user>`: any value selects the **MITM lane**, except `open` which selects the
  **open lane**.
- `<token>`: an `aug_`, `aub_` or `auc_` credential. Lookup by SHA-256 hash in
  `gateway_credentials` (cached `GATEWAY_RESOLVE_CACHE_TTL`, invalidated by
  `aurora_gateway_invalidate` scope `credential`/`agent`). Rejected when revoked,
  expired, the agent's workspace/org is gone, or the org has no active owner
  (org suspended). Context credentials are accepted until
  `expires_at` (turn end + `CONTEXT_CREDENTIAL_GRACE`).
- Resolution yields the **request identity**: `{ credential_id, kind,
  agent_id, workspace_id, org_id, conversation_id?, turn_id?, context_label?,
  principals }` where `principals = {agent_id} ∪ {user ids with can_use on the
  workspace}` (resolved once per connect, cached with the credential).

## Lanes

| Lane | Selected by | Gateway sees | Injection | Approvals |
|------|-------------|--------------|-----------|-----------|
| **MITM** (default) | any username except `open` | method, path, headers, bounded body prefix | yes | held per request |
| **Open** (tunnel) | username `open` | host and port only | **never** | a decision that would need approval refuses the CONNECT |
| **Database** (phase 2) | DB wire protocol on the proxy port | each statement | gateway logs in with stored credentials | statement-level, same machinery (`21`) |

The open lane exists for browsers (TLS interception breaks bot defences and
browsers cannot install the CA). It keeps credential auth, host-level policy
(block / approval → refuse with 403 / rate limit → 429), the destination guard,
and writes one request-log row per tunnel with byte counts. It never applies
the credentialed Default Rule and never injects; it **does** apply the
uncredentialed default (`07 §Evaluation`), so under the `credentialed_only`
egress posture browsers reach only allow-listed hosts, and it applies the LLM
posture (governed LLM hosts are refused under `sanctioned_only`, since nothing
can be injected in a tunnel).

## MITM lane: connect-time resolution

On `CONNECT host:port` the gateway builds a **ResolvedHost** for
(credential, host) and caches it for `GATEWAY_RESOLVE_CACHE_TTL` under keys
prefixed by org and workspace (so invalidation can drop whole scopes):

1. **Policy view**: published org and workspace generations + derived rules +
   default actions (`07`), filtered to rules whose targets can match this host
   (network patterns, app hosts of the snapshot, connection/secret hosts).
2. **App candidates**: snapshot host index lookup by host (exact first, then
   suffix patterns, longest suffix wins), giving `(package, app, host entry)`
   tuples whose claim is not lost to another package. For each app, the
   workspace's and org's `active` connections of that app (with `tenant_host`
   equal to the request host when the host entry is tenant-bound).
3. **Secret candidates**: workspace and org secrets whose `host_pattern`
   matches (path patterns are evaluated per request).
4. **LLM candidates**: if the host is an LLM provider host (`08 §LLM
   providers`), the LLM secrets for that provider/endpoint.
5. **Injection selection** (`07 §Injection selection`): from the candidates,
   only those selected by allow-rules whose identities match the request
   identity. Ungranted candidates are kept separately for guidance.
6. **App availability**: if the org posture is `restricted`, apps not available
   to the workspace are removed and remembered for `app_unavailable`.

Credential ciphertext/refs may be cached; **plaintext is fetched lazily** at
injection time and cached in a separate, zeroizing, 60 s in-memory cache keyed
by `(SecretRef, version)`.

TLS is then terminated with a leaf certificate for the requested host
(SAN = host or IP; validity 7 days; signed by the gateway CA; cached in an LRU
of 10 000 entries; regenerated when <1 day remains). ALPN offers `h2` and
`http/1.1`; the upstream connection negotiates independently.

## Per-request pipeline (MITM lane)

Order is normative. Policy is decided before any credential is decrypted or
minted.

1. **Header hygiene.** Strip and remember Aurora request headers:
   `x-aurora-connection-id`, `x-aurora-batch-id`, `x-aurora-batch-label`,
   `x-aurora-batch-total` (each ≤ `BATCH_HEADER_MAX`; invalid → ignored).
   Drop hop-by-hop headers and `Proxy-*`.
2. **Destination guard.** IP-literal hosts are judged immediately; names are
   resolved through the guarded resolver at dial time. Refuse (403
   `destination_refused`) private, loopback, link-local, CGNAT, multicast,
   unspecified and IPv6 ULA ranges unless covered by `gateway.allowed_private`
   (CIDRs, IPs, hostnames, `*.suffix`); cloud-metadata addresses
   (169.254.169.254, fd00:ec2::254) only by exact-IP entry. The gateway's own
   database and internal services are refused by address+port regardless of the
   allow-list. When `gateway.egress_proxy` is set, the name check runs locally
   and resolution happens at the egress proxy.
3. **Method-override and path hygiene.** Requests carrying
   `X-HTTP-Method-Override`, `X-HTTP-Method`, `X-Method-Override` or a
   `_method` query parameter are refused (400 `method_override_refused`):
   policy judges the request line. The path is normalized (collapse `//`,
   decode percent-encoded unreserved characters); a path containing `.` or `..`
   segments, an encoded `/` (`%2F`), an encoded `\` or a NUL is refused (400
   `ambiguous_path`) so that matcher and upstream can never disagree.
4. **Body prefix.** Buffer up to `CONDITION_BODY_BUFFER` (and at least
   `APPROVAL_BODY_PEEK`) of the request body *only if* some candidate rule has
   body conditions, a GraphQL- or git-tagged tool may match, a hold could
   occur, or a model allow-list applies to the host (`08 §11.5`); the rest
   stays in the pipe. Record `truncated`.
5. **App and tool identification.** Look up the app serving (host, path) in the
   snapshot host index (longest path prefix wins among non-lost claims); find
   the matching tool in that app's catalogue (host, path, methods, GraphQL kind
   via the fail-closed classifier, git operation). Unmatched requests to an app
   host have `tool = None`.
6. **Credential selection (ids only, nothing decrypted).** Among the
   *selected* candidates (`07 §Injection selection`) that serve this request:
   - prefer the one named by `x-aurora-connection-id` (unknown/ungranted id →
     404 `connection_not_found`);
   - else exactly one candidate → it wins;
   - else several → 409 `multiple_connections` (same app) or
     `multiple_providers` (different apps/secrets), body lists ≤
     `CONNECTION_CHOICE_MAX` choices `{ id, app_id, label, scope }`.
   Precedence applied before ambiguity is declared: app connection over generic
   secret on the same host; workspace scope over org scope; path-limited secret
   over whole-host secret. Successful responses include
   `x-aurora-connections` (JSON array of choices) when more than one candidate
   existed. The result is the **winning credential** (possibly none).
   Generic secrets and app connections are never candidates on a governed LLM
   host (`08 §6`).
6a. **LLM governance** (`08 §11`; cached effective governance, no DB, nothing
   decrypted). Only for governed LLM hosts:
   - known LLM host (no configured endpoint) and the effective LLM posture is
     `sanctioned_only` → 403 `llm_provider_not_permitted`;
   - endpoint not permitted for the org → 403 `llm_provider_not_permitted`;
   - the winning credential is an LLM secret whose scope is not permitted for
     the endpoint → 403 `llm_key_not_permitted`, or whose stored origin does
     not comply → 403 `llm_key_wrong_organization` / `llm_key_unverifiable`
     (never forwarded without the key instead);
   - no winning credential and the effective LLM posture is `sanctioned_only`
     → 403 `llm_key_required`;
   - a winning LLM secret and a non-empty effective model allow-list →
     `model_from_request` must yield an allowed model or `None`; otherwise 403
     `llm_model_not_allowed`.
7. **Policy decision** (`07 §Evaluation`) with the winning credential known →
   `block` (403 `blocked_by_policy`), `default_denied` (403
   `blocked_by_default_policy`), `egress_denied` (403
   `blocked_by_egress_policy`), `rate_limited` (429 with `retry-after`),
   `needs_approval`, or `allow`.
8. **Approval hold** (if `needs_approval`): see *Approval holds* below.
9. **Token interception.** If the host entry declares `intercept:
   oauth_token_endpoint` and the request targets that token path, answer locally
   with a synthetic token response carrying the winning connection's current
   access token (after step 10) — never forward a stub refresh upstream.
10. **Credential freshness.** For connections with `token: oauth2_refresh`,
    refresh when `credentials_expires_at < now + REFRESH_MARGIN` (`08 §OAuth
    refresh`). For `token: hook`, call `token.mint` through the hook host under
    the same lock discipline (`09`). `reauth_required` connections → 403
    `connection_reauth_required` (with the dashboard URL).
11. **Injection.** Apply the host entry's mechanisms (`09 §Mechanisms`) or the
    secret's injection recipe (`08`). Agent-supplied values for injected
    headers/params are overwritten, never merged.
    **LLM key origin preflight** (`08 §11.3`): before an LLM key value is
    injected for the first time (per `LLM_ORIGIN_RECHECK`), verify its origin
    with the provider; non-compliant → 403 `llm_key_wrong_organization` /
    `llm_key_unverifiable`; provider unreachable → 503
    `llm_origin_check_unavailable`.
12. **Body transform** (rare, named mechanism) and **finalizer** (signing, e.g.
    `aws_sigv4`) run last, over the final request.
13. **Forward.** Pooled hyper client per (scheme, host, port, TLS config);
    upstream TLS verifies against system roots plus `tls.extra_ca_bundle`
    (organization-internal CAs) (+ per-connection CA when a manifest allows
    it, phase 2). Bodies stream both ways (SSE-safe). Wait for
    response headers at most `UPSTREAM_HEADER_TIMEOUT` → 504
    `upstream_timeout` (never retried). Gateway-internal headers are never
    sent upstream.
14. **Post-response.**
    - Upstream 401 and the app opted into stale-token detection (manifest
      `token.stale_codes`) on a body-less request → refresh once and resend
      with only the credential replaced.
    - Upstream 401/403 where **nothing was injected** → replace with guidance
      (below), preserving the status.
    - Upstream 401 with a recorded tenant/host mismatch → 421 `host_mismatch`.
    - **LLM origin backstop** (`08 §11.3`): for an injected LLM key whose
      endpoint has a key-origin rule, the account the provider reports in the
      response headers must comply; otherwise the response is replaced with
      403 `llm_key_wrong_organization` and the secret is marked.
15. **Telemetry.** Append a request-log row to the in-memory batch; update
    metrics; update `last_used_at` churn columns at most once per minute per
    credential.

**WebSocket upgrades**: steps 1–11 run on the upgrade request; then the
connection becomes a byte pipe with `WEBSOCKET_IDLE_TIMEOUT` idle limit across
both directions.

**Plain HTTP (absolute-form)**: same pipeline without TLS termination.

## Approval holds

1. Build the **approval summary** (`09 §Approval summaries`): manifest template
   for the tool → `approval.summarize` hook (only if declared; bounded call to
   the hook host with the body prefix ≤ `APPROVAL_BODY_PEEK`, no egress) →
   generic redacting summarizer. Raw bodies are never stored.
2. `INSERT INTO approvals (…, status='pending', deadline=now()+APPROVAL_TIMEOUT,
   gateway_instance_id)`; `NOTIFY aurora_approval created`. Failure to insert →
   503 `approval_store_unavailable` (fail closed).
3. Wait on the instance's LISTEN connection for `decided` of that id, with a
   2 s poll fallback reading the row. Keep the client connection open; do not
   read further body bytes.
4. `approved` → continue at step 9. `denied` → 403 `manual_approval_denied`
   (`details.approval_id`). Deadline reached → the gateway sets
   `status='expired'` (`WHERE status='pending'`) and answers 403
   `manual_approval_expired`. Client disconnect → `status='abandoned'`.
5. On SIGTERM, every held request is answered 503 `gateway_restarting`
   (`retry-after: 1`) and its approval marked `abandoned`.

Decisions are written by the API (`07 §Approvals`); first decision wins.

## Guidance responses (agent-facing vocabulary)

All refusals use the error envelope; definite verdicts carry
`x-should-retry: false`; transient ones carry `retry-after` when meaningful.
Messages are written to be acted on by an LLM agent and include the exact
dashboard URL to fix the problem when one exists.

| Code | Status | When | `details` |
|------|--------|------|-----------|
| `proxy_auth_required` | 407 | missing/invalid/expired credential | — |
| `not_a_proxy_request` | 400 | origin-form request on 4102 | — |
| `destination_refused` | 403 | destination guard | `host` |
| `method_override_refused` | 400 | step 3 | — |
| `ambiguous_path` | 400 | step 3 | — |
| `blocked_by_policy` | 403 | rule block | `rule_name`, `method`, `path`, `manage_url` |
| `blocked_by_default_policy` | 403 | default-deny | `manage_url` |
| `blocked_by_egress_policy` | 403 | uncredentialed default (`credentialed_only`) | `host`, `manage_url` (org egress allow-list) |
| `llm_provider_not_permitted` | 403 | governed LLM host not permitted for the org | `host` |
| `llm_key_required` | 403 | `sanctioned_only` and no Aurora-held key for the endpoint | `endpoint_id`, `manage_url` |
| `llm_key_not_permitted` | 403 | key scope not permitted for the endpoint | `secret_id` |
| `llm_key_wrong_organization` | 403 | key belongs to an account the rules do not allow | `secret_id`, `manage_url` |
| `llm_key_unverifiable` | 403 | the key's account cannot be determined | `secret_id` |
| `llm_model_not_allowed` | 403 | model not on the allow-list (or body unreadable) | `model`, `allowed_models` |
| `llm_origin_check_unavailable` | 503 | provider unreachable during origin preflight | `endpoint_id` |
| `rate_limited` | 429 | rate-limit modifier | `limit`, `per` |
| `manual_approval_denied` / `manual_approval_expired` | 403 | holds | `approval_id` |
| `gateway_restarting` | 503 | shutdown during hold | — |
| `approval_store_unavailable` | 503 | cannot persist hold | — |
| `multiple_connections` / `multiple_providers` | 409 | step 9 | `choices`, `header: "x-aurora-connection-id"` |
| `connection_not_found` | 404 | stale/ungranted id | — |
| `connection_reauth_required` | 403 | refresh refused earlier | `connection_id`, `reconnect_url` |
| `credential_unavailable` | 502 | granted secret/connection could not be produced (store error, mint failure) | `kind` |
| `secret_store_unavailable` | 503 | external store unreachable | `store_id` |
| `app_unavailable` | 403 | org posture restricts the app | `app_id` |
| `access_restricted` | (upstream status) | upstream 401/403, nothing injected, workspace holds an ungranted candidate | `kind`, `provider`, `manage_url` |
| `app_not_connected` | (upstream status) | known app host, no connection in scope | `app_id`, `connect_url` |
| `app_path_unserved` | (upstream status) | known app host, path no app serves | `host` |
| `credential_not_found` | (upstream status) | unknown host, nothing stored | `create_secret_url` |
| `host_mismatch` | 421 | tenant-bound credential on wrong host | `expected_host` |
| `upstream_timeout` | 504 | step 13 | — |
| `bad_gateway` / `resolution_failed` | 502 | upstream connect/DNS failure | — |

The guidance decision list for upstream 401/403 without injection, in order:
(1) a granted tenant-bound connection on the wrong host → `host_mismatch`;
(2) a granted secret whose value couldn't be produced → `credential_unavailable`;
(3) an ungranted candidate exists → `access_restricted`;
(4) an account of this app exists but is `disabled` → `access_restricted` with
`details.reason = "disabled"`; (5) known app host → `app_not_connected`;
(6) known host but unserved path → `app_path_unserved`;
(7) unknown host → `credential_not_found`.

Upstream responses are passed through unchanged when a credential *was*
injected (the agent should see the real upstream error).

## Caches and invalidation

| Cache | Key | TTL | Invalidated by |
|-------|-----|-----|----------------|
| Credential → identity | token hash | 60 s | `credential`, `agent`, `workspace`, `org`, `all`; `aurora_auth_revoked` |
| ResolvedHost | `org/ws/credential/host` | 60 s | `org`, `workspace`, `agent`, `connection`, `all` |
| Plaintext values | `(SecretRef, version)` | 60 s | `connection`, `workspace`, `org`, `all` (zeroized on eviction) |
| Effective governance (`08 §11`) | org | 60 s | `org`, `all` |
| LLM key origin verifications | `(secret_id, sha256(plaintext))` | `LLM_ORIGIN_RECHECK` | `org`, `workspace`, `all` |
| Leaf certificates | host | until near expiry | never |
| Registry snapshot | content hash | until replaced | `aurora_registry` (+ 30 s poll of the active hash) |

On LISTEN connection loss the gateway flushes all caches except leaf
certificates, then reconnects.

## Registry snapshot loading

At boot the gateway loads the active snapshot for the highest contract it
supports (`09 §Registry snapshot`), validates that every mechanism, decoder and
finalizer it names is known, builds the host index and tool matchers, and swaps
the structures atomically (`ArcSwap`). A snapshot naming an unknown mechanism is
refused (logged, metric `gateway_snapshot_rejected_total`) and the previous one
kept. `readyz` is false until a snapshot is loaded (an empty snapshot is valid).

## Rate limiting

Counters keyed by `(rule logical_id, agent_id, window start)` in memory per
replica (fixed windows: minute/hour/day aligned to UTC). With N gateway
replicas the effective limit can reach N× the configured value; this is
documented in the dashboard next to rate-limit modifiers. A shared counter
backend is a later-phase trait implementation (`CounterStore`).

## Telemetry

- Request log: in-memory batch flushed every `REQUEST_LOG_FLUSH` via `COPY`;
  on flush failure the batch is retried once, then dropped with a metric (never
  blocks requests). Never records secret values, bodies or raw query strings.
- Metrics (Prometheus): requests by lane/decision/status, upstream latency
  histogram, holds pending, cache hit ratios, refresh/mint outcomes, snapshot
  hash info gauge.
- `gateway_instances` heartbeat every 15 s (used to abandon orphaned holds).

## Graceful shutdown

SIGTERM: stop accepting connections; answer held requests with
`gateway_restarting`; let in-flight requests finish for up to 25 s; close
tunnels; flush the request-log batch; set `gateway_instances.stopped_at`.

## Statefulness and replicas

Gateways hold only caches. Approvals and refreshed credentials are in Postgres;
refresh is serialized by advisory locks; so any number of replicas may run.
The only per-replica approximation is rate limiting (above).
