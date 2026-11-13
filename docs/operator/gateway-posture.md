# Gateway: TLS proxy listener, open lane, BYO agents and the database lane

(Phase 1 RFE-002 and RFE-003, phase 2 RFE-025 and P2-DBL.) This page covers the settings that decide who
can reach the gateway proxy port (4102) and what they may do there. The normative text is in
[docs/06](../06-gateway.md) and [docs/08 §11.9](../08-secrets-credentials.md#119-byo-posture-and-source-restriction-phase-2-rfe-025-d-59);
the key table is [docs/15 §Configuration reference](../15-deployment-operations.md#configuration-reference-keys-and-defaults).

In Helm, put the keys under `config:` in your values file (for example `config.byo.minimum_posture`), except where a
section below names a dedicated chart value.

## TLS proxy listener

The proxy listener is **TLS-only** for everyone: hosted sandboxes, in-cluster clients and external BYO agents.
There is no plaintext variant. A client that sends anything other than a TLS ClientHello is disconnected without an answer
(counted in `gateway_proxy_plaintext_refused_total`). By default the server certificate is a leaf issued by the
gateway CA (`gateway.ca_dir`, files `ca.pem` and `ca.key`), valid 30 days and renewed in memory without a restart. Every sandbox and every BYO
container config already trusts that CA.

| Key | Default | Meaning |
|-----|---------|---------|
| `gateway.advertised_proxy_url` | `https://<host of server.external_url>:4102` | What BYO agents are told to dial. Must be `https://`; startup refuses `http://`. Its host goes into the certificate. |
| `gateway.sandbox_proxy_url` | Kubernetes backend: `https://aurora-gateway:4102`; otherwise the advertised URL | What hosted sandboxes dial. `https://host[:port]` only (no path, query or credentials). Set it only when the in-cluster Service has another name. Never for BYO agents. |
| `gateway.proxy_tls.names` | `[]` | Extra DNS names or IPs for the CA-issued certificate, such as a load balancer hostname. |
| `gateway.proxy_tls.cert_file`, `gateway.proxy_tls.key_file` | unset | Your own certificate chain and key (PEM). Both or neither; they replace the CA-issued leaf. It must cover every name clients use. |

A client that connects by a name that is not in the certificate fails verification. Add the name; do not disable
verification. Exposing port 4102 outside the cluster needs a layer-4 (TCP, TLS passthrough) load balancer or a NodePort.
Do not terminate TLS in front of it unless the front end re-encrypts to the gateway.

Minimal example (plain TOML):

```toml
[gateway]
advertised_proxy_url = "https://proxy.aurora.example.org:4102"

[gateway.proxy_tls]
names = ["proxy.aurora.example.org"]
```

With the chart, set `gateway.proxy.service.type: LoadBalancer` and `gateway.proxy.tls.externalHost` (and `names`, or
`existingSecret` for your own certificate). The chart renders them into `gateway.advertised_proxy_url` and
`gateway.proxy_tls.*`. See [install-helm.md](install-helm.md#gateway-proxy-addresses).

Verify:

```bash
# the CA, as a signed-in user or key (GET /v1/gateway/ca.pem), then a TLS handshake that checks the name
curl -fsS -H "Authorization: Bearer $KEY" https://aurora.example.org/v1/gateway/ca.pem -o ca.pem
openssl s_client -connect proxy.aurora.example.org:4102 -servername proxy.aurora.example.org -CAfile ca.pem </dev/null | grep "Verify return code"
```

`Verify return code: 0 (ok)` means the name is covered. With your own certificate (`cert_file`) verify against the issuer of that certificate
instead. Clients such as curl and git need the CA for the proxy leg separately from the origin leg
(`--proxy-cacert`, `http.proxySSLCAInfo`); the dashboard's Connect tab shows the exact settings.

## Open lane

The open lane is a raw tunnel for browsers: the gateway sees only host and port, never injects a credential, and a rule that
would need approval refuses the `CONNECT`. A sandbox can use it only when its credential holds the **`browser` capability**
(the sandbox image declares it with the label `org.aurora.capabilities=browser`; BYO `aug_` and `auc_` credentials never hold it,
so they get 403 `open_lane_not_permitted`).

The **posture** is `enabled`, `allowlist_only` or `disabled`. The effective posture is the strictest of the instance, the organization and the workspace.

| Key | Default | Meaning |
|-----|---------|---------|
| `governance.egress.open_lane_minimum` | `enabled` | The instance floor. `disabled`: every open-lane `CONNECT` is refused (403 `open_lane_disabled`). `allowlist_only`: a tunnel is admitted only to hosts on the allow-list of every level that sets `allowlist_only`; at instance level that is `governance.egress.baseline_allowlist`, so set one. An entry with a `path` does not admit a tunnel. |
| `governance.egress.baseline_allowlist` | `[]` | `[{ host, path? }]`, merged into every organization's egress allow-list. `host` is a name or wildcard without scheme, port or path; `path` starts with `/`. |
| `governance.egress.minimum_posture` | `open` | `open` or `credentialed_only`. Under `credentialed_only` an uncredentialed request is allowed only to allow-listed hosts, and that applies to open-lane tunnels too. |

Organizations and workspaces can only tighten the posture: the workspace policy draft carries `open_lane_posture`
([docs/05 §Policy](../05-control-plane-api.md#policy)). A change takes effect for the next `CONNECT`; open tunnels end when idle or closed by the client.

```toml
[governance.egress]
open_lane_minimum = "allowlist_only"
baseline_allowlist = [ { host = "pypi.org" }, { host = "files.pythonhosted.org" } ]
```

Verify: `GET /v1/orgs/{org}/governance/effective` (any org member) shows `egress.open_lane` with its `source` (`instance` or `org`).
A workspace policy simulation with `lane: "open"` shows the decision for a given host (`POST /v1/workspaces/{ws}/policy/simulate`).
Details: [docs/06 §Open-lane admission and posture](../06-gateway.md#open-lane-admission-and-posture-phase-1-rfe-003).

## BYO agents: posture and source allow-lists

A BYO agent runs outside Aurora and dials the proxy with an `aug_` token. Hosted agents are not affected by this section (they are controlled by `hosted.enabled`).

| Key | Default | Meaning |
|-----|---------|---------|
| `byo.enabled` | `true` | Master switch. `false`: BYO creation is refused and every BYO credential is refused by the gateway (403 `byo_disabled`), whatever org and workspace say. Changed by restart. |
| `byo.minimum_posture` | `owners_only` | Floor for every org and workspace. `enabled`: any member who can use the workspace may create a BYO agent (the phase 0 behaviour). `owners_only`: only workspace owners and org owners/admins. `disabled`: nobody, and existing credentials are refused. |
| `byo.default_source_cidrs` | `[]` (unrestricted) | Default source CIDR list for every BYO agent that has no list of its own or of its organization. At most 32 entries; `/0` is refused. Orgs and agents can only narrow it. |
| `byo.require_context_credentials` | `false` | When `true`, a BYO agent must use context credentials (`auc_`) for any request that would inject a credential (403 `context_credential_required`). |
| `gateway.trusted_proxies` | `[]` | CIDRs of layer-4 load balancers or NAT hops that send a **PROXY protocol** v1/v2 header on the proxy port. `/0` is refused. |

Tightening to `disabled` never deletes tokens or agents; loosening restores service at once. The posture of a deployment that wants the old behaviour is
`byo.minimum_posture = "enabled"`.

**Source addresses and PROXY protocol.** The gateway judges the source list by the transport peer of the TLS connection, and only that. A load balancer that hides the client address
makes every list fail closed. Use a balancer that preserves the source address (Kubernetes `externalTrafficPolicy: Local`) or one that sends PROXY protocol,
and list its CIDR in `gateway.trusted_proxies`. A connection from a listed peer **must** begin with a PROXY header; a PROXY header from any other peer closes the
connection. `X-Forwarded-For` is never used on the proxy port. `gateway.trusted_proxies` is not `server.trusted_proxies`, which belongs to the API and the HTTP edge.
In the chart the value is `gateway.proxy.trustedProxies`.

```toml
[byo]
minimum_posture = "owners_only"
default_source_cidrs = ["192.0.2.0/24", "198.51.100.17/32"]

[gateway]
trusted_proxies = ["10.20.0.0/16"]      # the layer-4 load balancer in front of port 4102
```

Verify:

- `GET /v1/orgs/{org}/governance/effective` shows `byo.posture`, `byo.source_cidrs` and `byo.require_context_credentials` with their source.
- A BYO agent's detail shows `byo.effective_source_cidrs`.
- A blocked source gets 403 `source_not_permitted` with the address the gateway saw in `details.source`. If that is the balancer's address, the balancer is not preserving the source and PROXY protocol is not configured.
- The refusals are counted in `aurora_gateway_byo_refused_total{reason}`.

Details: [docs/06 §BYO admission](../06-gateway.md#byo-admission-phase-2-rfe-025), [docs/08 §11.9](../08-secrets-credentials.md#119-byo-posture-and-source-restriction-phase-2-rfe-025-d-59).

## Database lane

A database app (a manifest with `database: { engine: postgres }`, for example the built-in `postgresql` package) lets an agent run SQL against a database
**through the gateway**. The gateway holds the credentials, logs in to the target itself and judges every statement; the sandbox never sees a database password.

How it is reached and what to configure:

- **No extra listener and no new key.** The database protocol arrives on the same TLS proxy port (4102) and is recognised by its first bytes. A connection without TLS is refused before any password is requested.
  The certificate is the one described under [TLS proxy listener](#tls-proxy-listener); the gateway answers the name `<connection_id>.db.aurora.internal` with a leaf from its CA.
- **Inside a hosted sandbox** the supervisor writes a `pg_service.conf` (`PGSERVICEFILE`) and a loopback relay on `127.0.0.1:3130`, so the agent runs `psql service=<name> -c 'select 1'`
  with nothing else on the command line. The relay verifies the gateway certificate and supplies the credential; no credential is in the sandbox's files or environment. The sandbox image ships the `psql` client.
- **The target database** is registered as a connection of the app, with `host`, `port`, `dbname`, `user`, `password` and optionally `sslmode` (default `require`) and `ca_pem`. `sslmode` accepts `disable`, `allow`, `prefer`,
  `require`, `verify-ca` and `verify-full`. `require` verifies the server certificate: against `ca_pem` when you store one, otherwise against the system's trusted CAs (chain and host name), so a target with a private CA needs its CA certificate as `ca_pem` (use `verify-full` with `ca_pem` where the certificate names the host). If you truly cannot store the CA, the connect form's "Server certificate check" (offered under `require`) can be set to "Do not verify": the choice is audited, shown on the connection with a warning, and means the password goes to whichever server answers. The gateway refuses the platform's own database by address and port. A private target address is reachable only if you
  allow it with `gateway.allowed_private` (CIDRs, IPs or host names), because the gateway's dial goes through the destination guard.
- **Classes and policy.** Each statement is classified as `transaction`, `read`, `write`, `ddl` or `admin` (the strictest class of a multi-statement string wins) and judged as a request to
  `POST https://<connection_id>.db.aurora.internal/<class>` by the ordinary policy engine: allow, require approval, block or rate limit, per workspace policy and the agent's grant. `transaction` statements are never policy checked.
  A short list of dangerous statements is refused outright and never sent: `ALTER SYSTEM`, `SET ROLE`, `COPY … PROGRAM`, `COPY` to or from a server-side file, `LOAD`, large-object file functions, `dblink`, `postgres_fdw`, `pg_read_*`, `CREATE EXTENSION` of file or link extensions, and anything that does not parse or is over 1 MiB (the full list is in docs/06).
  Every decided statement writes a request-log row with `lane = db`. An approval card shows the statement with literals and comments removed.
- A freeze, a revoked credential or a tightened BYO posture ends live sessions.

```toml
[gateway]
allowed_private = ["10.30.0.0/16"]     # only if the database is on a private address
```

Verify: connect the app in the dashboard, grant it to an agent, then from the agent `psql service=<name> -c 'select 1'` (the instruction file names the service). A `select 1` is class `read`; check the request log for `lane = db`.
The spec is [docs/06 §Database lane](../06-gateway.md#database-lane-phase-2-p2-dbl-1-d-80), the relay is [docs/12 §Database relay](../12-supervisor-harness.md#database-relay-and-files-phase-2-p2-dbl-2-d-89), and the app section is [docs/09 §3.4](../09-app-plugins.md).
