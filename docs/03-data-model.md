# 03 — Data Model

This document owns every table. Migrations (`crates/aurora-db/migrations/`)
MUST implement it exactly; any change starts here.

## Conventions

- Primary keys `id uuid` (UUIDv7, app-generated) unless stated.
- Every table has `created_at timestamptz not null default now()`; mutable
  tables have `updated_at timestamptz not null default now()` maintained by the
  repository layer (not triggers) — **except** columns listed as *churn columns*,
  which are written without touching `updated_at` (they would bust etags).
- Closed vocabularies use `text` + `CHECK (col IN (…))`. Open vocabularies
  (error codes, reasons) use `text` with a length check (≤64) only.
- Sensitive values are stored as `SecretRef` strings in columns named `*_ref`
  (type `text`). Never store plaintext secrets. See `08`.
- Token hashes are `bytea` (SHA-256, 32 bytes), unique-indexed; display prefixes
  are `text`.
- `FK→x (cascade)` = `REFERENCES x(id) ON DELETE CASCADE`; `(restrict)` and
  `(set null)` likewise. **Ledger tables have no foreign keys** (marked
  *no FKs*): they outlive what they describe.
- Org deletion is a hand-written service (deletes children in order); FKs from
  org-owned tables to `organizations` are `restrict` so a forgotten table fails
  loudly. Workspace and agent deletes cascade.
- `jsonb` columns have a documented shape; the repository validates on write.

---

## 1. Identity and tenancy (owner doc: `04`)

### `users`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| email | citext unique not null | lower-cased, trimmed |
| email_verified_at | timestamptz null | |
| display_name | text not null | ≤80 |
| is_instance_admin | bool not null default false | |
| disabled_at | timestamptz null | instance-level disable |
| default_org_id | uuid null FK→organizations (set null) | |
| last_login_at | timestamptz null | churn column |
| approval_email_enabled | bool not null default true | per-user opt-out of approval e-mails (phase 2, RFE-011a; `PATCH /v1/me { approval_email }`) |
| idp_admin_source | text null | (phase 4, P4-IDP-1, D-139) `idp:<provider>:<value>` when the claim sync granted `is_instance_admin`; `null` for a manual, e-mail-list or `bootstrap-admin` flag (those are never revoked by the sync) |

### `user_idp_roles`
(phase 4, P4-IDP-1, D-139) The claim debugger: the role values of a user's last sign-in and what each mapped to.
| Column | Type | Notes |
|---|---|---|
| user_id | uuid pk FK→users (cascade) | |
| provider_id | text not null | the provider whose token carried the values |
| claim_values | text[] not null default `{}` | every value of the role claim, mapped or not |
| granted | jsonb not null default `[]` | one entry per matched mapping: `{value, instance_admin, org {slug, role}, workspaces [{slug, role}]}` |
| synced_at | timestamptz not null default now() | |

### `user_identities`
External identities linked to a user (OIDC).
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| user_id | uuid FK→users (cascade) | |
| provider_id | text not null | OIDC provider id from config |
| subject | text not null | `sub` claim |
| email_at_link | citext not null | |
| unique | (provider_id, subject) | |

### `sessions`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| user_id | uuid FK→users (cascade) | |
| token_hash | bytea unique | `aus_` token |
| ip | inet null | |
| user_agent | text null | ≤256 |
| last_seen_at | timestamptz not null | churn column; updated at most once per minute |
| expires_at | timestamptz not null | absolute expiry |
| revoked_at | timestamptz null | |
| reauth_at | timestamptz null | last successful OIDC re-authentication (`04 §OIDC`); account deletion needs it within 10 min (phase 1, RFE-004) |

### `organizations`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| slug | text unique not null | |
| name | text not null | ≤80 |
| app_posture | text CHECK in (`open`,`restricted`) default `open` | see `07 §App availability` |
| governance | jsonb not null default `{}` | org LLM and egress governance document (`08 §11.2`); `{}` = defaults derived from the instance; (phase 1, RFE-003) `egress.open_lane` = `enabled` \| `allowlist_only` \| `disabled` (default `enabled`), the org open-lane posture, not below the instance minimum (`08 §11.2`); (phase 2, RFE-025) `byo` = `{ "posture"?: "enabled"\|"owners_only"\|"disabled", "source_cidrs"?: [cidr], "require_context_credentials"?: bool }`, the org BYO posture, default source list and require-context default (`08 §11.2`, `§11.9`); (phase 3, RFE-013) `agent_tokens.max_age_days` = int 1…3650 \| absent, the org maximum age of BYO agent tokens; absent = none; never above the instance value (`08 §7`); no column, `governance` is `jsonb` |
| governance_revision | bigint not null default 0 | optimistic concurrency for governance edits |
| external_id | text unique not null | 32 random hex chars at creation; immutable; exposed to plugins as context variable `org_external_id` (e.g. AWS external id) |
| settings | jsonb not null default `{}` | `{ "max_agents"?: int, "max_members"?: int, "sandbox_overrides"?: ResourceProfile, "retention"?: { "conversations_days"?: int\|null, "attachments_days"?: int\|null, "attachment_quota_mib"?: int\|null } }`; `retention` (phase 2, RFE-010/015): per-org retention and attachment limits, null/absent = keep forever / unlimited |
| frozen_at | timestamptz null | (phase 2, RFE-006) set ⇒ frozen at this level (`04 §Freeze`); effective freeze of an agent = agent ∨ workspace ∨ org |
| frozen_by_user_id | uuid null FK→users (set null) | (phase 2, RFE-006) |
| frozen_reason | text null | (phase 2, RFE-006) 1..500 chars, sanitized; required when `frozen_at` is set |
| frozen_authority | text null CHECK in (`workspace_owner`,`org_admin`,`instance_admin`) | (phase 2, RFE-006) highest authority of the freezer; lifting needs at least this. A CHECK keeps the four `frozen_*` columns all null or (all but `frozen_by_user_id`) set |

### `org_members`
| Column | Type | Notes |
|---|---|---|
| org_id | uuid FK→organizations (cascade) | pk part |
| user_id | uuid FK→users (cascade) | pk part |
| role | text CHECK in (`owner`,`admin`,`member`) | |
| status | text CHECK in (`active`,`suspended`) default `active` | suspended = non-member for every check |
| idp_role | text null CHECK in (`owner`,`admin`,`member`) | (phase 4, P4-IDP-1, D-139) the role the IdP claims give; `null` = no claim-sourced role. `role` stays the effective role (the stronger of this and `base_role`) |
| idp_source | text null | `idp:<provider>:<value>` of the claim value behind `idp_role`; `null` exactly when `idp_role` is `null` |
| base_role | text null CHECK in (`owner`,`admin`,`member`) | the role the other sources gave before a claim raised it; restored when the claim goes; `null` when the claim created the row (the row goes with the claim) |

Invariant: every org has ≥1 active `owner` (enforced by the service; the last
owner cannot be demoted or removed).

### `invitations`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid FK→organizations (cascade) | |
| email | citext not null | |
| role | text CHECK in (`admin`,`member`) | |
| workspace_ids | uuid[] not null default `{}` | workspaces to bind on accept (role member) |
| approver_workspace_ids | uuid[] not null default `{}` | workspaces bound as `approver` on accept (disjoint from `workspace_ids`, which stay `member`) (phase 2, RFE-012) |
| token_hash | bytea unique | `aui_` |
| status | text CHECK in (`pending`,`accepted`,`cancelled`,`expired`) | |
| invited_by | uuid FK→users (set null) | |
| accepted_by | uuid FK→users (set null) | |
| expires_at | timestamptz | default 7 days |
| unique | (org_id, email) WHERE status = `pending` | |

### `workspaces`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid FK→organizations (restrict) | |
| slug | text not null | unique (org_id, slug) |
| name | text not null | |
| timezone | text not null default `UTC` | IANA name, validated with `chrono-tz` |
| memory_language | text not null default `english` | Postgres text-search configuration name or `simple`; validated against `pg_ts_config` |
| home_sync_generation | bigint not null default 0 | bumped when a workspace setting affects projections |
| byo_posture | text null CHECK in (`enabled`,`owners_only`,`disabled`) | (phase 2, RFE-025) null = inherit; can only tighten (`08 §11.9`) |
| frozen_at | timestamptz null | (phase 2, RFE-006) as on `organizations` |
| frozen_by_user_id | uuid null FK→users (set null) | (phase 2, RFE-006) |
| frozen_reason | text null | (phase 2, RFE-006) |
| frozen_authority | text null CHECK in (`workspace_owner`,`org_admin`,`instance_admin`) | (phase 2, RFE-006) |
| personal_owner_user_id | uuid null FK→users (set null) | (phase 4, P4-ONB-1, D-140, SEC-79) the user this personal workspace was created for; unique per (`org_id`, user) (partial index); the only way sign-in finds a personal workspace |

### `instance_placement`
(phase 4, P4-ONB-1, D-140) The instance admin's new-user placement (`04 §New-user placement`). At most one row; no row = unsaved (behaves as `personal_org` under `open`).
| Column | Type | Notes |
|---|---|---|
| singleton | boolean pk default true CHECK (singleton) | one row |
| mode | text CHECK in (`shared_org`,`personal_org`,`none`) | |
| default_org_id | uuid null FK→organizations (set null) | where `shared_org` users land; deleting the org leaves the mode set with no org, which behaves as `none` |
| default_org_role | text CHECK in (`member`,`admin`) default `member` | |
| shared_workspaces | jsonb not null default `[]` | `[{ workspace_id, role: member|approver }]`, workspaces of the default org |
| personal_enabled | boolean not null default false | |
| personal_role | text CHECK in (`owner`,`approver`,`member`) default `owner` | the user's role in their personal workspace |
| name_template | text not null | `{variable}` text; `\|` separates the fallback chain (`04 §New-user placement`) |
| starter_grants | jsonb not null default `[]` | `[{ kind: secret|connection, id }]`, org-scope credentials of the default org |
| updated_by | uuid null FK→users (set null) | `updated_at` is implicit (`§Conventions`) |

### `user_onboarding`
(phase 4, P4-ONB-2, D-141) Per-user onboarding progress: Welcome seen, dismissed guides, tour. Only what cannot be computed is stored; guide step completion is derived from real state (model readiness, grants, conversations) and never stored.
| Column | Type | Notes |
|---|---|---|
| user_id | uuid pk FK→users (cascade) | |
| welcome_seen | boolean not null default false | the Welcome page was opened (or skipped); people who existed when the migration ran start at `true` (they are not newcomers) |
| guide_dismissed | uuid[] not null default `{}` | workspaces whose getting-started guide the person dismissed (≤ 200) |
| tour_completed_at | timestamptz null | when the navigation tour was finished or skipped; `null` = not yet |

### `workspace_access`
| Column | Type | Notes |
|---|---|---|
| workspace_id | uuid FK→workspaces (cascade) | pk part |
| user_id | uuid FK→users (cascade) | pk part |
| role | text CHECK in (`owner`,`approver`,`member`) | `approver` = chats and decides approvals (`04 §The access law`); the migration did **not** change existing rows (members stay members) (phase 2, RFE-012) |
| idp_role | text null CHECK in (`owner`,`approver`,`member`) | (phase 4, P4-IDP-1, D-139) the role the IdP claims give; `null` = manual binding. `role` is the effective role |
| idp_source | text null | `idp:<provider>:<value>`; `null` exactly when `idp_role` is `null` |
| base_role | text null CHECK in (`owner`,`approver`,`member`) | the manual role under a claim-raised one; `null` when the claim created the row |

### `groups`
(phase 4, P4-GRP-1, D-130) Directory groups: flat, per org (`04 §Groups`). A table with no rows changes nothing about the access law.
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid not null FK→organizations (restrict) | unique `(id, org_id)` and `(id, org_id, source)` so child tables can carry a same-org composite foreign key |
| name | citext not null CHECK 1–80 chars | sanitized by the API; unique `(org_id, name)` |
| description | text null CHECK ≤ 280 | |
| source | text not null default `manual` CHECK in (`manual`,`scim`,`idp`) | immutable; `scim` groups are created and filled by the IdP (`04 §SCIM`), their name and membership are read-only in the dashboard; (D-156) `idp` groups are created in the dashboard and their membership is synced from a sign-in claim (`04 §IdP-linked groups`) |
| idp_provider | text null | (D-156) set together with `idp_value` iff `source = idp` (CHECK); unique `(org_id, idp_provider, idp_value)` |
| idp_value | text null CHECK ≤ 256 | (D-156) the claim value this group mirrors |
| org_role | text null CHECK in (`admin`,`member`) | the group → org-role mapping; `owner` is not a legal value (`04 §Groups`) |
| revision | bigint not null default 1 | optimistic concurrency (`If-Match`) |
| created_by | uuid null FK→users (set null) | null for SCIM-created groups |

