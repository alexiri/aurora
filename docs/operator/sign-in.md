# Sign-in: OIDC and the development IdP

(Phase 1, RFE-004.) Aurora signs people in **only through OpenID Connect**. There are no local passwords. The normative text is
[docs/04 §OIDC](../04-identity-tenancy.md#oidc-sso) and [docs/04 §Development and end-to-end sign-in](../04-identity-tenancy.md#development-and-end-to-end-sign-in-phase-1-rfe-004).
In Helm, put the keys under `config.auth` (and the client secret in `api.extraEnv` with `valueFrom`).

## OIDC providers (production)

At least one provider is required. Without one, and without the development IdP, the API refuses to start with `no_oidc_provider`.

| Key | Default | Meaning |
|-----|---------|---------|
| `auth.oidc_providers[].id` | required | Lowercase letters, digits, `_` or `-`; unique. `dev` is reserved. |
| `auth.oidc_providers[].display_name` | unset | Label on the sign-in page. |
| `auth.oidc_providers[].issuer` | required | Issuer URL (`issuer_url` is accepted as an alias). Needs discovery at `/.well-known/openid-configuration`. |
| `auth.oidc_providers[].client_id` | required | |
| `auth.oidc_providers[].client_secret_env` | required | The **name** of an environment variable (or `<NAME>_FILE`) that holds the client secret. The API fails at startup if it is not set. The secret itself never goes in the TOML. |
| `auth.oidc_providers[].scopes` | `["openid","email","profile"]` | |
| `auth.oidc_providers[].allowed_email_domains` | `[]` | If set, only these e-mail domains may sign in. |
| `auth.oidc_providers[].trusted_email_domains` | `[]` | Exact domains (e.g. `example.org`, no subdomains) this provider is **authoritative** for: an address in one of them counts as verified even when the ID token has no `email_verified = true`. Use it only for an IdP that never sends the claim, and only for domains whose mailboxes belong to the provider's own accounts. Other addresses still need the claim. See below. |
| `auth.oidc_providers[].jit` | unset | `{ org_slug, role, workspace_slugs, workspace_role }`: where a first-time user lands under `auth.signup_mode = "sso_jit"`. `role` is `member` or `admin`; `workspace_role` is `member` (default) or `approver`. |
| `auth.oidc_providers[].link_existing_users` | unset | (D-178) Whether a first sign-in through this provider may link to an existing account that was created through **another** provider with the same e-mail. Unset means yes, **except** for a provider with `trusted_email_domains`, which defaults to no (`email_linked_elsewhere`): its addresses are asserted by you, not proven by the IdP, so linking would let it take that account over. Set `true` only when every account in those domains really belongs to this provider. Accounts that have no identity yet (the bootstrap admin, people provisioned by SCIM) are always bound by their e-mail. |
| `auth.cli_key_ttl_days` | `90` | (D-178) How long an API key minted by the CLI device login (`aurora login`) is valid, in days; `0` = never expires. Keys minted earlier have no expiry and stay valid until revoked. |
| `auth.oidc_providers[].require_email_verified` | removed | An ID token without `email_verified = true` is always refused. Setting `false` fails startup; delete the key. |
| `auth.signup_mode` | `invite_only` | `open`, `invite_only` or `sso_jit`. |
| `auth.org_creation` | `instance_admins` | or `anyone`. |
| `auth.instance_admins` | `[]` | E-mail addresses. |

The redirect URI to register at the identity provider is `<server.external_url>/v1/auth/oidc/<id>/callback` (built in `services/aurora-api/src/domains/oidc/service.rs`, `redirect_url`).

```toml
[auth]
signup_mode = "sso_jit"

[[auth.oidc_providers]]
id = "corp"
display_name = "Company sign-in"
issuer = "https://idp.example.org/realms/main"
client_id = "aurora"
client_secret_env = "AURORA_OIDC_CORP_SECRET"
allowed_email_domains = ["example.org"]
jit = { org_slug = "example", role = "member", workspace_slugs = ["general"] }
```

With Helm, add the secret to the API pods: `api.extraEnv: [{name: AURORA_OIDC_CORP_SECRET, valueFrom: {secretKeyRef: {name: aurora-oidc, key: corp}}}]`. The migrate Job receives the same variables, since it validates the same configuration. Create the Secret before `helm install`, because the Job runs as a pre-install hook.

**`email_unverified` with a provider that never sends `email_verified`.** Aurora signs a user in only when the ID token says the e-mail is verified, since there is no local credential and the e-mail links accounts and invitations. Some IdPs never send the claim (CERN SSO, for example, documents no `email_verified` claim). For those, declare the domains the provider is authoritative for:

```toml
[[auth.oidc_providers]]
id = "cern"
issuer = "https://auth.cern.ch/auth/realms/cern"
client_id = "aurora"
client_secret_env = "AURORA_CERN_OIDC_CLIENT_SECRET"
trusted_email_domains = ["cern.ch"]
```

Addresses in other domains (for example the Google, GitHub, eduGAIN or guest accounts such an IdP also federates) are still refused with `email_unverified`. Add `allowed_email_domains = ["cern.ch"]` too if only those addresses may sign in at all.

**First administrator.** `aurora-api bootstrap-admin --email <you>` pre-provisions an instance administrator with a verified e-mail and no credential. Their first OIDC sign-in with that e-mail binds the identity
([install-helm.md](install-helm.md#post-install-checks)). It refuses to run when no provider is configured.

## Roles from the IdP's claims (phase 4, P4-IDP-1)

The IdP already knows who administers what. Aurora reads the **role values** in the token and maps them to Aurora roles; mapping e-groups to role values stays in the IdP. Normative text: [docs/04 §OIDC role claims](../04-identity-tenancy.md#oidc-role-claims-phase-4-p4-idp-1-d-139-sec-78) and D-139, D-143.

| Key (per `auth.oidc_providers[]`) | Default | Meaning |
|-----|---------|---------|
| `role_claim` | unset | Dot path into the **validated ID token** (or the userinfo response) holding the role values: `cern_roles`, `roles`, `resource_access.aurora.roles`. An array of strings, or one string separated by spaces or commas. |
| `role_claim_source` | `id_token` | or `userinfo`: Aurora then fetches the userinfo endpoint with the sign-in's own access token (and ignores the answer if its `sub` differs from the ID token's). |
| `require_role` | `false` | `true`: a sign-in whose claim maps to no grant is refused (`no_mapped_role`), including people who could otherwise sign in and instance administrators listed in `auth.instance_admins`. |
| `role_mappings[]` | `[]` | `{ value, instance_admin, org: { slug, role }, workspaces: [{ slug, role }] }`. `value` is matched **exactly** (case-sensitive, no wildcards). `org.role` is `owner`, `admin` or `member`; workspace roles are `owner`, `approver` or `member` in that org. |

Startup refuses unknown keys, duplicate `value`s, an invalid role or slug, a mapping that grants nothing, `workspaces` without `org`, `role_mappings` without `role_claim` and `require_role` without mappings, and names the key.

What happens at **every** sign-in:

- Roles match what the token says right now. New values add roles; values that disappeared remove the roles those values gave, at the next sign-in. A role removed in the IdP is therefore effective at the person's next sign-in (bounded by `auth.session_absolute_secs`); use SCIM or disable the user for immediate removal.
- Roles given any other way (invitation, dashboard, `jit`, `bootstrap-admin`, `auth.instance_admins`) are never changed or removed. A claim can only **raise** a manual role; when the claim goes, the manual role comes back. The last owner of an organization is never removed (audited as `org_role.idp_sync_blocked`).
- An organization or workspace named by a mapping that does not exist is created at the first sign-in that needs it (an organization with its default workspace and default agent), so a fresh instance needs no hand-made orgs.
- Under `invite_only` and `sso_jit` a **new** person whose claim maps to at least one grant is admitted (D-143); everything else (verified e-mail, `allowed_email_domains`, disabled users) still applies first.
- In the dashboard, roles from the IdP show "From IdP role `value`" and cannot be edited or removed there. People see "Roles from your sign-in" on their account page; instance admins see the same per user on **Admin › Users** and the mapping table on **Admin › Identity** (both read-only). Mappings live only in the Helm values.

**CERN SSO** (`cern_roles` carries the application roles that e-groups were given in the Application Portal):

```yaml
config:
  auth:
    signup_mode: invite_only
    oidc_providers:
      - id: cern
        display_name: CERN SSO
        issuer: https://auth.cern.ch/auth/realms/cern
        client_id: aurora
        client_secret_env: AURORA_CERN_OIDC_CLIENT_SECRET
        trusted_email_domains: [cern.ch]
        allowed_email_domains: [cern.ch]
        role_claim: cern_roles
        require_role: true
        role_mappings:
          - value: aurora-admins
            instance_admin: true
          - value: aurora-users
            org: { slug: users, role: member }
          - value: it-ai-team
            org: { slug: it, role: owner }
            workspaces: [{ slug: infra, role: owner }, { slug: research, role: approver }]
```

**Keycloak** (client roles are in `resource_access.<client>.roles`):

```yaml
config:
  auth:
    oidc_providers:
      - id: keycloak
        issuer: https://sso.example.org/realms/main
        client_id: aurora
        client_secret_env: AURORA_KEYCLOAK_OIDC_CLIENT_SECRET
        role_claim: resource_access.aurora.roles
        role_mappings:
          - value: admin
            instance_admin: true
          - value: member
            org: { slug: example, role: member }
```

**"It does not work."** Sign in, open **Account › Roles from your sign-in** and compare the values with the mapping table under **Admin › Identity**. A value that is not listed under *Grants* matched no mapping (values are case-sensitive); an empty list means the token carried no value at the `role_claim` path (check the client's mappers and scopes, or use `role_claim_source: userinfo`). To try it locally, run the development IdP and set a user's claims with `PUT <dev-idp>/dev/claims/<email>` and a body such as `{"cern_roles": ["aurora-admins"]}`.

## Groups from the IdP (phase 4, P4-GRP-1, D-156)

Aurora groups (`04 §Groups`) can mirror a claim value of your IdP so nobody maintains the same list twice. Per provider set the claim that carries the values:

| Key | Default | Meaning |
|---|---|---|
| `group_claim` | unset | Dot path into the **validated ID token** (or userinfo response) holding the group values, read exactly like `role_claim`. Unset: this provider feeds no group. |
| `group_claim_source` | `id_token` | or `userinfo` (ignored when the answer's `sub` differs from the ID token's). |

An org owner or admin then creates a group in the dashboard (**Groups › New group › Linked to sign-in provider**), picks the provider and types the exact claim value. At every sign-in through that provider the user's membership in each linked group of an organization they already belong to is set from the claim (added when present, removed when absent; an absent claim removes all). It **never** adds anybody to an organization: admission stays with invitations, `role_mappings`, `jit` and placement. Members of such a group are read-only in Aurora and show their last sync time. A change at the IdP takes effect at the person's next sign-in; shorten `auth.session_absolute_secs` / `auth.session_idle_secs` if you need faster removal.

**CERN SSO.** The recommended claim is `cern_roles`: in the Application Portal assign the application's roles to e-groups (CERN expands nested e-groups) and set `group_claim: cern_roles` (it may be the very claim used for `role_claim`). The plain `groups` claim also works once CERN has enabled it for the application (a ServiceNow request; CERN discourages it as the default and the value format is not documented, so look at a real token first). Check what arrived under **Account › Roles from your sign-in**, then link groups to those exact values.

```yaml
auth:
  oidc_providers:
    - id: cern
      # …
      role_claim: cern_roles
      group_claim: cern_roles
```

To try it locally, set the dev IdP's claims with `PUT <dev-idp>/dev/claims/<email>` and a body such as `{"groups": ["eg-platform"]}`, and set `auth.dev_idp.group_claim: groups`.

Removing `group_claim` from a provider stops the sync and leaves existing members until an admin deletes the group.

Verify: `curl -fsS https://<host>/v1/instance` lists every way to sign in under `oidc_providers`. Then sign in through the dashboard (`<external_url>/auth/login`).
A failed start names the key (for example `auth.oidc_providers[0].client_secret_env: environment variable AURORA_OIDC_CORP_SECRET (or AURORA_OIDC_CORP_SECRET_FILE) is not set`).

## Development IdP (development only)

`aurora-dev-idp` is a tiny OIDC server for laptops, kind clusters and end-to-end tests: its login page lists fixture users and signs in without a password. **Never use it on an instance that other people can reach.**
The configuration refuses it unless `server.external_url` is on `localhost`, `*.localhost`, `127.0.0.1` or `[::1]`; startup fails with `auth.dev_idp.enabled: refused: the development IdP requires server.external_url on localhost…`.
The provider id `dev` is reserved for it, the dashboard login page shows a banner, and the release guard `scripts/guards/no-dev-idp-in-release.sh` checks it stays out of release artifacts.

| Key | Default | Meaning |
|-----|---------|---------|
| `auth.dev_idp.enabled` | `false` | Registers the extra provider `dev` ("Development sign-in") with a fixed client. |
| `auth.dev_idp.issuer` | required when enabled | The issuer URL **as the browser reaches it**. |
| `auth.dev_idp.internal_url` | `issuer` | Where the API itself reaches the IdP (discovery, token, JWKS) when that differs, for example the in-cluster Service while the browser uses the ingress. |

```toml
[server]
external_url = "http://localhost:4000"

[auth.dev_idp]
enabled = true
issuer = "http://localhost:4000/dev-idp"
internal_url = "http://aurora-dev-idp:4107/dev-idp"
```

In Helm, set `devIdp.enabled: true` (default `false`): the chart renders `auth.dev_idp`, serves the IdP through the ingress at `devIdp.pathPrefix` (default `/dev-idp`) and **fails the render** unless `externalUrl` is a loopback origin.
`devIdp.users` is the user list and `devIdp.allowAnyHint` accepts a `login_hint` that is not listed. The dev kind stack enables it in `deploy/kind/values-dev.yaml` ([install-kind-dev.md](install-kind-dev.md)); the production checklist sets `devIdp.enabled: false`.
On the host, `just dev` runs it on port 4107.

Verify: `GET /v1/instance` lists provider `dev`. A scripted sign-in passes `login_hint=<email>` to `GET /v1/auth/oidc/dev/start`.
