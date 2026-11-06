# Backups and state

What state Aurora keeps, where it lives in each install, and how to back it up and restore it.
The design is in [docs/15 §Backups and state](../15-deployment-operations.md#backups-and-state).
Aurora does not ship a backup tool in phase 0. The steps below use standard Postgres,
volume and secret tooling. Test them on a non-production copy before you rely on them.

## What must be backed up

| State | Compose location | Helm location | Loss consequence |
|-------|------------------|---------------|------------------|
| **Postgres** (system of record) | Volume `aurora-pg` | External database; or the evaluation StatefulSet (`postgres.enabled`) | All data. |
| **Encryption keyring** (`AURORA_ENCRYPTION_KEYS`, `AURORA_ENCRYPTION_ACTIVE_KEY`) | `deploy/compose/.env` | Secret `<fullname>-secrets` (or `secrets.existingSecret`) | **Every inline secret becomes unreadable.** docs/08 §3: "Losing the keyring loses every inline value." |
| **Signing key** (`AURORA_SIGNING_KEY`) | `.env` | Same Secret as the keyring | Signed values stop verifying. |
| Internal secret, runner session key | `.env` | Same Secret | Services that share them stop talking until restored. |
| **Gateway CA** (`ca.pem`, `ca.key`) | Volume `aurora-ca` | Secret `<fullname>-gateway-ca` (or `gateway.ca.existingSecret`) | A new CA must be minted, and anything that trusted the old CA must trust the new one. |
| **Per-agent home volumes** | Podman volumes `aurora-home-<sandbox_id>` | PVCs `home-<sandbox_id>` in the sandboxes namespace | Each agent's files and workspace. |
| **Directory-sourced plugins** | Whatever you mount at `plugins.dir` (default `/var/lib/aurora/plugins`) | Not mounted by the chart (**not yet implemented**) | Packages must be re-supplied. |
| Configuration sources | `deploy/compose/aurora.toml`, `.env` | Your `values.yaml` and `config:` overrides | Install cannot be reproduced exactly. |

docs/15 says home volume backups are the operator's job (PVC snapshots or volume backups).

### Secrets are unrecoverable without the keyring

The keyring and signing key cannot be regenerated. The setup tooling never overwrites an
existing secret, but it creates any missing one:

- **Compose.** `setup.sh` fills only empty keys in `.env`. If you lose `.env`, running
  `setup.sh` again creates new keys, and existing inline secrets cannot be opened.
- **Helm.** The `secrets` hook creates a missing key. If you install into a namespace where
  the old Secret is gone, the hook generates new keys, and existing inline secrets cannot be opened.

Store the keyring, the signing key and the gateway CA in a secret manager, separate from the
database backups (docs/08 §3). Restore them before you start the services.

### Key rotation

docs/08 §3: all keys in `AURORA_ENCRYPTION_KEYS` stay usable for decryption until removed.
`AURORA_ENCRYPTION_ACTIVE_KEY` chooses the key for new writes. To rotate, add a new key id
(`[a-z0-9]{1,16}`) to the list, make it active, and keep the old key until no row uses it.
`aurora-api` has no rotation subcommand (`serve`, `openapi`, `migrate`, `bootstrap-admin`, `admin`).
Confirm that no row still references a key before you remove it from the keyring. docs/08 §4 lists
the HKDF purposes derived from the keyring.

## Postgres

Use your normal PostgreSQL backup method, such as `pg_dump` in custom format, a
physical base backup with WAL archiving, or your managed service's point-in-time recovery.
The database is the system of record, so back it up on a schedule.

Compose example (run on the host, with the `aurora-pg` volume running):

```bash
podman compose -f deploy/compose/compose.yaml exec postgres \
  pg_dump -U aurora -d aurora -Fc > aurora-$(date +%F).dump
```

Keep the dump in your backup location, never in the repository.

`request_log` grows by about 300 bytes per request and is partitioned and pruned by
`retention.request_log_days` (default 30). It can dominate the database size
(docs/15 §Sizing).

## Keyring, signing key and CA

Export them as Secrets, and store the export encrypted:

```bash
kubectl -n aurora get secret aurora-secrets -o yaml > aurora-secrets.backup.yaml      # keep encrypted
kubectl -n aurora get secret aurora-gateway-ca -o yaml > aurora-ca.backup.yaml        # keep encrypted
```

These names assume release `aurora`, so the fullname is `aurora` (`aurora.fullname`). The generated
Secret is `<fullname>-secrets` and the CA Secret is `<fullname>-gateway-ca`. For other release names,
or if you set `secrets.generatedName` or `gateway.ca.existingSecret`, use those names. Run
`kubectl -n aurora get secrets` to confirm.

For compose, back up `deploy/compose/.env` and the `aurora-ca` volume.

## Home volumes

- **Podman.** Back up the volumes `aurora-home-<sandbox_id>` with your container tooling.
- **Kubernetes.** Use your CSI snapshot class on the PVCs `home-<sandbox_id>`, or a volume backup tool.

Sandboxes and their homes are per agent. Restore a home only together with the agent's
database rows, or the agent's state will not match.

## Restore order

Restore in this order. It is a generic order. It is not a tested runbook.

1. Stop the services (`podman compose … down` or scale the Deployments to zero).
2. Restore the keyring, signing key, internal secret and runner session key **before** starting
   anything. Helm's `secrets` hook and `setup.sh` create any missing key.
3. Restore the gateway CA.
4. Restore Postgres into an empty database.
5. Run `aurora-api migrate` against the restored database. It applies the forward-only migrations
   in `crates/aurora-db/migrations` that the restored schema is missing. Use the same release that
   made the backup, or a later one.
6. Start the API, then the gateway, hookhost, runner and web app, as in [upgrades.md](upgrades.md#order).
7. Check `GET /v1/instance`, sign in, and open an agent that has a secret. An unreadable secret
   means the keyring is wrong.
8. Restore home volumes for the agents you need, and start them from the dashboard.

Helm: if your external database is restored to a new cluster, create the Secrets first, with the
restored keys, and then install. The chart uses the existing Secret and does not generate new keys.
