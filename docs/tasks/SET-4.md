# SET-4: Admin › Sign-in and Admin › Email — sign-up, sessions, OIDC providers and SMTP as live settings with sealed secrets — **medium–large, phase 5**

## Goal
Instance admins manage sign-in without `helm upgrade` (owner, D-235(b)): sign-up mode, org creation,
session lifetimes, CLI key lifetime and **OIDC providers** (including role and group claims,
mappings and JIT) on **Admin › Sign-in**, and the mailer on **Admin › Email**. Client secrets and
the SMTP password are entered once and sealed. A provider defined in Helm stays possible and is
shown "Managed by Helm"; `auth.instance_admins` stays in Helm as the break-glass list.

## Linked requirements
- D-235(b), **D-236**, **D-237** (sections `sign_in`, `email`; keyed list `auth.oidc_providers`;
  `email.smtp_password`; role mappings editable (g)), **D-238** (break-glass).
- Contracts **C-780** (providers, provider test), **C-781** (email, test mail), C-772..C-774.
- Spec: `15 §Instance settings` rows `sign_in`, `email`; `15 §Configuration reference` rows
  `auth.*`, `email.*`; `04 §Sign-up and organization creation`, `§OIDC (SSO)` (providers from two
  sources, last-provider rule, immutable id), `§OIDC role claims` (editable, loosening), `§Sessions`
  (shorter lifetimes apply to existing sessions), `§Instance admins` (break-glass); `08 §1`
  (`instance_setting`); `14 §Instance administration` (Settings; Identity page replaced).
- Security: SEC-89, SEC-90; SEC-78 (role claims) unchanged in meaning.
- Migration numbers if needed: `20261201000196..000197` (none expected).
- Depends on **SET-1**. Independent of SET-2/SET-3.

## Context
- Config: `crates/aurora-config/src/raw.rs` (`AuthSection`, `OidcProvider`, `OidcRoleMapping`,
  `OidcJit`, `EmailSection`), `validate.rs` (provider rules, `no_oidc_provider`, e-mail rules).
- API: `services/aurora-api/src/domains/oidc/{service,client,roles,routes}.rs` (provider registry,
  discovery, role sync), `domains/auth/{service,mail}.rs` (sessions, mailer), `domains/me/service.rs`
  (re-auth), `domains/admin/**` (`GET /v1/admin/identity`), `domains/placement/**` (sign-up mode
  shown read-only), CLI device-login key TTL.
- Web: `web/src/app/admin/identity/**`, `web/src/components/admin/placement-view.tsx`,
  `web/src/app/auth/**` (provider buttons), the SET-1 generic form.

## Scope
### In scope
**Part A — Sign-in (API)**
1. **Registry** for `sign_in`: `auth.signup_mode`, `auth.org_creation`, `auth.session_idle_secs`,
   `auth.session_absolute_secs`, `auth.cli_key_ttl_days`, and `auth.oidc_providers` as a keyed list
   by `id`, entries in the TOML shape with the secret field `client_secret` (C-780). Loosening:
   `invite_only` < `sso_jit` < `open`; `instance_admins` < `anyone`; longer lifetimes; a provider
   added; a mapping to `instance_admin` or org `owner` added or widened; a JIT block widened;
   `trusted_email_domains` added; `link_existing_users` turned on. Update `movable-keys.txt`.
2. **Provider registry live**: rebuilt from the effective list on every `sign_in` swap in every
   replica (discovery cached per provider, re-fetched when its entry changes); the sign-in page's
   provider list and `GET /v1/instance` (`oidc_providers`, `signup_mode`) reflect it; a callback
   for a removed provider → `oidc_provider_unknown`.
3. **Rules**: at least one provider across both sources (422 `last_oidc_provider`; the dev IdP
   counts as today); dashboard `id` immutable; save validates discovery (422 `oidc_discovery_failed`);
   a Helm entry and its secret env stay locked; a mirrored entry with `client_secret_env` keeps it
   until a sealed secret replaces it; preview warnings `removes_own_provider`,
   `grants_instance_admin`, `identities_orphaned` (count of `user_identities` for a removed id).
