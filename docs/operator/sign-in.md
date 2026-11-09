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
| `auth.oidc_providers[].jit` | unset | `{ org_slug, role, workspace_slugs, workspace_role }`: where a first-time user lands under `auth.signup_mode = "sso_jit"`. `role` is `member` or `admin`; `workspace_role` is `member` (default) or `approver`. |
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

With Helm, add the secret to the API pods: `api.extraEnv: [{name: AURORA_OIDC_CORP_SECRET, valueFrom: {secretKeyRef: {name: aurora-oidc, key: corp}}}]`.

**First administrator.** `aurora-api bootstrap-admin --email <you>` pre-provisions an instance administrator with a verified e-mail and no credential. Their first OIDC sign-in with that e-mail binds the identity
([install-helm.md](install-helm.md#post-install-checks)). It refuses to run when no provider is configured.

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