### `group_members`
(phase 4, P4-GRP-1) A group's members. Created only for users who already hold an `org_members` row of the group's org.
| Column | Type | Notes |
|---|---|---|
| group_id | uuid not null | pk part |
| org_id | uuid not null | denormalized; composite FKs `(group_id, org_id, source)` → `groups(id, org_id, source)` (cascade) and `(org_id, user_id)` → `org_members(org_id, user_id)` (cascade): a member must be a member of the group's org, and leaving the org leaves every group |
| user_id | uuid not null FK→users (cascade) | pk part |
| source | text not null default `manual` CHECK in (`manual`,`scim`,`idp`) | who added the row; equals the group's source (a `manual` group takes only `manual` rows; (D-156) an `idp` group only `idp` rows, written by the sign-in sync; enforced by the composite foreign key) |
| synced_at | timestamptz null | (D-156) `idp` rows: the sign-in that last confirmed the membership |
| added_by | uuid null FK→users (set null) | |
| index | `(org_id, user_id)`, `(user_id)`; primary key `(group_id, user_id)` | the index answers "which groups is this user in" (the access law, the gateway) |

### `group_workspace_access`
(phase 4, P4-GRP-1) A group's workspace bindings.
| Column | Type | Notes |
|---|---|---|
| group_id | uuid not null | pk part |
| workspace_id | uuid not null | pk part; composite FKs `(group_id, org_id)` → `groups(id, org_id)` (cascade) and `(workspace_id, org_id)` → `workspaces(id, org_id)` (cascade; unique index `workspaces_id_org_uq`): a group binds only workspaces of its own org |
| org_id | uuid not null | denormalized for the composite foreign keys |
| role | text not null CHECK in (`owner`,`approver`,`member`) | same vocabulary and meaning as `workspace_access.role` |
| granted_by | uuid null FK→users (set null) | |
| index | `(workspace_id)` | "who has access to this workspace" |

### `api_keys`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid FK→organizations (restrict) | |
| workspace_id | uuid null FK→workspaces (cascade) | null ⇒ org key (`auo_`), else workspace key (`auk_`) |
| user_id | uuid FK→users (cascade) | owner; key dies when owner is not an active member |
| kind | text CHECK in (`user`,`service`) | service keys are used by platform features and hidden from personal-key lists |
| name | text not null | |
| token_hash | bytea unique | |
| display_prefix | text not null | |
| last_used_at | timestamptz null | churn column, ≤1 write/min |
| expires_at | timestamptz null | |
| revoked_at | timestamptz null | |

### `cli_logins`
Device-style CLI login.
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| device_code_hash | bytea unique | polled by CLI |
| user_code | text unique | 8 chars shown to the person (`ABCD-EFGH`) |
| status | text CHECK in (`pending`,`approved`,`denied`,`expired`,`consumed`) | |
| approved_by | uuid null FK→users (cascade) | |
| workspace_id | uuid null FK→workspaces (cascade) | chosen at approval |
| api_key_id | uuid null FK→api_keys (cascade) | minted on consume (nothing reversible is stored), token returned once; spec fix S-04 |
| hostname | text null | sanitized client hostname from `POST /v1/auth/cli` (key name `CLI on <hostname>`); spec fix S-04 |
| expires_at | timestamptz | 10 min |

### `audit_log` — *no FKs*
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid null | |
| workspace_id | uuid null | |
| actor_kind | text CHECK in (`user`,`service`,`agent`,`system`,`instance_admin`) | |
| actor_id | uuid null | |
| actor_label | text not null | denormalized email/name |
| action | text not null | dotted, e.g. `secret.create`, `plugin.approve`, `approval.decide` |
| target_kind | text null | |
| target_id | text null | |
| outcome | text CHECK in (`success`,`failure`,`denied`) | |
| source | text CHECK in (`dashboard`,`api`,`cli`,`channel`,`system`) | |
| ip | inet null | |
| metadata | jsonb not null default `{}` | never contains secret values |
| critical | boolean not null default false | (phase 2, RFE-008) true for `outcome = success` rows of the security-critical set (`04 §Audit`); derived from the action by the insert, never set by callers |
| index | (org_id, created_at desc), (workspace_id, created_at desc), (org_id, critical, created_at desc) | |

Audit writes are best-effort and never fail the action (logged on failure). (phase 2, RFE-008) Except for the **security-critical set** of `04 §Audit`, whose rows are written in the same transaction as the change and make the action fail if they cannot be written.

---

## 2. Secrets, LLM keys, connections, OAuth clients (owner docs: `08`, `09`)

### `secrets`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid FK→organizations (restrict) | |
| workspace_id | uuid null FK→workspaces (cascade) | null ⇒ org-scoped |
| name | text not null | unique per scope (`(org_id, workspace_id, name)` with nulls-not-distinct) |
| kind | text CHECK in (`generic`,`llm`) | |
| llm_provider | text null | provider id (`litellm`, `anthropic`, …); required iff kind=`llm` |
| llm_auth_mode | text null | provider-defined auth mode id (e.g. `api_key`, `oauth_token`) |
| llm_endpoint_id | text null | operator-configured endpoint id (required for `litellm`) |
| host_pattern | text not null | generic: user-supplied; llm: derived from provider/endpoint |
| path_pattern | text null | |
| injection | jsonb not null | `InjectionRecipe` (see `08 §Injection recipes`) |
| value_source | text CHECK in (`managed`,`reference`) | managed = Aurora wrote the value; reference = points at an existing external path |
| value_ref | text not null | SecretRef; `pending:` while `status = proposed` |
| status | text CHECK in (`active`,`proposed`,`rejected`) default `active` | agent proposals (`08 §Agent-proposed custom secrets`); only `active` rows are candidates |
| origin | jsonb null | LLM secrets only: key-origin verification result (`08 §11.3`) |
| value_version | int not null default 1 | bumped on every value change |
| proposed_by_agent_id | uuid null FK→agents (set null) | agent-proposed custom connection |
| created_by | uuid null FK→users (set null) | |
| last_used_at | timestamptz null | churn column (gateway, ≤1 write/min/secret) |

