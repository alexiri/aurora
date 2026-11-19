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
| admin_attention_email_enabled | bool not null default true | (FIX-167 Part B, C-1024) per-user opt-out of the e-mail about new administration decisions; used for instance admins only (`PATCH /v1/me { admin_attention_email }`) |
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
| reauth_at | timestamptz null | last successful OIDC re-authentication (`04 §OIDC`); account deletion needed it within 10 min (phase 1, RFE-004); (phase 5, FIX-157, D-317) no operation reads it any more, the flow still stamps it |

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
| status | text CHECK in (`active`,`suspended`,`pending`) default `active` | suspended = non-member for every check. (phase 4, D-175, FIX-082) `pending` = prepared by SCIM for an existing account: counts against the seat cap, can be placed in SCIM groups, confers nothing (non-member for every check) and becomes `active` only in that person's next verified OIDC sign-in |
| idp_role | text null CHECK in (`owner`,`admin`,`member`) | (phase 4, P4-IDP-1, D-139) the role the IdP claims give; `null` = no claim-sourced role. `role` stays the effective role (the stronger of this and `base_role`) |
| idp_source | text null | `idp:<provider>:<value>` of the claim value behind `idp_role`; `null` exactly when `idp_role` is `null` |
| base_role | text null CHECK in (`owner`,`admin`,`member`) | the role the other sources gave before a claim raised it; restored when the claim goes; `null` when the claim created the row (the row goes with the claim) |

Invariant: every org has ≥1 active `owner` (enforced by the service; the last
owner cannot be demoted or removed).

### `org_member_removals`
(FIX-108, D-183) The removal marker: an admin removed this user from this org, so OIDC JIT provisioning does not restore the membership. Written by the admin removal, deleted in the same statement by the next `org_members` insert for the pair (invitation acceptance, admin add, role-claim sync, SCIM pending placement; FIX-140 closed the gap for role-claim sync). Only JIT consults it: an IdP role claim is the provider's explicit grant and is not blocked by it.
| Column | Type | Notes |
|---|---|---|
| org_id | uuid FK→organizations (cascade) | pk part |
| user_id | uuid FK→users (cascade) | pk part |
| removed_at | timestamptz not null default now() | |

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
| personal | boolean not null default false | (phase 4, FIX-097) sticky marker: set with `personal_owner_user_id` and kept when that id is nulled by the user's deletion; a personal workspace is never offered as a shared workspace (readers treat a row with an owner id as personal too) |

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

### `instance_setup`
(FIX-116, REVIEW-5 PLAT-4) The persisted fresh-instance gate of `04 §New-user placement`. One row, written by the migration.
| Column | Type | Notes |
|---|---|---|
| singleton | boolean pk default true CHECK (singleton) | one row |
| placement_pending | boolean not null | `true` when the migration found no organization and no saved placement (an instance installed with this release); cleared by the first saved placement (same statement as the `instance_placement` upsert) and never set again. Existing instances start `false` |

### `user_onboarding`
(phase 4, P4-ONB-2, D-141) Per-user onboarding progress: Welcome seen, dismissed guides, tour. Only what cannot be computed is stored; guide step completion is derived from real state (model readiness, grants, conversations) and never stored.
| Column | Type | Notes |
|---|---|---|
| user_id | uuid pk FK→users (cascade) | |
| welcome_seen | boolean not null default false | the Welcome page was opened (or skipped); people who existed when the migration ran start at `true` (they are not newcomers) |
| guide_dismissed | uuid[] not null default `{}` | workspaces whose getting-started guide the person dismissed (≤ 200) |
| tour_completed_at | timestamptz null | when the navigation tour was finished or skipped; `null` = not yet |
| notices_seen | text[] not null default `{renames-2026-11}` CHECK ≤ 50 | (phase 5, P5-IA-1, C-987) keys of one-time notices the person has dismissed, at most `ONBOARDING_NOTICES_MAX` (50). The migration gave every existing person a row with `{}` and then set the default, so only people who used the dashboard before see "Pages have new names"; a person without a row is answered as having seen it |

