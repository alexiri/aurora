# Plugins and connectors

Instance administrators control two kinds of extensions:

- **Plugins** (`.aurora-plugin` packages) add apps: connection flows, host claims,
  tools and approvals. Design: [docs/09](../09-app-plugins.md).
- **Connectors** bridge chat platforms to Aurora. A connector is a separate
  service with its own token and an approved manifest. Design: [docs/13 §3](../13-channels-connectors.md#3-connector-registration-manifest-and-trust).

All routes below are instance-administrator routes under `/v1/admin/`. Each
state change is audited.

## Plugins

### Sources

| Source | How packages arrive | Activation |
|--------|--------------------|------------|
| `builtin` | Shipped in the API image. Phase 0 ships none. | Automatic, unless permissions widened. |
| `directory` | Files in `plugins.dir` (default `/var/lib/aurora/plugins`), scanned at startup and every 60 s (`services/aurora-api/src/domains/plugins/scan.rs`). | Same rules as upload. |
| `upload` | `POST /v1/admin/plugins`, multipart with a field named `package` (the archive, at most 20 MiB). | Same rules as upload. |

Approval rules (docs/09 §10):

- **First install** and any **widening** need instance-admin approval. Widening means a new
  hook, egress host, context variable, host claim or zone.
- **Upgrades** with an identical or narrower permission set activate automatically. The
  previous version stays active until the new one is.

Package states: `pending_approval`, `active`, `superseded`, `rejected`, `disabled`, `invalid`.

### Review and approve

```bash
# Upload (multipart field "package")
curl -sS -X POST https://aurora.example.org/v1/admin/plugins -F package=@my-app-1.0.0.aurora-plugin
# List and inspect
curl -sS https://aurora.example.org/v1/admin/plugins
curl -sS https://aurora.example.org/v1/admin/plugins/<id>
# Decide
curl -sS -X POST https://aurora.example.org/v1/admin/plugins/<id>/approve -H 'Content-Type: application/json' -d '{"reason": "reviewed 2026-11-06"}'
```

Other actions use the same body (`{"reason": …}`, optional, at most 64 characters):
`POST /v1/admin/plugins/{id}/reject`, `/disable` and `/enable`. A disabled package leaves
the next registry snapshot. `DELETE /v1/admin/plugins/{id}` uninstalls it. It refuses with
409 while connections are live, unless you add `?force=true`, which marks those
connections `orphaned`.

Review the permissions, hooks, egress hosts and host claims on the package before you approve.
The registry snapshot the runtime uses is at `GET /v1/admin/registry/snapshot`.

### Trusted keys

A package must be signed by a key the instance trusts, unless `plugins.allow_unsigned` is set.

```bash
# Trust a publisher's public key (base64 ed25519)
curl -sS -X POST https://aurora.example.org/v1/admin/trusted-keys -H 'Content-Type: application/json' \
  -d '{"publisher": "example", "name": "release-2026", "public_key": "<base64 ed25519 public key>"}'
curl -sS https://aurora.example.org/v1/admin/trusted-keys
# Revoke: packages signed by this key become "disabled"; nothing is deleted
curl -sS -X DELETE https://aurora.example.org/v1/admin/trusted-keys/<id>
```

Revocation disables every package the key signed. Their connections stop injecting and show why.

`plugins.allow_unsigned` (default `false`) accepts unsigned packages. It is for development
only. The chart and compose do not set it.

### Host claims

When two packages claim overlapping hosts, an admin picks the winner for each conflicting claim:

- `GET /v1/admin/host-claims` lists conflicts.
- `POST /v1/admin/host-claims/{group_key}` with `{ "winner_package_id", "winner_app_id", "fingerprint" }`
  records the decision. Reversing a decision is allowed and audited.

The losing app's host entry never injects. A decision survives upgrades while the claim
fingerprint is unchanged. A changed claim needs a new decision (docs/09 §7).

Claims that overlap known LLM hosts are accepted, but the gateway never injects an app
connection on a governed LLM host.

### Plugin development (`aurora plugin`)

These commands are in `services/aurora-cli/src/plugin/mod.rs`. They do not approve anything.

| Command | Purpose |
|---------|---------|
| `aurora plugin new <id> [--dir .] [--publisher example]` | Scaffold a package directory. |
| `aurora plugin build [dir] [--out …] [--key …]` | Assemble `<id>-<version>.aurora-plugin`. |
| `aurora plugin validate <package> [--pubkey …] [--allow-unsigned] [--reserved-host …] [--external-url …] [--core-version …]` | Run the linter. |
| `aurora plugin compile <package>` | Print the registry snapshot the package contributes. |
| `aurora plugin test <package> [--pubkey …] …` | Run `tests/*.yaml` through the snapshot checker. Uses `aurora-gateway check-snapshot`, found on `PATH` or through `AURORA_GATEWAY_BIN`. |
| `aurora plugin keygen --out <path>` | Create an ed25519 key pair: `<path>` (private) and `<path>.pub`. |
| `aurora plugin sign <package> --key <file> [--out …]` | Write `SIGNATURE` into the archive. |
| `aurora plugin docs <package> [--format md\|html]` | Render the setup guide and permissions table. |

Keep private keys out of the repository and out of shared locations.

## Connectors

### Provision

```bash
curl -sS -X POST https://aurora.example.org/v1/admin/connectors -H 'Content-Type: application/json' \
  -d '{"id": "example-chat", "display_name": "Example Chat"}'
```

- `id` must match `[a-z][a-z0-9-]{1,31}`. `display_name` is 1 to 80 characters.
- The response is `201` with the anchor token `aun_…`. **Shown once.** Store it as
  `AURORA_CONNECTOR_TOKEN` in the connector's secret.
- The status is `awaiting_registration`.

Other routes: `GET /v1/admin/connectors` (status, manifest, digests, widening reasons,
replicas), `POST /v1/admin/connectors/{id}/token` (rotate; new `aun_` shown once),
`DELETE /v1/admin/connectors/{id}` (`409 connector_in_use` while integrations exist).

### Register and approve

The connector calls `POST /v1/connector/register` with its **manifest** and a detached
signature, authenticated with its `aun_` token (docs/13 §3). The core checks the
signature against the project key or a trusted publisher key. The signed content is the
SHA-256 of the canonical manifest JSON (I-28, `docs/90-decisions.md`).

- The status moves to `pending_approval` on first registration, or when the manifest widens
  (a new capability, setup field or public callback). `widening` lists the reasons.
- Unchanged or narrowed manifests are accepted automatically.
- `package.id` must equal the provisioned connector id, or the API returns 422
  `connector_id_mismatch`.

An administrator approves the exact manifest that was announced. Read the digest first:

```bash
curl -sS https://aurora.example.org/v1/admin/connectors      # note manifest_digest for the connector
curl -sS -X POST https://aurora.example.org/v1/admin/connectors/<id>/approve \
  -H 'Content-Type: application/json' -d '{"manifest_digest": "<manifest_digest>"}'
```

If the manifest changed since you read it, the API returns `409 stale_manifest`. Read again and approve again.

Integrations are not served until approval. Statuses: `awaiting_registration`,
`pending_approval`, `active`, `disabled`.

The core cannot verify which binary runs. docs/13 says operators SHOULD enforce image
signatures with their deployment tooling (for example cosign and an admission policy).

### Connector provisioning in compose and Helm

- **Compose.** `AURORA_BOOTSTRAP_CONNECTOR_TOKENS` is set to `fake:<token>`, but the API does
  not apply it (**not yet implemented**). Provision the fake connector with the routes above.
- **Helm.** Create the token Secret first, then list the connector in `connectors`
  ([install-helm.md](install-helm.md#connectors-optional)).

Do not set `plugins.allow_unsigned` to skip connector approval. The connector approval
code does not read that flag (**not yet implemented** as documented in docs/15).
