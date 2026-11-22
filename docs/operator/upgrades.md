# Upgrades

How to move an Aurora install to a new version. The rules are in
[docs/15 §Upgrades](../15-deployment-operations.md#upgrades). This page turns them into steps and
notes where the shipped artifacts do not follow them yet.

## Before you upgrade

1. Read the release notes for schema and contract changes.
2. Back up Postgres, the keyring, the signing key and the gateway CA ([backups.md](backups.md)).
3. Check the current state: `curl -i https://<external-url>/v1/instance`, and the pods are healthy.
4. Choose the new version. Set `image.tag` (default: the chart appVersion, `0.1.0`).
5. **External database: give the gateway its own login first** (FIX-106, D-178 Q-R5-11, D-182). From
   this release the chart refuses to render with an external database (`postgres.enabled: false`, the
   default) unless the gateway has its own least-privilege role: create the role and a Secret with key
   `AURORA_DATABASE_URL` that logs in as it, then set `database.gatewayRole` and
   `database.gatewayExistingSecret` (`aurora-api migrate` grants the role exactly what the gateway needs,
   [install-helm.md](install-helm.md) §Database, `03 §Gateway database role grants`). Or, knowingly, set
   `database.allowSharedLogin: true` to keep the gateway on the main login as before. The refusal happens
   while Helm renders, so nothing in the cluster changes until one of the two is set.
6. **Configuration that is now refused** (FIX-116). The API, runner and connector host refuse at startup
   (and the chart at render time): `plugins.allow_unsigned`, `hosted.allow_fake_harness`,
   `runner.dev_allow_external_network`, `connector_host.test_types` and `ratelimit.api.enabled = false`
   unless `server.external_url` is a loopback origin; credentials (`user:password@`) inside
   `email.smtp_url` or `gateway.egress_proxy` (move the SMTP password to `AURORA_SMTP_PASSWORD`, set
   `email.smtp_username`); `scim.token_max_days` above 730; `previews.session_ttl_secs`/`session_idle_secs`
   above their defaults or zero; `approvals.timeout_secs` below 60; `hosted.background_cap_secs` above a
   week; `relay.request_body_max_bytes` above 100 MiB. Log-only mail no longer logs sign-in or invitation
   links (set `email.log_links = true` in development only).
7. **Lock levels of two phase-4 migrations** (REVIEW-5 PLAT-10). `20261130000001_groups.sql` and
   `20261127000001_skill_libraries.sql` validate constraints and build indexes inside the migration
   transaction, which takes `ACCESS EXCLUSIVE` on the touched tables for the duration. On a large instance
   run them in a maintenance window (or ahead of the roll with `aurora-api migrate`); they cannot be
   changed after the fact because they have already shipped.
8. **New migration `20261201000060_instance_setup.sql`** adds the `instance_setup` marker. An instance that
   already has organizations or a saved placement is not "pending": its behaviour is unchanged. A fresh
   instance stays closed to ordinary users until a placement is saved, even after an administrator has
   signed in.
9. **One app per service: approve the new `github` and `aws-keys` once** (FIX-123, D-195(a), D-214..D-216).
   The GitHub App and the assumed IAM role are now connect methods of `github` / `github-ghes` and
   `aws-keys` (the packages `github-app` and `aws-role` are no longer shipped). The new versions (2.0.0)
   add hooks and egress hosts the old packages already used, which is a permission widening: after the
   upgrade each waits in *Administration › Apps & accounts* for one instance-administrator approval (until then the old
   packages keep working). Approving moves every connection of the old apps to the new app in the same
   transaction (connection ids, stored credentials, grants and history are kept; GitHub Enterprise Server
   instances, skill libraries and app availability follow), deletes the host-claim decisions between the
   old pairs and disables the old package (`superseded_by_<package>`, one audit row `plugin.supersede`).
   Rerunning changes nothing; nothing needs reconnecting. The migration `20261201000110_connection_connect_method.sql`
   adds `app_connections.connect_method` and fills it where the package version had a single method; the
   others read as unknown (the app's token, no tool hidden) until the connection is reconnected.

10. **App skills: the gateway and the hook host must roll after the API** (P5-APPSKILL-1/2, D-453,
    migration `20261201000272_plugin_blobs_agent_file.sql`). Every built-in package that had a guide is
    a new version that ships skills, and the compiled registry snapshot carries them
    (`apps[].agent.skills`). The snapshot reader of a gateway or hook host **one release older is
    strict** and refuses a snapshot with a key it does not know: it logs `registry snapshot refused;
    keeping previous` (gateway metric `gateway_snapshot_rejected_total`) and keeps serving the previous
    snapshot. That is safe, because the new built-in versions widen nothing, but until the gateway and
    hook host are upgraded they enforce the *previous* registry state: do **not** approve, reject,
    disable or enable a package, or change a host-claim decision, while the roll is in progress
    (a change made in that window takes effect when the gateway rolls). The rolling order
    (migrate → api → hookhost → gateway) already has this shape and the chart rolls every component of
    one `image.tag` in a single `helm upgrade`, so the window is the length of the roll. The migration
    renames the `plugin_blobs.kind` value `agent_guide` to `agent_file` and replaces its check: a package
    uploaded to an API replica of the previous release *during* the roll, if it carries agent files,
    fails to store its blobs and is refused; retry it after the roll. A newer gateway reads an older
    snapshot without trouble (the new key is optional).

## Rules that make a rolling upgrade safe

- **Migrations are forward-only.** The repository has no down migrations
  (`crates/aurora-db/migrations/*.sql` are forward-only). Migrations run before any new API replica
  serves. Every migration must work with the previous API and gateway that are still running.
  Schema changes use **expand → migrate → contract** over two releases (docs/15).
- **Wire contracts.** The API supports runner, connector and supervisor protocol versions N and
  N−1. Plugin manifest contracts N and N−1 are supported. Registry snapshots use "newest contract
  wins" (docs/09 §9).
- **Rolling order** (docs/15): **migrate → api → hookhost → gateway → runner → relay (when `relay.enabled`) → web → connectors.**
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

**(FIX-109) Policy path matching ignores one trailing `/`.** A rule path `/v1/users` now also
matches the request `/v1/users/` (and the reverse). For **block** and approval rules this
tightens: requests with the slash variant that used to slip past are now stopped. For **allow**
rules it loosens by exactly that slash variant (the request `/v1/users/` is now allowed where
`/v1/users` was). Review allow rules on endpoints whose slash form means something else
(rare); the policy simulator shows the new answers. Rollback of a policy now needs the draft
revision (`expect_revision`): custom API clients that call `…/policy/rollback` must send it.

## After the upgrade

- `GET /v1/instance` returns the new `version`.
- Check one hosted agent: start a turn, confirm it completes.
- Check the connectors (if any) in `GET /v1/admin/connectors`: they should be `active`.
- Watch the logs and the `/metrics` series listed in docs/15 §Observability on the internal listeners.

## Instance settings: locked keys and break-glass (phase 5, SET-1, D-236..D-238)

Instance-wide settings are edited in the dashboard (Administration › Settings) and applied live. A value
your Helm values set is **locked**: the dashboard shows it read-only and the API refuses to change
it. Upgrading to a release with instance settings changes nothing for an existing install: every key
your values file sets stays locked, and every other key keeps its application default until an
administrator edits it. The first start of the new API mirrors the locked values into the database
(`instance_settings.helm_sync` in Administration › Data › Audit), so **removing a key from your values file later
hands it to the dashboard with the value it had**, never with the default.

*Release-note text:* "N settings in your Helm values are now shown in Administration › Settings as
'Configured in Helm'. To manage one in the dashboard, remove it from your values file and run
`helm upgrade`; it keeps its current value. Administration › Settings lists them under 'Which settings'."

**Break-glass.** Recovery never depends on a stored setting:

1. `auth.instance_admins` (in Helm) still grants the instance-admin flag at sign-in, and a sign-in
   provider defined in Helm works whatever is stored.
2. Setting a key in Helm overrides any stored value at the next start.
3. The CLI works from any pod with the API image and configuration, and starts even when the stored
   settings are invalid:

   ```
   kubectl -n <ns> exec deploy/<release>-api -- aurora-api settings show [<section>]
   kubectl -n <ns> exec deploy/<release>-api -- aurora-api settings reset <section> [--key <k>] [--entry <id>] [--dry-run]
   kubectl -n <ns> exec deploy/<release>-api -- aurora-api settings export [--format yaml|json]
   ```

   When the API cannot start (invalid stored settings stop it, naming the section, the key and the
   reset command), run the same commands from a one-off pod built from the API image with the API's
   ConfigMap and Secret mounted (`kubectl -n <ns> debug` on an API pod, or a `Job` with the API
   Deployment's pod template and `command: ["aurora-api", "settings", "reset", "<section>"]`). The
   CLI writes through the same service as the dashboard (validation, an `instance_settings.reset`
   audit row with source `cli`, `NOTIFY`, an e-mail to every instance admin) and needs no
   re-authentication. Exit codes: 0 done, 2 refused by validation, 3 a Helm-locked key, 4 the
   database is unreachable.

## Skill libraries by address (P5-SKLIB-1)

The release that adds `20261201000270_skill_library_git_url.sql` changes how skill libraries are read
(D-448): a library is an https git address read by the **API**, no longer a GitHub/GitLab app connection
read through the gateway.

- **The migration** gives every GitHub and GitLab library its clone address: on the host of its connection's
  registered instance (host and base path) when it has one, else `github.com` / `gitlab.com`. A GitLab library
  whose connection was deleted, a library of any other app, and the later duplicate of two libraries of one
  organization with the same repository, branch and folder get **no address**: they show the error "Enter the
  repository's git address" and an organization admin sets it once. Pinned skills are untouched everywhere.
- **Private repositories**: the old reads used the connection's credential; the new ones use an optional read-only
  token saved on the library. After the upgrade a library of a **private** repository shows "The repository needs
  an access token" until an organization admin adds one (Libraries › Edit › access token); skills already added
  keep working. Libraries of public repositories need nothing.
- **The API pods need egress to the git hosts** (a server on a private network: `gateway.allowed_private`).
  Library reads go through the API's **own guarded client** (https only, public addresses only), **not** through
  `gateway.egress_proxy`: a deployment that forces all outbound traffic through that proxy must allow the git
  hosts from the API pods directly (D-448(b)).
- **Libraries migrated from a GitHub Enterprise or self-managed GitLab instance** whose registered base path was an
  *API prefix* (for example `/api/v3`) may need their address entered again: the migration builds the clone address
  from `host || base_path`, and such an address is not a repository. It fails safely: the library shows "The
  repository was not found" and an organization admin sets the address once (Libraries › Edit).
- **Rolling upgrade**: a new API replica never calls the gateway's old `POST /internal/v1/library-fetch`; an
  old API replica talking to a new gateway gets 404 and records a transient refresh error that the next pass of
  the new release clears. The gateway route and the `aurora_gateway_library_fetch_total` metric are gone; the
  built-in `github` 2.4.2 and `gitlab` 1.4.3 packages drop their `library` blocks (no re-approval: nothing widens).
- **Audit**: the plugin supersede audit row no longer has a `skill_libraries` key (libraries no longer belong to an app).

## Rollback

Rolling back the binaries is safe only inside the N and N−1 window above. Forward-only migrations
do not roll back. If a release must be undone after its migration ran, restore the database from the
pre-upgrade backup (see [backups.md](backups.md#restore-order)), then run the previous version.