### `workspace_access`
| Column | Type | Notes |
|---|---|---|
| workspace_id | uuid FK→workspaces (cascade) | pk part |
| user_id | uuid FK→users (cascade) | pk part |
| role | text CHECK in (`owner`,`approver`,`member`) | `approver` = chats and decides approvals (`04 §The access law`); the migration did **not** change existing rows (members stay members) (phase 2, RFE-012) |
| idp_role | text null CHECK in (`owner`,`approver`,`member`) | (phase 4, P4-IDP-1, D-139) the role the IdP claims give; `null` = manual binding. `role` is the effective role |
| idp_source | text null | `idp:<provider>:<value>`; `null` exactly when `idp_role` is `null` |
| base_role | text null CHECK in (`owner`,`approver`,`member`) | the manual role under a claim-raised one; `null` when the claim created the row |
| status | text not null default `active` CHECK in (`active`,`suspended`) | (phase 5, P5-APIGAP-4, D-399, C-1051, SEC-138; migration `20261201000258_workspace_access_status.sql`) a workspace owner's suspension of this person **in this workspace only**. A `suspended` row grants nothing: the `effective_workspace_access` view drops the direct row **and** the person's group-derived rows for this workspace (an org owner/admin's implicit access is not affected; such a person cannot be suspended here). The binding, its role and the person's data are kept; `PUT` and the sign-in syncs change the role and never the status. `created_at` is the person's **Joined** date (C-1050) |

### `workspace_pending_access`
(phase 5, P5-UX-5, D-325, SEC-128, C-919, C-920; migration `20261201000249_workspace_pending_access.sql`) Access a workspace owner gave to an e-mail address that is not yet an active member of the organization (`04 §Workspace access by e-mail`). Applied at a sign-in and deleted; it never makes anyone an organization member. The gateway role gets nothing on this table.
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| workspace_id | uuid not null FK→workspaces (cascade) | |
| org_id | uuid not null FK→organizations (cascade) | the workspace's organization (denormalized for the sign-in lookup); index `(org_id, email_normalized)` |
| email_normalized | text not null | trimmed and lower-cased like `invitations.email`; compared for equality only; unique `(workspace_id, email_normalized)` |
| role | text not null CHECK in (`member`,`approver`) | a pending row never carries `owner` (SEC-128) |
| invited_by | uuid FK→users (set null) | the owner who added it |
| expires_at | timestamptz not null | `created_at + PENDING_ACCESS_TTL` (30 days); an index on it serves the sweeper |

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
| external_id | text null CHECK ≤ 256 | (phase 4, P4-GRP-2) the IdP's SCIM `externalId`; only a `source = scim` group carries one (CHECK); unique `(org_id, external_id)` where not null. Stored for the IdP's correlation; never used to find, link or authenticate anything |
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

### `scim_tokens` (phase 4, P4-GRP-2, D-132)
The hashed `aud_` provisioning tokens of one org (`04 §SCIM`, `16 §W15`). At most `SCIM_TOKENS_PER_ORG` (5) live tokens per org; the gateway role gets nothing on this table and token liveness is read on every SCIM request (indexed by `token_hash`; no cache).
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid not null FK→organizations (restrict) | |
| name | text not null CHECK 1–80 chars | sanitized; shown in audit as `scim:<name>` |
| token_hash | bytea not null UNIQUE | SHA-256 of the `aud_` token (`00`); the token is shown once at creation |
| display_prefix | text not null CHECK ≤ 12 chars | first 12 characters of the token |
| created_by | uuid null FK→users (set null) | an org owner |
| expires_at | timestamptz not null | ≤ `SCIM_TOKEN_MAX_DAYS` after creation; there is no non-expiring token; expiry notices at 14 and 1 day (`05 §Background jobs`) |
| revoked_at | timestamptz null | revocation is effective at once |
| last_used_at | timestamptz null | churn column: written at most once a minute, without touching `created_at` |
| index | `(org_id)` | the management list |

### `scim_users` (phase 4, P4-GRP-2, D-132)
The link between an org's SCIM user and a global `users` row: the SCIM `id` is the link's own id (never the Aurora user id, which would leak across orgs). `removed` rows are tombstones that keep JIT and invitations from re-admitting the person into this org (`04 §SCIM`, SEC-73). The gateway role gets nothing on this table.
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | the SCIM `id` of the user |
| org_id | uuid not null FK→organizations (restrict) | unique `(org_id, user_id)`; unique `(org_id, external_id)` where not null |
| user_id | uuid not null FK→users (cascade) | the global user; resolved by e-mail only, never by `externalId`; never modified through this link once the user has an identity (`04 §SCIM`) |
| external_id | text null CHECK ≤ 256 | the IdP's id; correlation only |
| user_name | citext not null | the e-mail as asserted by the IdP; the domain was checked against the org's `email_domains` when the link was written |
| display_name, given_name, family_name | text null CHECK ≤ 120 each | sanitized |
| state | text not null default `active` CHECK in (`active`,`suspended`,`removed`) | `removed` = tombstone (idempotent `DELETE`); `suspended` mirrors the org membership |
| suspended_by_scim | bool not null default false | true only when SCIM made the org_members row `suspended`: a human suspension is never lifted by the IdP (reactivation is refused when false) |
| removed_at | timestamptz null | |
| index | `(user_id)` | the sign-in gates (SEC-73) |

`created_at`/`updated_at` follow §Conventions; the SCIM resource surfaces them as `meta.created` / `meta.lastModified`.

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
| requested_addr | text null | (FIX-108) coarse address of the `POST /v1/auth/cli` request (IPv4 /24, IPv6 /48), shown on the approval screen; start time = `created_at` |
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
| index | (org_id, created_at desc), (workspace_id, created_at desc), (org_id, critical, created_at desc); (FIX-125) (created_at desc, id desc) and the same partial `WHERE org_id IS NULL` for the instance audit list | |

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
| last_check_at | timestamptz null | (phase 5, P5-UX-14, D-335, D-350, C-944) when the account's health was last recorded: by Test (C-943) or by the gateway from real use (C-985) |
| last_check_result | text null CHECK in (`ok`,`rejected`,`unreachable`,`unexpected`) | what was recorded then; `not_testable` is answered but never stored. Informational: it never changes what an agent may do. (P5-POLISH-3, C-1033) No longer written: backfilled into the columns below by migration `20261201000254`, kept until a later cleanup |
| last_ok_at | timestamptz null | (phase 5, P5-POLISH-3, D-383, C-1033) the last 2xx/3xx answer to an **agent** call through this account, written by the gateway (at most once a minute per account); backfilled from `last_check_at` where `last_check_result = 'ok'` |
| last_error_at | timestamptz null | the last failed agent call the gateway attributes to the account (a 401/403 answer); the state is `failing` while it is newer than `last_ok_at` |
| last_error | text null CHECK in (`rejected`,`unreachable`,`unexpected`,`expired`) | why; backfilled from a non-`ok` `last_check_result` |
| last_test_at | timestamptz null | the last **Test** (C-943); never written by the gateway's use path |
| last_test_result | text null CHECK in (`ok`,`rejected`,`unreachable`,`unexpected`) | what Test said; shown as "Test passed … ago", never as the account working |

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
| connect_method | text null | (FIX-123, D-195, D-214) id of the manifest `connect[]` method that created or last reconnected the connection; the gateway's refresh/mint paths use that method's `token` (falling back to the app's), tools restricted by `connect_methods` apply by it. Null = unknown (rows older than the column, where the package had several methods): the app's `token`, no tool hidden. Backfilled by migration `000110` from the recorded package version's single method |
| access | text not null default `write` CHECK in (`read`,`write`) | (FIX-133, D-240, C-740) `read` = the gateway allows only the app's `read` tools through this connection and blocks write tools and untooled requests before any rule (`07 §Read-only connections`). Chosen at connect time or changed by `PATCH`; existing rows are `write`; a reconnect without a value keeps it. Migration `20261201000160` |
| credentials_ref | text not null | SecretRef of the credential JSON (fields per manifest `credentials`) |
| credential_keys | text[] null | (phase 3, RFE-022) **names** (never values) of the fields present in the sealed credential JSON; maintained by the API/gateway wherever credentials are sealed (connect, reconnect, refresh, mint). Null = unknown (legacy rows: the upgrade check derives the names from the manifest of the row's `app_version`, else fails closed). Used by the plugin major-upgrade check (`09 §10`). The gateway role's column-level `UPDATE` list includes it |
| credentials_version | bigint not null default 1 | CAS authority for refresh/mint writes |
| credentials_expires_at | timestamptz null | access-token expiry (refresh trigger) |
| scopes | text[] not null default `{}` | granted scopes; (FIX-136, D-241) for a connection made with a `fields` method that declares scopes: the scopes the person declared (or the read probes found) for the pasted token, editable; the gateway refuses a tool needing others only when this is non-empty |
| requested_scopes | text[] not null default `{}` | scopes requested at authorize time; `missing_scopes = requested_scopes − scopes` (I-33) |
| metadata | jsonb not null default `{}` | non-secret, plugin-declared keys only |
| tenant_host | text null | for tenant-bound hosts |
| oauth_client_id | uuid null FK→app_oauth_clients (restrict) | which client minted it |
| instance_id | uuid null FK→app_instances (restrict) | (phase 2, D-74) set for a connection of a registered self-hosted instance; null for `default_host` and every other app. The gateway injects it only on that instance's exact host. Deleting an instance first disables its connections (`status_reason = instance_deleted`) and clears this column and `oauth_client_id` in the same transaction; such a connection cannot be re-enabled |
| source_connection_id | uuid null FK→app_connections (cascade) | derived connections die with their source |
| connected_by | uuid null FK→users (set null) | |
| reauth_required_at | timestamptz null | set by gateway on `invalid_grant` |
| last_used_at | timestamptz null | churn column |
| last_check_at | timestamptz null | (phase 5, P5-UX-14, D-335, D-350, C-944) when the account's health was last recorded: by Test (C-943) or by the gateway from real use (C-985) |
| last_check_result | text null CHECK in (`ok`,`rejected`,`unreachable`,`unexpected`) | what was recorded then; informational, never changes `status` or what an agent may do. (P5-POLISH-3, C-1033) No longer written: backfilled into the columns below by migration `20261201000254`, kept until a later cleanup |
| last_ok_at | timestamptz null | (phase 5, P5-POLISH-3, D-383, C-1033) the last 2xx/3xx answer to an **agent** call through this account, written by the gateway (at most once a minute per account); backfilled from `last_check_at` where `last_check_result = 'ok'` |
| last_error_at | timestamptz null | the last failed agent call the gateway attributes to the account (a 401/403 answer); the state is `failing` while it is newer than `last_ok_at` |
| last_error | text null CHECK in (`rejected`,`unreachable`,`unexpected`,`expired`) | why; backfilled from a non-`ok` `last_check_result` |
| last_test_at | timestamptz null | the last **Test** (C-943); never written by the gateway's use path |
| last_test_result | text null CHECK in (`ok`,`rejected`,`unreachable`,`unexpected`) | what Test said; shown as "Test passed … ago", never as the account working |
| limit_level | text null CHECK in (`read`,`ask`,`act`) | (phase 5, P5-UX-11, D-353, C-990) the limit of an organization's account: no agent's access may exceed it. Set only for organization scope (`workspace_id IS NULL`), null otherwise; migration `20261201000251_app_connections_limit_level.sql` fills it from `access` (`read` → `read`, `write` → `act`). `access` stays in step (`read` for `read`, `write` otherwise) and keeps being what the gateway enforces |
| config | jsonb null CHECK (`config IS NULL OR jsonb_typeof(config) = 'object'`) | (phase 5, P5-WEBHOOK-2, D-320, C-909; migration `20261201000243_app_connections_config.sql`) the core-owned configuration of a send-mode secret-link connection (an outgoing webhook): the `WebhookConfig` of `aurora-webhook` (template, slots, headers, method, signing flag, `rate`, preset id). Non-null only for such a connection; **holds no secret**: the URL, the secret values and the signing secret are sealed in the credential JSON (`credential_keys` lists `url`, `secret_values`, `signing_secret` by name). The API enforces ≤ `WEBHOOK_CONFIG_MAX` (32 KiB). The gateway reads it under its table-level `SELECT`, with the org's `settings.outgoing_webhooks` (`{ enabled, per_minute_max, per_day_max }`), at every send. A reconnect without a configuration keeps it |
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
| base_path | text null | (phase 5, FIX-145, D-231, C-731) the path the app is served under on `host` (`/jira` for `https://its.cern.ch/jira`); null = served at the host root. Stored normalized (trailing `/` dropped; `""` and `/` stored as null; case kept). Set at registration and never updated (immutable like `host`). Every `{instance}` URL of the registration resolves to `https://<host><base_path>`, and the gateway injects the registration's connections only under it (`09 §3.4 Instance-bound hosts`, SEC-59). CHECK `app_instances_base_path_ck` mirrors the API validation (`instance_base_path_invalid`): starts with `/`, 1–8 segments of 1–64 characters from `[A-Za-z0-9._~-]`, no `.` or `..` segment, no trailing `/`, ≤ 128 characters |
| builtin | boolean not null default false | (phase 5, FIX-134, D-243, C-742) `true` for a **preset instance**: seeded from the manifest's `instances.presets` (`09 §3.4`) in the activation transaction of the package version that declares it (`org_id` null, `label` and `host` from the manifest, `created_by` null). Read-only to everyone: no PATCH, DELETE or promote (`409 instance_builtin`); a version that drops a preset deletes its row when nothing uses it, else disables it. CHECK `app_instances_builtin_scope_ck`: builtin ⇒ `org_id` null. Migration `20261201000161` |
| unique | (org_id, app_id, host, base_path) nulls not distinct | one app may be registered twice on one host in one scope under disjoint base paths. A host with overlapping base paths (either null, equal, or one a segment prefix of the other) is additionally refused across scopes that can see each other (instance-wide vs any org); enforced by the API in the creating transaction under the per-host lock (`instance_host_conflict`) — a unique key cannot express the prefix rule |
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

### `addon_sources` (phase 5, P5-ASRC, D-250; third-party app-package repositories)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| name | text not null | manifest `repo.name` |
| url | text unique not null | the resolved manifest URL (https, normalized at registration; never the pasted repo URL) |
| homepage | text null | manifest `repo.homepage` |
| contact | text null | manifest `repo.contact` (https or mailto) |
| publisher | text not null | the package publisher all addons of the repo use |
| signing_key | text not null | the manifest-published ed25519 key, base64 — the trust anchor of the source |
| key_fingerprint | text not null | sha256 of the key, `sha256:<12 hex>`; shown prominently at registration and re-confirm |
| new_key_fingerprint | text null | the new fingerprint while `status = signing_key_changed` |
| manifest_digest | text not null | sha256 of the last accepted manifest bytes; polls compare before parsing |
| manifest | jsonb not null | the last accepted manifest as parsed (repo info, addons, versions): the dashboard reads it, never the live source; "removed upstream" is derived from it |
| update_interval | text not null default `1d` CHECK in (`6h`,`12h`,`1d`,`1w`,`2w`,`1mo`,`never`) | the instance setting `plugins.addon_sources.update_interval` seeds it; `never` = manual checks only |
| status | text not null default `ok` CHECK in (`ok`,`check_failed`,`signing_key_changed`) | `check_failed` keeps serving the last known state |
| check_failed_at, check_failed_reason | timestamptz null, text null (≤ 200) | sanitized reason; never a URL query or body text |
| last_checked_at | timestamptz null | |
| next_check_at | timestamptz not null default now() | due work for the poll job (interval + jitter + failure backoff) |
| check_lease_until, lease_owner | timestamptz null, uuid null | the per-source lease of the poll job (`ADDON_LEASE`); a manual check takes the same lease |
| created_by | uuid FK→users (set null) | instance admin |
| index | (next_check_at) where status <> `signing_key_changed` | poll job claim set |

Registration (TOFU) creates the row and a `trusted_keys` row for `signing_key` in one audited
transaction; key re-confirm updates both (`09 §10`, SEC-86).

### `instance_settings` (phase 5, SET-1, D-236; instance-wide settings, one row per section)
| Column | Type | Notes |
|---|---|---|
| section | text pk CHECK (section ~ `'^[a-z][a-z_]{0,31}$'`) | the section id (`governance`, `llm`, `sign_in`, `email`, `secrets`, `agents`, `approvals`, `retention`, `skills`, `scim`, `previews`, `terminal`, `limits`; C-770). The vocabulary is the application's registry, not a CHECK, so a section added later needs no migration; a row the running release does not know is ignored with a warning (written by a newer release) |
| schema_version | int not null default 1 | shape version of `value` for this section. A release that changes a section's shape converts the stored rows in its own migration; readers ignore keys they do not know (D-238(c)) |
| value | jsonb not null default `'{}'` CHECK (jsonb_typeof(value) = 'object') | flat object `{ "<dotted key>": <value> }` of stored values: dashboard-edited values and, for keys the configuration currently sets, the mirrored configuration value (D-236(c)). Keyed lists are stored whole under their key (`"auth.oidc_providers": [ { "id": …, … } ]`). A secret field is `{ "$secret": { "ref": "<SecretRef>", "owner": "<uuid>", "updated_at": "<RFC 3339>" } }`: purpose `instance_setting`, always the `inline` store, org = nil UUID, owner = the field's own id (`08 §1`); never plaintext. Size ≤ `SETTINGS_SECTION_BYTES_MAX` (checked by the service; 413 `settings_too_large`) |
| generation | bigint not null default 0 | bumped by every write, including Helm mirroring and CLI resets; writes are `UPDATE … WHERE section = $1 AND generation = $2` (409 `stale_revision` on no row), or an insert of generation 1 that conflicts on the key |
| updated_by | uuid null FK→users (set null) | the instance admin of the last dashboard write; null for system writes (mirroring, CLI) |
| updated_by_label | text null | denormalized actor label (the admin's e-mail, `system`, or `cli`) so the page can name the writer after the user row is gone |

Rows are created on the first write to a section (no row = nothing stored: every key is `helm` or
`default`). The table holds no plaintext secret, so the **gateway database role gets `SELECT`** on
it (it reads the `governance`, `llm`, `approvals`, `limits` and `secrets` sections; it already reads
SecretRefs on `secrets`) and nothing else; only the API writes it. After a committed write the
writer sends `NOTIFY aurora_settings` `{ "section", "generation" }` (`05 §Event bus`). History is
the audit log (`instance_settings.update` rows carry the before/after diff, `04 §Audit`); no
history table. Migration `20261201000190_instance_settings.sql` (block `000190..199`,
SET-1..SET-5) creates the table; no data is written by the migration (the first API start mirrors
the configuration's locked keys, D-238(a)).

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
| source | text CHECK in (`builtin`,`directory`,`upload`,`addon`) | (phase 5, P5-ASRC) gains `addon`: an app package ingested from an addon source (D-250) |
| source_id | uuid null FK→addon_sources (set null) | (phase 5, P5-ASRC) the addon source row for `source = addon`; null otherwise |
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
| kind | text CHECK in (`hooks_wasm`,`icon`,`icon_dark`,`setup_guide`,`archive`) | (phase 5, P5-GUIDE-1, C-642) gains `agent_guide`: one row per agent guide file (`24 §4`), keyed like every blob by the sha256 of its bytes. Because the key is the digest alone, a guide file whose bytes equal an existing blob keeps that row's kind: guide files are read **by digest only** |
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
| approval_timeout_secs | int null | (phase 2, RFE-011a) only with `require_approval`; 60 … `approvals.max_timeout_secs` (`approvals.park.max_timeout_secs` while parking is on; CHECK 60…604800 since P4-011B-2, the instance maximum in force is checked at authoring); null = no preference (`07 §Approval deadlines`) |

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
| ask_timeout_secs | int null | (phase 2, RFE-011a) custom connection grants with `ask_tools`; compiled into the derived ask rule as `approval_timeout_secs` (CHECK 60…604800 since P4-011B-2; the instance maximum in force is checked at authoring) |
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
| connection_id | uuid null | the app connection whose credential the held request would use (set by the gateway; null for a generic secret); no FK, history survives. Revoking the grant or disabling/deleting the connection abandons the pending row (`07 §Lifecycle`) |
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
| parked | bool not null default false | (phase 5, RFE-011b, D-138) true for a parked approval: nobody holds the request (`07 §Park and resume`); excluded from the stale-holder sweep and from the stall exemption |
| fp_salt | bytea null | 16 random bytes; set iff `parked` |
| fingerprint | bytea null | `SHA-256(fp_salt ‖ canonical request)` (32 bytes), canonical form in `07`/C-821; never the request itself; set iff `parked` |
| resume_state | text null CHECK in (`ready`,`consumed`,`expired`,`revoked`) | null while `pending`/`denied`/`expired`; `ready` from approval until the window ends; consumed by exactly one request (compare-and-set) |
| resume_expires_at | timestamptz null | `decided_at + approvals.park.resume_window_secs` |
| consumed_at | timestamptz null | set with `resume_state = 'consumed'` |
| resume_turn_id | uuid null | the system follow-up/turn queued at approval (tracing only; no FK, history survives) |

Indexes: `(agent_id, conversation_id) WHERE resume_state = 'ready'` (grant lookup); `(resume_expires_at) WHERE resume_state = 'ready'` (window expiry); `(agent_id, conversation_id) WHERE parked AND status = 'pending'` (pending-per-agent limit, de-duplication). CHECK `(parked = (fp_salt IS NOT NULL)) AND (parked = (fingerprint IS NOT NULL))`.

The gateway database role's column-level `SELECT (id, status, gateway_instance_id)` on `approvals` gains `agent_id, conversation_id, parked, fp_salt, fingerprint, resume_state, resume_expires_at, method, host, path, rule_logical_id` and `UPDATE` stays as granted (it consumes with `resume_state`, `consumed_at`); it never writes `fingerprint`/`fp_salt` of a row it did not insert. Pending approval stays a closed `status` vocabulary: the resume lifecycle is `resume_state`, not a new status. `resume_state` moves to `revoked` by the API only (C-826). Migration `20261201000220_approval_park.sql` (P4-011B-1).

### `approval_watchers`
(phase 2, RFE-011a) A live `approvals/stream` or `approvals/pending` connection; a *live delivery surface* (`07 §Approvals`).
| Column | Type | Notes |
|---|---|---|
| replica_id | uuid | API replica holding the connection; pk part |
| user_id | uuid FK→users (cascade) | pk part |
| workspace_id | uuid FK→workspaces (cascade) | pk part; the workspace whose approvals the connection watches |
| last_seen_at | timestamptz not null | churn column; live when within `APPROVAL_WATCHER_TTL`; index (workspace_id, last_seen_at) |

### `admin_attention_notices`
(FIX-167 Part B, D-370, C-1023) One row per (decision item, instance admin): the claim that makes the e-mail and the Web Push of a new item happen once. Written in the transaction of the state change that creates the item (`connectors/plane.rs` registration, `plugins/ingest.rs` and `plugins/registry.rs` for package holds, `addon_sources` for a changed key), with `ON CONFLICT DO NOTHING`, so a replay inserts nothing. Rows are inserted only for active (not disabled) instance admins at that moment.
| Column | Type | Notes |
|---|---|---|
| kind | text CHECK in (`connector_approval`,`package_approval`,`addon_key_changed`,`host_claim_conflict`) | pk part; the informational kinds of `GET /v1/admin/attention` never get a row |
| subject_id | text | pk part: the connector id (text), or the package row / addon source uuid as text |
| subject_version | text | pk part: the manifest digest of a connector, the version of a package, the new key fingerprint of a source; a new one is a new subject |
| user_id | uuid FK→users (cascade) | pk part |
| title, detail | text not null | the fixed sentences of `14 §Needs your attention` (names and versions only); the e-mail and the title-bearing push read them |
| mailed_at, pushed_at | timestamptz null | claimed (`now()`) by the e-mail sweeper and the push loop with `FOR UPDATE SKIP LOCKED`, then sent; a null half is open work |
| index | (created_at) where `mailed_at IS NULL OR pushed_at IS NULL` | the sweepers' work set; rows older than 30 days are pruned hourly |

### `app_setup_requests`
(phase 5, P5-POLISH-4, D-386, C-1030, SEC-135; migration `20261201000255`) A person's ask that the organization's owners and admins make an app ready. A request is a record and a notice: it gives nothing and changes no availability, OAuth client or access. **No free text anywhere in these three tables.**
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid FK→organizations (cascade) | |
| subject | text CHECK in (`app`,`chat_platform`,`ai_key`) default `app` | (P5-APIGAP-2, D-399, C-1047; migration `20261201000257`) what is asked for; existing rows are `app` |
| app_id | text | the catalogue app id for subject `app`; the connector type (or `''`) for `chat_platform`; `''` for `ai_key` |
| status | text CHECK in (`open`,`resolved`,`declined`,`withdrawn`) default `open` | at most one `open` row per `(org_id, subject, app_id)` (partial unique index `app_setup_requests_open`); a second person's ask joins it |
| needs | jsonb | the `needs` of C-1029 at the last ask (`oauth_client`, `instance`, `admin_material`) |
| unavailable | boolean | the app is switched off for the organization (nothing in `needs` then) |
| closed_at | timestamptz null | null while open (`created_at`, `updated_at` per §Conventions) |
| closed_by | uuid null FK→users (set null) | the decliner; null for `resolved` (the system) and `withdrawn` |
Closed rows (with their askers and notices) are deleted 90 days after `closed_at` by the hourly sweep (`app_setup_requests::service::spawn`). The gateway role has no grant on them.

### `app_setup_request_askers`
| Column | Type | Notes |
|---|---|---|
| request_id | uuid FK→app_setup_requests (cascade) | pk part |
| user_id | uuid FK→users (cascade) | pk part: one ask per person |
| workspace_id | uuid FK→workspaces (cascade) | where the person asked (the resolution link) |
| asked_at | timestamptz | |
A person who withdraws, or for whom the app became ready, loses the row; the request closes (`withdrawn` / `resolved`) when none is left.

### `app_setup_request_notices`
One row per (request, recipient, kind): the claims that keep the e-mail and the Web Push exactly-once across replicas (as `admin_attention_notices`). Kinds: `asked` (each active owner and admin, when the request opens), `more:<yyyymmddhh>` (one digest per hour when others join), `resolved` and `declined` (each asker).
| Column | Type | Notes |
|---|---|---|
| request_id, user_id, kind | uuid, uuid, text | primary key |
| title, detail, href | text not null | fixed sentences built from the app name and display names, and the dashboard path the notice opens (`/o/<org>/apps?tab=…&app=<id>` or the workspace's `/apps`); never a decision link |
| mailed_at, pushed_at | timestamptz null | claimed (`now()`) by the e-mail sweeper and the push loop with `FOR UPDATE SKIP LOCKED`, then sent; push only for notices younger than an hour |

### `approval_notifications`
(phase 2, RFE-011a) One row per (approval, decider) e-mail claim.
| Column | Type | Notes |
|---|---|---|
| approval_id | uuid | gateway or action approval id; pk part (no FK: approval history survives) |
| user_id | uuid FK→users (cascade) | pk part |
| stage | text not null default `initial` CHECK in (`initial`,`reminder`) | (P4-011B-2) pk part: the primary key is `(approval_id, user_id, stage)`; the parked reminder e-mail (`07 §Park and resume`) is its own idempotent claim |
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
| decision | text | `allow`,`blocked`,`default_denied`,`egress_denied`,`governance_refused`,`rate_limited`,`approval_denied`,`approval_expired`,`approved`,`guard_refused`,`error`, and (phase 5, P4-011B, D-265(a)) `approval_parked` (a parked refusal and its identical repeats), and (phase 5, P4-EVAL-2, C-868) `eval_read_only` (a write made with the context credential of an eval turn) |
| approval_id | uuid null | (phase 5, P4-011B) the approval a request was held, parked or resumed under (no FK, ledger table); set by every hold, park and grant consumption (C-827) |
| resumed | bool not null default false | (phase 5, P4-011B) true when a parked approval's grant was consumed by this request |
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
| llm_model | text null | (phase 5, P5-MODELS-2, D-304, C-880) `model_from_request` of a request with an injected key on a governed LLM endpoint (`08 §11.5`); null otherwise or when `Unknown`. Audit only: no decision reads it. Added on the partitioned parent, so every partition inherits it |

Partitions are created ahead by the API (next 2 months); rows are deleted when older than
`retention.request_log_days` (default 30), checked daily: whole partitions past the period are
dropped, aged rows of the oldest kept partition and of the default partition are deleted in
bounded batches (FIX-111).

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
| harness | text null | hosted: adapter id (`claude-code`; phase 4, P4-HAR: also `codex`, an open vocabulary validated against `hosted.enabled_harnesses` on write) |
| model_provider | text null | stamped with the provider of `model_id` (`08 §Model selection`) |
| model_endpoint_id | text null | LLM endpoint id from configuration |
| model_id | text null | provider's model id; null ⇒ endpoint/provider default. (phase 5, P5-MODELS, D-302(a)) The harness's **default** model, not a limit |
| model_roles | jsonb null | (phase 5, P5-MODELS-1, D-303) `{ endpoint_id, large?, medium?, small? }`: role models stamped with the endpoint they were chosen for; CHECK: an object with a string `endpoint_id`, only those keys, role ids matching `MODEL_ID_PATTERN` (migration `20261201000245`). Dropped per role under the override rule (`08 §Model roles`) |
| subagent_model | text not null default `'inherit'` | (phase 5, P5-MODELS-1) CHECK in (`inherit`,`large`,`medium`,`small`); which role subagents use when their definition names no model |
| narration_cards | bool not null default false | (phase 5, P4-NARR, D-279, migration `20261201000231`) the agent's live progress card on chat platforms that can edit messages (`10 §Live narration cards`); existing rows are `false` |
| proactive_messaging | text CHECK in (`ask`,`allow`,`off`) default `ask` | `13 §Messaging tools` |
| effort | text null CHECK in (`low`,`medium`,`high`,`max`) | |
| brief | text not null default `''` | ≤20 000 chars |
| eval_judge_secret_id | uuid null FK→secrets (set null) | (phase 5, P4-EVAL-1, D-292, C-863, migration `20261201000239`) the judge key: an LLM secret, granted to the agent, governance-admitted at every call (`10 §Evals`) |
| eval_judge_model_id | text null | (phase 5, P4-EVAL-1) the judge's model (a model the key's endpoint serves); written together with `eval_judge_secret_id` by `PUT eval-settings`; ignored while the secret id is null (deleting the secret nulls the id only) |
| eval_regression_notify | bool not null default true | (phase 5, P4-EVAL-1, C-871) whether the Evals page shows a regression notice for this agent |
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
| trace_context | text null CHECK (trace_context ~ `'^00-[0-9a-f]{32}-[0-9a-f]{16}-[0-9a-f]{2}$'`) | (phase 5, FIX-146, D-232, C-732) kind=`context` hosted: the turn's `turns.trace_context`, **copied at mint** so the gateway (which has no grant on `turns`) parents its spans on the turn's trace; never taken from the sandbox; NULL otherwise. Migration `20261201000150_turn_trace_context.sql` |
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
| kind | text CHECK in (`direct`,`group`,`schedule`,`webhook`,`watch`,`peer`,`api`, and (phase 5, P4-EVAL-2, C-860) `eval`) | |
| external_ref | text not null | source-specific stable key; unique (agent_id, kind, external_ref) |
| direct_user_id | uuid null FK→users (cascade) | kind=direct; partial unique (agent_id, direct_user_id) WHERE kind=`direct` |
| title | text null | |
| origin_conversation_id | uuid null FK→conversations (set null) | where reports go (schedule/webhook/peer) |
| harness_session_ref | text null | opaque, adapter-owned |
| harness_session_epoch | bigint not null default 0 | bumped to force a fresh session (phase 4, P4-HAR: on every conversation of an agent when its harness changes; `harness_session_ref` is also cleared and a `session_purges` row is queued per old session, `12`) |
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
| reply_to_seq | bigint null | (phase 5, P5-CHAT-1, D-228, C-725) the `transcript_events.seq` of the earlier `text`, `turn.queued` or `attachment` event of the same conversation this message quotes; no FK (events are not keyed rows a turn may point at), validated at insert under the conversation lock; copied onto the turn a follow-up is promoted into. Partial index `turns_reply_to_idx (conversation_id, reply_to_seq) WHERE reply_to_seq IS NOT NULL` finds the quoting turns for the redaction cascade. Migration `20261201000171_turn_reply_to.sql` |
| abort_requested_at | timestamptz null | |
| send_now_requested_at | timestamptz null | |
| error_code | text null | open vocabulary |
| error_message | text null | |
| usage | jsonb null | `{ input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, model, provider }`. (phase 5, P4-LLM) It also holds `endpoint_id`. (phase 5, P5-MODELS-2, D-304, C-879) It also holds `by_model: [{ model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens }]` and an optional `unverified_models: true`. The totals are the sum of `by_model`, and `model` is the default (`10 §Usage and metering`) |
| model_basis | jsonb null | (phase 5, P5-MODELS-2, D-304) what the dispatcher chose: `{ provider, endpoint_id, model, roles? }` (`roles` = the resolved role model ids). The trust rule at turn end reads the default and the roles from here, never from the harness. Null before the migration and never read for a turn that is not `role = 'turn'` |
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
| trace_context | text null CHECK (trace_context ~ `'^00-[0-9a-f]{32}-[0-9a-f]{16}-[0-9a-f]{2}$'`) | (phase 5, FIX-146, D-232, C-732) the W3C `traceparent` of the span that inserted the turn (`repo/turns.rs`; NULL when there is no valid one). Written only by the API at insert and never updated; never taken from a sandbox or supervisor (SEC-88). Read by the runner plane (work items carry it, `16 §W5`), the API's supervisor-report and channel-event handling, and copied onto the turn's context credential at mint (`gateway_credentials.trace_context`) for the gateway, so their spans are children of the turn's context. A follow-up promoted to a turn keeps the follow-up's value. Migration `20261201000150_turn_trace_context.sql` |

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

### `transcript_search`
(phase 5, P5-CHAT-4, D-229) One row per searchable durable event, written in the transaction that appends it (`transcript::append_durable_event` is the only insert path). Migration `20261201000172_transcript_search.sql`.
| Column | Type | Notes |
|---|---|---|
| conversation_id, seq | uuid, bigint | pk; FK→`transcript_events (conversation_id, seq)` on delete cascade (conversation delete and retention purge cascade) |
| workspace_id, agent_id | uuid FK (cascade) | copied at insert |
| type | text CHECK in (`turn.queued`,`text`,`attachment`) | |
| author_kind | text CHECK in (`agent`,`person`,`other`) | `person`/`other` from the turn's `speaker_kind` |
| author_label | text not null default '' | the turn's `speaker_label` (agent rows use the agent's name at read) |
| speaker_user_id | uuid null | the turn's speaker (`author=me`) |
| has_file | bool | an `attachment`, or a `turn.queued` with attachments |
| at | timestamptz | the event's `created_at` |
| body | text, ≤ 32 KiB | `turn.queued` text + attachment file names; agent `text` as plain text (D-227 Markdown reduced: code kept, link text and URLs kept); `attachment` file name + caption. Control characters replaced; the rest of a longer message is not searchable |
| doc | tsvector | `setweight(to_tsvector(<workspace memory_language>, body), 'A') ‖ setweight(to_tsvector('simple', body), 'B')`; a later change of the workspace language does not re-index (exact words keep working) |

Indexes: GIN `doc`; btree `(workspace_id, at desc)` and `(agent_id, at desc)`. Redaction deletes the row in its transaction. The filename row of a purged attachment stays (the name was said in the conversation). Not indexed: tool events, approvals, notices, errors, thinking, deltas.

### `transcript_search_backfill`
One row: the resumable backfill's progress (`10 §Search`). `complete` is the route's `index_complete`.
| Column | Type | Notes |
|---|---|---|
| singleton | bool pk default true CHECK (singleton) | one row only |
| cursor_conversation | uuid null | last `(conversation_id, seq)` indexed, in key order |
| cursor_seq | bigint null | |
| complete | bool not null default false | set by the job when a batch comes back short |
| indexed | bigint not null default 0 | rows the backfill wrote (log counter) |

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

### `storage_snapshots`
(FIX-126) The cached numbers behind the Storage pages, shared by every API replica. Rewritten by the daily attachment sweep, by a read that finds the snapshot missing or older than `STORAGE_SNAPSHOT_MAX_AGE`, and by the Refresh routes; one writer at a time (advisory lock). The live org counter (`org_storage_usage`) is never cached: quota notices stay exact.
| Column | Type | Notes |
|---|---|---|
| scope | text pk | `instance`, or an organization id (text) |
| payload | jsonb not null | the figures of the page (instance breakdown, or the org's file counts and per-workspace breakdown) |
| computed_at | timestamptz not null default now() | when the figures were computed; returned by the routes |

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
| head_epoch | integer not null default 0 | bumped whenever an add clears `last_head_sha`; a refresh writes its head only if the epoch is the one it read (compare-and-set; FIX-096) |
| created_by_user_id | uuid null FK→users (set null) | |
| unique | (org_id, label) (`skill_libraries_label_uq`) | |
| index | (org_id); (next_refresh_at) where status in (active, error, over_budget) | |

### `skill_library_heads` (FIX-096, D-178 Q-R5-3, D-210; heads browse and add accept)
| Column | Type | Notes |
|---|---|---|
| library_id | uuid FK→skill_libraries (cascade) | pk part |
| commit_sha | text | pk part; a commit the library's ref pointed at (recorded by refresh, browse, create, patch) |
| seen_at | timestamptz not null default now() | the newest 20 per library are kept (`HEADS_KEPT`) |
| index | (library_id, seen_at desc) | |

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
| previous_token_hash | bytea null unique | (FIX-121, D-211) the token a rotation replaced, accepted until `previous_token_expires_at`; set together with it, one at most |
| previous_token_expires_at | timestamptz null | end of the rotation grace; past it the previous token is refused (and the pair may be cleared) |
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
| channel_held_at | timestamptz null | the first `channel/hold` of the current boot; NULL until the supervisor's channel was admitted, reset by every new boot. A bootstrap token may be re-presented (idempotent claim) while it is NULL, then it is spent (FIX-094, SEC-25) |
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
| preview_wake_at | timestamptz null | a preview request found the sandbox stopped; the start arm treats it as due work for `PREVIEW_WAKE_WINDOW` (churn column) (phase 4, P4-PRV-1) |
| preview_start_requested_at | timestamptz null | a request is waiting for the share's port; drives `preview.start` (churn column) (phase 4, P4-PRV-1) |
| preview_active_until | timestamptz null | `now() + PREVIEW_AWAKE_GRACE` at each relay heartbeat that reports an open or recent stream; keep-awake reason `preview` (phase 4, P4-PRV-1) |
| preview_awake_since | timestamptz null | first heartbeat of the current boot; the `PREVIEW_AWAKE_CAP` clock; reset at every start claim (phase 4, P4-PRV-1) |
| terminal_active_until | timestamptz null | `now() + TERMINAL_AWAKE_GRACE` at each runner keep-awake stamp while a terminal session carries bytes; keep-awake reason `terminal` (churn column) (phase 4, P4-TERM, C-620) |
| terminal_awake_since | timestamptz null | first session byte of the current boot; the `TERMINAL_AWAKE_CAP` clock; reset at every start claim (churn column) (phase 4, P4-TERM, C-620) |
| terminal_wake_at | timestamptz null | a terminal open found the sandbox parked; the start arm treats it as due work for `TERMINAL_WAKE_WINDOW` (churn column) (phase 4, P4-TERM, C-621) |
| files_active_until | timestamptz null | `now() + FILES_AWAKE_GRACE` stamped by the API at a Files request on a running sandbox (at most once per 30 s per sandbox per replica); keep-awake reason `files` (churn column) (phase 5, P5-FILES-1, migration `20261201000242`) |
| ssh_active_until | timestamptz null | `now() + SSH_AWAKE_GRACE` at each runner keep-awake stamp (`kind: "ssh"`) while an SSH channel carries bytes; keep-awake reason `ssh` (churn column) (phase 5, P4-SSH-2, migration `20261201000205`) |
| ssh_awake_since | timestamptz null | first SSH channel byte of the current boot; the `SSH_AWAKE_CAP` clock; reset at every start claim (churn column) (phase 5, P4-SSH-2). The SSH **wake demand** reuses `terminal_wake_at` and the terminal's wake budget (shared, D-239(h)) |
| files_awake_since | timestamptz null | first Files stamp of the current boot; the `FILES_AWAKE_CAP` clock; reset at every start claim (churn column) (phase 5, P5-FILES-1). The Files **wake demand** reuses `terminal_wake_at` and the terminal's per-agent wake budget |

`organizations.settings.previews_enabled` (jsonb key, no column): bool, default absent = off
(`11 §Port previews`). The home-sync desired generation (`12 §Sync protocol`) already moves with
the agent's own generation; a share change bumps `agents.home_sync_generation`.

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
| workspace_id | uuid not null | (phase 5, P5-USAGE-1, C-1002; migration `20261201000247`) indexes (workspace_id, day) and (day); the migration truncated the never-written table and the hourly "Runtime daily rollup" rebuilds it (`05 §Background jobs`) |

### `usage_daily` — *no FKs*
A ledger of tokens and turn time, added to at turn end (the fenced terminal transition) and never decremented or deleted (phase 5, P5-USAGE-1, D-359, C-1001; migration `20261201000247`, which also backfills it from the turns still present).
| Column | Type | Notes |
|---|---|---|
| org_id, workspace_id, agent_id | uuid not null | |
| day | date not null | UTC day of `turns.finished_at` |
| provider, model | text null | the turn's stamp (`turns.usage`, else `turns.model_basis`); `model` is the `by_model` entry's for tokens, the default model's for turns |
| endpoint_id | text null | the operator-configured endpoint id (a slug, not a uuid; the contract row's `uuid` was a defect) |
| turns, turn_seconds | bigint not null default 0 | counted under the default model's key |
| input_tokens, output_tokens, cache_read_tokens, cache_write_tokens | bigint not null default 0 | |
| unique | (workspace_id, agent_id, day, provider, endpoint_id, model) NULLS NOT DISTINCT | the upsert key |
| index | (workspace_id, day); (org_id, day); (day) | the workspace, organization and instance views |

### `usage_rollup_state`
One row: the runtime rollup's watermark (phase 5, P5-USAGE-1, C-1002; migration `20261201000247`).
| Column | Type | Notes |
|---|---|---|
| singleton | bool pk | default true, CHECK (singleton) |
| runtime_watermark | timestamptz null | start of the last successful "Runtime daily rollup" run; null = rebuild everything; returned as `runtime_as_of` |

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
| token_id | uuid null fk → connector_host_tokens (set null) | (FIX-120) the bound token that registered this instance; null for an instance registered with the shared bootstrap token. Another token may not register, poll or report as it |

### `connector_host_tokens`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| token_hash | bytea unique not null | `auh_`, from `AURORA_BOOTSTRAP_CONNECTOR_HOST_TOKEN` (shared, unbound) or issued per host |
| name | text null CHECK 1–80 chars | (FIX-120) the issued token's name; unique among live tokens (`connector_host_tokens_name_idx`); null for the bootstrap token |
| prefix | text null | display prefix of an issued token (never enough to use it) |
| type_ids | text[] null CHECK 1–64 entries | (FIX-120) the connector types the token is bound to; **null = the shared bootstrap token** (unbound; refused once a live bound token exists) |
| created_by | uuid null fk → users (set null) | the instance admin who issued it |
| last_used_at | timestamptz null | churn column; written at most once a minute per token |
| revoked_at | timestamptz null | set by revoke; a revoked token is 401 on its next call |

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
| platform_display_name | text null | (FIX-112) the person's display name on the platform when the request was made; shown on the confirm page, never authorizes |
| index | | `(integration_id, platform_user_ref, created_at desc)`: the "at most one prompt per 10 minutes per person" lookup |

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
| addressee_ref | text null | (FIX-112) the platform user a card was DMed to; survives `space_ref` being replaced by the learned DM channel, and is what the DM-menu click is checked against |
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
view `gateway_users`)), `agents`, `gateway_credentials`, `secrets`,
`app_connections`, `app_oauth_clients`, `app_instances`, (phase 4, P4-GRP-1) `groups`,
`group_members`, `group_workspace_access`, (phase 5, SET-1) `instance_settings`, `grants`, `derived_rules`,
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
`gateway_credentials`; (phase 4, P4-CNT) `SELECT, INSERT, UPDATE` on `rate_limit_windows` (never `DELETE`, nothing on `rate_limit_buckets`); nothing on `api_keys` (no gateway query reads it; FIX-106 — `migrate` revokes the grant earlier releases made);
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

#### `attachments` (S3 blob backend; phase 6, D-318)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| blob_backend | (existing column) | gains the value **`s3`** next to `postgres` (the Notes of §6 say `postgres (phase 0)`); no schema change. A row's bytes live in the backend it names, so both can coexist and `aurora-api blobs migrate` moves rows one by one (`10 §Attachments`) | 6 (P4-S3, D-318) |
| blob_key | (existing column) | for `s3` rows: `attachments/<org_id>/<attachment_id>` (the object key is `<blobs.s3.prefix>` + this); for `postgres` rows unchanged. Cleared on purge, as today | 6 (P4-S3, D-318) |
| failure_reason | (existing open vocabulary) | gains `blob_store_unavailable` (a connector file could not be stored) | 6 (P4-S3, D-318) |

`blobs` (the Postgres backend table), `agents.avatar_blob_key` (avatars) and `plugin_blobs` are unchanged and stay in Postgres. No new table: S3 state is the bucket. The `attachments` index on `blob_key` serves the S3 orphan sweep (rows are looked up by `(blob_backend, blob_key)`). `GET /v1/admin/storage` reports bytes and counts **per backend** from `attachments` (never by listing the bucket).

#### `secrets` (additional LLM providers; phase 5, D-138)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| llm_provider | (existing open vocabulary) | gains `openai`, `azure_openai`, `bedrock`, `vertex`, `openai_compatible` (`08 §Phase 4 providers`); no schema change | 5 (P4-LLM, D-138) |
| llm_auth_mode | (existing open vocabulary) | gains `aws_sigv4` (sealed JSON `{ access_key_id, secret_access_key, session_token? }`) and `gcp_service_account` (sealed service-account key JSON); `api_key` is unchanged; (P4-LLM spec batch, D-261) also `aws_assume_role` (sealed JSON `{ access_key_id, secret_access_key, role_arn, session_duration_secs? }`) and `azure_entra` (sealed JSON `{ tenant_id, client_id, client_secret }`) | 5 (P4-LLM, D-138) |
| injection | (existing jsonb `InjectionRecipe`) | gains the LLM-only kinds `header`, `aws_sigv4 { region, service }`, `gcp_service_account_token { scope }`; (D-261) `aws_assume_role { region, service }`, `entra_token { scope }`; fixed by the provider and auth mode, read-only | 5 (P4-LLM, D-138) |
| origin | (existing jsonb) | for `bedrock` `organization_id` is the AWS account id (with `aws_assume_role` the **role's** account, `user_id` the role ARN); for `vertex` `workspace_id` is the GCP project id and `user_id` the service-account e-mail; for `openai` the organization and project of the key; (D-261) for `azure_openai` with `azure_entra` `organization_id` is the Entra tenant id and `user_id` the application (client) id (`08 §Phase 4 providers`) | 5 (P4-LLM, D-138) |

(P4-LLM spec batch, D-260..D-262) **No migration**: the provider, auth-mode and recipe vocabularies are open text/jsonb, minted credentials live only in gateway memory, and endpoints are instance settings (`instance_settings`, SET-1). Migration numbers `20261201000210..219` stay reserved for P4-LLM lanes and are unused unless a lane finds a need (it then adds the row here first). The gateway reads `organizations.external_id` for `aws_assume_role` under its existing `SELECT` on `organizations`.

(P4-GRP-1 landed `groups`, `group_members` and `group_workspace_access`: see §1. P4-GRP-2 landed `groups.external_id`, `scim_tokens` and `scim_users`: see §1.)

`organizations.settings.scim` (jsonb key, no column): `{ "enabled": bool, "email_domains": [domain], "exclusive": bool }`, default absent = SCIM off. The gateway role gets nothing on these tables. Token liveness is read on every SCIM request (indexed by `token_hash`); there is no cache.

`effective_access(user, workspace)` is one statement: the user's `org_members` row (active), the maximum of `org_members.role` and `groups.org_role` over the user's groups, the direct `workspace_access` row, and the maximum role over `group_workspace_access ⋈ group_members` for the workspace, returning the strongest role and the list of sources (`04 §Groups`). The gateway database role gains `SELECT` on `groups`, `group_members` and `group_workspace_access` (the speaker's group set is resolved at credential resolution, `06 §Credential authentication`); it never writes them. The `workspace_access` table, its CHECK and the `can_*` meaning are unchanged: a deployment without groups reads the same rows as before.

(Landed, P5-UX-5: `workspace_pending_access` is in §2 next to `workspace_access`, migration `20261201000249_workspace_pending_access.sql`.)
C-923 (a personal workspace named after the person) adds no column and no setting: the name is derived when
the workspace is created.

(Landed, P5-UX-11, D-353, C-990: `app_connections.limit_level` is in the `app_connections` section above, migration `20261201000251_app_connections_limit_level.sql`.)

(Landed, FIX-146 Part B: `turns.trace_context` and `gateway_credentials.trace_context` are in §6 and §3 with migration `20261201000150_turn_trace_context.sql`. The gateway role gets no grant on `turns`: the value is copied onto the context credential at mint.)

(Landed, P5-UX-14, D-335, D-350: `last_check_at` and `last_check_result` are in the `secrets` and `app_connections` sections above, migration `20261201000248_account_health.sql`. They are set by `POST /v1/accounts/{type}/{id}/test` and the post-connect test (C-943, C-945), and by the gateway when real use shows a change (C-985): an upstream 401 on a request with this account's credential, or a later success; the gateway writes only on a change of state, at most once a minute per account, next to its `last_used_at` write. The gateway role has `UPDATE (last_check_at, last_check_result)` on both tables, next to its `last_used_at` grant.)

### `agent_access_requests`
(phase 5, P5-UX-17, D-348, C-984, SEC-132; migration `20261201000250_agent_access_requests.sql`) An agent asks for an app (or a website's token) it cannot use. A row gives no access.
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid FK→organizations (cascade) | the agent's |
| workspace_id | uuid FK→workspaces (cascade) | the agent's |
| agent_id | uuid FK→agents (cascade) | |
| conversation_id | uuid FK→conversations (cascade) | where the agent asked |
| turn_id | uuid null FK→turns (set null) | the asking turn, so the API knows whether it has ended |
| kind | text not null | CHECK in (`app`, `website`) |
| app_id | text null | for `app` |
| secret_id | uuid null FK→secrets (cascade) | for `website`: the proposed secret (`08 §5`) |
| reason | text not null | the agent's words, one line, at most 300 characters |
| status | text not null default `open` | CHECK in (`open`, `given`, `dismissed`, `expired`) |
| resolved_at | timestamptz null | |
| resolved_by | uuid null FK→users (set null) | the person who dismissed it; null when given or expired |

UNIQUE (agent_id, app_id) WHERE status = `open` and kind = `app`; UNIQUE (agent_id, secret_id) WHERE status = `open` and kind = `website`; index (workspace_id, created_at DESC) WHERE status = `open`. Open rows expire after `ACCESS_REQUEST_TTL` (14 days). The gateway role gets nothing on this table.

### `calendar_invitations`
(phase 5, P5-CALINV, D-274, C-844, SEC-108; migration `20261201000230_calendar_invitations.sql`) One row per calendar invitation an agent e-mailed to the person it works for, so that update, cancel and list can act on it. **The description and the location are never stored**; they exist only in the sent message.
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | the invitation id the tools return; `UID` = `<id>@<instance host>` |
| org_id | uuid FK→organizations (cascade) | |
| workspace_id | uuid FK→workspaces (cascade) | |
| agent_id | uuid FK→agents (cascade) | the sender; update, cancel and list reach only this agent's rows |
| person_id | uuid FK→users (cascade) | the only recipient, at their verified e-mail |
| conversation_id | uuid null FK→conversations (set null) | where the agent sent it |
| uid | text not null | the iCalendar `UID` |
| sequence | integer not null default 0 | `SEQUENCE`; +1 on every update and cancel |
| status | text not null | CHECK in (`sent`, `cancelled`) |
| title | text not null | at most 200 characters |
| starts_at | timestamptz not null | UTC; an all-day event: its first day at 00:00 UTC |
| ends_at | timestamptz not null | UTC; an all-day event: its last day (inclusive) at 00:00 UTC |
| timezone | text not null | IANA name |
| all_day | boolean not null default false | |
| repeat | jsonb null | `{ freq, interval, by_day[], count?, until? }` as validated; null for a single event |

Indexes `(agent_id, person_id, created_at DESC)`, `(org_id)`, `(person_id)`, `(updated_at)`. Retention (daily pass, `05 §Background jobs`): a single or cancelled row 90 days after its end or last change (the later), a repeating row 5 years after its last change. The gateway role gets nothing on this table.

### `calendar_invitation_sends`
(phase 5, P5-CALINV, D-274, SEC-108; same migration) The rate-limit ledger: one row per message sent (invite, update or cancel), so that an updated or cancelled invitation still counts (20 per person per agent per hour, 60 per person per day across agents). A send that fails at the mailer is rolled back with its row. Pruned after two days.
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid FK→organizations (cascade) | |
| agent_id | uuid FK→agents (cascade) | |
| person_id | uuid FK→users (cascade) | |
| sent_at | timestamptz not null default now() | indexes `(person_id, sent_at DESC)`, `(agent_id, person_id, sent_at DESC)`, `(org_id)` |

### `approval_push_claims`
(phase 5, P4-011B-3, D-138; migration `20261201000223`) One row per (approval, decider, stage) Web Push claim.
| Column | Type | Notes |
|---|---|---|
| approval_id | uuid | gateway or action approval id; pk part (no FK: history survives) |
| user_id | uuid FK→users (cascade) | pk part |
| stage | text CHECK in (`initial`,`reminder`) | pk part |
| kind | text CHECK in (`gateway`,`action`) | |
| delivery | text CHECK in (`push`,`batch`,`digest`) | its own push; folded into the push of its `batch_id`; folded into the hourly digest (`PUSH_PER_USER_HOUR`) |
| sent_at | timestamptz not null default now() | insert-on-conflict claim, idempotent across replicas; pruned daily with the other notification claims; index `(user_id, sent_at)` |

### `webpush_vapid_keys`
(phase 5, P4-011B-3, D-138; instance-wide) The instance's VAPID (RFC 8292) keys.
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | the `vapid_key_id` of subscriptions |
| public_key | text not null | base64url, 65-byte uncompressed P-256 point; the application server key |
| private_ref | text not null | SecretRef, purpose `webpush.vapid` (`08 §1`), **`inline` store**; owner id = this id, org = nil UUID; never returned |
| status | text CHECK in (`active`,`retired`) | exactly one `active` (partial unique index `webpush_vapid_keys_one_active`); `retired` ⇔ `retired_at` set |
| retired_at | timestamptz null | set at rotation; destroyed after `PUSH_VAPID_RETIRE` or when unreferenced |

Absent when the operator supplies the key through `push.vapid.private_key_env` (nothing is stored). The gateway role has no access.

### `push_subscriptions`
(phase 5, P4-011B-3, D-138) One row per browser subscription, bound to the session that registered it.
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| user_id | uuid not null FK→users (cascade) | at most `PUSH_SUBSCRIPTIONS_PER_USER` (10) |
| session_id | uuid not null FK→sessions (cascade) | the session that registered it; delivery only while that session is live (`07 §Web Push`) |
| vapid_key_id | uuid null FK→webpush_vapid_keys (restrict) | null with an operator-managed key |
| endpoint_hash | bytea unique not null | SHA-256 of the endpoint URL; one row per endpoint (re-registration under another user moves the row) |
| endpoint_host | text not null | for the allow-list check and metrics; not secret |
| secret_ref | text not null | SecretRef, purpose `webpush.subscription`, owner id = this id: sealed JSON `{ endpoint, p256dh, auth }` — the endpoint URL and keys are send capabilities and are never stored in clear or returned |
| label | text not null | ≤ 80, sanitized, user-visible (e.g. "Firefox on Linux") |
| show_details | bool not null default false | include agent and workspace names in the notification (`07 §Web Push`) |
| paused_at | timestamptz null | |
| expires_at | timestamptz null | the browser's `expirationTime` when given |
| failure_count | int not null default 0 | consecutive; 5 sets `disabled_at` |
| disabled_at | timestamptz null | |
| last_success_at, last_failure_at | timestamptz null | churn columns |
| index | `(user_id)`, `(session_id)` | delivery and cascade |

The gateway role has no access to any of these tables. Rows of revoked or expired sessions are removed by the daily session pruning (the foreign key cascades). (P4-011B spec batch, D-266(c)) Sign-out, "sign out other sessions", an administrator's revocation and deactivation delete the session's `push_subscriptions` rows in the revoking transaction (the cascade only covers deleted session rows). The sealed secrets (`webpush.vapid`, `webpush.subscription`) are always in the `inline` store (D-266(b)). Migration `20261201000223_web_push.sql` (P4-011B-3) creates `webpush_vapid_keys`, `push_subscriptions` and `approval_push_claims` together; the partial unique index `webpush_vapid_keys (status) WHERE status = 'active'` enforces one active key.

#### `app_instances` (base path; phase 5, FIX-145, D-231) — now in `§app_instances`
Migration `20261201000140_app_instance_base_path.sql` (block `000140..149`) adds the column null
(existing rows keep `null`: they are served at the host root, today's behaviour). The `(host)`
index still serves the conflict check and the gateway's instance lookup. The gateway role already
has table-level `SELECT` on `app_instances` (`§Gateway database role grants`), so it reads
`base_path` without a new grant (instance selection, `06 §MITM lane: connect-time resolution`).

#### `agents` and `request_log` columns (phase 5, P5-MODELS, D-302..D-304)
| Planned column | Type | Notes | Phase |
|---|---|---|---|
| agents.model_roles | jsonb null | (phase 5, P5-MODELS-1, D-303) `{ endpoint_id, large?, medium?, small? }`: role models stamped with the endpoint they were chosen for. Dropped per role under the override rule (`08 §Model roles`) | 5 (P5-MODELS-1) |
| agents.subagent_model | text not null default `'inherit'` | (phase 5, P5-MODELS-1) CHECK in (`inherit`,`large`,`medium`,`small`) | 5 (P5-MODELS-1) |

The migrations land with P5-MODELS-1 and -2 (next free numbers when placed); the rows then move into `§agents` and `§request_log` above.

#### Narration cards and user tokens (phase 5, P4-NARR, P4-UTOK, D-279..D-284, SEC-111..SEC-113)

Migrations `20261201000231` (P4-NARR) and `…000232`, `…000233` (P4-UTOK); `…000234` and `…000235` are unused and
returned to the block. No gateway database role grant: the gateway reads none of these.

(P4-NARR landed: `agents.narration_cards` is in `§agents` above; migration `20261201000231_agent_narration_cards.sql`.)

(P4-UTOK landed: `user_tokens` and `user_token_agents` are real sections below (after `§ssh_keys`);
migrations `20261201000232_user_tokens.sql` and `20261201000233_user_token_agents.sql`.)

#### SSH access (phase 5, P4-SSH-1, P4-SSH-2, D-239, SEC-92..SEC-96)

Five tables and two `sandboxes` columns, all real sections above (after `file_stream_grants`):
`ssh_keys` and `ssh_mint_counters` (P4-SSH-1, migrations `…000200`, `…000201`); `ssh_sessions`,
`ssh_stream_grants` and `ssh_terminator_replicas` (P4-SSH-2, `…000202` … `…000204`) and the
`sandboxes.ssh_active_until` / `ssh_awake_since` columns (`…000205`; see the `sandboxes` table). The
SSH **wake demand** reuses `terminal_wake_at` and the terminal's wake budget
(`TERMINAL_WAKES_PER_AGENT_HOUR`, shared): both are a person asking for an interactive shell on a
parked sandbox, and one budget keeps two front doors from doubling the wake rate (D-239(h)). The
gateway database role gets nothing on any of them.

### `preview_shares` (port previews, phase 4, P4-PRV-1, D-144, D-145)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| org_id | uuid not null FK→organizations (restrict) | denormalized for the per-org limit and the gateway-free authorization checks |
| workspace_id | uuid not null FK→workspaces (cascade) | |
| agent_id | uuid not null FK→agents (cascade) | hosted agents only (service-enforced); the share dies with the agent |
| port | int not null CHECK (port BETWEEN 1024 AND 65535) | unique `(agent_id, port)`; supervisor-reserved ports refused by the service |
| label | citext not null | the hostname label, `p` + 24 base32 chars, **unique index**, immutable, never reused (`11 §Port previews`) |
| visibility | text not null default `private` CHECK in (`private`) | the only value in phase 4 |
| state | text not null default `active` CHECK in (`active`,`paused`) | |
| start_command | text null CHECK (length ≤ 1024 and no newline) | run on demand by the supervisor; never audited or sent to the relay |
| cwd | text null | relative to `/workspace`, no `..`, ≤ 200 |
| last_start_at | timestamptz null | for the dashboard (churn columns) |
| last_start_exit_code | int null; last_start_tail text null (≤ 4 KiB, sanitized, secrets redacted like transcripts) | for the dashboard (churn columns) |
| start_attempts | jsonb not null default `[]` | timestamps of the attempts inside `PREVIEW_START_WINDOW` (≤ 3) |
| wake_attempts | jsonb not null default `[]` | timestamps of the wakes inside the hour (≤ `PREVIEW_WAKES_PER_SHARE_HOUR`) |
| created_by_user_id | uuid null FK→users (set null) | |
| created_by_label | text null | the creating user's label, denormalized (kept when the user row is deleted) |
| last_accessed_at | timestamptz null | written at most once a minute per share (relay heartbeat) |
| index | unique `(label)`; unique `(agent_id, port)`; `(org_id)`; `(workspace_id)` | |

The gateway database role gets nothing on this table (the gateway has no part in previews).
Deleting the agent, workspace or org deletes the shares (the org delete service removes them in
dependency order). The `previews` home-sync part is computed from this table (`16 §W5`).

### `preview_labels` — tombstones of spent hostname labels (phase 4, FIX-043, D-144, SEC-80)
| Column | Type | Notes |
|---|---|---|
| label | citext pk | the minted hostname label, `p` + 24 base32 chars — exactly the value the share holds in `preview_shares.label` |

A ledger of every label that has ever been minted, so the contract "unique and **never reused**"
(`11 §Port previews`, `17 §Security`) holds after a delete: `create` writes the minted label here
in the same transaction as the share row, and deleting the share never removes the tombstone — the
mint excludes every existing row **union** every tombstone. No FK by design (a ledger, per
§Conventions): the tombstone outlives the share it describes. `created_at` follows §Conventions.
The gateway database role gets nothing on this table. Backfilled from `preview_shares` by the
migration that adds it (every existing label is already spent).

### `preview_sessions` (port preview access, phase 4, P4-PRV-2, D-146, SEC-81)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| token_hash | bytea unique not null | SHA-256 of the `auv_` value; the value is returned once to the relay and never stored |
| display_prefix | text not null | first 12 characters, for support |
| share_id | uuid FK→preview_shares (cascade) | the one share (hostname) the credential is valid for |
| user_id | uuid FK→users (cascade) | |
| session_id | uuid FK→sessions (cascade) | the dashboard session that authorized it: signing out or revoking it deletes this row |
| last_seen_at, expires_at | timestamptz | `expires_at = created_at + PREVIEW_SESSION_TTL` (capped by the dashboard session's own absolute expiry); `last_seen_at` is advanced at most once a minute; idle past `PREVIEW_SESSION_IDLE` is dead |

### `preview_grant_uses` (single-use ledger of authorization grants, phase 4, P4-PRV-2, D-146)
| Column | Type | Notes |
|---|---|---|
| jti | uuid pk | insert-on-conflict claims a grant exactly once |
| expires_at | timestamptz not null | grant expiry + 60 s; pruned by the preview housekeeping job |

### `preview_stream_grants` (single-use stream grants, phase 4, P4-PRV-2, D-147, SEC-82)
| Column | Type | Notes |
|---|---|---|
| token_hash | bytea pk | SHA-256 of the `aux_` grant |
| share_id, sandbox_id, boot_id | uuid | the binding the runner receives; no FK (short-lived) |
| port | int not null | copied from the share at mint time |
| session_hash | bytea not null | the preview session it was minted for (for accounting only) |
| expires_at, consumed_at | timestamptz | ≤ 30 s; `consumed_at` set by one compare-and-set on redemption |

### `relay_tokens` (relay anchor tokens, phase 4, P4-PRV-2)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| token_hash | bytea unique not null | SHA-256 of the `aul_` value from `AURORA_BOOTSTRAP_RELAY_TOKEN` (same shape as `connector_host_tokens`) |

### `relay_replicas` (registered relay replicas, phase 4, P4-PRV-2)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | the replica id returned by `register` |
| token_id | uuid FK→relay_tokens (cascade) | the anchor token the replica registered with |
| version | text not null | the relay's version string (churn) |
| streams_open | int not null default 0 | heartbeat churn (`POST /v1/relay/report`) |
| last_seen_at | timestamptz not null | refreshed by the heartbeat; rows older than 24 h pruned |

The gateway role gets nothing on any preview table. Preview sessions are deleted by cascade when
their share, user or dashboard session goes; the `sessions` retention job's deletes cascade too.

### `terminal_stream_grants` (single-use web-terminal grants, phase 4, P4-TERM, D-161, SEC-87, C-618)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| agent_id | uuid not null | the terminal's agent; the workspace authority (`can_use`) was checked at mint |
| sandbox_id | uuid not null | the bound sandbox |
| boot_id | uuid not null | the bound boot (stale boot → redemption refused) |
| granted_by | uuid not null | the user whose session opened the tab (the audit actor) |
| grant_hash | text not null unique | SHA-256 of the `auy_` grant (the value is never stored) |
| expires_at | timestamptz not null | `now() + TERMINAL_GRANT_TTL` (30 s) |
| consumed_at | timestamptz null | set by the single-use compare-and-set at redemption |
| shell | text not null default `'bash'` check `bash`/`zsh` | (FIX-138) the person's shell choice (the enumerated `?shell=`, never a path); returned by the redemption answer so the runner's `pty.open` carries it |
| index | unique `(grant_hash)`; `(expires_at)` | redemption, pruning |

No foreign keys (the rows die young, like `preview_stream_grants`); rows pruned after 24 h. The
grant value never reaches the browser and is never stored; terminal bytes are never stored
anywhere — the table holds the open's metadata only. The gateway database role gets nothing on
this table.

### `file_stream_grants` (single-use Files grants, phase 5, P5-FILES-1, D-316, SEC-125, C-901; migration `20261201000241`)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| agent_id | uuid not null | the session's agent; `can_use` was checked at mint; no FK |
| sandbox_id | uuid not null | the bound sandbox; no FK |
| boot_id | uuid not null | the bound boot (stale boot → redemption refused); no FK |
| granted_for | uuid not null FK→users (cascade) | the person whose request opened the SFTP session |
| grant_hash | text not null unique | SHA-256 of the `auf_` grant; presented once by the API to the runner, never stored |
| expires_at | timestamptz not null | `now() + FILES_GRANT_TTL` (30 s) |
| consumed_at | timestamptz null | set by the single-use compare-and-set at redemption |
| index | unique `(grant_hash)`; `(expires_at)` | redemption, pruning after 24 h |

A table of its own, not rows in `terminal_stream_grants` or `ssh_stream_grants`: the `auf_` prefix
routes redemption to this table alone, so a files grant can only ever open an `sftp` exec (D-239(f)
pattern). File contents are never stored anywhere; the gateway database role gets nothing on it.

### `ssh_keys` (a person's registered SSH public keys, phase 5, P4-SSH-1, D-239(b), SEC-93, C-790; migration `20261201000200`)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| user_id | uuid not null FK→users (cascade) | owned by the person, not by an org or workspace; at most `SSH_KEYS_PER_USER` (10) |
| name | text not null CHECK (char_length(name) BETWEEN 1 AND 80) | sanitized (`02 §Untrusted text`); defaults to the key's comment, else "SSH key" |
| algorithm | text not null CHECK in (`ssh-ed25519`) | the only accepted type (D-239(b)) |
| public_key | text not null | **canonical** OpenSSH form `ssh-ed25519 <base64>`, rebuilt from the parsed key (never the raw paste; the comment and any options are dropped) |
| fingerprint | text not null | `SHA256:<unpadded base64>` of the key blob, computed by the server (OpenSSH format) |
| last_used_at | timestamptz null | stamped at a certificate mint and at a session open with this key, at most once a minute (churn column) |
| unique | `(user_id, fingerprint)` | the same key twice for one person is 409 `ssh_key_exists`; two people may register the same public key (a row grants nothing by itself: a certificate also needs the private key) |

The gateway database role gets nothing on this table.

### `user_tokens` (a person's chat-platform token, phase 5, P4-UTOK, D-281..D-284, SEC-112, SEC-113, C-853, C-855; migration `20261201000232`)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | id format of `00`; not a bearer token, so no `au?_` prefix |
| user_id | uuid not null FK→users (cascade) | the owner; the token is usable only for this person's own link; at most `USER_TOKENS_PER_PERSON` (5) not `revoked` |
| integration_id | uuid not null FK→channel_integrations (cascade) | one platform integration per token |
| platform_user_ref | text not null | the platform user the verify op returned; equal to the owner's `user_links.platform_user_ref` on this integration. A partial unique index on `(integration_id, platform_user_ref)` where `status <> 'revoked'`: one live token per platform user (409 `user_token_exists`) |
| platform_user_label | text not null | the handle the verify op returned, shown to the owner (never the platform user id) |
| label | text null CHECK (char_length(label) BETWEEN 1 AND 80) | the owner's name for the token |
| token_ref | text null | a SecretRef, purpose `user_token`, owner = this row's id (`08 §1`); never returned. CHECK: set whenever `status = 'active'`. After the token ends (`invalid`, `revoked`) it stays only until the sealed value is discarded (right after the ending transaction commits, else by the custody sweep, `13 §5.11`) and is then null |
| status | text not null CHECK in (`active`,`invalid`,`revoked`) | `invalid` set by a platform `401` on a verify or send; `revoked` on any end of standing (D-284(a)): the owner's `DELETE`, the owner's link on the integration removed, SCIM deprovisioning or deletion of the owner, deletion of the integration or its organization |
| last_used_at | timestamptz null | set when a send is claimed (churn column) |
| revoked_at | timestamptz null | |
| index | `(user_id, created_at DESC, id DESC)`, `(integration_id)` | and two partial expression indexes that find a token's open work: `action_approvals ((payload->>'token_id'))` for pending `send_as_me` rows and `channel_ops ((payload->'sender'->>'user_token'))` for pending or claimed `message.post` rows |

Per-op copies of a token are not columns: they live in `channel_ops.payload.user_credential_ref` (`08 §1`),
with the internal keys `user_org_id` and `user_meta` (agent, conversation, approval, owner for the audit row);
all three are stripped at claim and the reference is set to JSON `null` and discarded at the op's terminal
state (`13 §4.2`, `§5.11`). The gateway database role gets nothing on this table.

### `user_token_agents` (the owner's grants of a token to single agents, phase 5, P4-UTOK, D-283(a), C-855; migration `20261201000233`)
| Column | Type | Notes |
|---|---|---|
| user_token_id | uuid not null FK→user_tokens (cascade) | pk part |
| agent_id | uuid not null FK→agents (cascade) | pk part; a grant ends with the agent (its pending posts are cancelled by the deletion path, `13 §5.11`) |
| granted_at | timestamptz not null default now() | |
| index | `(agent_id)` | |

A grant of one agent is no grant of another. The gateway database role gets nothing on this table.

### `eval_questions` (phase 5, P4-EVAL-1, D-289, C-861; migration `20261201000236`)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| agent_id | uuid not null FK→agents (cascade) | hosted agents only (`eval_not_supported` for BYO) |
| workspace_id | uuid not null FK→workspaces (cascade) | the audience of §Evals |
| position | int not null | unique `(agent_id, position)` (deferrable) |
| question | text not null | ≤ `EVAL_QUESTION_CHARS` |
| expected_kind | text not null CHECK in (`numeric`,`text`,`judged`) | |
| expected_number | numeric null | `numeric` only |
| tolerance | numeric null default 0 | `numeric` only; ≥ 0 |
| expected_text | text null | `text` only; ≤ `EVAL_EXPECTED_TEXT_CHARS`; agent-audience data (`10 §Evals`, SEC-116) |
| criteria | text null | `judged` only; ≤ `EVAL_CRITERIA_CHARS` |
| enabled | bool not null default true | |
| version | int not null default 1 | bumped by every edit; part of `question_set_version` |
| created_by | uuid null FK→users (set null) | |
| unique | `(agent_id, position)` | |

No foreign key to runs: a run keeps its question text and expected kind in `eval_results` (snapshot), so an edit never rewrites history. The gateway database role gets nothing on this table.

### `eval_runs` (phase 5, P4-EVAL-2, D-290, D-291, C-864, C-865; migration `20261201000237`)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| agent_id | uuid not null FK→agents (cascade) | |
| workspace_id | uuid not null FK→workspaces (cascade) | |
| trigger | text not null CHECK in (`manual`,`schedule`) | |
| schedule_id | uuid null FK→eval_schedules (set null) | |
| started_by_user_id | uuid null FK→users (set null) | null for scheduled runs (fire-time authority: `10 §Evals`) |
| status | text not null default `queued` CHECK in (`queued`,`running`,`done`,`failed`,`cancelled`) | |
| question_set_version | text not null | SHA-256 of the enabled questions' ids and versions |
| fingerprint | jsonb not null | canonical object of C-866, hashes and ids only, immutable |
| fingerprint_hash | text not null | SHA-256 of the canonical `fingerprint` |
| questions_total | int not null default 0 | set at start |
| questions_passed | int not null default 0 | counts, written when the run ends |
| questions_failed | int not null default 0 | |
| questions_errored | int not null default 0 | |
| questions_not_scored | int not null default 0 | |
| refused_writes | int not null default 0 | total `eval_read_only` refusals of the run (C-868) |
| judge_usage | jsonb null | summed token counts of judge calls (never `turns.usage`) |
| regression | jsonb null | `{ from_run_id, drop_points }` (C-871) |
| error_code | text null | run-level end reason (`access_lost`, `frozen`, `cancelled`, an API code for `failed`) |
| cancel_requested_at | timestamptz null | |
| lease_until | timestamptz null | claim clock; released at the end of every tick |
| started_at | timestamptz null | |
| finished_at | timestamptz null | |
| index | partial unique `(agent_id)` WHERE status IN (`queued`,`running`) | one active run per agent (`EVAL_RUNS_ACTIVE_PER_AGENT`) |

Rows of a finished run are immutable except by the retention job (`EVAL_RUN_RETENTION`, 90 days) and by deletion of the run. The gateway database role gets nothing on this table.

### `eval_results` (phase 5, P4-EVAL-2, D-290, C-867; migration `20261201000237`)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| run_id | uuid not null FK→eval_runs (cascade) | unique `(run_id, position)`; all rows are created at run start (`not_scored`, no `finished_at`) |
| position | int not null | 1-based over the enabled questions at start |
| question_id | uuid null FK→eval_questions (set null) | the question as it was at start: `question_text`, `expected_kind` and `expected_snapshot` carry the rest |
| question_text | text not null | snapshot |
| expected_kind | text not null CHECK in (`numeric`,`text`,`judged`) | snapshot |
| expected_snapshot | jsonb not null | snapshot of the expected answer; agent-audience data, never copied to audit or logs |
| conversation_id | uuid null FK→conversations (set null) | the question's `eval` conversation |
| turn_id | uuid null | |
| outcome | text not null default `not_scored` CHECK in (`pass`,`fail`,`error`,`not_scored`) | final once `finished_at` is set |
| reason_code | text null | `no_answer_line`, `answer_not_a_number`, `timeout`, `judge_not_available` (until P4-EVAL-3), `judge_*`, `access_lost`, `frozen`, `cancelled`, a turn's `error_code`, … (C-867, C-872) |
| extracted_value | text null | `numeric`/`text`: the value read from `ANSWER:` |
| answer_text | text null | ≤ `EVAL_ANSWER_CHARS`, sanitised; agent-audience data |
| judge_verdict | text null CHECK in (`pass`,`fail`) | judged questions |
| judge_rationale | text null | ≤ `EVAL_RATIONALE_CHARS`, one line |
| refused_writes | int not null default 0 | `tool_refusals` plus the gateway's `eval_read_only` request-log rows, reconciled when the run ends |
| tool_refusals | int not null default 0 | the platform tools' share of `refused_writes` |
| started_at | timestamptz null | |
| finished_at | timestamptz null | |

The gateway database role gets nothing on this table.

### `eval_schedules` (phase 5, P4-EVAL-2, D-293, C-870; migration `20261201000238`)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| agent_id | uuid not null FK→agents (cascade) | |
| name | text not null | 1..200 characters |
| cron | text not null | 5-field; interval ≥ `EVAL_SCHEDULE_MIN_INTERVAL` over a full week of occurrences |
| timezone | text not null | IANA |
| enabled | bool not null default true | |
| disabled_reason | text null CHECK in (`user`,`authorization`,`failures`) | |
| consecutive_failures | int not null default 0 | a fire with no questions and a run that ends `failed` count; a `done` run resets |
| creator_user_id | uuid null FK→users (set null) | fire-time authority anchor (`04 §Fire-time authority`) |
| anchor | text not null default `creator` CHECK in (`creator`,`workspace`) | |
| next_fire_at | timestamptz null | the schedules engine's claim fields |
| fire_lease_until | timestamptz null | |
| last_fire_at | timestamptz null | |
| last_outcome | text null CHECK in (`ok`,`failed`,`skipped_busy`,`skipped_frozen`) | a reached daily cap is `skipped_busy` |
| last_run_id | uuid null FK→eval_runs (set null) | |

The gateway database role gets nothing on this table.

### `ssh_mint_counters` (certificate-mint rate limit, a ledger, phase 5, P4-SSH-1, D-239(c), C-791; migration `20261201000201`)
| Column | Type | Notes |
|---|---|---|
| user_id | uuid not null | pk part; no FK (a ledger, like `preview_grant_uses`) |
| agent_id | uuid not null | pk part; no FK |
| window_start | timestamptz not null | pk part; `date_trunc('hour', now())` |
| count | int not null default 0 | incremented by one `INSERT … ON CONFLICT DO UPDATE SET count = count + 1 WHERE count < SSH_MINTS_PER_USER_AGENT_HOUR RETURNING count` in the mint transaction: no row returned → 429 `ssh_mint_rate_limited`. Rows older than 24 h are pruned by the retention sweep |

Its own table rather than `rate_limit_windows`/`aurora-counters` (D-239(c)): the mint limit must be exact
across API replicas whatever `ratelimit.counters.backend` says (the default `local` counts per
replica) and must never fail open; the audit row is best-effort and cannot be the counter. The
gateway database role gets nothing on this table.

### `ssh_sessions` (one row per authenticated SSH connection, phase 5, P4-SSH-2, D-239, SEC-94, SEC-95, C-793, C-794; migration `20261201000202`)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | the session id the terminator carries |
| user_id | uuid not null FK→users (cascade) | the certificate's person |
| ssh_key_id | uuid null FK→ssh_keys (set null) | the key the certificate certified; deleting the key nulls it and the next heartbeat revokes the session (`key_deleted`) |
| key_fingerprint | text not null | kept for audit after the key row is gone |
| cert_serial | bigint not null | the certificate's serial (the `agent.ssh_certificate.mint` audit row names it too) |
| org_id | uuid not null FK→organizations (cascade) | denormalized for the org audit and the org delete order |
| workspace_id | uuid not null FK→workspaces (cascade) | |
| agent_id | uuid not null FK→agents (cascade) | the certificate's principal; hosted agents only (service-enforced) |
| terminator_replica_id | uuid not null | the replica that holds the connection (no FK: replica rows are pruned) |
| client_address | text null | the SSH client's address as the terminator saw it, **coarsened** at insert (IPv4 /24, IPv6 /48, like `04 §CLI login`); terminator-reported, audit metadata only |
| state | text not null default `open` CHECK in (`open`,`closed`) | the guarded transition `open → closed` (`UPDATE … WHERE state = 'open' RETURNING`) is the **close-exactly-once** gate; the one `agent.ssh.close` audit row is written by whichever path wins it |
| lease_until | timestamptz not null | `now() + SSH_SESSION_LEASE` (90 s) at open and at every heartbeat that keeps it; a session whose lease passed is dead whatever `state` says (liveness = lease) |
| last_activity_at | timestamptz null | the last channel byte the terminator reported (heartbeat); never keystroke content |
| revoke_requested_at | timestamptz null | set by `DELETE /v1/ssh-sessions/{id}`; the next heartbeat answers the session revoked |
| revoke_reason | text null | `user_disconnect` / `owner_disconnect` |
| closed_at | timestamptz null | |
| close_reason | text null | open vocabulary: `client_disconnect`, `idle_timeout`, `max_duration`, `access_revoked`, `key_deleted`, `frozen`, `ssh_disabled`, `user_disconnect`, `owner_disconnect`, `agent_deleted`, `lease_expired`, `wake_timeout`, `terminator_shutdown`, `relay_error` |
| channels_total | int not null default 0 | heartbeat and close accounting |
| bytes_in, bytes_out | bigint not null default 0 | byte totals only; never content, never commands |
| index | `(user_id) WHERE state = 'open'`; `(agent_id) WHERE state = 'open'`; `(lease_until) WHERE state = 'open'`; `(created_at)` | caps (`SSH_SESSIONS_PER_USER`, `_PER_AGENT`, counted over open, lease-current rows: global, not per replica), the stale-session sweep, retention |

Closed rows are kept for `SSH_SESSION_RETENTION` (30 days) and then deleted by the retention sweep
(the audit rows outlive them). The gateway database role gets nothing.

### `ssh_stream_grants` (single-use SSH channel grants, phase 5, P4-SSH-2, D-239(f), SEC-95, C-794, C-796; migration `20261201000203`)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| session_id | uuid not null FK→ssh_sessions (cascade) | the session the channel belongs to; redemption is refused unless it is still `open` and lease-current |
| agent_id, sandbox_id, boot_id | uuid not null | the binding the runner receives (stale boot → redemption refused); no FK |
| grant_hash | text not null | SHA-256 of the `auz_` grant; the value is returned once to the terminator and never stored; unique |
| mode | text not null CHECK in (`shell`,`exec`,`sftp`) | the channel mode the grant was minted for, kept for the channel counts of the audit trail (never content) |
| expires_at | timestamptz not null | `now() + SSH_GRANT_TTL` (30 s); index; pruned after 24 h |
| consumed_at | timestamptz null | set by the single-use compare-and-set at redemption |

A table of its own, not rows in `terminal_stream_grants` (D-239(f)): the `auz_` prefix routes redemption
to this table alone, so a terminal grant can never open an SSH channel or the reverse, and the binding
includes the session whose liveness redemption re-checks. The gateway database role gets nothing.

### `ssh_terminator_replicas` (registered terminator replicas, phase 5, P4-SSH-2, D-239, SEC-92, C-794; migration `20261201000204`)
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | the replica id returned by `POST /v1/terminator/register` |
| version | text not null | (churn) |
| host_public_key | text not null | the replica's SSH host public key, canonical `ssh-ed25519 <base64>`; the API serves the distinct keys of live replicas as the known-hosts entries (`05 §SSH access`) |
| host_key_fingerprint | text not null | `SHA256:…` |
| sessions_open | int not null default 0 | heartbeat churn |
| last_seen_at | timestamptz not null | refreshed by each heartbeat; a replica is live while `last_seen_at > now() − SSH_SESSION_LEASE`; rows older than 24 h are pruned |

There is no `terminator_tokens` table: the single anchor token's SHA-256 is computed by the API from
`AURORA_BOOTSTRAP_TERMINATOR_TOKEN` at startup and held in memory (rotation = a new Secret value and a
rollout of the API and the terminator; `15 §SSH access`). The gateway database role gets nothing.

`eval_questions` is a real table (P4-EVAL-1, migration `20261201000236`): see `### eval_questions` above.

`eval_runs`, `eval_results` and `eval_schedules` are real tables (P4-EVAL-2, migrations `20261201000237`, `…238`): see their `###` sections above.

#### `conversations.kind` (phase 5, P4-EVAL-2, C-860; migration `20261201000240`)
`conversations.kind` CHECK gained `eval` (external_ref `eval:<run_id>:<position>`; the unique `(agent_id, kind, external_ref)` index is unchanged). The same migration adds `eval_read_only` to `request_log.decision` (§request_log). A turn of an `eval` conversation gets a `context` gateway credential whose `capabilities` hold `eval_read_only` (set at mint from the conversation's kind); the gateway refuses every write made with it (C-868).

(Landed, P5-POLISH-3, D-383, C-1033: the five columns `last_ok_at`, `last_error_at`, `last_error`, `last_test_at`, `last_test_result` are in the `secrets` and `app_connections` sections above, migration `20261201000254_account_use_and_tests.sql`. The gateway writes `last_ok_at` / `last_error_at` / `last_error` for agent calls only, at most once a minute per account and outcome; the Test route writes only `last_test_*`; `last_check_at` and `last_check_result` were backfilled into the use columns (`ok` → `last_ok_at`, otherwise `last_error_at`/`last_error`) and are no longer written, kept until a later cleanup migration. The gateway role's column grant is `UPDATE (…, last_ok_at, last_error_at, last_error)`, never `last_test_*`.)

(Landed, P5-POLISH-4, D-386, C-1030, SEC-135: `app_setup_requests`, `app_setup_request_askers` and `app_setup_request_notices` are in their own sections above, migration `20261201000255_app_setup_requests.sql`.)
