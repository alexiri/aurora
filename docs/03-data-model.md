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
| password_hash | text null | argon2id PHC string; null for SSO-only users |
| is_instance_admin | bool not null default false | |
| disabled_at | timestamptz null | instance-level disable |
| default_org_id | uuid null FK→organizations (set null) | |
| last_login_at | timestamptz null | churn column |

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

### `email_tokens`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| user_id | uuid FK→users (cascade) | |
| purpose | text CHECK in (`verify_email`,`reset_password`) | |
| token_hash | bytea unique | `aui_` |
| expires_at | timestamptz | verify 24 h, reset 1 h |
| used_at | timestamptz null | |

### `organizations`
| Column | Type | Notes |
|---|---|---|
| id | uuid pk | |
| slug | text unique not null | |
| name | text not null | ≤80 |
| app_posture | text CHECK in (`open`,`restricted`) default `open` | see `07 §App availability` |
| governance | jsonb not null default `{}` | org LLM and egress governance document (`08 §11.2`); `{}` = defaults derived from the instance |
| governance_revision | bigint not null default 0 | optimistic concurrency for governance edits |
| external_id | text unique not null | 32 random hex chars at creation; immutable; exposed to plugins as context variable `org_external_id` (e.g. AWS external id) |
| settings | jsonb not null default `{}` | `{ "max_agents"?: int, "max_members"?: int, "sandbox_overrides"?: ResourceProfile }` |

### `org_members`
| Column | Type | Notes |
|---|---|---|
| org_id | uuid FK→organizations (cascade) | pk part |
| user_id | uuid FK→users (cascade) | pk part |
| role | text CHECK in (`owner`,`admin`,`member`) | |
| status | text CHECK in (`active`,`suspended`) default `active` | suspended = non-member for every check |

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

### `workspace_access`
| Column | Type | Notes |
|---|---|---|
| workspace_id | uuid FK→workspaces (cascade) | pk part |
| user_id | uuid FK→users (cascade) | pk part |
| role | text CHECK in (`owner`,`member`) | |

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
| index | (org_id, created_at desc), (workspace_id, created_at desc) | |

Audit writes are best-effort and never fail the action (logged on failure).

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
| status_reason | text null | open vocabulary |
| credentials_ref | text not null | SecretRef of the credential JSON (fields per manifest `credentials`) |
| credentials_version | bigint not null default 1 | CAS authority for refresh/mint writes |
| credentials_expires_at | timestamptz null | access-token expiry (refresh trigger) |
| scopes | text[] not null default `{}` | granted scopes |
| requested_scopes | text[] not null default `{}` | scopes requested at authorize time; `missing_scopes = requested_scopes − scopes` (I-33) |
| metadata | jsonb not null default `{}` | non-secret, plugin-declared keys only |
| tenant_host | text null | for tenant-bound hosts |
| oauth_client_id | uuid null FK→app_oauth_clients (restrict) | which client minted it |
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
| unique | (org_id, workspace_id, package_id, client_key) nulls not distinct | |

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
| status_reason | text null | |
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
| rule | jsonb not null | same fields as `policy_rules` minus ids/priority |
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
| deadline | timestamptz not null | |

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

Partitions are created ahead by the API (next 2 months) and dropped after
`REQUEST_LOG_RETENTION`.

### `rate_limit_windows` — not used in phase 0
Reserved name; phase 0 rate limiting is in-memory per gateway replica (`07`).

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
| avatar_public_key | text null | unguessable, rotating; gates the public avatar GET |
| resource_overrides | jsonb null | `ResourceProfile` (instance admins only) |
| home_sync_generation | bigint not null default 0 | desired projection generation |
| created_by | uuid null FK→users (set null) | |

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
| expires_at | timestamptz null | agent: null; boot: null until stop; context: turn end + grace |
| revoked_at | timestamptz null | |
| index | (agent_id, kind) | |

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
| source_event_id | text null | partial unique (conversation_id, source_event_id) |
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

### `transcript_events`
| Column | Type | Notes |
|---|---|---|
| conversation_id | uuid FK→conversations (cascade) | pk part |
| seq | bigint | pk part; = conversation `last_seq` after atomic increment |
| turn_id | uuid null | |
| type | text not null | canonical vocabulary (`10 §Transcript events`) |
| payload | jsonb not null | bounded (≤64 KiB serialized) |

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
| blob_key | text not null | |
| caption | text null | |
| failure_reason | text null | |
| created_by_user_id | uuid null | |

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
| agent_id | uuid null FK→agents (cascade) | null ⇒ org skill |
| name | text not null | `[a-z0-9]+(-[a-z0-9]+)*` ≤64, immutable; `aurora-*` reserved |
| description | text not null | single line ≤500 |
| body | text not null | `SKILL.md` body |
| enabled | bool not null default true | |
| created_by_kind | text CHECK in (`user`,`agent`) | |
| unique | (agent_id, name) where agent_id not null; (org_id, name) where agent_id null | |

### `skill_files`
| Column | Type | Notes |
|---|---|---|
| skill_id | uuid FK→skills (cascade) | pk part |
| path | text | pk part; relative, validated |
| content | bytea not null | |
| executable | bool not null default false | |

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
| last_outcome | text null CHECK in (`ok`,`failed`,`skipped_busy`,`gated_quiet`) | |

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
| stop_reason | text null CHECK in (`idle`,`cap`,`evict`,`restart`,`delete`,`failure`) | |
| stop_requested_at | timestamptz null | |
| restart_requested_at | timestamptz null | |
| last_engaged_at | timestamptz null | person/turn engagement clock |
| last_active_at | timestamptz null | stamped also by every boot |
| home_applied_generation | bigint not null default 0 | reset to 0 on each start claim |
| listening_ports | int[] not null default `{}` | churn column |
| failure_code | text null; failure_message text null | |
| resource_profile | jsonb null | effective profile of current boot |
| start_failures | int not null default 0 | consecutive failed starts (backoff); reset on running; spec fix S-10 |

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
| token_hash | bytea unique | `aun_` |
| display_name | text not null | from manifest, sanitized |
| version | text null | announced |
| contract | int null | announced |
| manifest | jsonb null | latest announced |
| manifest_digest | text null | |
| approved_manifest_digest | text null | |
| status | text CHECK in (`awaiting_registration`,`pending_approval`,`active`,`disabled`) | |
| approved_by | uuid null; approved_at timestamptz null | |

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
| config | jsonb not null | non-secret setup fields |
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
| integration_id | uuid null FK→channel_integrations (cascade) | |
| presence_id | uuid null | |
| kind | text not null | contract op kind (`13 §Operations`) |
| payload | jsonb not null | |
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

When `database.gateway_role` is configured, the migrations grant that role:
`SELECT` on identity/tenancy tables it needs (`organizations`, `workspaces`,
`workspace_access`, `org_members`, `users` (id, email, disabled_at only via a
view `gateway_users`), `api_keys`), `agents`, `gateway_credentials`, `secrets`,
`app_connections`, `app_oauth_clients`, `grants`, `derived_rules`,
`policy_generations`, `policy_rules`, `registry_snapshots`, `plugin_packages`
(status only via view), `host_claim_decisions`, `org_app_availability`;
`INSERT, UPDATE` on `approvals`, `gateway_instances`; `INSERT` on
`request_log`; `UPDATE (credentials_ref, credentials_version,
credentials_expires_at, status, status_reason, reauth_required_at,
last_used_at)` on `app_connections`; `UPDATE (last_used_at, origin)` on `secrets`;
`EXECUTE` on `pg_advisory_lock` family; `LISTEN` is implicit.
