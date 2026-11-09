# App instances and the built-in catalogue

(Phase 2, D-74, P2-CAT and P2-INST.) The general plugin and connector flow is in [plugins-and-connectors.md](plugins-and-connectors.md). This page covers the two pieces specific to phase 2:
the packages that ship in the API image, and registering self-hosted servers for apps that support it. No new configuration key is involved besides `plugins.builtin_dir`;
everything else is done in the dashboard or the admin API.

## The built-in catalogue

The API image ships signed app packages in `/usr/share/aurora/plugins` (`plugins.builtin_dir`, default that path, must not be empty). They are scanned like `plugins.dir`, but **fail-closed**:
only archives signed by the compiled-in Aurora project key (publisher `aurora`) load. Unsigned archives, archives signed by any other key (a trusted key included) and invalid archives are refused and logged as `plugin file refused`.
`plugins.allow_unsigned` does not apply to this directory. A built-in package activates automatically on first sight and on upgrade, unless its permissions widened compared with the version you approved before; then it waits for instance-administrator approval.

What ships (the package directories in `plugins/builtin/`):

| Package | Product | Self-hosted instances |
|---------|---------|-----------------------|
| `atlassian` | Atlassian | yes |
| `aws-keys` | AWS (access keys) | no |
| `aws-role` | AWS (assume role) | no |
| `github` | GitHub | yes |
| `github-app` | GitHub App | yes |
| `gitlab` | GitLab | yes |
| `google-workspace` | Google Workspace | no |
| `indico` | Indico | yes |
| `linear` | Linear | no |
| `microsoft-365` | Microsoft 365 | no |
| `notion` | Notion | no |
| `postgresql` | PostgreSQL (database lane, see [gateway-posture.md](gateway-posture.md#database-lane)) | no |
| `sentry` | Sentry | yes |

Each package carries a setup guide (`plugins/builtin/<id>/docs/setup.md`) written for the person connecting the app. Versions are in each `plugin.yaml`.
Organizations choose which apps their members may use (`GET` and `PUT /v1/orgs/{org}/app-availability`, posture plus allow-list).

**Signing.** The release pipeline signs the archives with a private key held as a CI secret; the matching public key is compiled into the API (`services/aurora-api/src/domains/plugins/keys.rs`). If the pipeline has no key, the archives are left unsigned and the API
refuses them, so the catalogue stays empty rather than unsafe. This applies equally to an API image **you build yourself** (see [install-helm.md](install-helm.md#images-publish-pull-or-build-and-load)): `podman build` without the project key gives an image whose built-in packages are all refused.
Details and the first-run checks are in [docs/ops/builtin-plugin-signing.md](../ops/builtin-plugin-signing.md). To use a package without the project key, upload it as an ordinary plugin and approve it ([plugins-and-connectors.md](plugins-and-connectors.md#plugins)).

**Icons and provenance.** The dashboard icons are rendered from Simple Icons 16.37.0 (CC0-1.0), or from the vendor's own official artwork, or a neutral monogram when neither exists (decision D-99). The per-package source, slug, brand colour and checksums are in
[`plugins/builtin/ICONS.md`](../../plugins/builtin/ICONS.md). Trademarks stay with their owners.

Verify: `GET /v1/admin/plugins` (instance administrator) lists each package with `source: builtin`, `status: active` and `signer.kind: builtin`; the API log has no `plugin file refused` warnings.

## App instances (self-hosted servers)

SaaS apps live on fixed hosts. A company's own GitLab, Jira Data Center or Sentry (the examples in docs/09) lives on a domain nobody can declare in advance. An app whose manifest declares `instances` therefore does not send credentials anywhere until an administrator
**registers the host** as an *app instance*. Registration is the trust decision about where that app's credentials may be sent; members never type a host. The gateway injects a connection's credential only on the host of the instance it was connected to.

**Who registers.**

| Scope | Who | Where |
|-------|-----|-------|
| Instance-wide (usable by every organization) | instance administrator | the admin pages; `GET` and `POST /v1/admin/app-instances` |
| One organization | organization owner or admin | Organization settings, Instances; `GET` and `POST /v1/orgs/{org}/app-instances` with `{ app_id, host, label }` |

Members of the organization see the registered instances (instance-wide plus their own) in the connect form and pick one. `PATCH /v1/app-instances/{id}` changes the label or status; `DELETE` disables the instance's connections. An app with a `default_host` (for example `gitlab.com`) can also be
connected without choosing an instance.

**What registration checks.** The host must be a lower-case DNS name: no IP literal, user info, port or path, and nothing under `aurora.internal`. It must not equal or fall under a host that another active package already claims, a governed LLM host, or another instance registered in the same scope
(error `instance_host_conflict`). If the app declares a `probe`, Aurora runs it through the gateway's destination guard as part of registration. A **private address** is reachable only if the operator allows it:

```toml
[gateway]
allowed_private = ["10.40.0.0/16", "gitlab.internal.example.org"]   # CIDRs, IPs or host names; also used by the API for this probe
```

Verify: register the instance, check the probe result in the response, connect the app to a test workspace, grant it to an agent and make one read call. The request log shows the instance host; a call to any other host gets no credential.
The specification is [docs/09 §3.4 Instance-bound hosts](../09-app-plugins.md#instance-bound-hosts-self-hosted-apps-phase-2-d-74) and [docs/05 §App instances](../05-control-plane-api.md#app-instances-self-hosted-apps-phase-2-d-74).
