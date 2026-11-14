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

`plugins.allow_unsigned` (default `false`) accepts unsigned packages and auto-approves connector
manifests. It is for development only. The chart does not set it.

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

Aurora runs the connectors it ships itself. You add one in the dashboard, with nothing to
copy by hand and no deployment to change. A connector for a platform Aurora does not ship, or one
you run elsewhere, is a self-hosted connector: [see below](#self-hosted-connectors-advanced).

### Add a managed connector (dashboard)

You need instance-administrator rights and a running **connector host**
(`connectorHost.enabled`, on by default in the chart; [install-helm.md](install-helm.md#connector-host)).
Design: [docs/13 §3.1](../13-channels-connectors.md#31-managed-connectors-phase-4-p4-mcon-d-128).

1. Open **Admin → Connectors → Add**. The page lists the connector types the connector host
   announced. A type that is already added is marked. If the page says the connector host is not
   running, no host has registered for 90 seconds: check
   `kubectl -n <namespace> get pods -l app.kubernetes.io/component=connector-host` and its logs.
2. **Review** what the type is: name, icon, version, publisher, capabilities, public callbacks and
   setup fields. This is the same summary as the approval screen. Saving approves exactly the
   manifest you reviewed.
3. **Settings**: fill in the type's connector-level settings. Secret fields are password inputs
   and are never shown again. A generated secret (for Mattermost, the key that signs buttons and
   menus) is created by Aurora; you can regenerate it later.
4. **Save.** The connector is `awaiting_registration`, then `active` as soon as the host has
   started it. No token is shown, because nobody handles one.

The connector page then shows the status the host reports (`running` or `error`, with a short
message that never contains a secret). From there you can change settings and secrets (the host
restarts the connector), disable it (the host stops it and its integrations are not served;
enabling restores it), or delete it (`409 connector_in_use` while integrations exist; the stored
secrets are discarded).

What a managed connector can and cannot do:

- The connector host runs only the types built into it (phase 4: **Mattermost**; the `fake` test
  type only when `connectorHost.testTypes` is on, development and end-to-end tests). Nothing you
  enter names an image, a binary, a URL or code (SEC-69).
- The id of a managed connector is its type id, and there is one per type.
- A newer connector host with a **widened** manifest (a new capability, setup field or public
  callback) puts the connector in `pending_approval`, as for any connector. Review and approve it
  as under [Register and approve](#register-and-approve).
- The platform reaches the connector's callbacks (buttons, menus, slash commands) at
  `<externalUrl>/connectors/<id>/…`; the chart's edge rules route `/connectors` to the connector
  host ([install-helm.md](install-helm.md#routing)). Set a connector's *Public URL*
  setting only when the platform reaches Aurora under another name.
- Every connector host replica runs every enabled managed connector; the connector protocol's
  integration ownership handles the replicas.
- The connector host signs nothing at run time. Its release image embeds the manifest signatures
  made with the project key, and the API refuses a type whose signature does not verify (the
  catalogue then does not list it). A build without the project key, such as a local development
  build, announces unsigned manifests: with the default `plugins.allow_unsigned: false` the
  catalogue shows no types, and with `plugins.allow_unsigned: true` (development only) the types
  are accepted, logged with a warning and badged "Unsigned (development)" in the dashboard. A
  signature that is present but invalid is refused either way.

The API routes behind the dashboard: `GET /v1/admin/connector-types`, `POST /v1/admin/connectors`
with `"managed": true`, and `PATCH /v1/admin/connectors/{id}` (docs/05).

## Mattermost from zero

The whole path, one actor per step. Each step names who does it and what the next step is;
design in [docs/13 §5](../13-channels-connectors.md), screens in [docs/14](../14-web-dashboard.md).

**Before you start (in Mattermost).** Bot accounts must be enabled (System Console → Integrations
→ Integration Management). Create a bot account for Aurora (Integrations → Bot Accounts → Add Bot
Account), make it a member of the team, and copy its access token. Optionally create a
personal access token of a system administrator: with it Aurora can reply privately to a person
and register `/aurora`; without it, replies to individuals arrive as direct messages.
The Mattermost server must be reachable from the connector host; a server on a private
network needs `AURORA_MM_ALLOW_PRIVATE_NETWORKS` on the connector host
([docs/20](../20-mattermost-connector.md)), and one behind a private CA needs the CA bundle
(`extraCaBundle`, which the chart also hands to the connector host as `AURORA_TLS_EXTRA_CA_BUNDLE`,
[install-helm.md](install-helm.md#connector-host)).

1. **Instance administrator: add the connector.** *Admin → Connectors → Add connector → Add
   Mattermost.* Nothing needs typing: every Mattermost setting is optional or generated, and
   the *Advanced* section holds the two you almost never change (the address Mattermost uses
   to reach Aurora, left empty for this Aurora's own address, and two tuning numbers). *Admin →
   Setup* shows a callout "Mattermost is available — add it" until you do. Adding is explicit, not
   automatic: it exposes the connector's public callback endpoints and approves its manifest.
   Next: tell an organization owner or admin. They see your name on *Channels* until it is added.
2. **Organization owner or admin: connect the server.** *Organization → Channels → Connect
   Mattermost.* Enter the server address and the bot token, press **Check** (Aurora checks the
   credentials and stores nothing yet), choose the team from the list (a single team is
   preselected), optionally add the admin token, and press **Connect**. The done screen shows
   "Mattermost is connected as @bot in <team>" with three buttons: attach an agent, link your
   Mattermost account, open a direct message with the bot. If *Channels* says why Mattermost is
   not available (not added, waiting for the connector host, disabled, needs approval), that
   message names who acts next.
3. **Anyone: link your Mattermost account.** *Account → Channels* (or the "Link your Mattermost
   account" button on the done screen; or write to the bot, which answers with a sign-in link).
   Without a link Aurora does not know who you are in Mattermost, and you cannot bind a channel from
   the dashboard.
4. **Workspace owner: attach an agent.** *Agent → Channels.* With one integration it is
   preselected; choose the shared bot and **Attach**. A *Next steps* panel appears: **Open a DM
   with @bot** to talk to the agent, and **Add to a channel**.
5. **Workspace owner: put the agent in a channel.** Invite the bot to the channel in Mattermost
   (a private channel too), then *Add to a channel* in the agent's tab: the list shows the channels you
   are a member of where the bot is already present; choose one and how the agent listens
   (*Only when mentioned* by default, or *Every message*), and **Add to this channel**. The bot announces
   itself in the channel. The alternative is to write `@bot bind <handle>` in the channel.

If a step is refused the message says what to do: *Link your chat account* when you have not
linked one, *Invite @bot to the channel, then try again* when the bot is not in the channel, and
a plain "you are not a member" when you are not in the channel (nobody can bind an agent to a
conversation they are not part of). Direct conversations cannot be bound.

## Self-hosted connectors (advanced)

A self-hosted connector is a service you deploy yourself, with its own `aun_…` token and the
settings in its environment. Use it for a platform Aurora does not ship a connector for, or to run
a connector outside the cluster. It follows the same trust rules: a signed manifest, approval,
widening review.

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

### Connector provisioning in Helm

- **Dev stack on kind.** `deploy/kind/values-connector-fake.yaml` sets `AURORA_BOOTSTRAP_CONNECTOR_TOKENS` to
  `fake:<generated token>`. At startup the API upserts the connector `fake` with that token
  (`awaiting_registration`); it still needs manifest approval, as described above
  ([install-kind-dev.md](install-kind-dev.md)).
- **Helm.** Create the token Secret first, then list the connector in `connectors`
  ([install-helm.md](install-helm.md#connectors-optional)).

`plugins.allow_unsigned` also auto-approves connector manifests on registration (with a warning
log), including manifests that widen the approved one. Never set it outside development.