### `app_connections`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid FK→organizations (restrict) | |
| workspace_id | uuid null FK→workspaces (cascade) | null ⇒ org-scoped |
| app_id | text not null | immutable app id from the manifest |
| package_id | text not null | |
| app_version | text not null | package version that created/last reconnected it |
| label | text not null | sanitized; auto-filled from probe metadata |
| status | text CHECK in (`active`,`reauth_required`,`disabled`,`orphaned`) | |
| status_reason | text null (≤ 300 chars) | open vocabulary; a plugin major upgrade sets `plugin_major_upgrade: <package> <old> -> <new>; missing: <names>` (`09 §10`, ≤300 chars) |
| credentials_ref | text not null | SecretRef of the credential JSON (fields per manifest `credentials`) |
| credential_keys | text[] null | (phase 3, RFE-022) **names** (never values) of the fields present in the sealed credential JSON; maintained by the API/gateway wherever credentials are sealed (connect, reconnect, refresh, mint). Null = unknown (legacy rows: the upgrade check derives the names from the manifest of the row's `app_version`, else fails closed). Used by the plugin major-upgrade check (`09 §10`). The gateway role's column-level `UPDATE` list includes it |
| credentials_version | bigint not null default 1 | CAS authority for refresh/mint writes |
| credentials_expires_at | timestamptz null | access-token expiry (refresh trigger) |
| scopes | text[] not null default `{}` | granted scopes |
| requested_scopes | text[] not null default `{}` | scopes requested at authorize time; `missing_scopes = requested_scopes − scopes` (I-33) |
| metadata | jsonb not null default `{}` | non-secret, plugin-declared keys only |
| tenant_host | text null | for tenant-bound hosts |
| oauth_client_id | uuid null FK→app_oauth_clients (restrict) | which client minted it |
| instance_id | uuid null FK→app_instances (restrict) | (phase 2, D-74) set for a connection of a registered self-hosted instance; null for `default_host` and every other app. The gateway injects it only on that instance's exact host. Deleting an instance first disables its connections (`status_reason = instance_deleted`) and clears this column and `oauth_client_id` in the same transaction; such a connection cannot be re-enabled |
| source_connection_id | uuid null FK→app_connections (cascade) | derived connections die with their source |
| connected_by | uuid null FK→users (set null) | |
| reauth_required_at | timestamptz null | set by gateway on `invalid_grant` |
| last_used_at | timestamptz null | churn column |
| index | (org_id, workspace_id, app_id) | |

### `app_oauth_clients` (BYOC)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid null FK→organizations (restrict) | null ⇒ instance-level client (instance admins) |
| workspace_id | uuid null FK→workspaces (cascade) | requires org_id |
| package_id | text not null | |
| client_key | text not null | manifest `oauth_clients[].id` |
| fields | jsonb not null | non-secret fields (client id, tenant…) |
| secret_ref | text not null | SecretRef of secret fields JSON |
| enabled | bool not null default true | |
| instance_id | uuid null FK→app_instances (cascade) | (phase 2, D-74) a BYOC client bound to one self-hosted instance; its templated OAuth URLs resolve to that host |
| unique | (org_id, workspace_id, package_id, client_key, instance_id) nulls not distinct | one client per scope, key and instance |

### `app_instances` (self-hosted apps; phase 2, D-74)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid null FK→organizations (cascade) | null ⇒ instance-wide (instance admins) |
| package_id | text not null | |
| app_id | text not null | an app that declares `instances` (`09 §3.4`) |
| host | text not null | lower-case DNS name; no IP, port, path or userinfo (CHECK mirrors the API validation) |
| label | text not null | 1–120 characters |
| status | text CHECK in (`active`,`disabled`) default `active` | `disabled` stops injection without deleting |
| created_by | uuid null FK→users (set null) | |
| unique | (org_id, app_id, host) nulls not distinct | a host is additionally unique across scopes that can see each other (instance-wide vs any org); enforced by the API in the creating transaction |
| index | (host); (org_id) | |

---

## 3. Plugin registry (owner doc: `09`)

### `trusted_keys`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| publisher | text not null | must equal manifest `package.publisher` |
| name | text not null | |
| public_key | text unique not null | ed25519, base64 |
| added_by | uuid FK→users (set null) | instance admin |
| revoked_at | timestamptz null | revocation disables every package signed by it |

The project's built-in publisher key (`aurora`) is compiled in and never stored.

### `plugin_packages`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| package_id | text not null | |
| version | text not null | semver |
| contract | int not null | manifest contract version |
| publisher | text not null | |
| digest | text not null | sha256 of the canonical package digest (see `09 §Package format`) |
| signer_key_id | text not null | `builtin` or `trusted_keys.id` |
| manifest | jsonb not null | parsed manifest |
| permissions | jsonb not null | normalized permissions block |
| source | text CHECK in (`builtin`,`directory`,`upload`) | |
| status | text CHECK in (`pending_approval`,`active`,`superseded`,`rejected`,`disabled`,`invalid`) | |
| status_reason | text null | open vocabulary; (phase 3, RFE-022) gains the value `breaks_connections`: a major upgrade held in `pending_approval` because it would set connections to `reauth_required` (`09 §10`); no schema change |
| violations | jsonb not null default '[]' | validation violations for status invalid; spec fix S-05 |
| approved_by | uuid null FK→users (set null) | |
| approved_at | timestamptz null | |
| unique | (package_id, version, digest) | |
| partial unique | (package_id) WHERE status = `active` | one active version per package |

### `plugin_blobs`
| Column | Type | Notes |
|---|---|---|
| digest | text pk | sha256 of the bytes |
| kind | text CHECK in (`hooks_wasm`,`icon`,`icon_dark`,`setup_guide`,`archive`) | |
| content | bytea not null | |

### `host_claim_decisions`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| claim_key | text unique not null | normalized `host_pattern + "|" + path_prefix` |
| claim_fingerprint | text not null | hash of the conflicting claims set; a change invalidates the decision |
| winner_package_id | text not null | |
| winner_app_id | text not null | |
| decided_by | uuid FK→users (set null) | |

### `registry_snapshots`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| contract | int not null | highest manifest contract the snapshot was compiled for |
| content_hash | text unique not null | sha256 of canonical JSON |
| snapshot | jsonb not null | see `09 §Registry snapshot` |
| compiled_by | text not null | API version |
| activated_at | timestamptz not null | |

Active snapshot for a consumer = row with the greatest `contract` the consumer
supports, then greatest `activated_at`.

### `org_app_availability`
| Column | Type | Notes |
|---|---|---|
| org_id | uuid FK→organizations (cascade) | |
| app_id | text | |
| workspace_id | uuid null FK→workspaces (cascade) | null ⇒ whole org |
| unique | (org_id, app_id, workspace_id) NULLS NOT DISTINCT | spec fix S-01 (was pk (org_id, app_id), contradicting `07 §App availability`) |

Only consulted when `organizations.app_posture = 'restricted'`.

---

## 4. Policy, grants, approvals (owner doc: `07`)

### `policy_generations`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid FK→organizations (restrict) | |
| workspace_id | uuid null FK→workspaces (cascade) | null ⇒ org scope |
| number | int not null | 1, 2, … per scope; unique (org_id, workspace_id, number) nulls not distinct |
| status | text CHECK in (`draft`,`published`,`superseded`) | |
| default_action | text CHECK in (`none`,`allow`,`block`) default `none` | the scope's Default Rule (credentialed requests) |
| uncredentialed_default | text CHECK in (`none`,`block`) default `none` | workspace scope only (CHECK: `none` when `workspace_id IS NULL`; the org scope uses org governance, `08 §11.6`) |
| uncredentialed_allowlist | jsonb not null default `[]` | `[{ "host": pattern, "path"?: pattern }]`, ≤500 entries |
| open_lane_posture | text CHECK in (`enabled`,`allowlist_only`,`disabled`) not null default `enabled` | (phase 1, RFE-003) the workspace's open-lane posture (`08 §11`, `06 §Open-lane admission and posture`); can only tighten. CHECK: `enabled` when `workspace_id IS NULL` (org scope uses `organizations.governance.egress.open_lane`) |
| draft_revision | bigint not null default 0 | optimistic concurrency for draft edits |
| published_by | uuid null FK→users (set null) | |
| published_at | timestamptz null | |
| partial unique | (org_id, workspace_id) WHERE status=`draft`; same WHERE status=`published` | |

### `policy_rules`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| generation_id | uuid FK→policy_generations (cascade) | |
| logical_id | uuid not null | stable across generations |
| priority | int not null | unique (generation_id, priority); lower = earlier |
| name | text not null | |
| description | text null | |
| enabled | bool not null default true | |
| identities | jsonb not null | `[Identity]` (empty = anyone) |
| targets | jsonb not null | `[Target]` (empty = anywhere) |
| action | text CHECK in (`allow`,`block`) | |
| require_approval | bool not null default false | allow only |
| rate_limit | jsonb null | `{ "count": int, "per": "minute"|"hour"|"day" }`; allow only |
| conditions | jsonb not null default `[]` | `[Condition]` |
| approval_timeout_secs | int null | (phase 2, RFE-011a) only with `require_approval`; 60 … `approvals.max_timeout_secs` (CHECK 60…14400, the instance maximum is checked at authoring); null = no preference (`07 §Approval deadlines`) |

Shapes of `Identity`, `Target`, `Condition`: `07 §Rule anatomy`.

### `grants`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| workspace_id | uuid FK→workspaces (cascade) | |
| agent_id | uuid FK→agents (cascade) | |
| connection_id | uuid null FK→app_connections (cascade) | exactly one of connection_id / secret_id |
| secret_id | uuid null FK→secrets (cascade) | |
| mode | text CHECK in (`full`,`custom`) null | connections only |
| allow_tools | text[] not null default `{}` | custom mode |
| ask_tools | text[] not null default `{}` | custom mode |
| ask_timeout_secs | int null | (phase 2, RFE-011a) custom connection grants with `ask_tools`; compiled into the derived ask rule as `approval_timeout_secs` (CHECK 60…14400; the instance maximum is checked at authoring) |
| created_by | uuid null FK→users (set null) | |
| unique | (agent_id, connection_id), (agent_id, secret_id) | |

### `derived_rules`
Rules compiled from grants and app blocklists; always live (not drafted).
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid FK→organizations (restrict) | |
| workspace_id | uuid null FK→workspaces (cascade) | |
| source_kind | text CHECK in (`grant`,`blocklist`) | |
| source_id | uuid not null | grant id or connection id |
| ordinal | int not null | order within the source |
| logical_id | uuid not null | stable while the source exists |
| rule | jsonb not null | same fields as `policy_rules` minus ids/priority (phase 2, RFE-011a: including `approval_timeout_secs`) |
| unique | (source_kind, source_id, ordinal) | |

### `approvals`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id, workspace_id, agent_id | uuid not null | no FK on agent (approval history survives); index (workspace_id, status) |
| conversation_id | uuid null | from context credential |
| turn_id | uuid null | |
| gateway_instance_id | uuid not null | holder |
| method | text not null | |
| host | text not null | |
| path | text not null | redacted (query values masked) |
| app_id | text null | |
| tool_id | text null | |
| connection_id | uuid null | |
| rule_logical_id | uuid null | |
| rule_name | text null | |
| summary | jsonb not null | `ApprovalSummary` (`09 §Approval summaries`) |
| batch_id | text null | ≤128 |
| batch_label | text null | ≤128 |
| batch_total | int null | |
| status | text CHECK in (`pending`,`approved`,`denied`,`expired`,`abandoned`) | |
| decided_by_user_id | uuid null | |
| decided_via | text null CHECK in (`dashboard`,`api`,`channel`) | |
| decided_at | timestamptz null | |
| context_label | text null | (phase 2, RFE-025) label of the BYO context credential that made the request, ≤128, sanitized; shown on the approval card (`08 §11.9`) |
| deadline | timestamptz not null | `created + effective timeout` (`07 §Approval deadlines`) |
| resolved_at | timestamptz null | (phase 2, RFE-011a) set on every terminal transition (approve, deny, expire, abandon); the stall clock restarts here (`07 §Approval deadlines`). Gateway role may write it with `status` |

### `approval_watchers`
(phase 2, RFE-011a) A live `approvals/stream` or `approvals/pending` connection; a *live delivery surface* (`07 §Approvals`).
| Column | Type | Notes |
|---|---|---|
| replica_id | uuid | API replica holding the connection; pk part |
| user_id | uuid FK→users (cascade) | pk part |
| workspace_id | uuid FK→workspaces (cascade) | pk part; the workspace whose approvals the connection watches |
| last_seen_at | timestamptz not null | churn column; live when within `APPROVAL_WATCHER_TTL`; index (workspace_id, last_seen_at) |

### `approval_notifications`
(phase 2, RFE-011a) One row per (approval, decider) e-mail claim.
| Column | Type | Notes |
|---|---|---|
| approval_id | uuid | gateway or action approval id; pk part (no FK: approval history survives) |
| user_id | uuid FK→users (cascade) | pk part |
| kind | text CHECK in (`gateway`,`action`) | |
| sent_at | timestamptz not null default now() | insert-on-conflict claim makes the e-mail idempotent across replicas; pruned with the daily retention pruning; index (user_id, sent_at) |
| delivery | text not null default `mail` CHECK in (`mail`,`batch`,`digest_mail`,`digest`) | how the claim was served: its own e-mail; folded into the e-mail of its `batch_id`; the "N more waiting" digest e-mail; or folded silently into a digest already sent this hour (hourly cap `APPROVAL_EMAIL_PER_USER_HOUR`, D-81) |

### `request_log` — *no FKs*, range-partitioned by month on `ts`
| Column | Type | Notes |
|---|---|---|
| id | uuid | pk (id, ts) |
| ts | timestamptz not null | |
| org_id, workspace_id, agent_id | uuid | |
| conversation_id, turn_id | uuid null | |
| credential_kind | text | `agent`,`boot`,`context` |
| lane | text CHECK in (`mitm`,`open`,`db`) | |
| method | text | |
| host | text | |
| path | text | redacted |
| app_id, tool_id | text null | |
| connection_id, secret_id | uuid null | |
| decision | text | `allow`,`blocked`,`default_denied`,`egress_denied`,`governance_refused`,`rate_limited`,`approval_denied`,`approval_expired`,`approved`,`guard_refused`,`error` |
| rule_logical_id | uuid null | |
| rule_name | text null | denormalized; survives rule deletion |
| upstream_status | int null | |
| status | int not null | status returned to the agent |
| error_code | text null | |
| injected | smallint not null default 0 | number of credentials injected |
| latency_ms | int not null | |
| bytes_up, bytes_down | bigint not null default 0 | |
| context_label | text null | (phase 2, RFE-025) label of the BYO context credential (a labelled `aurora run`), ≤128, sanitized (`08 §11.9`) |
| speaker_user_id | uuid null | (phase 2, RFE-005) the request identity's speaker (no FK, ledger table); lets operators see which person's turn caused a request |

Partitions are created ahead by the API (next 2 months) and dropped after
`REQUEST_LOG_RETENTION`.

### `rate_limit_windows` (phase 4, P4-CNT, D-119; shared fixed-window counters)
`UNLOGGED` table (counters are not a system of record: a Postgres crash or a failover to a standby that lacks unlogged data resets windows, which at worst grants one extra window of quota and is listed in `17` residual risks). Used only when `ratelimit.counters.backend = postgres` (`15 §Shared rate-limit counters`); the migration creates it whatever the backend is (empty, no cost).

| Column | Type | Notes |
|---|---|---|
| key | text not null | `gw:rl:<rule logical_id>:<agent_id>:<period>`, `api:…` (`06 §Rate limiting`, `05 §Conventions`); ids and keyed hashes only, 1–200 chars |
| window_start | timestamptz not null | start of the UTC-aligned window, computed by the database (`date_bin(period, clock_timestamp(), 'epoch')`) |
| count | bigint not null | units spent in the window (≥ 0) |
| expires_at | timestamptz not null | `window_start + period + 1 minute`; rows past it are dead |

Primary key `(key, window_start)`; index `(expires_at)` for pruning.

Atomic consume (one statement per unit, all units of a call in **one transaction** with rows taken in key order, rolled back on the first refusal so the call consumes nothing): `INSERT … ON CONFLICT (key, window_start) DO UPDATE SET count = count + $cost WHERE count + $cost <= $limit RETURNING count`; no returned row means refused. Peek is a plain `SELECT`.

### `rate_limit_buckets` (phase 4, P4-CNT, D-119; shared token buckets of the management-API limiter)
`UNLOGGED`, same remarks as above. One row per bucket key, GCRA state.

| Column | Type | Notes |
|---|---|---|
| key | text pk | `api:general:user:<id>`, `api:class:<name>:key:<id>`, `api:org:<id>`, `api:ip:<hash>` (`05 §Conventions`); ids and keyed hashes only, 1–200 chars |
| tat | timestamptz not null | theoretical arrival time of the next unit; a take is allowed when `max(tat, now) + emission_interval − now ≤ burst × emission_interval` (all of a call's buckets locked `FOR UPDATE` in key order inside one transaction and updated only if every bucket allows) |
| expires_at | timestamptz not null | `tat` plus 60 s; idle buckets are pruned |

Index `(expires_at)` for pruning.

Pruning: the API's `counters_prune` due-work arm (every 60 s, any replica, `FOR UPDATE SKIP LOCKED`, batches of 5 000 rows, only rows with `expires_at < now()`) deletes dead rows from both tables. The gateway database role has `SELECT, INSERT, UPDATE` on `rate_limit_windows` (and nothing on `rate_limit_buckets`: the gateway keeps only rule counters); it never deletes. Neither table holds an e-mail address, an IP address or a secret (`05 §Conventions`).

### `action_approvals`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| workspace_id | uuid FK→workspaces (cascade) | |
| agent_id | uuid FK→agents (cascade) | |
| conversation_id | uuid null FK→conversations (set null) | |
| turn_id | uuid null | |
| action_key | text not null | e.g. `send_message` |
| payload | jsonb not null | frozen arguments |
| summary | text not null | platform-composed one line |
| status | text CHECK in (`pending`,`approved`,`executed`,`failed`,`rejected`,`expired`) | |
| decided_by_user_id | uuid null | |
| decided_at, executed_at | timestamptz null | |
| error | text null | |
| expires_at | timestamptz not null | 24 h |

### `agent_links`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| agent_a_id | uuid FK→agents (cascade) | `agent_a_id < agent_b_id` (CHECK) |
| agent_b_id | uuid FK→agents (cascade) | |
| policy_a | text CHECK in (`ask`,`allow`,`blocked`) | decided by a person with access to a's workspace |
| policy_b | text CHECK in (`ask`,`allow`,`blocked`) | |
| revision | bigint not null default 0 | |
| unique | (agent_a_id, agent_b_id) | |

### `peer_tasks`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| link_id | uuid FK→agent_links (cascade) | |
| from_agent_id, to_agent_id | uuid FK→agents (cascade) | |
| requester_user_id | uuid null FK→users (set null) | |
| origin_conversation_id | uuid FK→conversations (cascade) | where the report goes |
| task_conversation_id | uuid FK→conversations (cascade) | conversation on the receiving agent |
| status | text CHECK in (`queued`,`open`,`completed`,`cancelled`,`expired`) | |
| messages_used | int not null default 0 | |
| message_budget | int not null | |
| report | text null | |
| expires_at | timestamptz not null | |
| partial unique | (from_agent_id, to_agent_id) WHERE status=`open` | |

---

## 5. Agents, gateway credentials (owner doc: `10`, `08`)

### `agents`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid FK→organizations (restrict) | |
| workspace_id | uuid FK→workspaces (cascade) | |
| handle | text not null | unique (workspace_id, handle) |
| name | text not null | display name and the identity the agent states |
| kind | text CHECK in (`byo`,`hosted`) | immutable |
| is_default | bool not null default false | the workspace's Daneel; partial unique (workspace_id) WHERE is_default |
| harness | text null | hosted: adapter id (`claude-code`) |
| model_provider | text null | stamped with the provider of `model_id` (`08 §Model selection`) |
| model_endpoint_id | text null | LLM endpoint id from configuration |
| model_id | text null | provider's model id; null ⇒ endpoint/provider default |
| proactive_messaging | text CHECK in (`ask`,`allow`,`off`) default `ask` | `13 §Messaging tools` |
| effort | text null CHECK in (`low`,`medium`,`high`,`max`) | |
| brief | text not null default `''` | ≤20 000 chars |
| avatar_blob_key | text null | |
| avatar_public_key | text null | unguessable, rotating; gates the public avatar GET; partial index (avatar_public_key) WHERE avatar_public_key IS NOT NULL |
| resource_overrides | jsonb null | `ResourceProfile` (instance admins only) |
| home_sync_generation | bigint not null default 0 | desired projection generation |
| created_by | uuid null FK→users (set null) | |
| byo_source_cidrs | text[] null | (phase 2, RFE-025) BYO only (CHECK `kind = 'byo'` when set); 1–32 CIDR strings (`cidr` syntax, no `/0`); null = inherit org/instance default list (`08 §11.9`). Validated by the API; never empty |
| byo_require_context | bool not null default false | (phase 2, RFE-025) BYO only: credentialed requests must use a context credential (`08 §11.9`) |
| frozen_at | timestamptz null | (phase 2, RFE-006) as on `organizations` |
| frozen_by_user_id | uuid null FK→users (set null) | (phase 2, RFE-006) |
| frozen_reason | text null | (phase 2, RFE-006) |
| frozen_authority | text null CHECK in (`workspace_owner`,`org_admin`,`instance_admin`) | (phase 2, RFE-006) |

### `gateway_credentials`
All tokens the gateway accepts.
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| kind | text CHECK in (`agent`,`boot`,`context`) | |
| token_hash | bytea unique | |
| display_prefix | text not null | |
| agent_id | uuid FK→agents (cascade) | |
| boot_id | uuid null | kind=boot |
| conversation_id | uuid null FK→conversations (cascade) | kind=context |
| turn_id | uuid null | kind=context (hosted) |
| context_label | text null | kind=context (BYO), ≤128, sanitized |
| speaker_user_id | uuid null | (phase 2, RFE-005) kind=`context` hosted only: the person who authored the turn (`turns.speaker_user_id` when `speaker_kind = person`); null for every other credential. No FK (the resolver also requires the speaker to still hold `can_use`). Policy `user` identities match only this (`07`) |
| capabilities | text[] not null default `{}` | (phase 1, RFE-003) credential capabilities; phase 1 value `browser` (open-lane use), settable on `boot` credentials only (`08 §7`) |
| expires_at | timestamptz null | agent: null unless an effective maximum age is configured, then `created + max age` (phase 3, RFE-013, `08 §7`); boot: null until stop; context: turn end + grace (BYO: capped by the agent token's) |
| last_used_at | timestamptz null | (phase 3, RFE-013) churn column: set by the gateway on successful authentication and by the API on agent-token routes, ≤1 write/min/credential; drives the stale-token views. Gateway role has `UPDATE (last_used_at)` |
| revoked_at | timestamptz null | |
| index | (agent_id, kind); partial (created_at desc, id desc) WHERE kind = 'agent' AND revoked_at IS NULL (phase 3) | |

Invariant: at most one non-revoked `agent` credential per agent.

---

## 6. Conversations, turns, transcript, attachments (owner doc: `10`)

### `conversations`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| agent_id | uuid FK→agents (cascade) | |
| workspace_id | uuid FK→workspaces (cascade) | |
| kind | text CHECK in (`direct`,`group`,`schedule`,`webhook`,`watch`,`peer`,`api`) | |
| external_ref | text not null | source-specific stable key; unique (agent_id, kind, external_ref) |
| direct_user_id | uuid null FK→users (cascade) | kind=direct; partial unique (agent_id, direct_user_id) WHERE kind=`direct` |
| title | text null | |
| origin_conversation_id | uuid null FK→conversations (set null) | where reports go (schedule/webhook/peer) |
| harness_session_ref | text null | opaque, adapter-owned |
| harness_session_epoch | bigint not null default 0 | bumped to force a fresh session |
| last_seq | bigint not null default 0 | incremented atomically per durable event |
| app_turn_streak | int not null default 0 | bot-authored consecutive turns |
| last_activity_at | timestamptz not null | churn column |
| archived_at | timestamptz null | hidden from default lists; a person message unarchives (`10 §Archive, delete, redaction, retention, export`); phase 2, RFE-010 |
| archived_by_user_id | uuid null FK→users (set null) | phase 2, RFE-010 |

### `turns`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| conversation_id | uuid FK→conversations (cascade) | |
| agent_id | uuid FK→agents (cascade) | |
| role | text CHECK in (`turn`,`followup`) | |
| status | text CHECK in (`queued`,`dispatched`,`running`,`done`,`failed`,`aborted`,`joining`,`joined`,`promoted`) | `turn` rows use the first six; `followup` rows use `joining`/`joined`/`promoted` |
| parent_turn_id | uuid null FK→turns (cascade) | follow-up's live turn |
| door | jsonb not null | `{ "kind": "web"|"api"|"thread"|"schedule"|"webhook"|"watch"|"peer"|"system", "thread_link_id"?: uuid, "ref"?: string }` |
| speaker_kind | text CHECK in (`person`,`bot`,`agent`,`system`) | |
| speaker_user_id | uuid null FK→users (set null) | |
| speaker_label | text not null | sanitized, denormalized |
| text | text not null | ≤ `TURN_TEXT_MAX` |
| source_event_id | text null | partial unique (conversation_id, source_event_id); (phase 3, RFE-018) value convention, no schema change: turns and follow-ups created by `POST …/messages` with an `Idempotency-Key` store `api:<user_id>:<key>` (≤ 170 chars); connector event ids starting with `api:` are rejected (`13`); promoted turns keep null (`10 §Turns`) |
| abort_requested_at | timestamptz null | |
| send_now_requested_at | timestamptz null | |
| error_code | text null | open vocabulary |
| error_message | text null | |
| usage | jsonb null | `{ input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, model, provider }` |
| dispatched_at, started_at, finished_at | timestamptz null | |
| last_progress_at | timestamptz null | churn column |
| dispatch_lease_until | timestamptz null | claim clock (dispatch) |
| revived_at | timestamptz null | fence: a cold-dead dispatch is revived at most once |
| ceiling_warned_at | timestamptz null | single-purpose clock |
| steer_delivered_at | timestamptz null | follow-ups: single-purpose clock |
| promoted_at | timestamptz null | follow-ups |
| boot_id | uuid null | boot that ran it |
| last_event_n | int not null default 0 | highest applied supervisor event number (`16 §W5` idempotency) |
| partial unique | (conversation_id) WHERE role=`turn` AND status IN (`queued`,`dispatched`,`running`) | one active turn per conversation |

### `turn_answer_buffers`
| Column | Type | Notes |
|---|---|---|
| turn_id | uuid pk FK→turns (cascade) | |
| msgs | jsonb not null default `[]` | answer-rule buffer `[{id,text,reply_to?}]` (text after the turn's last tool call, `10 §Transcript events`); written with each `turn.events` batch, deleted when `turn.result` emits the durable `text` events; spec fix FUP-M4.1 |

### `transcript_events`
| Column | Type | Notes |
|---|---|---|
| conversation_id | uuid FK→conversations (cascade) | pk part |
| seq | bigint | pk part; = conversation `last_seq` after atomic increment |
| turn_id | uuid null | |
| type | text not null | canonical vocabulary (`10 §Transcript events`) |
| payload | jsonb not null | bounded (≤64 KiB serialized) |
| redacted_at | timestamptz null | set when the payload was replaced by the redaction marker `{ "redacted": true, "redacted_at", "redacted_by", "reason"? }` (≤64 KiB bound unchanged); phase 2, RFE-010 |
| redacted_by_user_id | uuid null FK→users (set null) | phase 2, RFE-010 |

### `session_purges`
Pending deletions of harness session files after a conversation delete or a `reset_session` redaction (phase 2, RFE-010; `12 §Session purge`).
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| agent_id | uuid FK→agents (cascade) | |
| conversation_id | uuid null | the conversation that is gone (no FK) |
| session_ref | text not null | `harness_session_ref` to purge |
| harness | text null | (phase 4, P4-HAR, D-153) the harness that created the session; the supervisor deletes the files with **that** harness's adapter. `NULL` = the sandbox's own harness (every row before a switch) |
| leased_until | timestamptz null | live-delivery lease: the `session.purge` work item is re-sent after it lapses |
| done_at | timestamptz null | stamped on `session.purged { ok: true }`; index (agent_id) WHERE done_at IS NULL |

### `attachments`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| workspace_id | uuid FK→workspaces (cascade) | |
| conversation_id | uuid FK→conversations (cascade) | |
| turn_id | uuid null FK→turns (set null) | bound turn |
| direction | text CHECK in (`in`,`out`) | |
| status | text CHECK in (`pending`,`bound`,`failed`) | |
| filename | text not null | sanitized, path-safe |
| content_type | text not null | |
| size_bytes | bigint not null | |
| sha256 | text not null | |
| blob_backend | text not null | `postgres` (phase 0) |
| blob_key | text not null | partial index (blob_key) WHERE blob_key <> '' |
| caption | text null | |
| failure_reason | text null | |
| created_by_user_id | uuid null | |
| purged_at | timestamptz null | bytes deleted (retention or explicit delete); the row stays for the transcript chip, `blob_key` is cleared and content GET answers 410; partial index (created_at) WHERE purged_at IS NULL AND status <> 'failed'; phase 2, RFE-015 |
| purge_reason | text null CHECK in (`retention`,`deleted`) | phase 2, RFE-015 |

`failure_reason` gains the open-vocabulary value `quota_exceeded` (phase 2, RFE-015). A row **holds bytes** while `purged_at IS NULL AND status <> 'failed'`; only those rows count towards `org_storage_usage`.

### `org_storage_usage`
Transactional counter of an org's attachment bytes (phase 2, RFE-015; `10 §Attachments`). Maintained by database triggers on `attachments` (insert, purge, delete, including cascades), locked `FOR UPDATE` by the quota check, reconciled daily from the table.
| Column | Type | Notes |
|---|---|---|
| org_id | uuid pk FK→organizations (cascade) | |
| attachment_bytes | bigint not null default 0 | sum of `size_bytes` of the org's rows that hold bytes (`updated_at` is implicit, `§Conventions`) |

### `blobs`
Phase 0 blob backend (`postgres`).
| Column | Type | Notes |
|---|---|---|
| key | text pk | |
| content | bytea not null | |

---

## 7. Memory and skills (owner doc: `10`)

### `memories`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| agent_id | uuid FK→agents (cascade) | |
| key | text not null | `[a-z0-9]+(-[a-z0-9]+)*`, ≤64, immutable; unique (agent_id, key) |
| title | text null | ≤120 |
| description | text null | ≤240 (index line) |
| content | text not null | ≤ `MEMORY_CONTENT_MAX` |
| revision | int not null | latest revision number |

### `memory_revisions`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| memory_id | uuid FK→memories (cascade) | |
| revision | int not null | unique (memory_id, revision) |
| op | text CHECK in (`save`,`restore`) | |
| author_kind | text CHECK in (`user`,`agent`) | |
| author_user_id | uuid null | |
| author_label | text not null | |
| title, description | text null | |
| content | text not null | |
| provenance | jsonb not null default `{}` | `{ conversation_id?, turn_id?, restored_from? }` (dangling ok) |
| redacted_at | timestamptz null | content replaced by `[redacted]` in place |
| redacted_by | uuid null | |

### `skills`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid FK→organizations (restrict) | |
| agent_id | uuid null FK→agents (cascade) | set ⇒ agent-tier skill; null ⇒ org skill, or a workspace skill when `workspace_id` is set |
| workspace_id | uuid null FK→workspaces (cascade) | (phase 3, RFE-020, D-64) non-null ⇒ **workspace-tier** skill (`10 §Skills`). CHECK `skills_one_tier`: `workspace_id IS NULL OR agent_id IS NULL` (agent skills keep it null; their workspace is the agent's). Tier = agent (`agent_id` set) \| workspace (`workspace_id` set) \| org (both null). Deleting the workspace deletes its skills |
| name | text not null | `[a-z0-9]+(-[a-z0-9]+)*` ≤64, immutable; `aurora-*` reserved |
| description | text not null | single line ≤500 |
| body | text not null | `SKILL.md` body |
| enabled | bool not null default true | |
| created_by_kind | text CHECK in (`user`,`agent`) | |
| library_id | uuid null FK→skill_libraries (set null) | (phase 4, P4-GSL, D-148) non-null ⇒ the skill was added from a git skill library and is **read-only** (`10 §Skills`); set null when the library is deleted (the skill is then detached; the service also nulls `library_blobs` and deletes the pending update in the same transaction) |
| library_label, library_repo, library_path | text null | (phase 4, P4-GSL) provenance snapshot (survives library deletion and detach) |
| library_commit_sha | text null | (phase 4, P4-GSL) the commit the stored content was read from (the pin) |
| library_blobs | jsonb null | (phase 4, P4-GSL) `{ "<relative path>": "<blob id>" }` for `SKILL.md` and every file, used to detect change without refetching |
| library_content_sha256 | bytea null | (phase 4, P4-GSL) SHA-256 of the canonical content (description, body, then files sorted by path with the executable bit) |
| library_pinned_at, library_pinned_by_user_id | timestamptz null, uuid null FK→users (set null) | (phase 4, P4-GSL) who added or last approved |
| check | `skills_library_cols`: `library_id IS NULL OR (library_path IS NOT NULL AND library_commit_sha IS NOT NULL AND library_blobs IS NOT NULL AND library_content_sha256 IS NOT NULL)` | |
| unique | (agent_id, name) where agent_id not null; (workspace_id, name) where workspace_id not null (`skills_workspace_name_uq`); (org_id, name) where agent_id null and workspace_id null (`skills_org_name_uq`, replaced by the P3-020 migration) | |
| index | (workspace_id) where workspace_id not null (`skills_workspace_idx`) | effective-set computation |
| index | (library_id) where library_id not null (`skills_library_idx`) | (phase 4, P4-GSL) |

A skill is **library-sourced and read-only iff `library_id IS NOT NULL`**. A **detach** (a person) and
a **library deletion** both set `library_id = NULL`, `library_blobs = NULL` and delete the skill's
pending update, and keep the text provenance columns as an informational "was from <repo>@<sha>" note:
the skill becomes an ordinary editable skill of its tier. The tiers, uniqueness and indexes of `skills`
are unchanged; the home-sync generation moves exactly as for any skill change.

Home-sync generation (`12 §Sync protocol`, I-32): a change to a workspace-tier skill
or its files bumps `workspaces.home_sync_generation` in the same transaction (part
of every agent's desired generation, so it reaches every agent of that workspace and no
other workspace); `skill_files` is unchanged.

### `skill_files`
| Column | Type | Notes |
|---|---|---|
| skill_id | uuid FK→skills (cascade) | pk part |
| path | text | pk part; relative, validated |
| content | bytea not null | |
| executable | bool not null default false | |

### `skill_libraries` (phase 4, P4-GSL, D-148, D-149; git skill library)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid FK→organizations (cascade) | |
| label | text not null | 1..80, unique per org (`skill_libraries_label_uq`) |
| connection_id | uuid null FK→app_connections (set null) | **org-scoped** connection (service-enforced at create and at every refresh); set null when the connection is deleted → `status = error`, `last_error = connection_deleted` |
| app_id | text not null | the git-hosting app (snapshot id) at creation, for the dashboard |
| repo | text not null | validated against the package's `repo_pattern` |
| ref | text null | validated ref; null = the repository's default branch |
| path | text not null default `` | directory holding the skill directories, ≤ 4 segments |
| status | text not null default `active` CHECK in (`active`,`paused`,`error`,`over_budget`) | `paused` = an admin stopped refreshing; `error` and `over_budget` are still refreshed (the job re-checks them) |
| last_head_sha, last_refresh_at, next_refresh_at | text null, timestamptz null, timestamptz | churn columns; `next_refresh_at` includes jitter and error backoff (15 min → 1 h → 6 h). `last_head_sha` is the head the last refresh *compared* (an add of a commit other than that head clears it) |
| refresh_failures | integer not null default 0 | consecutive failed refreshes: drives the backoff (reset by a success) (D-154) |
| last_error | text null | sanitized, ≤ 200 chars, never a token or a URL with credentials |
| refresh_lease_until, lease_owner | timestamptz null, uuid null | the per-library lease of the refresh job (`LIBRARY_LEASE`) |
| check_requested_at | timestamptz null | the last "Check now" (at most once per `LIBRARY_CHECK_NOW_INTERVAL`, 429 `library_check_too_soon`) (D-154) |
| created_by_user_id | uuid null FK→users (set null) | |
| unique | (org_id, label) (`skill_libraries_label_uq`) | |
| index | (org_id); (next_refresh_at) where status in (active, error, over_budget) | |

### `skill_library_updates` (phase 4, P4-GSL, D-148; proposed updates)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| skill_id | uuid FK→skills (cascade) | at most one `pending` per skill (partial unique index) |
| library_id | uuid null FK→skill_libraries (set null) | |
| kind | text CHECK in (`changed`,`removed`,`invalid`) | |
| status | text CHECK in (`pending`,`applied`,`rejected`,`superseded`) | |
| commit_sha | text not null | the proposed commit |
| proposed | jsonb null | for `changed`: `{ body, description, files: [{ path, content_b64, executable }], blobs }` validated; bounded by the skill limits; null for `removed`/`invalid` |
| content_sha256 | bytea null | what an approval must quote: SHA-256 of the canonical proposed state (`changed`), or of the marker (`removed`) or of the blob set and reason (`invalid`): the same repository state hashes the same, which is also how a rejection is remembered |
| reason | text null | sanitized validation failure for `invalid` |
| bytes | integer not null default 0 | size of `proposed` in bytes, summed for the per-org open-proposal budget `LIBRARY_PENDING_BYTES_PER_ORG` (D-154) |
| decided_at, decided_by_user_id | timestamptz null, uuid null FK→users (set null) | |
| unique | (skill_id) where status = `pending` (`skill_library_updates_pending_uq`) | |
| index | (library_id); (decided_at) where status <> `pending` | |

Decided rows older than 90 days are pruned by the daily retention job (the audit row remains). The
gateway database role gets nothing on these tables. Deleting an agent, workspace or org deletes its
skills and, with them, their updates; deleting an org deletes its libraries.

---

## 8. Schedules, webhooks, processes, watches (owner doc: `10`)

### `schedules`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| agent_id | uuid FK→agents (cascade) | |
| name | text not null | |
| prompt | text not null | ≤20 000 |
| cron | text not null | 5-field |
| timezone | text not null | IANA |
| gate_command | text null | ≤2 000 |
| gate_description | text null | ≤200 |
| enabled | bool not null default true | |
| disabled_reason | text null CHECK in (`user`,`authorization`,`failures`) | |
| consecutive_failures | int not null default 0 | |
| creator_user_id | uuid null FK→users (set null) | fire-time authority anchor |
| origin_conversation_id | uuid null FK→conversations (set null) | |
| run_conversation_id | uuid null FK→conversations (set null) | created lazily |
| next_fire_at | timestamptz null | |
| fire_lease_until | timestamptz null | claim clock |
| last_fire_at | timestamptz null | |
| last_outcome | text null CHECK in (`ok`,`failed`,`skipped_busy`,`gated_quiet`,`skipped_frozen`) | `skipped_frozen` (phase 2, RFE-006): fire skipped because the agent is effectively frozen (`10 §Schedules`); `schedule_runs.outcome` (open text) records `skipped_frozen` too |
| anchor | text not null default `creator` CHECK in (`creator`,`workspace`) | fire-time authority: the creator's access (`creator`) or "an active owner exists" (`workspace`); `creator_user_id` stays as provenance (`04 §Fire-time authority`) |

### `schedule_runs`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| schedule_id | uuid FK→schedules (cascade) | |
| turn_id | uuid null | |
| outcome | text not null | |
| fired_at | timestamptz not null | |

### `webhooks`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| agent_id | uuid FK→agents (cascade) | |
| name | text not null | |
| instructions | text not null | |
| token_hash | bytea unique | `auw_` |
| display_prefix | text not null | |
| verification | jsonb null | `{ "kind": "hmac_sha256", "header": "X-Signature", "secret_ref": SecretRef, "prefix"?: "sha256=" }` |
| enabled | bool not null default true | |
| disabled_reason | text null CHECK in (`user`,`authorization`,`failures`) | |
| consecutive_failures | int not null default 0 | |
| creator_user_id | uuid null FK→users (set null) | |
| origin_conversation_id | uuid null FK→conversations (set null) | |
| run_conversation_id | uuid null FK→conversations (set null) | |
| anchor | text not null default `creator` CHECK in (`creator`,`workspace`) | fire-time authority: the creator's access (`creator`) or "an active owner exists" (`workspace`); `creator_user_id` stays as provenance (`04 §Fire-time authority`) |

### `webhook_deliveries`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| webhook_id | uuid FK→webhooks (cascade) | |
| turn_id | uuid null | |
| status | text CHECK in (`accepted`,`rejected`,`paused`) | |
| size_bytes | int not null | |
| reason | text null | |

### `processes`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | supervisor-assigned uuid |
| agent_id | uuid FK→agents (cascade) | |
| sandbox_id | uuid FK→sandboxes (cascade) | |
| boot_id | uuid not null | |
| name | text not null | |
| command | text not null | |
| status | text CHECK in (`running`,`exited`,`lost`,`parked`) | |
| busy | bool not null default false | churn column |
| exit_code | int null; signal text null | |
| tail | text not null default `''` | ≤ `PROCESS_TAIL` |
| last_output_at | timestamptz null | churn column |
| started_at, ended_at | timestamptz | |

### `watches`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| process_id | uuid FK→processes (cascade) | |
| agent_id | uuid FK→agents (cascade) | |
| conversation_id | uuid FK→conversations (cascade) | where the fire lands |
| kind | text CHECK in (`exit`,`output`,`silence`) | |
| pattern | text null | literal substring |
| silence_secs | int null | |
| status | text CHECK in (`armed`,`fired`,`lost`,`parked`,`expired`,`cancelled`) | |
| fired_turn_id | uuid null | |
| expires_at | timestamptz not null | |

---

## 9. Runners and sandboxes (owner doc: `11`)

### `runners`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| name | text unique not null | |
| token_hash | bytea unique | `aur_` |
| installation_fingerprint | text not null | sha256(token) hex prefix; labels infra objects |
| backend | text null | declared at registration |
| capabilities | jsonb null | `RunnerCapabilities` (`16`) |
| last_registered_at | timestamptz null | |

### `runner_replicas`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| runner_id | uuid FK→runners (cascade) | |
| advertised_host | text not null | sandbox-facing address |
| version | text not null | |
| started_at | timestamptz not null | |
| last_seen_at | timestamptz not null | churn column; poll = heartbeat |
| retired_at | timestamptz null | clean shutdown |

### `sandboxes`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| agent_id | uuid unique FK→agents (cascade) | |
| runner_id | uuid null FK→runners (set null) | placement |
| state | text CHECK in (`unprovisioned`,`starting`,`running`,`stopping`,`stopped`,`failed`) | |
| owner_replica_id | uuid null | lifecycle owner (fence) |
| channel_replica_id | uuid null | replica holding the supervisor socket |
| lifecycle_lease_until | timestamptz null | claim clock for start/stop |
| boot_id | uuid null | current/last boot |
| container_ref | text null | |
| home_ref | text null | |
| bootstrap_token_hash | bytea null | `aup_`; single use |
| bootstrap_used_at | timestamptz null | |
| stop_reason | text null CHECK in (`idle`,`cap`,`evict`,`restart`,`delete`,`failure`,`frozen`) | `frozen` (phase 2, RFE-006): agent effectively frozen (`11`) |
| stop_requested_at | timestamptz null | |
| restart_requested_at | timestamptz null | |
| last_engaged_at | timestamptz null | person/turn engagement clock |
| last_active_at | timestamptz null | stamped also by every boot |
| home_applied_generation | bigint not null default 0 | reset to 0 on each start claim |
| listening_ports | int[] not null default `{}` | churn column |
| failure_code | text null; failure_message text null | |
| resource_profile | jsonb null | effective profile of current boot |
| start_failures | int not null default 0 | consecutive failed starts (backoff); reset on running; spec fix S-10 |
| start_attempt_at | timestamptz null | when the current (or last) start attempt was claimed; the attempt's clock origin; reset by every new boot (phase 4, FIX-025) |
| start_phase | text null | the last phase the runner reported (`claimed`, `waiting_capacity`, `provisioning`, `scheduling`, `creating`, `booting`, `ready`) (phase 4, FIX-025) |
| start_detail | text null | the phase's detail as the runner reported it (sanitised) (phase 4, FIX-025) |
| start_events | jsonb not null default `'[]'` | the pod's Warning events (bounded, newest first) (phase 4, FIX-025) |
| start_progress_at | timestamptz null | when the runner last reported progress (phase 4, FIX-025) |
| running_at | timestamptz null | when the runner reported `running` for the current boot (phase 4, FIX-025) |
| start_log | jsonb not null default `'[]'` | the boot's start log: a ring of ≤ 30 entries `{ts, kind: "phase"\|"note", phase?, detail?}` (≤ 300 chars per detail, sanitised, 8 KiB total, oldest dropped), reset by every new boot (phase 4, FIX-029, C-612) |

### `sandbox_start_failures` (per-runner start failures for Admin › Runners; phase 4, FIX-025, D-158, C-587)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| runner_id | uuid null FK→runners (cascade) | null only for a legacy row (the runner is recorded on every write) |
| sandbox_id | uuid not null FK→sandboxes (cascade) | |
| agent_id | uuid not null | denormalized for the admin list |
| boot_id | uuid null | the attempt's boot |
| code | text not null | machine reason (C-581) |
| message | text not null | sanitised human reason |
| phase | text null | the phase the attempt failed in |
| events | jsonb not null default `'[]'` | the pod's Warning events |
| elapsed_secs | int null | seconds the attempt ran before it failed |
| index | (runner_id, created_at desc); (created_at) | the admin list (`C-586`); pruning |

### `runtime_intervals` — *no FKs*
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id, workspace_id, agent_id, sandbox_id | uuid | |
| boot_id | uuid unique | |
| started_at | timestamptz not null | control-plane clock |
| ended_at | timestamptz null | partial unique (sandbox_id) WHERE ended_at IS NULL |
| end_reason | text null | |

### `runtime_daily` — *no FKs*
| Column | Type | Notes |
|---|---|---|
| org_id, agent_id | uuid | pk part |
| day | date | pk part (UTC) |
| awake_seconds | bigint not null | rebuildable from intervals |

---

## 10. Channels (owner doc: `13`)

### `connectors`
| Column | Type | Notes |
|---|---|---|
| id | text pk | connector id, e.g. `mattermost` (`[a-z][a-z0-9-]{1,31}`) |
| token_hash | bytea null unique | `aun_`; null for a managed connector (CHECK: `managed` or not null) |
| display_name | text not null | from manifest, sanitized |
| version | text null | announced |
| contract | int null | announced |
| manifest | jsonb null | latest announced |
| manifest_digest | text null | |
| approved_manifest_digest | text null | |
| status | text CHECK in (`awaiting_registration`,`pending_approval`,`active`,`disabled`) | |
| approved_by | uuid null; approved_at timestamptz null | |
| managed | bool not null default false | (phase 4, P4-MCON) added from the dashboard and run by a connector host (`13 §3.1`) |
| type_id | text null | managed only: the built-in type (= `id` in phase 4) |
| settings | jsonb null | managed only: non-secret `instance_setup` values (≤ 16 KiB), validated on write |
| secret_owner | uuid not null default gen_random_uuid() | owner id of the sealed settings (purpose `connector.settings`, org = nil UUID) |
| secret_refs | jsonb not null default '{}' | field name → sealed reference; never a value |
| secret_updated_at | jsonb not null default '{}' | field name → RFC 3339 time the sealed value was last written (the `updated_at` of `secrets`, C-452) |

### `connector_hosts`
| Column | Type | Notes |
|---|---|---|
| instance_id | uuid pk | host process instance |
| version | text not null | |
| types | jsonb not null | `[{type_id, manifest, manifest_digest}]` with verified signatures only |
| last_seen_at | timestamptz not null | heartbeat (`register`); churn column; rows older than 24 h are pruned |

### `connector_host_tokens`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| token_hash | bytea unique not null | `auh_`, from `AURORA_BOOTSTRAP_CONNECTOR_HOST_TOKEN` |

### `connector_host_status`
| Column | Type | Notes |
|---|---|---|
| connector_id | text fk → connectors (cascade) | pk with `host_instance` |
| host_instance | uuid | |
| state | text CHECK in (`running`,`error`) | |
| message | text null CHECK (char_length ≤ 300) | sanitized; never a secret; `updated_at` is a churn column (set on every report) |

The assignments revision (`13 §3.1`, C-456) is the sequence `connector_assignments_rev`, advanced with
`nextval` in the transaction of every managed connector change.

### `connector_replicas`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| connector_id | text FK→connectors (cascade) | |
| version | text not null | |
| last_seen_at | timestamptz not null | churn column |
| retired_at | timestamptz null | |

### `channel_integrations`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid FK→organizations (restrict) | |
| connector_id | text FK→connectors (restrict) | |
| tenant_ref | text not null | platform tenant id; unique (connector_id, tenant_ref) |
| tenant_label | text not null | |
| config | jsonb not null | non-secret setup fields, incl. `verified_email_autolink` (bool, default `false`; phase 1, RFE-001): org-admin opt-in to create `verified_email` user links from the connector's asserted `email`/`email_verified` (`13 §5.9`); requires the `email_verified` capability |
| credentials_ref | text not null | SecretRef |
| credentials_version | bigint not null default 1 | CAS for `credentials.update` |
| shared_bot | jsonb null | `{ "platform_user_ref", "handle", "display_name" }` |
| status | text CHECK in (`validating`,`active`,`needs_attention`,`disabled`) | |
| status_reason | text null | |
| owner_replica_id | uuid null | churn column |
| owner_lease_until | timestamptz null | churn column |
| created_by | uuid null FK→users (set null) | |

### `channel_presences`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| agent_id | uuid FK→agents (cascade) | |
| integration_id | uuid FK→channel_integrations (cascade) | unique (agent_id, integration_id) |
| mode | text CHECK in (`dedicated`,`shared`) | |
| platform_user_ref | text null | dedicated: bot user id; unique (integration_id, platform_user_ref) |
| handle | text null | |
| credentials_ref | text null | dedicated only |
| credentials_version | bigint not null default 1 | |
| status | text CHECK in (`pending_setup`,`active`,`disabled`,`needs_attention`) | |
| status_reason | text null | |

Presences are served by the replica that owns their integration (no
presence-level ownership).

### `space_bindings`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| integration_id | uuid FK→channel_integrations (cascade) | |
| space_ref | text not null | |
| space_label | text not null | |
| agent_id | uuid FK→agents (cascade) | |
| listen | text CHECK in (`mentions`,`all`) default `mentions` | |
| followups_need_mention | bool not null default false | |
| active | bool not null default true | |
| bound_by_user_id | uuid null FK→users (set null) | must be a workspace owner at bind time |
| partial unique | (integration_id, space_ref) WHERE active | |

### `dm_defaults`
| Column | Type | Notes |
|---|---|---|
| integration_id | uuid FK→channel_integrations (cascade) | pk part |
| platform_user_ref | text | pk part |
| agent_id | uuid FK→agents (cascade) | |

### `user_links`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| integration_id | uuid FK→channel_integrations (cascade) | |
| platform_user_ref | text not null | unique (integration_id, platform_user_ref) |
| user_id | uuid FK→users (cascade) | |
| method | text CHECK in (`verified_email`,`sign_in_link`,`admin`) | |

### `link_requests`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| integration_id | uuid FK→channel_integrations (cascade) | |
| platform_user_ref | text not null | |
| token_hash | bytea unique | `aui_` |
| expires_at | timestamptz | 30 min |
| used_at | timestamptz null; user_id uuid null | |
| platform_email_hint | citext null | (phase 1, RFE-001) email the platform asserted for the person; only pre-fills the sign-in (`login_hint`) and the confirm page; never authorizes |

### `thread_links`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| integration_id | uuid FK→channel_integrations (cascade) | |
| agent_id | uuid FK→agents (cascade) | |
| conversation_id | uuid FK→conversations (cascade) | |
| kind | text CHECK in (`direct`,`group`) | |
| space_ref | text not null | |
| thread_ref | text not null | `''` when the platform has no threads |
| sender | jsonb not null | `{ "mode": "dedicated", "presence_id" } | { "mode": "shared" }` |
| revision | bigint not null default 0 | |
| unique | (integration_id, agent_id, space_ref, thread_ref) | |

### `reach_grants`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| agent_id | uuid FK→agents (cascade) | |
| integration_id | uuid FK→channel_integrations (cascade) | |
| subject_kind | text CHECK in (`space`,`person`) | |
| subject_ref | text not null | unique (agent_id, integration_id, subject_kind, subject_ref) |
| subject_label | text not null | |
| state | text CHECK in (`pending`,`approved`,`blocked`,`left`,`expired`) | |
| listen | text CHECK in (`mentions`,`all`) default `mentions` | |
| revision | bigint not null default 0 | optimistic concurrency |
| decided_by_user_id | uuid null | |
| decided_at | timestamptz null; expires_at timestamptz null | |

### `mention_anchors`
| Column | Type | Notes |
|---|---|---|
| conversation_id | uuid FK→conversations (cascade) | pk part |
| alias | text | pk part (normalized) |
| integration_id | uuid FK→channel_integrations (cascade) | |
| kind | text CHECK in (`user`,`space`,`bot`) | |
| ref | text not null | |
| label | text not null | |

### `channel_inbox`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| connector_id | text not null | |
| integration_id | uuid null | |
| event_id | text not null | platform event id; unique (connector_id, event_id) |
| kind | text not null | |
| payload | jsonb not null | |
| status | text CHECK in (`pending`,`processing`,`done`,`failed`,`expired`) | |
| attempts | int not null default 0 | |
| next_attempt_at | timestamptz not null | |
| lease_until | timestamptz null | claim clock |
| outcome | jsonb null | `{ "kind": "turn"|"refused"|"ignored"|"binding"|…, "turn_id"? }` |
| error | text null | |

### `channel_ops`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| connector_id | text not null | |
| integration_id | uuid null FK→channel_integrations (cascade) | `NULL` for `presence.deprovision` (so the cascade of `channel_integrations` cannot delete it); any live replica of the connector may claim it (`13 §4.2`) |
| presence_id | uuid null | no FK |
| kind | text not null | contract op kind (`13 §Operations`) |
| payload | jsonb not null | for `presence.deprovision` (RFE-024): `{ presence_id, reason, platform_user_ref, handle, tenant_ref, tenant_label, config, credentials_ref, org_id }`; `credentials_ref` is a SecretRef sealed under the op id (`purpose channel_op_deprovision`, owner = op id, org = `org_id`), set to JSON `null` and discarded at the terminal state or by the pruning job; `credentials_ref` and `org_id` are internal and never sent to the connector (`13 §4.2`) |
| dedupe_key | text null unique | e.g. `answer:<turn_id>:<thread_link_id>` |
| correlation_id | uuid null | for synchronous waits |
| status | text CHECK in (`pending`,`claimed`,`done`,`failed`,`dropped`) | |
| attempts | int not null default 0 | |
| not_before | timestamptz not null default now() | backoff |
| lease_until | timestamptz null | claim clock |
| claimed_by_replica | uuid null | |
| result | jsonb null | |
| error_code | text null | neutral error |
| completed_at | timestamptz null | |
| index | (connector_id, status, not_before) | |
| index | (kind, status) WHERE kind = `presence.deprovision` AND (status IN (`pending`,`claimed`) OR the sealed `credentials_ref` is still set) | the expiry and pruning job finds deprovision ops past `DEPROVISION_RETRY_WINDOW` and finished ops whose sealed copy was not yet discarded (RFE-024) |

### `channel_cards`
Posted interactive messages, so they can be updated/settled.
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| subject_kind | text CHECK in (`approval`,`action_approval`,`reach`,`binding`,`link`,`agent_link`,`connection_request`) | |
| subject_id | uuid not null | index (subject_kind, subject_id) |
| integration_id | uuid FK→channel_integrations (cascade) | |
| post_op_id | uuid not null | |
| space_ref, thread_ref | text not null | |
| message_ref | text null | filled from the op result |
| state | text CHECK in (`posting`,`posted`,`settled`,`failed`) | |

### `action_token_uses`
| Column | Type | Notes |
|---|---|---|
| jti | uuid pk | single use |
| used_at | timestamptz not null | |
| expires_at | timestamptz not null | pruned after expiry |

---

## 11. Instance (owner doc: `05`, `06`)

### `gateway_instances`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | per process start |
| hostname | text not null | |
| version | text not null | |
| snapshot_hash | text null | churn column |
| started_at | timestamptz | |
| last_seen_at | timestamptz not null | churn column (every 15 s) |
| stopped_at | timestamptz null | |

### `migrations` — managed by sqlx (`_sqlx_migrations`).

## Gateway database role grants

When `database.gateway_role` is configured, `aurora-api migrate` grants that role (it does not
create it: the operator creates the login role, `15 §Postgres`; a missing role is an error).
The gateway then connects as that role and nothing else (Helm: `database.gatewayExistingSecret`).
The grants are:
`SELECT` on identity/tenancy tables it needs (`organizations`, `workspaces`,
`workspace_access`, `org_members`, `users` (id, email, disabled_at only via a
view `gateway_users`), `api_keys`), `agents`, `gateway_credentials`, `secrets`,
`app_connections`, `app_oauth_clients`, `app_instances`, (phase 4, P4-GRP-1) `groups`,
`group_members`, `group_workspace_access`, `grants`, `derived_rules`,
`policy_generations`, `policy_rules`, `registry_snapshots`, `plugin_packages`
(status only via view), `host_claim_decisions`, `org_app_availability`;
`INSERT, UPDATE` on `approvals`, `gateway_instances`, plus the column-level
`SELECT (id, status, gateway_instance_id)` on `approvals` and `SELECT (id)` on
`gateway_instances` (an `UPDATE … WHERE` needs `SELECT` on the columns it filters on, and
the gateway reads the hold's `status`; FIX-016); `INSERT` on
`request_log`; `UPDATE (credentials_ref, credentials_version,
credentials_expires_at, status, status_reason, reauth_required_at,
last_used_at, updated_at, credential_keys)` on `app_connections` (`updated_at` is set by credential
rotation and `reauth_required`, FIX-016; `credential_keys` by every reseal, RFE-022); `UPDATE (last_used_at, origin)` on `secrets`; `UPDATE (last_used_at)` on
`gateway_credentials`; (phase 4, P4-CNT) `SELECT, INSERT, UPDATE` on `rate_limit_windows` (never `DELETE`, nothing on `rate_limit_buckets`);
`EXECUTE` on `pg_advisory_lock` family; `LISTEN` is implicit.

---

## Planned changes (later phases)

This section lists schema and document-shape changes that later phases will
introduce. It is **normative for those phases** but is *not* part of the
phase-0 schema: the tables above are machine-checked against the migrations
(`scripts/extract-schema.py`), and this section deliberately uses level-4
headings and a different header so that the extractor ignores it. A migration
for a planned change lands with its phase; the corresponding rows move into the
tables above in the same change (and this section shrinks).

#### `attachments` (S3 blob backend)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| blob_backend | (existing column) | gains the value **`s3`** next to `postgres` (the Notes of §6 say `postgres (phase 0)`); no schema change. A row's bytes live in the backend it names, so both can coexist and `aurora-api blobs migrate` moves rows one by one (`10 §Attachments`) | 4 (P4-S3) |
| blob_key | (existing column) | for `s3` rows: `attachments/<org_id>/<attachment_id>` (the object key is `<blobs.s3.prefix>` + this); for `postgres` rows unchanged. Cleared on purge, as today | 4 (P4-S3) |
| failure_reason | (existing open vocabulary) | gains `blob_store_unavailable` (a connector file could not be stored) | 4 (P4-S3) |

`blobs` (the Postgres backend table), `agents.avatar_blob_key` (avatars) and `plugin_blobs` are unchanged and stay in Postgres. No new table: S3 state is the bucket. The `attachments` index on `blob_key` serves the S3 orphan sweep (rows are looked up by `(blob_backend, blob_key)`). `GET /v1/admin/storage` reports bytes and counts **per backend** from `attachments` (never by listing the bucket).

#### `secrets` (LLM providers of phase 4)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| llm_provider | (existing open vocabulary) | gains `openai`, `azure_openai`, `bedrock`, `vertex`, `openai_compatible` (`08 §Phase 4 providers`); no schema change | 4 (P4-LLM) |
| llm_auth_mode | (existing open vocabulary) | gains `aws_sigv4` (sealed JSON `{ access_key_id, secret_access_key, session_token? }`) and `gcp_service_account` (sealed service-account key JSON); `api_key` is unchanged | 4 (P4-LLM) |
| injection | (existing jsonb `InjectionRecipe`) | gains the LLM-only kinds `header`, `aws_sigv4 { region, service }`, `gcp_service_account_token { scope }`; fixed by the provider, read-only | 4 (P4-LLM) |
| origin | (existing jsonb) | for `bedrock` `organization_id` is the AWS account id; for `vertex` `workspace_id` is the GCP project id and `user_id` the service-account e-mail; for `openai` the organization and project of the key (`08 §Phase 4 providers`) | 4 (P4-LLM) |

#### `grants`, `plugin_packages` (app-level grant restrictions)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| grants.mode | (existing) | no schema change: a `full` row for a `custom_only` app cannot be written (`07`); activation of a package that introduces `custom_only` rewrites existing `full` rows to `custom` with `allow_tools` = the whole catalogue (`09 §10`) | 4 (P4-GRANTS) |
| plugin_packages.status_reason | (existing open vocabulary) | gains `converts_grants`: an upgrade held in `pending_approval` because it would convert full grants | 4 (P4-GRANTS) |
| registry snapshot `apps[].grants` | `{ custom_only, block_untooled }` booleans, in the snapshot JSON only | no table; copied from the manifest (`09 §9`). Tool entries may also carry `params` (Q8, `09 §3.4`) | 4 (P4-GRANTS) |

(P4-GRP-1 landed `groups`, `group_members` and `group_workspace_access`: see §1.)

#### `groups` (SCIM columns; phase 4)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| external_id | text null | SCIM `externalId` (≤ 256); only for `source = scim`; unique `(org_id, external_id)` where not null. Stored for the IdP's correlation; never used to find, link or authenticate anything | 4 (P4-GRP-2) |

#### `scim_tokens` (new table)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| id | uuid pk | | 4 (P4-GRP-2) |
| org_id | uuid not null FK→organizations (restrict) | at most `SCIM_TOKENS_PER_ORG` (5) with `revoked_at` null and unexpired | 4 (P4-GRP-2) |
| name | text not null | ≤ 80, sanitized; shown in audit as `scim:<name>` | 4 (P4-GRP-2) |
| token_hash | bytea unique not null | SHA-256 of the `aud_` token (`00`); the token is shown once | 4 (P4-GRP-2) |
| display_prefix | text not null | first 12 characters | 4 (P4-GRP-2) |
| created_by | uuid null FK→users (set null) | an org owner | 4 (P4-GRP-2) |
| expires_at | timestamptz not null | ≤ `SCIM_TOKEN_MAX_DAYS` after creation; there is no non-expiring token | 4 (P4-GRP-2) |
| revoked_at | timestamptz null | | 4 (P4-GRP-2) |
| last_used_at | timestamptz null | churn column, ≤ 1 write/min | 4 (P4-GRP-2) |
| created_at | timestamptz not null default now() | | 4 (P4-GRP-2) |

#### `scim_users` (new table; the link between an org's SCIM user and a global user)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| id | uuid pk | the SCIM `id` of the user (not the Aurora user id) | 4 (P4-GRP-2) |
| org_id | uuid not null FK→organizations (restrict) | unique `(org_id, user_id)` and `(org_id, external_id)` where not null | 4 (P4-GRP-2) |
| user_id | uuid not null FK→users (cascade) | the global user; never modified through this link once the user has an identity (`04 §SCIM`) | 4 (P4-GRP-2) |
| external_id | text null | ≤ 256, the IdP's id; correlation only | 4 (P4-GRP-2) |
| user_name | citext not null | the e-mail as asserted by the IdP (domain checked against the org's `email_domains`) | 4 (P4-GRP-2) |
| display_name, given_name, family_name | text null | ≤ 120 each, sanitized | 4 (P4-GRP-2) |
| state | text not null CHECK in (`active`,`suspended`,`removed`) | `removed` rows are tombstones that keep JIT and invitations from re-admitting the person (`04 §OIDC` note) | 4 (P4-GRP-2) |
| suspended_by_scim | bool not null default false | true only when SCIM made the membership `suspended`; reactivation by the IdP is refused when false (a human suspension wins) | 4 (P4-GRP-2) |
| removed_at | timestamptz null | | 4 (P4-GRP-2) |
| created_at, updated_at | timestamptz not null default now() | `meta.created` / `meta.lastModified` | 4 (P4-GRP-2) |

`organizations.settings.scim` (jsonb key, no column): `{ "enabled": bool, "email_domains": [domain], "exclusive": bool }`, default absent = SCIM off. The gateway role gets nothing on these tables. Token liveness is read on every SCIM request (indexed by `token_hash`); there is no cache.

`effective_access(user, workspace)` is one statement: the user's `org_members` row (active), the maximum of `org_members.role` and `groups.org_role` over the user's groups, the direct `workspace_access` row, and the maximum role over `group_workspace_access ⋈ group_members` for the workspace, returning the strongest role and the list of sources (`04 §Groups`). The gateway database role gains `SELECT` on `groups`, `group_members` and `group_workspace_access` (the speaker's group set is resolved at credential resolution, `06 §Credential authentication`); it never writes them. The `workspace_access` table, its CHECK and the `can_*` meaning are unchanged: a deployment without groups reads the same rows as before.

#### `approvals` (park and resume; phase 4)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| parked | bool not null default false | true for a parked approval: nobody holds the request (`07 §Park and resume`); excluded from the stale-holder sweep and from the stall exemption | 4 (RFE-011b) |
| fp_salt | bytea null | 16 random bytes; set iff `parked` | 4 (RFE-011b) |
| fingerprint | bytea null | `SHA-256(fp_salt ‖ canonical request)` (32 bytes), canonical form in `07`; never the request itself | 4 (RFE-011b) |
| resume_state | text null CHECK in (`ready`,`consumed`,`expired`,`revoked`) | null while `pending`/`denied`/`expired`; `ready` from approval until the window ends; consumed by exactly one request (compare-and-set) | 4 (RFE-011b) |
| resume_expires_at | timestamptz null | `decided_at + approvals.park.resume_window_secs` | 4 (RFE-011b) |
| consumed_at | timestamptz null | | 4 (RFE-011b) |
| resume_turn_id | uuid null | the system follow-up/turn queued at approval (tracing only; no FK, history survives) | 4 (RFE-011b) |
| indexes | `(agent_id, conversation_id) WHERE resume_state = 'ready'`; `(resume_expires_at) WHERE resume_state = 'ready'`; `(agent_id, conversation_id) WHERE parked AND status = 'pending'` | grant lookup, resume-window expiry, pending-per-agent limits and de-duplication | 4 (RFE-011b) |

The gateway database role's column-level `SELECT (id, status, gateway_instance_id)` on `approvals` gains `agent_id, conversation_id, parked, fp_salt, fingerprint, resume_state, resume_expires_at, method, host, path, rule_logical_id` and `UPDATE` stays as granted (it consumes with `resume_state`, `consumed_at`); it never writes `fingerprint`/`fp_salt` of a row it did not insert. Pending approval stays a closed `status` vocabulary: the resume lifecycle is `resume_state`, not a new status.

#### `approval_push_claims` (new table)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| approval_id | uuid | gateway or action approval id; pk part (no FK: history survives) | 4 (RFE-011b) |
| user_id | uuid FK→users (cascade) | pk part | 4 (RFE-011b) |
| stage | text CHECK in (`initial`,`reminder`) | pk part | 4 (RFE-011b) |
| kind | text CHECK in (`gateway`,`action`) | | 4 (RFE-011b) |
| delivery | text CHECK in (`push`,`batch`,`digest`) | its own push; folded into the push of its `batch_id`; folded into the hourly digest (`PUSH_PER_USER_HOUR`) | 4 (RFE-011b) |
| sent_at | timestamptz not null default now() | insert-on-conflict claim, idempotent across replicas; pruned daily with the other notification claims; index `(user_id, sent_at)` | 4 (RFE-011b) |

#### `webpush_vapid_keys` (new table; instance-wide)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| id | uuid pk | the `vapid_key_id` of subscriptions | 4 (RFE-011b) |
| public_key | text not null | base64url, 65-byte uncompressed P-256 point; the application server key | 4 (RFE-011b) |
| private_ref | text not null | SecretRef, purpose `webpush.vapid` (`08 §1`); owner id = this id, org = nil UUID; never returned | 4 (RFE-011b) |
| status | text CHECK in (`active`,`retired`) | exactly one `active` (partial unique index) | 4 (RFE-011b) |
| created_at / retired_at | timestamptz | `retired_at` set at rotation; destroyed after `PUSH_VAPID_RETIRE` or when unreferenced | 4 (RFE-011b) |

Absent when the operator supplies the key through `push.vapid.private_key_env` (nothing is stored). The gateway role has no access.

#### `push_subscriptions` (new table)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| id | uuid pk | | 4 (RFE-011b) |
| user_id | uuid not null FK→users (cascade) | at most `PUSH_SUBSCRIPTIONS_PER_USER` (10) | 4 (RFE-011b) |
| session_id | uuid not null FK→sessions (cascade) | the session that registered it; delivery only while that session is live (`07 §Web Push`) | 4 (RFE-011b) |
| vapid_key_id | uuid null FK→webpush_vapid_keys (restrict) | null with an operator-managed key | 4 (RFE-011b) |
| endpoint_hash | bytea unique not null | SHA-256 of the endpoint URL; one row per endpoint (re-registration under another user moves the row) | 4 (RFE-011b) |
| endpoint_host | text not null | for the allow-list check and metrics; not secret | 4 (RFE-011b) |
| secret_ref | text not null | SecretRef, purpose `webpush.subscription`, owner id = this id: sealed JSON `{ endpoint, p256dh, auth }` — the endpoint URL and keys are send capabilities and are never stored in clear or returned | 4 (RFE-011b) |
| label | text not null | ≤ 80, sanitized, user-visible (e.g. "Firefox on Linux") | 4 (RFE-011b) |
| show_details | bool not null default false | include agent and workspace names in the notification (`07 §Web Push`) | 4 (RFE-011b) |
| paused_at | timestamptz null | | 4 (RFE-011b) |
| expires_at | timestamptz null | the browser's `expirationTime` when given | 4 (RFE-011b) |
| failure_count | int not null default 0 | consecutive; 5 sets `disabled_at` | 4 (RFE-011b) |
| disabled_at | timestamptz null | | 4 (RFE-011b) |
| last_success_at, last_failure_at | timestamptz null | churn columns | 4 (RFE-011b) |
| created_at | timestamptz not null default now() | | 4 (RFE-011b) |
| index | `(user_id)`, `(session_id)` | delivery and cascade | 4 (RFE-011b) |

The gateway role has no access to any of these tables. Rows of revoked or expired sessions are removed by the daily session pruning (the foreign key cascades).

#### `preview_shares` (new table; port previews, phase 4, P4-PRV-1, D-144, D-145)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| id | uuid pk | | 4 (P4-PRV-1) |
| org_id | uuid FK→organizations (restrict) | denormalized for the per-org limit and the gateway-free authorization checks | 4 (P4-PRV-1) |
| workspace_id | uuid FK→workspaces (cascade) | | 4 (P4-PRV-1) |
| agent_id | uuid FK→agents (cascade) | hosted agents only (service-enforced); the share dies with the agent | 4 (P4-PRV-1) |
| port | int not null CHECK (port BETWEEN 1024 AND 65535) | unique `(agent_id, port)`; supervisor-reserved ports refused by the service | 4 (P4-PRV-1) |
| label | text not null | the hostname label, `p` + 24 base32 chars, **unique index**, immutable, never reused (`11 §Port previews`) | 4 (P4-PRV-1) |
| visibility | text not null default `private` CHECK in (`private`) | the only value in phase 4 | 4 (P4-PRV-1) |
| state | text not null default `active` CHECK in (`active`,`paused`) | | 4 (P4-PRV-1) |
| start_command | text null CHECK (length ≤ 1024 and no newline) | run on demand by the supervisor; never audited or sent to the relay | 4 (P4-PRV-1) |
| cwd | text null | relative to `/workspace`, no `..`, ≤ 200 | 4 (P4-PRV-1) |
| last_start_at, last_start_exit_code, last_start_tail | timestamptz null, int null, text null (≤ 4 KiB, sanitized, secrets redacted like transcripts) | for the dashboard (churn columns) | 4 (P4-PRV-1) |
| start_attempts | jsonb not null default `[]` | timestamps of the attempts inside `PREVIEW_START_WINDOW` (≤ 3) | 4 (P4-PRV-1) |
| wake_attempts | jsonb not null default `[]` | timestamps of the wakes inside the hour (≤ `PREVIEW_WAKES_PER_SHARE_HOUR`) | 4 (P4-PRV-1) |
| created_by_user_id | uuid null FK→users (set null) | label denormalized (`created_by_label`) | 4 (P4-PRV-1) |
| created_at, updated_at, last_accessed_at | timestamptz | `last_accessed_at` is written at most once a minute per share (relay heartbeat) | 4 (P4-PRV-1) |
| indexes | unique `(label)`; unique `(agent_id, port)`; `(org_id)`; `(workspace_id)` | | 4 (P4-PRV-1) |

The gateway database role gets nothing on this table (the gateway has no part in previews).
Deleting the agent, workspace or org deletes the shares (the org delete service removes them in
dependency order). The `previews` home-sync part is computed from this table (`16 §W5`).

#### `preview_sessions` (new table; phase 4, P4-PRV-2, D-146)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| id | uuid pk | | 4 (P4-PRV-2) |
| token_hash | bytea unique not null | SHA-256 of the `auv_` value; the value is returned once to the relay and never stored | 4 (P4-PRV-2) |
| display_prefix | text not null | first 12 characters, for support | 4 (P4-PRV-2) |
| share_id | uuid FK→preview_shares (cascade) | the one share (hostname) the credential is valid for | 4 (P4-PRV-2) |
| user_id | uuid FK→users (cascade) | | 4 (P4-PRV-2) |
| session_id | uuid FK→sessions (cascade) | the dashboard session that authorized it: signing out or revoking it deletes this row | 4 (P4-PRV-2) |
| created_at, last_seen_at, expires_at | timestamptz | `expires_at = created_at + PREVIEW_SESSION_TTL` (capped by the dashboard session's own absolute expiry); `last_seen_at` is advanced at most once a minute; idle past `PREVIEW_SESSION_IDLE` is dead | 4 (P4-PRV-2) |
| indexes | `(share_id)`, `(session_id)`, `(user_id)`, `(expires_at)` | | 4 (P4-PRV-2) |

#### `preview_grant_uses` (new table; single-use ledger of authorization grants)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| jti | uuid pk | insert-on-conflict claims a grant exactly once | 4 (P4-PRV-2) |
| expires_at | timestamptz not null | grant expiry + 60 s; pruned by the preview housekeeping job | 4 (P4-PRV-2) |

#### `preview_stream_grants` (new table; phase 4, P4-PRV-2, D-147)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| token_hash | bytea pk | SHA-256 of the `aux_` grant | 4 (P4-PRV-2) |
| share_id, sandbox_id, boot_id | uuid | the binding the runner receives; no FK (short-lived) | 4 (P4-PRV-2) |
| port | int not null | copied from the share at mint time | 4 (P4-PRV-2) |
| session_hash | bytea not null | the preview session it was minted for (for accounting only) | 4 (P4-PRV-2) |
| expires_at, consumed_at | timestamptz | ≤ 30 s; `consumed_at` set by one compare-and-set on redemption | 4 (P4-PRV-2) |

#### `relay_tokens`, `relay_replicas` (new tables; phase 4, P4-PRV-2)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| `relay_tokens.id`, `.token_hash` | uuid pk, bytea unique | `aul_`, from `AURORA_BOOTSTRAP_RELAY_TOKEN` (same shape as `connector_host_tokens`) | 4 (P4-PRV-2) |
| `relay_replicas.id` | uuid pk | the replica id returned by `register` | 4 (P4-PRV-2) |
| `relay_replicas.version`, `.last_seen_at`, `.streams_open` | text, timestamptz, int | heartbeat (`report`); churn columns; rows older than 24 h pruned | 4 (P4-PRV-2) |

The gateway role gets nothing on any preview table. Preview sessions are deleted by cascade when
their share, user or dashboard session goes; the `sessions` retention job's deletes cascade too.

#### `sandboxes` (preview columns; phase 4, P4-PRV-1)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| preview_wake_at | timestamptz null | a preview request found the sandbox stopped; the start arm treats it as due work for `PREVIEW_WAKE_WINDOW` (churn column) | 4 (P4-PRV-1) |
| preview_start_requested_at | timestamptz null | a request is waiting for the share's port; drives `preview.start` (churn column) | 4 (P4-PRV-1) |
| preview_active_until | timestamptz null | `now() + PREVIEW_AWAKE_GRACE` at each relay heartbeat that reports an open or recent stream; keep-awake reason `preview` | 4 (P4-PRV-1) |
| preview_awake_since | timestamptz null | first heartbeat of the current boot; the `PREVIEW_AWAKE_CAP` clock; reset at every start claim | 4 (P4-PRV-1) |

`organizations.settings.previews_enabled` (jsonb key, no column): bool, default absent = off
(`11 §Port previews`). The home-sync desired generation (`12 §Sync protocol`) already moves with
the agent's own generation; a share change bumps `agents.home_sync_generation`.

#### `sandboxes` (start diagnostics; phase 4, FIX-025, D-158, C-582)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| start_attempt_at | timestamptz null | when the current (or last) start attempt was claimed; the attempt's clock origin | 4 (FIX-025) |
| start_phase | text null | the last phase the runner reported (`claimed`, `waiting_capacity`, `provisioning`, `scheduling`, `creating`, `booting`, `ready`) | 4 (FIX-025) |
| start_detail | text null | the phase's detail as the runner reported it (sanitised) | 4 (FIX-025) |
| start_events | jsonb not null default `'[]'` | the pod's Warning events (bounded, newest first) | 4 (FIX-025) |
| start_progress_at | timestamptz null | when the runner last reported progress | 4 (FIX-025) |
| running_at | timestamptz null | when the runner reported `running` for the current boot | 4 (FIX-025) |
| start_log | jsonb not null default `'[]'` | (phase 4, FIX-029, D-160, C-612) the boot's start log: a ring of ≤ 30 entries `{ts, kind: "phase"\|"note", phase?, detail?}` (≤ 300 chars per detail, sanitised, 8 KiB total, oldest dropped), reset by every new boot | 4 (FIX-029) |

Reset by every new boot. A start failure also writes a `sandbox_start_failures` row (below). A retry of a failed start (C-610) clears the failure fields and resets `start_failures`; it never bypasses the ordinary start arm.

#### `terminal_stream_grants` (new table; single-use web-terminal grants, phase 4, P4-TERM, D-161, C-618)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| id | uuid pk | | 4 (P4-TERM) |
| agent_id | uuid not null | the terminal's agent | 4 (P4-TERM) |
| sandbox_id | uuid not null | the bound sandbox | 4 (P4-TERM) |
| boot_id | uuid not null | the bound boot (stale boot → redemption refused) | 4 (P4-TERM) |
| granted_by | uuid not null | the user whose session opened the tab | 4 (P4-TERM) |
| grant_hash | text not null unique | SHA-256 of the `auy_` grant (the value is never stored) | 4 (P4-TERM) |
| expires_at | timestamptz not null | `now() + TERMINAL_GRANT_TTL` (30 s) | 4 (P4-TERM) |
| consumed_at | timestamptz null | set by the single-use compare-and-set at redemption | 4 (P4-TERM) |
| created_at | timestamptz not null default now() | rows older than 24 h pruned on insert | 4 (P4-TERM) |
| indexes | `(grant_hash)`, `(expires_at)` | redemption, pruning | 4 (P4-TERM) |

#### `sandbox_start_failures` (new table; per-runner start failures for Admin › Runners; phase 4, FIX-025, D-158, C-587)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| id | uuid pk | | 4 (FIX-025) |
| runner_id | uuid null FK→runners (cascade) | null only for a legacy row (the runner is recorded on every write) | 4 (FIX-025) |
| sandbox_id | uuid not null FK→sandboxes (cascade) | | 4 (FIX-025) |
| agent_id | uuid not null | denormalized for the admin list | 4 (FIX-025) |
| boot_id | uuid null | the attempt's boot | 4 (FIX-025) |
| code | text not null | a start-failure code (`10 §Failure semantics`, C-581) | 4 (FIX-025) |
| message | text not null | the human reason, sanitised at write time | 4 (FIX-025) |
| phase | text null | the attempt's last phase | 4 (FIX-025) |
| events | jsonb not null default `'[]'` | the pod's Warning events | 4 (FIX-025) |
| elapsed_secs | int null | seconds the attempt ran before it failed | 4 (FIX-025) |
| created_at | timestamptz not null default now() | rows older than 30 days are pruned on insert | 4 (FIX-025) |
| indexes | `(runner_id, created_at DESC)` | the admin list (`C-586`) | 4 (FIX-025) |

#### `agents`, `conversations` (harness selection)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| harness | (existing text) | gains `codex` (open vocabulary, validated against `hosted.enabled_harnesses` on write); no schema change | 4 (P4-HAR) |
| harness_session_epoch | (existing bigint) | bumped on every conversation of an agent when its harness changes; `harness_session_ref` is also cleared, and a `session_purges` row is queued per old session (`12`) | 4 (P4-HAR) |

`turns.usage` and `agents.model_provider` already carry free-text provider and model values; nothing changes except that the API stamps `provider`, `endpoint_id` and `model` from the turn's `ModelChoice` rather than from harness-reported values.