4. **Sessions**: `session_idle_secs`/`session_absolute_secs` read live; a shorter value applies to
   existing sessions at their next request (expiry = earlier of stored and recomputed), a longer
   one to new sessions; `cli_key_ttl_days` to keys minted afterwards.
5. `POST /v1/admin/settings/sign_in/provider-test` (C-780): discovery only.
6. `GET /v1/admin/identity` stays read-only until the Identity page is removed in Part C, then is
   removed from the router and the OpenAPI document (and its `05` row deleted).

**Part B — Email (API)**
1. **Registry** for `email`: `email.smtp_url`, `email.from`, `email.smtp_username`,
   `email.smtp_password` (secret; locked — `secret.from_env` — when `AURORA_SMTP_PASSWORD` or its
   `_FILE` is set). Existing rules (no userinfo in the URL, `from` required with a URL) on the
   merged view. `email.log_links` is **not** a setting (D-237(b)).
2. Mailer rebuilt live on swap; mail in flight finishes with the old transport.
3. `POST /v1/admin/settings/email/test` (C-781): to the caller's own address, effective or proposed
   values, `export` rate class, never logs the password.

**Part C — Dashboard**
1. `/admin/settings/sign-in`: Sign-up (mode, org creation), Sessions (idle, absolute, CLI keys),
   **Identity providers**: `ItemList` of providers with source `Badge` ("Managed by Helm" read-only),
   **Add provider…** / edit in a `FormDialog` (`2xl`) with sections Basics (id, display name,
   issuer, client id, client secret write-only, scopes), E-mail (allowed and trusted domains, link
   existing users), Roles (role claim and source, require role, mapping table editor), Groups
   (group claim and source), JIT (org, role, workspaces, workspace role); **Test discovery**; the
   loosening `ConfirmDialog` names grants of instance admin. Replaces `/admin/identity` (nav entry
   removed; the route redirects to the new page).
2. `/admin/settings/email`: SMTP URL, from, username, password (write-only), **Send test e-mail**;
   a `Notice` in log-only mode.
3. Sign-up & placement: the read-only sign-up mode links to Sign-in.
4. Docs: `docs/operator/sign-in.md` (dashboard first; Helm providers locked; break-glass:
   `auth.instance_admins`, a Helm provider, `aurora-api settings reset sign_in`).

### Out of scope
- New sign-in methods; SCIM settings (SET-5); changing role-claim semantics (SEC-78).

## Constraints / file ownership
Owns: registry entries for `sign_in`/`email`, `domains/oidc/**` registry wiring, `domains/auth/**`
session-lifetime and mailer wiring, `domains/admin/**` identity route removal,
`web/src/app/admin/settings/{sign-in,email}/**`, `web/src/app/admin/identity/**` (redirect),
`web/src/components/admin/settings/{sign-in,email}-*.tsx`, `placement-view.tsx` link, admin nav
entry, `docs/operator/sign-in.md`, `docs/05` identity row, `web/openapi.json`, `schema.d.ts`,
`movable-keys.txt`. After SET-1. Shared primitives, tokens, both themes, 390 px.

## Acceptance criteria
1. IT: add a provider live (fake OIDC issuer fixture) → its button appears and a sign-in through it
   works on another replica without restart; the client secret is sealed inline and appears in no
   response, export, audit row, log or e-mail (canary).
2. IT: a provider in the test config is locked as an entry (read-only, unchanged by any PUT);
   dashboard entries with other ids coexist; a dashboard entry reusing its `id` → 422
   `entry_id_in_helm` on `/auth.oidc_providers/<id>/id`.
3. IT: removing the last provider → 422 `last_oidc_provider`; removing a provider with identities →
   preview `identities_orphaned` with the count; a mapping to `instance_admin` → preview
   `grants_instance_admin` and loosening.
