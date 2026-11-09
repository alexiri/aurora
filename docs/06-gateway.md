# 06 — The Gateway

Phase: **0** (database lane: phase 2). Binary: `aurora-gateway`.

The gateway turns "an agent with no secrets" into "an agent that can use
exactly what it was granted". It is a tokio/hyper/rustls service, PID 1 in its
container, horizontally scalable.

## Listeners

| Port | Name | Reachable from | Serves |
|------|------|----------------|--------|
| 4102 | **proxy** | sandboxes, BYO agents | Phase 0: plain TCP. **From phase 1 (RFE-002): TLS only** (see *Proxy listener TLS* below): inside the TLS session, `CONNECT host:port`; absolute-form `http://` requests; (phase 2) database wire protocols (the database lane's transport is specified with that lane and MUST also be TLS-only). **Nothing else**: origin-form requests get 400 `not_a_proxy_request`. |
| 4103 | **internal** | API replicas and monitoring only | `GET /healthz`, `GET /readyz` (phase 0–2: snapshot loaded + DB reachable; **from phase 3 (RFE-014)** three states — `ready` 200, `degraded` 200 (serving from cache), `down` 503; `?strict=true` makes `degraded` 503; see *Operation during a Postgres outage*), `GET /metrics`, `GET /internal/v1/status` (instance id, snapshot hash, cache sizes, pending holds). Auth: `Authorization: Bearer <internal secret>` except health/metrics (phase 2, RFE-007: any key of the internal-secret ring, `X-Aurora-Key-Id` naming it; the status response lists the ring's key **ids**, `08 §4`). |

The proxy port is raw TCP and cannot sit behind an HTTP path router. External
BYO agents reach it directly (LoadBalancer/NodePort). The advertised address is
configured separately (`gateway.advertised_proxy_url`). Phase 0 said "VPN
recommended" because the listener was plaintext; from phase 1 the listener is
TLS-only and that caveat no longer applies (restricting source networks remains
good practice).

### Proxy listener TLS (phase 1, RFE-002)

Owner decision (D-44): **one TLS proxy listener, used by in-cluster and external
clients alike; no plaintext listener exists.** `Proxy-Authorization` (the `aug_`,
`aub_`, `auc_` token) and the `CONNECT` preamble therefore never cross a network
unencrypted.

- **Protocol**: TLS 1.2+ (rustls, aws-lc-rs provider; TLS 1.3 preferred, no
  legacy suites) terminated *before* any HTTP parsing. ALPN offers only
  `http/1.1` (proxy requests are HTTP/1.1; `h2` is not offered on this
  listener). Inside the session the semantics are exactly those of this
  document (authentication, lanes, errors). The client's `CONNECT` tunnel then
  carries the *inner* TLS (or the open-lane byte pipe) over the outer TLS session.
- **A plaintext connection** (first bytes are not a TLS ClientHello) is closed
  immediately without a response (nothing parseable to answer, and no credential
  is read); the gateway counts it in `gateway_proxy_plaintext_refused_total`.
- **Server certificate source** (default, recommended): a **leaf issued by the
  gateway CA** (`ca.pem`), `serverAuth` EKU, ECDSA P-256, validity
  `PROXY_TLS_LEAF_VALIDITY` (30 days), regenerated in memory and hot-swapped
  when < 10 days remain (no restart, no stored key). Rationale: every hosted
  sandbox and every BYO container config already trusts this CA, so no new trust
  distribution is needed. The SAN set is the union of: the host of
  `gateway.advertised_proxy_url`; the host of `gateway.sandbox_proxy_url`, the
  sandbox-facing proxy address in the boot document (`11`, FIX-010); the in-cluster service names (`aurora-gateway`,
  `aurora-gateway.<ns>`, `aurora-gateway.<ns>.svc`,
  `aurora-gateway.<ns>.svc.cluster.local`) and the gateway's fixed address on
  the podman `aurora-sandbox` network (`10.89.250.2`, `docs/operator/runners.md`;
  introduced by the phase-0 compose stack) when it is local; and `gateway.proxy_tls.names` (extra DNS
  names/IPs, e.g. a LoadBalancer hostname). A client connecting by a name not in
  the SAN set fails verification — operators add the name, they do not disable
  verification. Because `aurora-gateway generate-ca` mints the CA with
  `pathlen 0` the proxy leaf is a leaf of that CA (no intermediate).
- **Optional operator-supplied certificate**: `gateway.proxy_tls.cert_file` +
  `key_file` (PEM chain + key, reloaded on change) replace the CA-issued leaf —
  for example a publicly trusted certificate on the external name so BYO hosts
  need no CA for the outer leg. The certificate must cover every name clients
  use; startup refuses a missing/unreadable pair or an expired certificate,
  naming the key. Clients that use this mode still need `ca.pem` for the *inner*
  MITM leaves (unchanged).
- **Client side**: every Aurora-controlled client verifies the server
  certificate against the gateway CA bundle (plus the system roots when an
  operator certificate is in use) and the name it dialled: the supervisor's
  loopback forwarder (`12`), `aurora run`, and the container config (`08 §10`).
  Verification is never optional.
- **Client caveats for BYO tooling** (documented in the dashboard Connect tab
  and the operator guide): the proxy URL is `https://…` and the HTTPS proxy leg
  must be trusted *separately* from the origin leg by clients that distinguish
  them (curl and git need `--proxy-cacert` / `http.proxySSLCAInfo`; the
  container config sets the equivalent env where one exists and
  `proxy_ca_path` otherwise); Node's global `fetch`/`https` need an HTTPS-capable
  proxy agent (`undici` `ProxyAgent`, `https-proxy-agent`); Python needs
  `urllib3` ≥ 2 (TLS-in-TLS) on Python ≥ 3.10; Go's `net/http` and Chromium
  support `https://` proxies natively; stock Java (`HttpsURLConnection`) does
  **not** speak TLS to a proxy and needs a client library that does. A tool that
  cannot talk to an HTTPS proxy cannot use the gateway directly; the supported
  workaround is a local forwarder (`aurora run --forward`, which listens on
  loopback and speaks TLS upstream — the same pattern as the sandbox
  supervisor) rather than a plaintext listener. The conformance matrix in `18`
  pins which clients are verified.

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
  `aurora_gateway_invalidate` scope `credential`/`agent`). (phase 3, RFE-013: an `agent` credential with a maximum age carries `expires_at`; once past it the answer is 407 `proxy_auth_required` with `details.reason = "agent_token_expired"`; the gateway writes `last_used_at` at most once a minute per credential.) Rejected when revoked,
  expired, the agent's workspace/org is gone, or the org has no active owner
  (org suspended). Context credentials are accepted until
  `expires_at` (turn end + `CONTEXT_CREDENTIAL_GRACE`).
- (phase 2, RFE-006) Also rejected when the agent is **effectively frozen**
  (agent, workspace or org, `04 §Freeze`): 403 `frozen` (`details.level`), not
  407 — the credential is fine, the principal is paused. The frozen flags are part
  of the cached identity (invalidated by scopes `agent`, `workspace`, `org`); on
  such an invalidation the gateway **closes the live tunnels and WebSockets** of
  the affected credentials and answers requests held for approval with 403
  `frozen`.
- Resolution yields the **request identity**: `{ credential_id, kind,
  agent_id, workspace_id, org_id, conversation_id?, turn_id?, context_label?,
  principals }` where, in phase 0, `principals = {agent_id} ∪ {user ids with
  can_use on the workspace}` (resolved once per connect, cached with the
  credential).
  **From phase 2 (RFE-005, D-49)** the roster is gone: `principals = {agent_id}`
  and the identity carries `speaker_user_id?` — the person who authored the turn
  the credential is bound to. It is set only for **hosted `context` credentials**
  (minted by the API at `turn.deliver` from `turns.speaker_user_id` when the turn's
  `speaker_kind = person`; stored in `gateway_credentials.speaker_user_id`) and
  only while that user still has `can_use` on the workspace (checked at connect
  resolution; invalidated with the `workspace`/`org` scopes). Every other
  credential — `boot`, `agent` (BYO), BYO `context` (the agent mints those itself,
  so a conversation id it names proves nothing), and context credentials of
  turns with no person speaker (schedule, webhook, watch, peer, system, bot or
  agent speakers) — has **no speaker**.

### BYO admission (phase 2, RFE-025)

For `aug_` credentials and BYO `auc_` credentials (kinds `agent` and BYO
`context`) the gateway applies three more checks, in this order, **after** the
credential has authenticated (so an unauthenticated prober learns nothing) and
the freeze check, and before the lane is chosen. They run for every `CONNECT` and
absolute-form request (the identity cache carries the inputs; no DB on the hot path).

1. **BYO posture** (`08 §11.9`). Effective posture `disabled` — instance
   `byo.enabled = false`, instance floor, org or workspace — → **403
   `byo_disabled`** (`details.level`), not 407. The posture is part of the cached
   identity (invalidated by scopes `agent`, `workspace`, `org`); on such an
   invalidation the gateway closes the live tunnels and WebSockets of the affected
   BYO credentials and answers requests held for approval with 403 `byo_disabled`
   (approvals `abandoned`), exactly as for `frozen`. The order is: the 403 is
   answered, the approval is abandoned (never released upstream), then the tunnel
   closes (D-97).
2. **Source address.** If the agent has an **effective source list**
   (`08 §11.9`), the client address must lie inside one of its CIDRs, else **403
   `source_not_permitted`** (`details.source` = the address the gateway saw,
   `details.manage_url`). The client address is **only** the transport peer of
   the TLS connection, with one exception: when the peer lies inside
   `gateway.trusted_proxies` (CIDRs, default empty; the gateway's own key — the
   API's `server.trusted_proxies` does not apply), the connection **must begin
   with a PROXY protocol v1/v2 header** and the address in it is used (a layer-4
   load balancer or NodePort SNAT hides the real address otherwise; configure the
   balancer to send PROXY or to preserve the source address). A connection from a
   trusted proxy without that header, or a PROXY header from any other peer, is
   closed before the TLS handshake. HTTP headers (`X-Forwarded-For`, `Forwarded`)
   are **never** consulted: the client controls everything inside the tunnel.
   IPv4-mapped IPv6 peers are normalised to IPv4 before matching. Behind a NAT
   the list must contain the NAT's egress address; if the gateway only ever sees
   a load balancer's address (no PROXY header, balancer not preserving source),
   every request fails closed.
   A refusal is counted (`aurora_gateway_byo_refused_total{reason}`,
   `reason ∈ posture|source|context_required`), logged at most once per
   credential per minute, and recorded as one `request_log` row per credential
   per minute (`outcome = blocked`, `reason = source_not_permitted`) so owners can
   see attempted use of a leaked token.
3. **Context credential required.** Applies later, in the MITM pipeline, once
   injection selection has run: if the agent's effective `byo_require_context`
   is true, the credential is an `aug_` (not `auc_`), and at least one candidate
   is selected (`07 §Injection selection`) → **403 `context_credential_required`**
   before any approval is created or any value fetched (`details.mint`). Open-lane
   tunnels are not affected (BYO credentials never get the lane anyway).

## Lanes

| Lane | Selected by | Gateway sees | Injection | Approvals |
|------|-------------|--------------|-----------|-----------|
| **MITM** (default) | any username except `open` | method, path, headers, bounded body prefix | yes | held per request |
| **Open** (tunnel) | username `open` **and** a credential holding the `browser` capability (phase 1, RFE-003) | host and port only | **never** | a decision that would need approval refuses the CONNECT |
| **Database** (phase 2) | DB wire protocol on the proxy port | each statement | gateway logs in with stored credentials | statement-level, same machinery (`21`) |

The open lane exists for browsers (TLS interception breaks bot defences and
browsers cannot install the CA). It keeps credential auth, host-level policy
(block / approval → refuse with 403 / rate limit → 429), the destination guard,
and writes one request-log row per tunnel with byte counts. It never applies
the credentialed Default Rule and never injects (the Default Rule also never applies to governed LLM hosts, `07 §Why the Default Rule skips LLM hosts`, RFE-023); it **does** apply the
uncredentialed default (`07 §Evaluation`), so under the `credentialed_only`
egress posture browsers reach only allow-listed hosts, and it applies the LLM
posture (governed LLM hosts are refused under `sanctioned_only`, since nothing
can be injected in a tunnel).

### Open-lane admission and posture (phase 1, RFE-003)

Phase 0 gave every credential the open lane unconditionally (any client that set
the username `open` got an uncredentialed tunnel under the default `open` egress
posture). From phase 1 the open lane is admitted in this order, **before** any
policy evaluation, with the first failure refusing the `CONNECT`:

1. **Capability** (RFE-003 c). The authenticated credential must hold the
   **`browser` capability** (`gateway_credentials.capabilities`, `08 §7`). Only
   `aub_` boot credentials can hold it, and only when the sandbox's boot was
   started with it (`11 §The boot document`: the sandbox image declares it and
   the effective posture is not `disabled`). `aug_` BYO agent tokens and `auc_`
   context credentials never hold it → 403 `open_lane_not_permitted`. A client
   that merely sets the username `open` therefore gains nothing.
2. **Posture** (RFE-003 a). The **effective open-lane posture** is the strictest
   of instance, org and workspace (`08 §11.1`), ordered `enabled` <
   `allowlist_only` < `disabled`:
   - `disabled` → 403 `open_lane_disabled` (details `manage_url`);
   - `allowlist_only` → the target host:port must be covered by the allow-list
     of **every level that sets `allowlist_only`**: instance → the instance
     baseline allow-list; org → org egress allow-list ∪ baseline; workspace → the
     workspace's `uncredentialed_allowlist`. Matching uses the CONNECT-time
     semantics of `07 §CONNECT-time evaluation` (an allow-list entry that carries
     a `path` does **not** admit a tunnel). An explicit allow rule that is fully
     knowable at CONNECT time also admits (as for `credentialed_only`). Otherwise
     403 `blocked_by_egress_policy` with `details.open_lane = true`;
   - `enabled` → no extra restriction.
3. Then: destination guard, LLM posture, **CONNECT-time policy evaluation**
   (`07 §CONNECT-time evaluation`), tunnel, one request-log row.

The posture is read from the cached effective governance and the workspace's
published policy generation (no DB on the hot path; invalidation scopes `org`
and `workspace`), so changing it takes effect for the next `CONNECT`; open
tunnels end at the idle limit or when the client closes them.

## Database lane (phase 2, P2-DBL-1, D-80)

Spec: `21 §Phase 2 — Database lane`; invariant SEC-60. Code: `services/aurora-gateway/src/db/`
(engine-agnostic core: synthetic request, stored endpoint, authorizer) and `db/pg/` (PostgreSQL
wire adapter: `wire`, `server`, `relay`); classification is the `aurora-sqlclass` crate (links
libpg_query, used by the gateway only).

**Sniffing.** After the PROXY header (when present) and before the TLS check, the first 8 bytes
decide: a PostgreSQL `SSLRequest`, `GSSENCRequest`, `CancelRequest` or protocol-3 `StartupMessage`
(length and code consistent) enters the lane; a TLS record or HTTP goes where it went before. A
`StartupMessage` without TLS is answered with a FATAL error (28000) and closed **before** any
password is requested. `SSLRequest` → `S` and a TLS handshake whose certificate is chosen by SNI:
`<connection_id>.db.aurora.internal` gets a leaf from the gateway CA, anything else the listener's
own certificate. (Direct-TLS ALPN `postgresql` is not supported; clients use `SSLRequest`.)

**Login.** User = connection id (a UUID), password = the agent's gateway credential, requested as
cleartext password inside TLS; `database` and other startup parameters are ignored. Order: resolve
the credential (unknown → 28P01; frozen → 28000 `frozen`; store down → 57P03), BYO posture and
source address (`byo_gate`, as HTTP), then admission of the connection by id: it must belong to the
credential's organization and workspace (or the org), be `active`, belong to an app with a
`database: { engine: postgres }` section that the org's app posture allows, and be **selected for
the agent by an allow rule** (grant). Unknown id and missing grant give the same answer (42501).
Only then are the stored credentials opened (`host`, `port`, `dbname`, `user`, `password`,
optional `sslmode` (default `require`), `ca_pem`) and the gateway opens **its own** server
session through the guarded dial (the platform database is refused by address and port, SEC-06):
`sslmode` `disable`, `allow`/`prefer` (TLS first, plaintext only if the server answers `N`),
`require` (TLS; verifies the chain when `ca_pem` is stored, as libpq), `verify-ca`, `verify-full`;
authentication with cleartext, MD5 or SCRAM-SHA-256. The server's outcome is relayed as a FATAL
error (28P01 / 08006 / 08001); the exchange itself never is. The client then gets
`AuthenticationOk`, the server's `ParameterStatus` messages, a **gateway-generated**
`BackendKeyData` and `ReadyForQuery`. Live sessions are registered with the tunnel registry, so a
freeze, a revoked credential or a tightened BYO posture ends them (FATAL 57P01).

**Classification** (`aurora-sqlclass::classify`, off the async runtime through a bounded pool,
1 MiB cap, parse tree walked completely). Classes, strictest wins for multi-statement strings:
`transaction` (BEGIN/COMMIT/ROLLBACK/SAVEPOINT, `SET TRANSACTION`, `DEALLOCATE`, `CLOSE`, `FETCH`,
`PREPARE` follows its body, safe session settings `extra_float_digits`, `application_name`,
`timezone`, `datestyle`, `intervalstyle`, `bytea_output`, `client_min_messages`; never policy
checked), `read` (SELECT without locking clause, data-modifying CTE or INTO; SHOW; `EXPLAIN`
without `ANALYZE`; `COPY … TO STDOUT`), `write` (INSERT/UPDATE/DELETE/MERGE, `COPY FROM STDIN`,
`SELECT … FOR UPDATE/SHARE`), `ddl` (CREATE/ALTER/DROP/TRUNCATE/COMMENT/GRANT/REVOKE/RENAME,
CREATE TABLE AS, SELECT INTO, REFRESH MATERIALIZED VIEW), `admin` (everything else: VACUUM,
ANALYZE, other `SET`/`RESET`, LISTEN/NOTIFY, DO, CALL, EXECUTE, `EXPLAIN ANALYZE`, any function
not in the pure-function allow-list (unqualified or `pg_catalog`)). **Always refused** (SQLSTATE
42501, never sent): `ALTER SYSTEM`, `SET ROLE`/`RESET ROLE`/`SET SESSION AUTHORIZATION`, `COPY …
PROGRAM` and `COPY` to or from a server-side file, `LOAD`, `lo_import`/`lo_export`/`lo_from_bytea`/
`lo_put`, `dblink*`, `postgres_fdw*`, `pg_read_*`, `pg_ls_*`, `pg_stat_file`, `pg_file_*`,
`pg_reload_conf`, `pg_rotate_logfile`, `pg_promote`, `set_config`, `SET` of
`standard_conforming_strings`/`backslash_quote`/`escape_string_warning`/`client_encoding` (they
would make the server read the text differently from the classifier), foreign-data-wrapper, server
and user-mapping DDL, `CREATE EXTENSION` of `dblink`/`postgres_fdw`/`file_fdw`/`adminpack`,
fast-path function calls (`F`), anything unparsable (including statements the parser rejects
because of nesting depth) and anything over 1 MiB. The gateway always starts its server session
with `client_encoding=UTF8`.

**Policy.** A statement of class *c* (not `transaction`, not an empty query) is the synthetic
request `POST https://<connection_id>.db.aurora.internal/<c>` evaluated by the same policy engine
(`decide.rs`): the connection is the winning credential, the app's tool whose host/path match
(`*.db.aurora.internal`, `/read`, `/write`, `/ddl`, `/admin`) is the tool, the statement text is the
body for rule conditions. Verdict handling is the HTTP one: allowed, needs approval (the card shows
the statement redacted lexically of literals and comments and clamped to 2,000 characters; the
agent's connection simply waits), blocked or rate limited. A refusal is an `ErrorResponse`
(severity ERROR, SQLSTATE `42501`, the guidance message and `manage_url` text); policy or approval
stores unavailable give `57P03`. Every decided statement writes a `request_log` row with
`lane = db`, `method = POST`, the synthetic host and path, `connection_id`, `tool_id` and rule.

**Protocol state.** Simple `Query`: classified and judged before it is forwarded. Extended
protocol: `Parse` classifies (an always-refused or unparsable text is refused at once; a text that
is merely not allowed by policy parses fine) and records the statement; `Bind` records the portal
with the statement it was bound to; **`Execute` is where policy is applied**, once per portal (a
suspended portal's repeated `Execute` is not re-judged), so each execution is one decided request.
Name reuse the server would reject (`Parse` of an existing named statement, `Bind` to an existing
named portal) is rejected by the gateway with the server's SQLSTATEs (42P05, 42P03), so the table
the gateway judges by cannot diverge from what the server will run; `Bind`/`Execute` of unknown
names are rejected (26000, 34000); portals are forgotten when the server is idle after a `Sync`.
A refusal inside a pipeline follows PostgreSQL's error protocol: the gateway first has the server
finish what was already sent (a synthetic `Sync`, whose `ReadyForQuery` it holds back), writes its
`ErrorResponse` in order, discards messages up to the client's `Sync` and answers that with the
status; if the server was inside a transaction it is aborted with a failing statement whose
response is swallowed, so the server really is in the aborted state (`25P02`, `COMMIT` rolls back).
Client messages above 16 MiB end the session; `Terminate` ends it; `COPY` data messages are
relayed without inspection (the statement that started the copy was judged).

**Cancel.** `BackendKeyData` carries a random gateway `(pid, key)`; the server's pair is kept in a
registry (removed with the session). A `CancelRequest` (plain, or inside TLS) for a live gateway
pair makes the gateway send the real `CancelRequest` over a fresh guarded connection; unknown pairs
get no answer.

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
   (Phase 2, D-74) The host is also looked up in the **registered app
   instances** visible to the org (instance-wide and the org's own, `09 §3.4
   Instance-bound hosts`), exact host match only: a hit yields the app's
   `{instance}` host entries with `{instance}` = the request host, and only
   connections whose `instance_id` is that instance are candidates. The
   instance list is cached per org and dropped by the `org` invalidation scope
   (instance-wide changes invalidate every org).
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
   via the fail-closed classifier, git operation; (phase 4, P4-GRANTS, Q8) a tool's
   `params` conditions evaluated on the query, the urlencoded body prefix of step 4 or
   `x-amz-target`, fail closed: an absent, repeated, conflicting, undecodable or truncated
   parameter means the tool does not match, `09 §3.4 tools`). Unmatched requests to an app
   host have `tool = None`. (phase 4, P4-GRANTS) The step-4 body prefix is also buffered when a
   `form`-`params` tool may match, and the app's `grants` flags (`custom_only`,
   `block_untooled`) are read from the snapshot for step 7 (`07 §Grants and their compilation`).
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
   - (phase 4, P4-LLM, SEC-65) a winning LLM secret of a provider with an
     `inference_paths` set (`08 §Phase 4 providers`): the method and path (and, for
     `vertex`, the project and location in it) must be in the set, else 403
     `llm_path_not_permitted`; for `bedrock` with `aws_sigv4` a request body above
     `LLM_SIGN_BODY_MAX` (8 MiB) → 413 `llm_request_too_large`. Both are decided before
     policy and before any credential is opened (SEC-03).
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
    headers/params are overwritten, never merged. (phase 4, P4-LLM) LLM recipes add
    `header` (Azure `api-key`), `aws_sigv4` (all agent `Authorization` and `X-Amz-*`
    headers removed, then the final request signed; Bedrock) and
    `gcp_service_account_token` (a gateway-minted access token from the in-memory
    token cache, single-flight per secret version, no database lock; Vertex); the
    mint or the signing key is never sent anywhere but the provider's token endpoint
    and signing happens in the gateway only (`08 §Phase 4 providers`, SEC-64).
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
2. `INSERT INTO approvals (…, status='pending', deadline=now()+<effective timeout>,
   gateway_instance_id)`; `NOTIFY aurora_approval created`. (phase 2, RFE-011a) The
   effective timeout comes from the matching rules (`07 §Approval deadlines`,
   default `APPROVAL_TIMEOUT`) and is part of the cached rule data, so the hold
   needs no extra lookup. Failure to insert →
   503 `approval_store_unavailable` (fail closed).
3. Wait on the instance's LISTEN connection for `decided` of that id, with a
   2 s poll fallback reading the row. Keep the client connection open; do not
   read further body bytes.
4. `approved` → continue at step 9. `denied` → 403 `manual_approval_denied`
   (`details.approval_id`). Deadline reached → the gateway sets
   `status='expired'` (`WHERE status='pending'`; phase 2: and `resolved_at`) and answers 403
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
| `frozen` | 403 | (phase 2, RFE-006) the agent, its workspace or its org is frozen | `level` |
| `not_a_proxy_request` | 400 | origin-form request on 4102 | — |
| `open_lane_not_permitted` | 403 | (phase 1, RFE-003) open lane requested by a credential without the `browser` capability | — |
| `open_lane_disabled` | 403 | (phase 1, RFE-003) effective open-lane posture is `disabled` | `manage_url` |
| `destination_refused` | 403 | destination guard | `host` |
| `method_override_refused` | 400 | step 3 | — |
| `ambiguous_path` | 400 | step 3 | — |
| `blocked_by_policy` | 403 | rule block (open lane, phase 1: `connect_time: true`; an approval-requiring rule matched at CONNECT time adds `reason: "approval_required"`) | `rule_name`, `method`, `path`, `manage_url` |
| `blocked_by_default_policy` | 403 | default-deny | `manage_url` |
| `blocked_by_egress_policy` | 403 | uncredentialed default (`credentialed_only`) | `host`, `manage_url` (org egress allow-list) |
| `llm_provider_not_permitted` | 403 | governed LLM host not permitted for the org | `host` |
| `llm_key_required` | 403 | `sanctioned_only` and no Aurora-held key for the endpoint | `endpoint_id`, `manage_url` |
| `llm_key_not_permitted` | 403 | key scope not permitted for the endpoint | `secret_id` |
| `llm_key_wrong_organization` | 403 | key belongs to an account the rules do not allow | `secret_id`, `manage_url` |
| `llm_key_unverifiable` | 403 | the key's account cannot be determined | `secret_id` |
| `llm_model_not_allowed` | 403 | model not on the allow-list (or body unreadable) | `model`, `allowed_models` |
| `llm_origin_check_unavailable` | 503 | provider unreachable during origin preflight | `endpoint_id` |
| `llm_path_not_permitted` | 403 | (phase 4, P4-LLM) path outside the provider's inference set, or another Vertex project/location | `provider`, `endpoint_id` |
| `llm_request_too_large` | 413 | (phase 4, P4-LLM) Bedrock request body above `LLM_SIGN_BODY_MAX` that must be buffered to be signed | `limit_bytes` |
| `rate_limited` | 429 | rate-limit modifier | `limit`, `per` |
| `manual_approval_denied` / `manual_approval_expired` | 403 | holds | `approval_id` |
| `gateway_restarting` | 503 | shutdown during hold | — |
| `approval_store_unavailable` | 503 | cannot persist hold | — |
| `gateway_degraded` | 503 | (phase 3, RFE-014) database unreachable and the identity, host or credential value is not in cache (fail closed for new state); not for an invalid token | `reason` (`database_unavailable` \| `credential_store_unavailable`), `retry-after: 5` |
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
| `host_mismatch` | 421 | tenant-bound (or, phase 2 D-74, instance-bound) credential on wrong host | `expected_host` |
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
| Credential → identity | token hash | 60 s | `credential`, `agent`, `workspace`, `org`, `all`; `aurora_auth_revoked` (phase 2, RFE-025: the identity carries the effective BYO posture, source list and require-context flag; the instance switch comes from configuration) |
| ResolvedHost | `org/ws/credential/host` | 60 s | `org`, `workspace`, `agent`, `connection`, `all` |
| Plaintext values | `(SecretRef, version)` | 60 s | `connection`, `workspace`, `org`, `all` (zeroized on eviction) |
| Effective governance (`08 §11`) | org | 60 s | `org`, `all` |
| LLM key origin verifications | `(secret_id, sha256(plaintext))` | `LLM_ORIGIN_RECHECK` | `org`, `workspace`, `all` |
| Leaf certificates | host | until near expiry | never |
| Registry snapshot | content hash | until replaced | `aurora_registry` (+ 30 s poll of the active hash) |

Phase 0–2: on LISTEN connection loss the gateway flushes all caches except
leaf certificates, then reconnects. **From phase 3 (RFE-014) the order is
reversed — reconnect first, flush after** (see *Operation during a Postgres
outage*): the flush no longer destroys the only state that could carry the data
plane through a database blip.

## Operation during a Postgres outage (phase 3, RFE-014, D-61)

Phase 0–2 left this unspecified, and flushing every cache on LISTEN loss turned a
database blip into a full data-plane outage. The decision (fail closed for *new*
state, no cliff for *existing* state) is now explicit, with its availability
consequence stated: **an outage of Postgres degrades the gateway to what its
caches already know, for at most `GATEWAY_RESOLVE_CACHE_TTL`, and refuses
everything else.**

**States.** The gateway tracks one of three states, exposed by `/readyz`,
`/internal/v1/status` and the `aurora_gateway_state` gauge:

| State | Condition | Behaviour |
|---|---|---|
| `ready` | database reachable, LISTEN connection established, snapshot loaded | normal |
| `degraded` | snapshot loaded, but the database is unreachable **or** the LISTEN connection is down | serve from cache, refuse new state (below) |
| `down` | no snapshot loaded (e.g. started while the database was unreachable), or draining (SIGTERM) | refuses new `CONNECT`s: 503 `gateway_degraded` / `gateway_restarting` |

*Unreachable* = a readiness probe (`SELECT 1`, every `GATEWAY_DB_PROBE_INTERVAL` = 5 s,
timeout 2 s) failed twice in a row, **or** a hot-path query failed with a
connection-class error (pool acquire timeout 2 s, connection reset, `57P01`/`08xxx`
SQLSTATE) — whichever comes first. Recovery needs one successful probe **and** an
established LISTEN subscription. A statement-level error (constraint, permission,
query bug) is not an outage and does not change the state.

**(a) Serving during an outage.** Nothing is extended: no cache entry is served
past its TTL, and nothing is refreshed.
- **Served** while their cache entry is within TTL: the credential → identity entry
  and the ResolvedHost entry for `(credential, host)` (so existing agents keep
  working for the rest of their TTL), the plaintext cache (60 s), the effective
  governance and posture, the registry snapshot (kept in memory indefinitely),
  all open tunnels, WebSockets and in-flight requests (they need no database),
  and in-memory rate limits.
- **Refused** (fail closed, 503 `gateway_degraded`, `retry-after: 5`,
  `details.reason = "database_unavailable"`, `x-should-retry: true`):
  a credential not in the identity cache (it is **not** answered 407 — the gateway
  cannot tell an invalid token from an unreadable store, and a client must not
  conclude that its token is bad); a host with no ResolvedHost entry; a request
  that needs a **new approval hold** (cannot persist it) →
  `approval_store_unavailable`; a credential that needs refresh or a plaintext
  not in the plaintext cache (inline-store values and OAuth refresh need Postgres)
  → 503 `gateway_degraded` (`details.reason = "credential_store_unavailable"`);
  context credentials minted by the API after the outage began (not yet cached).
- Uncredentialed traffic whose identity and host entries are cached is served by
  policy as usual; traffic that would have required a lookup is refused, so
  **default-deny is never weakened** by degradation.
- **Existing holds** keep waiting in memory until their deadline; a decision
  cannot arrive while the database is down, so a hold whose deadline passes is
  answered `manual_approval_expired`, and its `approvals` row is updated on
  recovery (idempotent `UPDATE … WHERE status = 'pending'`, retried until it
  succeeds). The hold's fallback poll failing is **never** read as a denial or an
  approval.
- **Request log.** Rows are buffered in memory during the outage, up to
  `REQUEST_LOG_OUTAGE_BUFFER` (10 000 rows); the oldest are dropped beyond that
  (`aurora_gateway_request_log_dropped_total`) and the buffer is flushed on
  recovery. (`last_used_at` writes are skipped.) Never blocks requests.
- **Revocation lag.** NOTIFY cannot arrive while the database or the LISTEN
  connection is down, so a revocation, freeze, posture change or policy publish
  takes effect at the latest when the affected entries' TTL runs out. This is the
  same bound as phase 0's listener-loss window and is the price of availability;
  operators who prefer less exposure lower `gateway.resolve_cache_ttl_secs`
  (shorter outage tolerance), those who prefer more raise it (longer revocation
  lag). Raising it above 300 s makes the gateway log a warning at start.

**(b) LISTEN loss: reconnect first, flush after.** On loss of the LISTEN
connection the gateway (1) sets `listener_connected = false` (state `degraded`
only if the database is also unreachable or the listener has been down more than
`GATEWAY_LISTEN_GRACE` = 30 s; a brief reconnect does not flap readiness),
(2) **keeps all caches** and keeps serving under the rules above, (3) reconnects
with jittered exponential backoff (0.5 s → 10 s), (4) after the new connection
has re-issued `LISTEN` on every channel — so any invalidation from then on is
received — **flushes** every cache except leaf certificates (invalidations sent
during the gap were missed; the flush covers them) and re-polls the active
registry hash, then (5) returns to `ready`. Entries are repopulated on demand
(each miss is a database lookup, so a flush right after recovery is bounded by
the pool; misses are coalesced per key). If the reconnect itself keeps failing the
state stays `degraded` and entries age out at their TTL: at no time is anything
served beyond TTL. A LISTEN connection that stops delivering without closing is
detected by a heartbeat (phase 3, RFE-014, D-108): every `GATEWAY_DB_PROBE_INTERVAL`
the gateway NOTIFYs an instance-specific heartbeat through the pool and expects it back
on the LISTEN connection; two in a row missed make the listener down (`degraded` at
once) and trigger the same reconnect-then-flush, so a freeze or BYO tightening can
never stay unseen behind a dead listener.

**(c) Observability.**
- `/readyz` (internal listener, unauthenticated like the other health routes):
  `ready` → 200 `{ "state": "ready" }`; **`degraded` → 200**
  `{ "state": "degraded", "reason": "database_unavailable" | "listener_down",
  "since": ts, "cache_entries": n, "cache_oldest_age_secs": n }` — still 200 so
  orchestrators keep it in rotation (pulling every gateway out because the
  database is down would turn "degraded" into "down"); `down` → 503
  `{ "state": "down", "reason": "snapshot_not_loaded" | "draining" }`. Monitors
  that must alert on degraded call `GET /readyz?strict=true`, which answers 503
  for `degraded` as well.
- `/internal/v1/status` adds `state`, `db_unreachable_since`, `listener_connected`,
  `cache` sizes and `request_log_buffered`.
- Metrics: `aurora_gateway_state{state}` (1 for the current state),
  `aurora_gateway_db_unreachable_seconds` (0 when reachable),
  `aurora_gateway_degraded_requests_total{outcome="served_from_cache"|"refused", reason}`,
  `aurora_gateway_listener_reconnects_total`,
  `aurora_gateway_cache_flush_total{reason="listener_reconnect"|"invalidate_all"}`,
  `aurora_gateway_request_log_buffered`, `aurora_gateway_request_log_dropped_total`.
  A transition logs one structured line (`gateway.state_change`, from, to,
  reason). Alert guidance: `15 §Observability`.

## Registry snapshot loading

At boot the gateway loads the active snapshot for the highest contract it
supports (`09 §Registry snapshot`), validates that every mechanism, decoder and
finalizer (phase 4, Q8: and tool classifier, `params`) it names is known, builds the host index and tool matchers, and swaps
the structures atomically (`ArcSwap`). A snapshot naming an unknown mechanism is
refused (logged, metric `gateway_snapshot_rejected_total`) and the previous one
kept. `readyz` is false until a snapshot is loaded (an empty snapshot is valid).

## Rate limiting

Counters keyed by `(rule logical_id, agent_id, window start)` in memory per
replica (fixed windows: minute/hour/day aligned to UTC). With N gateway
replicas the effective limit can reach N× the configured value; this is
documented in the dashboard next to rate-limit modifiers.

**Shared counters (phase 4, P4-CNT, D-119, SEC-62).** Everything above is the
**`memory` backend**, which stays the default (`ratelimit.counters.backend =
"memory"`, no new infrastructure, exact with one gateway replica). From phase 4
the counters sit behind the `CounterStore` trait of `15 §Shared rate-limit
counters` with two more backends, `postgres` and `redis`; with either, N replicas
share one count and the N× approximation is gone for the limiters that use it.
Phase 0–3 text above remains the description of the default; the rules below
apply only to what the trait changes:

- **What uses it.** Only the `rate_limit` modifier of policy rules (the sole
  counter the gateway keeps). A request that matches no rate-limited rule never
  touches the store, so the hot path of every other request is unchanged and the
  P3 performance budgets (`18 §Performance budgets`) still describe it.
- **Two steps per request, both through the store.** The evaluator
  (`aurora-policy`) stays pure and synchronous (`07 §Evaluation`): before it runs,
  the gateway reads the current counts of the rate-limited rules that match the
  request (one batched `fixed_window_peek`, only when the cached ResolvedHost has
  such a rule for this host), and passes them in. After the **final verdict is
  `allow` or `needs_approval`** (unchanged rule: a blocked request consumes
  nothing) it consumes one unit of each matched rule with one all-or-nothing
  `fixed_window_consume`. The peek is advisory; **the consume is the authority**:
  if another replica spent the last unit in between, the consume refuses and the
  request is answered `rate_limited` exactly as if the evaluator had said so (the
  request log row records it). So a shared limit is never exceeded, only
  (rarely) refused slightly earlier than a peek suggested. A held (approval)
  request consumes when the hold is created, as today, and is not charged again
  on approval.
- **Window clock.** With a shared backend, window starts and `retry_after_secs` are
  computed by the **store's** clock (Postgres `clock_timestamp()`, Redis `TIME`),
  not each replica's, so skewed replica clocks cannot split one window in two. The
  `memory` backend uses the local UTC clock as before.
- **Budget and breaker.** Each store call has a timeout (`COUNTER_TIMEOUT`, 50 ms
  default, `ratelimit.counters.timeout_ms`). `COUNTER_BREAKER_FAILURES` (3)
  consecutive failures or timeouts open a circuit breaker for
  `COUNTER_BREAKER_COOLDOWN` (5 s), then one probe call decides; an open breaker
  answers without calling the store.
- **When the backend is unreachable** (timeout, connection error, breaker open;
  also during a Postgres outage with the `postgres` backend, `§Operation during a
  Postgres outage`): the gateway applies the policy configured for the class
  `gateway` (`ratelimit.counters.on_unavailable.gateway`):
  - `local` (**default**): count in the replica's own in-memory fixed windows
    (today's behaviour, N× approximation) until the store answers again; the
    local state is separate, so counts spent in the store are not lost and local
    counts are not folded back (the extra over-allowance is bounded by the
    outage). Never unlimited.
  - `closed`: a request whose matched rules need a count the store cannot give is
    refused **503 `counter_unavailable`** (`retry-after: 5`, `details.limiter =
    "gateway_rule"`); requests that match no rate-limited rule are unaffected.
  There is **no fail-open value**: configuration naming one refuses startup. The
  rate-limit modifier is a quota on an *allowed* access, so degrading to the
  per-replica quota keeps the limit and keeps agents working through a cache
  outage; operators for whom a quota is a hard contractual cap choose `closed`.
  Neither value ever allows a request that a loaded policy refuses; a store
  failure is never read as "under the limit" and never as an approval or denial.
- **Failure is visible**: the gateway logs once per state change, the request log
  row of a request decided on the local fallback carries `reason =
  counter_fallback`, and the metrics of `15 §Shared rate-limit counters` show the
  state. `readyz` is unaffected (a counter fallback is not `degraded`: no
  authority is widened, `SEC-62`).
- **Keys** are `gw:rl:<rule logical_id>:<agent_id>:<period>` plus the window
  start the store derives. They contain only ids; no secret, URL or token is ever
  part of a key.

## Telemetry

- Request log: in-memory batch flushed every `REQUEST_LOG_FLUSH` via `COPY`;
  on flush failure the batch is retried once, then dropped with a metric (never
  blocks requests); (phase 3, RFE-014) during a database outage rows are
  buffered up to `REQUEST_LOG_OUTAGE_BUFFER` instead and flushed on recovery. Never records secret values, bodies or raw query strings.
- Metrics (Prometheus; phase 3, RFE-014 adds the state and degraded-mode series listed under *Operation during a Postgres outage*): requests by lane/decision/status, upstream latency
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
The only per-replica approximation is rate limiting (above). (phase 4, P4-CNT)
With `ratelimit.counters.backend = postgres` or `redis` it disappears; the
gateway then holds one more piece of external state (the counters), which is not a
system of record: losing it resets windows and never changes any decision other
than a quota count.
