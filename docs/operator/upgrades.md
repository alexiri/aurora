# Upgrades

How to move an Aurora install to a new version. The rules are in
[docs/15 §Upgrades](../15-deployment-operations.md#upgrades). This page turns them into steps and
notes where the shipped artifacts do not follow them yet.

## Before you upgrade

1. Read the release notes for schema and contract changes.
2. Back up Postgres, the keyring, the signing key and the gateway CA ([backups.md](backups.md)).
3. Check the current state: `curl -i https://<external-url>/v1/instance`, and the pods are healthy.
4. Choose the new version. Set `image.tag` (default: the chart appVersion, `0.1.0`).

## Rules that make a rolling upgrade safe

- **Migrations are forward-only.** The repository has no down migrations
  (`crates/aurora-db/migrations/*.sql` are forward-only). Migrations run before any new API replica
  serves. Every migration must work with the previous API and gateway that are still running.
  Schema changes use **expand → migrate → contract** over two releases (docs/15).
- **Wire contracts.** The API supports runner, connector and supervisor protocol versions N and
  N−1. Plugin manifest contracts N and N−1 are supported. Registry snapshots use "newest contract
  wins" (docs/09 §9).
- **Rolling order** (docs/15): **migrate → api → hookhost → gateway → runner → web → connectors.**
  Gateways and runners of the previous version keep working during the roll.

## Order

### Helm

```bash
helm upgrade aurora deploy/helm/aurora -n aurora -f my-values.yaml --set image.tag=<new>
```

- The `migrate` job is a `pre-install,pre-upgrade` hook. It runs before the upgraded Deployments
  roll out.
- The `secrets` job is also a `pre-install,pre-upgrade` hook. Its Kubernetes API egress is
  allowed by a hook-rendered NetworkPolicy (`<fullname>-hooks`, weight −15, plus the Cilium
  policy with `networkPolicy.cilium.enabled`): on upgrade the release's default-deny policy is
  already in place, and a job pod without an allow rule hangs in `kubectl` until the upgrade
  times out. On a Cilium cluster the NetworkPolicy alone is not enough — the API server is the
  `kube-apiserver` entity, which no `ipBlock` matches (see install-helm §NetworkPolicies).
- The chart uses one `image.tag` for all core components. It does not roll them one at a time,
  so the staged order above is not enforced. Per-component tags do not exist yet
  (**not yet implemented**). `NOTES.txt` tells operators to roll in the documented order, which
  the chart cannot do alone.
- The N and N−1 rule above is what makes this tolerable. The chart does not check it.
- Connector images are set per entry in `connectors[]`, so they can be bumped separately from the
  core images, which follow `image.tag`.

Watch the rollout: `kubectl -n aurora rollout status deploy/aurora-api` (and for the others).

## Rehearse the upgrade

`scripts/e2e/upgrade.sh` (OPS-014) rehearses the order above on one host with podman: it builds a
baseline ref (`UPGRADE_BASE`, default `8ba42973`; there is no release tag yet) and HEAD, starts the
baseline stack, creates an agent with a running sandbox (fake harness), a conversation, a memory and a
schedule, then upgrades in place (`aurora-api migrate`, then api, gateway, runner) without touching the
sandbox container. It checks that:

- the migrations apply over the baseline schema and are all recorded as successful;
- the running sandbox keeps its container and boot, and the old supervisor still completes a turn
  through the new runner and API;
- conversation history, memories and schedules are readable after the upgrade;
- after an agent restart the sandbox runs the new image.

What it does not cover: it runs the services as host processes (as `scripts/e2e/hosted.sh` does), not
as images; the baseline has no schedules API, so the schedule row is seeded with SQL; every protocol
plane is at version 1 on both refs, so the N−1 window cannot be exercised end to end yet.
Defects it finds are printed as `DEFECT` lines (`UPGRADE_STRICT=1` makes them fail the run).

The kind e2e (`scripts/e2e/kind.sh`, OPS-024) rehearses the Helm chart path itself, hooks included:
after the test scenarios, two value-only `helm upgrade` legs (`web.replicas` 1 → 2 → 1, each
`--wait --timeout 4m`) run against the installed release, asserting helm exit 0, release status
`deployed` and the new web pod Ready. On upgrade the `pre-install,pre-upgrade` hooks (the `secrets`
job, its RBAC and its hook-rendered NetworkPolicy / CiliumNetworkPolicy) run before any regular
manifest under the previous release's default-deny policies — the exact condition of the
2026-11-11 production timeouts — so a chart change that breaks a hook's access under those policies
fails the e2e (with the hook jobs, pod logs and NetworkPolicies in `target/kind-logs/`), not a
production upgrade. The host-process script above stays the rehearsal for migration ordering and
the protocol N/N−1 window; `AURORA_E2E_UPGRADE_LEGS=0` skips the legs.

## Running sandboxes

A running sandbox keeps the image it started with until its next boot (docs/15). The dashboard
shows **"restart to update"** when an agent's boot image differs from the runner's. Restarting
an agent is how it picks up the new image. Plan that with your users.

**(phase 1, RFE-002) Upgrading to phase 1 requires restarting running sandboxes; the dashboard
flags them.** The gateway proxy port (4102) becomes TLS-only and has no plaintext fallback, so the
supervisor inside a sandbox booted before the upgrade (which forwards plaintext) can no longer
reach the gateway: its outbound requests fail closed (nothing is sent in the clear). The API
flags every such agent `restart_to_update` (the boot inputs changed: `gateway.tls`). Restart them
(`POST /v1/agents/{agent}/restart` or the dashboard); the next boot works over TLS. Do this
right after the gateway roll, not at your leisure: until then those agents cannot make
outbound calls.

**Not yet implemented as documented:** the API computes `restart_to_update` from the boot's
environment/stub-file hash only, so after an image-only upgrade (and for boots started before that hash
was recorded) it stays `false` while the sandbox still runs the old image (found by
`scripts/e2e/upgrade.sh`). Until the boot image is recorded, restart agents after an image upgrade
without waiting for the flag; `GET /v1/agents/{agent}/sandbox` shows the runner's image, not the
sandbox's.

## After the upgrade

- `GET /v1/instance` returns the new `version`.
- Check one hosted agent: start a turn, confirm it completes.
- Check the connectors (if any) in `GET /v1/admin/connectors`: they should be `active`.
- Watch the logs and the `/metrics` series listed in docs/15 §Observability on the internal listeners.

## Rollback

Rolling back the binaries is safe only inside the N and N−1 window above. Forward-only migrations
do not roll back. If a release must be undone after its migration ran, restore the database from the
pre-upgrade backup (see [backups.md](backups.md#restore-order)), then run the previous version.
