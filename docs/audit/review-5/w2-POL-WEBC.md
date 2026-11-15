# REVIEW-5 wave 2 — POL (policy/approvals/freeze) and WEBC (web core) — saved by coordinator

## POL-1 (high, CONFIRMED; a test pins it) — a lower authority can lift a freeze a higher authority placed; a re-freeze by a higher authority is silently dropped
- `crates/aurora-db/src/repo/freeze.rs:122-144` (`UPDATE … WHERE id=$1 AND frozen_at IS NULL`), `:50-57` (ranks); `services/aurora-api/src/domains/freeze/service.rs:138-196` (`created == false` skips effects + audit), `:198-251` (unfreeze allows rank >= stored). Test `tests/it/freeze.rs:144-150`.
- D-50 (docs/90:157) "a higher authority's freeze must not be reversible by a lower one"; SEC-45.
- Fix: re-freeze by a higher authority raises `frozen_authority` (+ by/ reason) and writes an audit row (`escalated: true`), for agent/workspace/org scopes.

## POL-2 (medium) — gateway and batch approvals decidable after their deadline until the sweep (30 s grace; never if the sweep is down)
- `crates/aurora-db/src/repo/approvals.rs:221-245` (`decide`), `:271-306` (`decide_batch`), `:308-318` (`expire_overdue` deadline + 30 s; `jobs.rs:10-11`); `approvals/service.rs:61-95`, `:120-150`. Compare `action_approvals/service.rs:191-197`. docs/07 §Approval deadlines "Expiry still denies".
- Fix: `AND deadline > now()` in both UPDATEs; expire on miss → 409 `already_decided`.

## POL-3 (medium, PLAUSIBLE, spec-defect) — literal path rule doesn't match the trailing-slash form (`/v1/users` vs `/v1/users/`); a block is bypassable
- `crates/aurora-policy/src/path.rs` `split()`/`matches()` (unit test asserts `!m("/v1/users", "/v1/users/")`); `services/aurora-gateway/src/hygiene.rs:195`. docs/07 §Matching primitives silent; "doubt only tightens".
- Proposed spec: trailing `/` not significant for matching.

## POL-4 (low) — non-ASCII host patterns accepted at authoring, never match punycode (`crates/aurora-policy/src/host.rs parse_inner`/`matches`; `policy/validate.rs:119`) → reject non-ASCII with hint
## POL-5 (low) — policy rollback overwrites the draft with no revision check (`policy/service.rs:100-130`; `repo/policy.rs:268-293`)
## POL-6 (low) — app blocklist that becomes empty leaves its old derived block rule (`connect/service.rs:510-527`)
## POL-7 (low) — SSE event for a freeze-abandoned approval labelled `expired` (`freeze/service.rs apply_effects`)

## POL not covered: gateway hold/park/resume; approval notify fan-out; action-approval handlers; budgets; automation fire-time authority; policy warnings.rs; bulk_check.

---

## WEBC-1 (medium) — approvals SSE swallows `Resync`/`Lagged`; pending list can stay stale
- `services/aurora-api/src/domains/approvals/routes.rs:117`; `approvals/bus.rs:19`; `web/src/lib/api/approvals.ts:98-107`. Conversations stream handles it (`conversations/routes.rs:573`). C-10.
- Fix: send `event: resync`; client invalidates.

## WEBC-2 (medium; extends WEBO-6) — more single-page list hooks: `account.ts:49` useMyApiKeys, `:144-150` useWsKeys; `org.ts:104-115` useOrgInvitations, `:134-145` useOrgKeys; `approvals.ts:57-59` fetchPending (limit 200); `action-approvals.ts:36-41` (limit 100 → later ones undecidable from UI); `governance.ts:80-92` blocked-host count (`slice(0,10)` workspaces + limit 100)

## WEBC-3 (low) — connect popup redirect keeps `window.opener` (reverse tabnabbing) (`web/src/components/apps/connect-view.tsx:101-104`; `lib/api/apps.ts:293-295`) → `window.opener = null` before `location.assign`
## WEBC-4 (low) — secret plaintext retained in the mutation cache 5 min (`lib/api/secrets.ts:54-60`, `:74-80`, `:102-108` no `gcTime: 0`; others set it)
## WEBC-5 (low) — `safeNext` accepts control chars the server rejects → raw JSON error (`lib/auth/use-unauthorized-redirect.ts:9-11` vs `oidc/service.rs:70-86`)
## WEBC-6 (low) — policy publish/rollback/draft save don't invalidate agent `effective`/`grants` caches (`lib/api/policy.ts:74-106`, keys :31-32)
## WEBC-8 (low) — one-off widths (`max-w-[880px]` admin-users-view.tsx:22, governance-view.tsx:118; `max-w-[1160px]` repeated)
## WEBC-9 (low, spec) — docs/14 Account section still describes Web Push / notification switches (phase 5 per D-138)

## WEBC not covered: grant editor/give-access/proposals/llm-form; rule summaries vs enforcement (bidi chars), simulate-panel; governance/BYO units; mocks vs openapi; 390 px.
