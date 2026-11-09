# HashiCorp Vault secret store

Phase 3. Aurora can keep secret values in a HashiCorp Vault **KV version 2** mount instead of
(or next to) the database. Design: [`docs/21 §Phase 3 — HashiCorp Vault`](../21-later-phases.md),
[`docs/08 §2`](../08-secrets-credentials.md), contracts C-370..C-375, invariant SEC-61. The
configuration keys are the `secrets.stores[]` rows of
[`docs/15`](../15-deployment-operations.md#configuration-reference-keys-and-defaults).

Two things can live in Vault:

| Mode | What | Who writes it |
|------|------|---------------|
| **Managed** | Aurora's own secret values, connection credentials, OAuth client secrets, integration, presence and webhook credentials. Each lives at `<kv_mount>/data/<managed_prefix>/<org_id>/<purpose>/<owner_id>`; a row holds the ref `ext:<store>:<path>@<version>`. Moving them is [`secrets migrate`](#migrating-values). | Aurora |
| **Reference** | A secret that already exists in Vault (a team's API key, say). The Aurora secret holds only a pointer, `ext:<store>:<path>#<field>@latest`, and always reads the **latest** version, so rotation in Vault is picked up within the gateway's 60 s plaintext cache. Reference values are never migrated or deleted by Aurora. | You, in Vault |

Both services (api and gateway) build the store from their own configuration, so each can use
its own Vault role.

## 1. Vault side

### KV mount

Use a KV **v2** mount (the dev server and `vault secrets enable -path=secret kv-v2` both give
`secret/`). Set `max_versions` to **at least 10** on it:

```sh
vault secrets enable -path=secret -version=2 kv          # skip if it exists
vault write secret/config max_versions=20                 # >= 10
```

Why: every managed ref pins the KV version it was written at, and Vault silently prunes the
oldest versions beyond `max_versions`. A path rewritten many times in a short window (a busy
OAuth refresh, concurrent writers) must not push a still-referenced version out. Aurora
destroys superseded versions itself, so the mount does not grow.

### Policies

Two roles, two policies. Replace `secret` with your `kv_mount`, `aurora` with your
`managed_prefix`, and the reference areas with the paths you will put in `reference_scopes`.
`+` matches one path segment.

API (writes and destroys managed values of every purpose, reads reference paths):

```hcl
path "secret/data/aurora/*"        { capabilities = ["create", "update", "read"] }
path "secret/destroy/aurora/*"     { capabilities = ["update"] }
path "secret/data/teams/*"         { capabilities = ["read"] }   # reference areas
```

Gateway (reads managed and reference paths; writes only the connection credentials it refreshes):

```hcl
path "secret/data/aurora/*"                              { capabilities = ["read"] }
path "secret/data/aurora/+/connection.credentials/+"     { capabilities = ["create", "update", "read"] }
path "secret/destroy/aurora/+/connection.credentials/+"  { capabilities = ["update"] }
path "secret/data/teams/*"                               { capabilities = ["read"] }
```

Both roles use the built-in `default` policy for `auth/token/renew-self` and
`auth/token/lookup-self`. Neither needs `list`, `metadata` or `sys` access. The health probe
uses `sys/health`, which is unauthenticated.

Write the policies with `vault policy write aurora-api api.hcl` and `vault policy write
aurora-gateway gateway.hcl`, then attach them to the roles below.

### Authentication

Aurora supports `kubernetes` and `approle` for production; `token` is for a development server
only (startup logs a warning, and the chart does not render it).

**kubernetes** (recommended on Kubernetes). The pod presents a ServiceAccount JWT. With the
Helm chart it is a **projected, audience-bound token**, not the API token, and each service has
its own ServiceAccount (`<release>-api`, `<release>-gateway`):

```sh
vault auth enable kubernetes
vault write auth/kubernetes/config kubernetes_host="https://<api-server>:443"
vault write auth/kubernetes/role/aurora-api \
    bound_service_account_names=aurora-api \
    bound_service_account_namespaces=aurora \
    audience=vault token_policies=aurora-api token_ttl=1h
vault write auth/kubernetes/role/aurora-gateway \
    bound_service_account_names=aurora-gateway \
    bound_service_account_namespaces=aurora \
    audience=vault token_policies=aurora-gateway token_ttl=1h
```

`audience` must equal `vault.auth.audience` (default `vault`). Use your release's
ServiceAccount names (`kubectl -n aurora get sa`; the chart names them `<fullname>-api` and
`<fullname>-gateway`).

**approle**:

```sh
vault auth enable approle
vault write auth/approle/role/aurora-api     token_policies=aurora-api     token_ttl=1h
vault write auth/approle/role/aurora-gateway token_policies=aurora-gateway token_ttl=1h
vault read  auth/approle/role/aurora-api/role-id
vault write -f auth/approle/role/aurora-api/secret-id
```

The role id and secret id reach each service only through environment variables named in the
configuration (`role_id_env`, `secret_id_env`). Never put them in the configuration file or in
Helm values.

### TLS

Aurora **always verifies** the Vault certificate and never follows redirects. `http://` is
accepted only for a loopback address (a dev server). To trust a private CA, point
`tls.extra_ca_bundle` at a PEM file (it also applies to every other outbound connection) or
`secrets.stores[].ca_file` to add trust anchors for this store only. Neither replaces the
system roots by accident: `ca_file` replaces them for the store, `tls.extra_ca_bundle` adds.

## 2. Aurora configuration

```toml
[secrets]
default_store = "inline"        # change to "vault" only after migrating (below)

[[secrets.stores]]
id = "vault"                     # no ':'; appears inside stored refs
kind = "hashicorp-vault"
address = "https://vault.example.org:8200"
kv_mount = "secret"              # default
managed_prefix = "aurora"        # default
[secrets.stores.auth]
method = "kubernetes"
role = "aurora-api"              # the gateway's own file says "aurora-gateway"
jwt_path = "/var/run/secrets/aurora-vault/token"

[[secrets.stores.reference_scopes]]
org_id = "0197a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b"   # the org's id, never its slug
paths = ["teams/acme/*", "shared/api-key"]
```

### Helm

```yaml
vault:
  enabled: true
  address: https://vault.example.org:8200
  referenceScopes:
    - orgId: 0197a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b
      paths: ["teams/acme/*", "shared/api-key"]
  egress: {cidrs: ["10.30.0.0/24"], port: 8200}   # only for an in-cluster or private Vault
extraCaBundle:
  existingConfigMap: org-ca       # or existingSecret; PEM under `key` (default ca-bundle.pem)
```

The chart renders the store into the api config and into a separate gateway config
(`aurora-gateway.toml`) so each service gets its own role (`vault.auth.apiRole`, `gatewayRole`),
creates the two ServiceAccounts, projects the audience-bound token at
`/var/run/secrets/aurora-vault/token` (`vault.auth.audience`, `tokenExpirationSeconds` ≥ 600),
mounts `extraCaBundle` read-only in every workload and sets `tls.extra_ca_bundle`.
`vault.makeDefault: true` sets `secrets.default_store`. For AppRole set `vault.auth.method:
approle` and `vault.auth.approle.existingSecret` (keys `API_ROLE_ID`, `API_SECRET_ID`,
`GATEWAY_ROLE_ID`, `GATEWAY_SECRET_ID`). The NetworkPolicies deny private ranges: list a private
Vault's CIDR in `vault.egress.cidrs`. See `deploy/helm/cases/` for complete examples; check them
with `bash deploy/helm/check-vault.sh` (it runs `helm lint`, `helm template`, asserts the
rendered output and validates the rendered configs with the real `aurora-api` loader).

### Reference scopes and org ids

`reference_scopes` says which **organization** may point at which Vault paths (SEC-61). It is
keyed by the organization's immutable **id** (a UUID), never its slug: a slug can be renamed,
freed and taken by another org, which must never inherit a scope. A non-UUID refuses startup;
an id that matches no org grants nothing. Find an org's id with:

```sh
# instance administrator
curl -fsS -H "Authorization: Bearer $TOKEN" https://<host>/v1/admin/orgs      # id, slug, name
# a member of the org
curl -fsS -H "Cookie: $SESSION" "https://<host>/v1/resolve?org=<slug>"        # org.id
```

Paths are relative to `kv_mount`: an exact path or a prefix ending in one `*`. A glob that
equals, lies inside or covers `<managed_prefix>/` refuses startup (no reference can reach
another org's managed values). The scope is checked at creation **and at every open**, so
removing a scope from the configuration stops existing references at their next read.

## 3. When Vault is unreachable

- **Startup.** The store is built without contacting Vault and the services start normally.
  Nothing crash-loops on a Vault outage; only a *configuration* error (bad address, unreadable
  CA file, missing environment variable for AppRole, covering glob, non-UUID org id) refuses
  startup.
- **During the outage.** Every operation that needs Vault fails closed with `StoreUnavailable`:
  the API answers 503 `secret_store_unavailable`, the gateway answers 503
  `secret_store_unavailable` with `details.store_id`. Values already in the gateway's 60 s
  plaintext cache keep working until the entry expires. Inline values are unaffected.
- **Recovery.** The next operation logs in again; no restart.
- A wrong, expired or revoked credential looks the same as an outage (503); the log line
  `vault auth call failed` carries the HTTP status, never a token or value.

## 4. Migrating values

`aurora-api secrets migrate --to <store_id> [--purpose <p>] [--dry-run] [--discard-grace-secs N]`
moves **managed** values between stores, in either direction (`--to vault`, `--to inline`).
Reference secrets (`value_source = reference`) are never touched. It reads the same
configuration and environment as the API and runs where the API runs:

```sh
kubectl -n aurora exec deploy/aurora-api -- /usr/local/bin/aurora-api secrets migrate --to vault --dry-run
```

Per row: open the old value, write the new one, then in one transaction `UPDATE … SET ref = new
WHERE id = … AND ref = old` (a compare-and-swap), commit, and only then discard the old
value. If the row changed meanwhile the swap affects nothing, the new value is discarded and
the row counts as **skipped**; it is never overwritten. A failure before the commit leaves the
old ref in place and valid and removes the unused new value. Every row is opened under its
own context (purpose, owner, organization); a mismatch fails that row, not the run. A crash
can at worst leave an orphaned value in the store, never a dangling ref. Values never appear
in the log, the output or an error.

| Flag | Meaning |
|------|---------|
| `--to` | target store id (`inline` or a configured Vault store); must support managed values |
| `--purpose` | only one of `secret.value`, `connection.credentials`, `oauth_client.secret`, `integration.credentials`, `presence.credentials`, `webhook.verification`, `channel_op.deprovision`, `connector.settings` |
| `--dry-run` | count what would move; reads and writes nothing, not even to the store |
| `--discard-grace-secs` | wait (default 2) after a purpose's commits before discarding old values, so gateway caches (which reload on commit) never hold a ref whose value is gone |

Output, one line per purpose: `moved`, `skipped` (lost the swap), `failed` (kept their old
ref; see the log: the row id and the error kind), `already in target`. The exit status is
non-zero when any row failed. A second run moves nothing; a failed run can simply be run
again. Gateways receive an invalidation on every commit. Do not run two migrations at once
(it is safe for inline → Vault: the loser skips and cleans up; going the other way the loser
may fail rows whose old value the winner already discarded).

### Procedure: inline → Vault

1. Set up Vault ([§1](#1-vault-side)) and add the store to the configuration with
   `default_store = "inline"`; roll out and check there is no startup error.
2. `secrets migrate --to vault --dry-run` and compare the counts with what you expect.
3. `secrets migrate --to vault`. Repeat until every line shows `failed 0` (rerun after fixing
   the cause).
4. Set `secrets.default_store = "vault"` (Helm: `vault.makeDefault: true`) and roll out, so new
   values go to Vault.
5. Check: exercise a connection through the gateway and read a secret in the dashboard.

### Rollback: Vault → inline

1. Set `secrets.default_store = "inline"` and roll out (new values go to the database again).
2. `secrets migrate --to inline [--dry-run]` until `failed 0`. Old Vault versions are destroyed
   after each commit.
3. Verify nothing refers to the store any more (all counts zero in the next `--dry-run`, or):

   ```sql
   SELECT 'secrets' t, count(*) FROM secrets WHERE value_ref LIKE 'ext:vault:%' AND value_source = 'managed'
   UNION ALL SELECT 'app_connections', count(*) FROM app_connections WHERE credentials_ref LIKE 'ext:vault:%'
   UNION ALL SELECT 'app_oauth_clients', count(*) FROM app_oauth_clients WHERE secret_ref LIKE 'ext:vault:%'
   UNION ALL SELECT 'channel_integrations', count(*) FROM channel_integrations WHERE credentials_ref LIKE 'ext:vault:%'
   UNION ALL SELECT 'channel_presences', count(*) FROM channel_presences WHERE credentials_ref LIKE 'ext:vault:%'
   UNION ALL SELECT 'webhooks', count(*) FROM webhooks WHERE verification->>'secret_ref' LIKE 'ext:vault:%';
   ```

   (`vault` is your store id.) Reference secrets pointing at Vault (`…@latest`) remain until
   their owners delete them.
4. Only then remove the store from the configuration: a ref to an unconfigured store cannot be
   read.

The managed area in Vault can be removed afterwards (`vault kv metadata delete` under
`<managed_prefix>/`), once the counts are zero and you have no use for the data.

`aurora-api secrets rekey` is the same engine targeting `inline` with the active encryption
key: it re-seals the inline values that are not under it (key rotation, see
[key-rotation.md](key-rotation.md#rotating-the-inline-encryption-keyring-aurora-api-secrets-rekey)).

## 5. Checking it works

- `GET /v1/secret-stores` lists the store as `{ id, kind: "hashicorp-vault", capabilities: {
  managed: true, references: true } }`.
- Create a secret with `reference: { store_id: "vault", path: "teams/acme/key" }` as a member of
  the scoped org: it succeeds. Anything outside the scope, under the managed prefix, missing,
  or with a wrong field answers one indistinguishable 422 `reference_not_permitted`.
- Rotate the value in Vault; within a minute the gateway injects the new one.
- Stop Vault: operations return 503 `secret_store_unavailable`, the pods stay up; start it and
  they recover without a restart.