4. IT: lowering `session_absolute_secs` expires an older session at its next request; raising it
   does not extend existing sessions.
5. IT: e-mail settings live → an invitation uses the new transport (SMTP fixture); `AURORA_SMTP_PASSWORD`
   set → password locked; test mail only to the caller.
6. IT: break-glass — a broken dashboard provider set plus a Helm provider: sign-in through the Helm
   provider works; `aurora-api settings reset sign_in` restores the defaults and is audited.
7. Web tests: provider list with Helm badge, provider dialog sections and secret field, Test
   discovery, loosening confirm, email page and test; `/admin/identity` redirects; 390 px, themes.
8. fmt, clippy, targeted api ITs, full api suite at the end, guards exit 0, web checks and build.

## Suggested agent/model and reason
Sonnet (`rust-developer` for A/B, `nextjs-developer` for C); Opus review of the provider registry
swap, the last-provider rule and the break-glass test (sign-in lockout risk).

## Completion report (lane `p5-apps`, 2026-11-15) — API, tests and docs DONE; dashboard editors OPEN
**Done (commit `SET-4: …`):** registry (`sign_in`: `auth.signup_mode/org_creation/session_idle_secs/session_absolute_secs/cli_key_ttl_days`, keyed `auth.oidc_providers`; `email`: `smtp_url/from/smtp_username/smtp_password` secret), `OidcProvider.client_secret` (sealed `$secret`) and `EmailSection.smtp_password_sealed`, live `AppState::auth()` / `LiveMailer`, sessions follow `created_at + absolute`, `POST /admin/settings/sign_in/provider-test`, `POST /admin/settings/email/test` (rate class `export`), provider rules/loosening/warnings (`domains/settings/sign_in.rs`, D-309), `GET /v1/admin/identity` removed (OpenAPI, `schema.d.ts`, docs/05, C-511, matrix), Identity page/nav/view/hook removed with a router redirect `/admin/identity → /admin/settings/sign-in`, `docs/operator/sign-in.md`, 12 ITs in `tests/it/settings_signin.rs` (all green), config unit tests.
**Dashboard editors DONE (second commit, 2026-11-15):** `sign-in-settings-view.tsx` (Sign-up, Sessions, Identity providers `ItemList` with "Managed by Helm" read-only rows, "You signed in with this" badge), `sign-in-provider-dialog.tsx` (`FormDialog` 2xl: Basics, E-mail, Roles with mapping editor, Groups, JIT; write-only client secret sent as `{ $secret: "<text>" }`, kept as `keep`, never an env/file reference; changing issuer or client id of a stored secret requires entering it again — mirrors `keep_retargets`; **Test discovery** sends no secret), `email-settings-view.tsx` (log-only `Notice`, **Send test e-mail** with the unsaved values or the settings in force), `SettingsSectionForm` got a `footer` slot and a fix so a typed `{ $secret: "<plaintext>" }` inside a keyed entry is no longer rewritten to `keep`, placement link, `useTestProvider`/`useTestEmail`. Vitest `admin-settings-sign-in.test.tsx` (7), `admin-settings-email.test.tsx` (3); mocked e2e in `admin-settings.spec.ts` (sign-in dialog + email page, both themes, 1280/390 px, axe, no overflow). Gate level: targeted (typecheck, eslint, vitest admin-settings*, Playwright admin-settings); batch gate pending. The follow lines below are kept for the record.
**Open (superseded, kept for the record):** `web/src/components/admin/settings/{sign-in,email}-*.tsx` — provider `ItemList` + `FormDialog` (Basics/E-mail/Roles/Groups/JIT, Test discovery, loosening confirm naming instance-admin grants, `warnings` display), Email page with **Send test e-mail** and a log-only `Notice`, placement-view link, vitest + mocked e2e (390 px, both themes), `settings-section-view.tsx` dispatch for `sign_in`/`email`. Until then both pages render in the generic section form (secret fields work there; the provider list is a raw keyed list).
