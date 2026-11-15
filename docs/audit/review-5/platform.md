# REVIEW-5 findings — platform (saved by the coordinator from the reviewer's hand-back)

## PLAT-1 (high, CONFIRMED) — deleting an org fails whenever it has a directory group, SCIM token or SCIM link (FK RESTRICT; `delete_cascade` not extended for phase-4 tables)
- `crates/aurora-db/src/repo/orgs.rs:109-131`; `migrations/20261130000001_groups.sql:11`; `migrations/20261201000001_scim.sql:18`, `:34`; `tenancy/service.rs:335-340`. docs/03:22-25; docs/04:1082-1084. Only org-delete test `tests/it/tenancy.rs:193` has none of these.
- Fix: delete `scim_users`, `scim_tokens`, `groups` before `organizations`.

## PLAT-2 (medium, CONFIRMED) — SCIM refusal of an invitation is audited in a transaction that is rolled back (denied row lost)
- `invitations/service.rs:293-303`, `:214`, `:238-241`, `:270`; contrast `oidc/service.rs:812-824`. docs/04:236-238.

## PLAT-3 (medium; = SCIM-5) — new-user placement bypasses SCIM exclusive mode and the seat cap `max_members`
- `placement/service.rs:122-133`; `oidc/service.rs:632`; cf. `invitations/service.rs:293`, `:311`. docs/04:883-886, :1067-1068, :743 (ambiguous → decide).

## PLAT-4 (medium, spec-defect) — fresh-instance "being set up" gate closes when the admin's own first sign-in creates an org
- `placement/service.rs:66-72`; `oidc/service.rs:698-704`, `:765-775`. docs/04:125-128. Fix: persisted marker cleared by first saved placement.

## PLAT-5 (medium, PLAUSIBLE) — account deletion leaves a personal workspace ownerless and offered as a shared-workspace choice
- `migrations/20261124000001_new_user_placement.sql` (ON DELETE SET NULL); `repo/placement.rs:130-133`; `placement/service.rs:537`; docs/04:1074-1078.

## PLAT-6 (medium, unwired) — `previews.session_ttl_secs`, `session_idle_secs`, `bytes_per_share_day` parsed, never validated or used
- `crates/aurora-config/src/raw.rs:575-592`; docs/15:96.

## PLAT-7 (low) — SCIM token lifetime ceiling: 730 d (docs/02:305, docs/03:225, `limits.rs:242`) vs 1825 d (validate.rs:513-514, docs/15:202)
## PLAT-8 (low, unwired) — constants with no enforcing code: LIBRARY_REFRESH_INTERVAL (limits.rs:118), SCIM_RATE (:226), SCIM_TOKEN_MAX_DAYS, TERMINAL_AUTH_CACHE, TERMINAL_AWAKE_CAP, TERMINAL_WAKE_WAIT; PREVIEW_AWAKE_GRACE, PREVIEW_LISTEN_WAIT, PREVIEW_AUTH_CACHE_NEG, PREVIEW_REQUEST_BODY_MAX, PREVIEW_AWAKE_CAP, PREVIEW_IDLE_TIMEOUT
## PLAT-9 (low) — name sanitising keeps Unicode format characters (bidi overrides, zero-width) (`placement/template.rs:175-184`; `oidc/service.rs:428-436`; `tenancy/service.rs:42`)
## PLAT-10 (low, PLAUSIBLE) — migrations take ACCESS EXCLUSIVE for constraints/indexes on large tables (`20261130000001_groups.sql:7`; `20261127000001_skill_libraries.sql`)

## Not covered
- `oidc/roles.rs` owner floor; validate.rs key-by-key for relay/terminal; depth of each permission-matrix exemption's test; whether LLM endpoint base_url in /admin/setup can carry credentials.
