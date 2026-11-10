# P4-GRANTS: App-level "custom grants only" and parameter-level tool matching (Q8)

## Goal
Implement D-123 (manifest `apps[].grants { custom_only, block_untooled }`) and D-124 (Q8: optional per-tool `params` matching), as specified by SPEC-P4a. The owner's motivation: Indico's broad `full:everything` token must be confinable to its catalogue, and read-only AWS calls (`Describe*`, `GetCallerIdentity`) should be read tools instead of held writes.

## Spec (source of truth; read, don't re-decide)
- D-123, D-124 in `docs/90-decisions.md`; SEC-67, SEC-68 in `docs/17-security.md`.
- `docs/09` §3.4 (manifest: `grants`, tool `params` classifier, linter rules); `docs/07` (grant modes, evaluation order, `app_untooled_blocked` before rules, conversion on activation); `docs/06` (classification, snapshot refusal for an unknown classifier); `docs/05` §Grants and effective views (422 `full_grant_forbidden`, `app_restrictions` in grant responses); `docs/14` agent Access bullet (UI copy); `docs/03 §Planned changes` (grants / plugin_packages / snapshot rows).
- `docs/18` §Custom grants only and parameter-level matching (the tests).

## Scope
1. **Registry** (`crates/aurora-registry`): parse/validate `grants` and tool `params` (1–4 conditions; `source` query|form|header=x-amz-target; `match` equals|in|prefix); linter requirements (negative fixtures); snapshot carries `apps[].grants` and `params`.
2. **Policy/evaluator** (`crates/aurora-policy`): `custom_only` defence in depth (a toolless target never allows/selects such an app's connection); `block_untooled` blocks before rules; `params` classification with fail-closed rules (exactly-once, no query/body conflict, urlencoded uncompressed valid UTF-8 untruncated body, else no match → fall through).
3. **Gateway**: buffer the body only when a `form` tool may match; refuse a snapshot that uses a classifier it doesn't know (previous kept).
4. **API**: `full_grant_forbidden` (422, `details.app_id`) in the **shared grant function** FIX-020 created (`domains/grants/service.rs` `grant_*_in_tx`), so agent-side PUT and the credential-side bulk routes both enforce it; `app_restrictions` on grant responses, `GET …/grants` and the bulk response (C-504); activation that introduces `custom_only` converts existing full grants to custom grants with exactly today's tools in the activation transaction, audited per grant, held for approval with `converts_grants` (pattern of RFE-022's `breaks_connections`, `domains/plugins/upgrade.rs`).
5. **Packages**: the Indico built-in adopts `custom_only` (and `block_untooled` if the spec says so); the AWS packages adopt `params` for `ec2_describe` / `sts_get_caller_identity` per D-124 (splitting a tool: new ids arrive blocked in existing custom grants — say so in the package changelog). Re-run `scripts/plugins/check-builtin.sh`.
6. **Web**: replace FIX-021/FIX-022's defensive reads with the real `app_restrictions` field (`web/src/lib/api/apps.ts` `appIsCustomOnly`/`appRestrictions`); "Custom access only" badge on catalogue and connection cards; conversion notice on the Access tab; plugin approval banner "N agent grants will be converted".
7. **Contracts**: write `docs/API_CONTRACTS.md` entries **C-410..C-419** only (section "Phase 4 — P4-GRANTS"). OpenAPI + `schema.d.ts` regenerated.

## Acceptance
- Every test listed in `18` for this item, SEC-67 and SEC-68 rows set to `proven` with test names; the FIX-020 bulk routes covered for `full_grant_forbidden`.
- `cargo test` for registry, policy, gateway (`-p aurora-gateway`), full `aurora-api` (`--test-threads=8`); `bash scripts/plugins/check-builtin.sh`; `bash scripts/e2e/gateway.sh` (ask the coordinator before running it); web typecheck/lint/test/build + Playwright `stage4-routes`/`stage4-axe` for changed screens; fmt, clippy `-D warnings`, guards, OpenAPI drift test.

## Constraints
- Never loosen a grant: the flags only tighten; `params` doubt never yields `read`.
- Change files only with the Edit/Write tools; never python, sed, awk, perl, `cat >>` or heredocs.
