# REVIEW-5 wave 2 — TOOL (agent tools etc.), PLUG (plugin system), AUTH (auth/sessions/tenancy) — saved by coordinator

## TOOL-1 (high, CONFIRMED; C-29 vs docs/04+07 conflict) — a plain workspace member can set their agent's link side to `allow`, bypassing the approver-only decision for cross-workspace messaging
- `services/aurora-api/src/domains/agent_links/service.rs:57-68` (`set_policy` → `load_agent` → only `c.workspace()`), `agent_links/routes.rs:40`. docs/07 §Agent links ("phase 2, RFE-012: `can_approve`"); docs/04 ~531; D-55; C-29 (API_CONTRACTS:496, :527 "caller needs can_use") is the conflicting side; docs/05:720.
- Fix: require `can_approve` for `allow`; keep `can_use` for `blocked`; amend C-29.

## TOOL-2 (medium, CONFIRMED) — cron minimum-interval check examines only four gaps (`schedules/cron.rs:43-56`); `0,5,10,15,20,21 * * * *` accepted at the right minute
## TOOL-3 (medium, CONFIRMED) — PATCH of schedules and agent `webhook_update` write back a stale full row with no lock, clobbering fire-time state (double fire; stale secret_ref) (`schedules/routes.rs:173`; `schedules/service.rs:135-180`; `repo/schedules.rs:121-137`; `tools/webhooks.rs:159`; `repo/webhooks.rs:192-208`)
## TOOL-4 (medium, unwired/spec gap) — no per-agent cap on skills or skill files (`tools/skills.rs:267-290`; `skills/service.rs:409-442`, `:489-520`; `repo/skills.rs:190-210`) → add SKILL_PER_AGENT_MAX, SKILL_FILES_MAX
## TOOL-5 (medium) — peer tasks queue without bound (`peer_tasks/service.rs:256-282`; `repo/peer_tasks.rs:105`) → PEER_TASK_QUEUE_MAX
## TOOL-6 (medium, unwired/spec gap) — `webhook_deliveries` never pruned; rejected deliveries written per request (≤ 60/min) (`webhooks/receive.rs:150-190`, `:116-127`; `repo/webhooks.rs:285-300`)
## TOOL-7 (low) — webhook `Content-Type` header interpolated outside the untrusted fence (`webhooks/receive.rs:234-238`, `:409-420`); SEC-31
## TOOL-8 (low) — skill file validation checks only the final segment for `skill.md` (`skills/service.rs:104-128`)
## TOOL-9 (low, spec) — no replay window for unkeyed signed webhook deliveries (`webhooks/receive.rs:23-27`, `:437-453`)
## TOOL-10 (low, PLAUSIBLE) — tool calls not gated on the caller agent's freeze state (`tools/service.rs:38-80`; `peer_tasks/service.rs:124-127`); SEC-45
## TOOL-11 (low) — `WATCH_PATTERN_MAX` unused (supervisor `process.rs:38` own const); schedule (50) / webhook (20) per-agent caps not in docs/02; `attachments/quota.rs:9-10` comment wrong

---

## PLUG-1 (high, CONFIRMED; spec gap) — an upgrade that changes `package.publisher` auto-activates: any trusted publisher key can take over another publisher's installed package id, including a built-in
- `services/aurora-api/src/domains/plugins/ingest.rs:179-194` (`decide`: `previous_approved` by package_id only), `:226-230`; `crates/aurora-registry/src/diff.rs:57-117` (`widens` never compares publisher); `repo/plugins.rs:138-150`. SEC-15.
- Fix: publisher change = first install → `pending_approval` (`publisher_changed`); refuse upload/directory packages with a builtin-owned id.

## PLUG-2 (high, CONFIRMED) — key revocation misses packages whose SIGNATURE names the key in a non-canonical base64 spelling (URL-safe vs standard)
- `crates/aurora-registry/src/signature.rs:148-170`; `ingest.rs:217-221`; `crates/aurora-crypto/src/ed25519.rs:10-17` (`b64` accepts STANDARD, URL_SAFE_NO_PAD, STANDARD_NO_PAD); `plugins/keys.rs:97-117`; `repo/plugins.rs:272-288` (`disable_signed_by` string equality); `lifecycle.rs:132-135`. SEC-15 "revoking a key disables its packages".
- Fix: canonicalise key_id (decode → re-encode STANDARD; reject non-canonical).

## PLUG-3 (high, CONFIRMED; spec gap SEC-17) — a static OAuth `token_url` isn't checked against any claimed/declared host; changing it is not a widening → client secret + refresh tokens go to any public https host after an auto-activated upgrade
- `crates/aurora-registry/src/validate/mod.rs:297-366` (`check_oauth_url`); `diff.rs:31-47` (zones cover Templated only); `connect/service.rs:1083`, `:1098-1138`; `services/aurora-gateway/src/refresh.rs:541-552`.
- Fix: token/authorize hosts must be claimed or declared (`permissions.token_hosts`), shown at approval, change = widening.

## PLUG-4 (medium) — no version ordering: an older signed version with no widening auto-activates (rollback) (`ingest.rs:179-194`; `repo/plugins.rs:138-150`; `plugins/scan.rs:43-115`)
## PLUG-5 (medium) — minor/patch upgrades that change the required credential vocabulary are never gated (flip check only for major) (`plugins/upgrade.rs:147`; `ingest.rs:274-289`; `diff.rs:57-117`; `crates/aurora-registry/src/upgrade.rs:24-29`); SEC-57 scope
## PLUG-6 (medium) — hook host holds one lock across module pull + compile; one slow package stalls every hook call and snapshot refresh (`services/aurora-hookhost/src/snapshot.rs:182-209`, `:150-153`; `server.rs:171`)
## PLUG-10 (medium, test-gap) — invalid-fixture set lacks entries for 14 rule codes (`plugins/fixtures/expected-invalid.txt`: hooks_wasm_import_forbidden, hooks_wasm_export_unexpected, hooks_wasm_not_component, hooks_wasm_invalid, unknown_context, unknown_oauth_client, bad_tenant_field, bad_version, archive_corrupt, archive_entry_type, guide_not_utf8, manifest_not_utf8, tool_path_invalid, undeclared_field); sandbox import ban test env-dependent
## PLUG-7 (low) — `connect.authorize` output URL only scheme-checked (`hookhost/src/validate.rs:110-111`)
## PLUG-8 (low) — `random_bytes` returns zeros when /dev/urandom fails (`hookhost/src/runtime.rs:195-204`)
## PLUG-9 (low) — docs/09:59-60 standing dashboard warning for `plugins.allow_unsigned` not implemented; "fuel" wording wrong (§8.3)
## PLUG-11 (low) — no manifest size limit; decompressed cap 4×PACKAGE_MAX undocumented (`manifest/mod.rs:48-58`; `package.rs:31-40`)

---

## AUTH-1 (high, CONFIRMED; spec self-contradiction docs/04:226 vs :32) — JIT sign-in re-adds a member an admin removed, on every sign-in (non-SCIM orgs: removal is a hard DELETE, no tombstone)
- `oidc/service.rs:629` (`apply_jit` every sign-in), `:794-828`; `repo/members.rs:173`. Test `tests/it/oidc.rs:540` covers first sign-in only.
- Fix: JIT only on the sign-in that creates the user, or a removal tombstone; fix docs/04:226.

## AUTH-2 (medium, PLAUSIBLE) — re-auth freshness skipped when the IdP omits `auth_time` (`oidc/service.rs:217-221`, `:520-524`; docs/04:243-244); gates account deletion + SCIM token creation
## AUTH-3 (medium, PLAUSIBLE) — device-code approval shows the approver no context (hostname/IP/time) → phishing; CLI keys never expire (`cli_login/service.rs:221`, `:101-150`; `web/src/components/auth/cli-view.tsx:76-96`; docs/04:409-416)
## AUTH-7 (medium, test-gap) — permission matrix cannot fail for validation-first routes: 400/422 on a denial row is INCONCLUSIVE and passes (`tests/it/permission_matrix.rs:1269-1282`); unseeded paths accept any of 401/403/404
## AUTH-4 (low) — auth-cache put after revocation can keep a revoked session/key ≤ 30 s (`http/auth.rs:500`, `:526`, invalidate ~:280)
## AUTH-5 (low) — revoking instance-admin via PATCH not propagated to the auth cache (≤ 30 s) (`admin/service.rs` patch_user ~33-72; `repo/account.rs:213`)
## AUTH-6 (low) — `user.admit_by_claim` not in SECURITY_CRITICAL_ACTIONS (`crates/aurora-core/src/audit.rs`; `oidc/service.rs:747-763`); SEC-78, D-143
## AUTH-8 (low, PLAUSIBLE, spec) — e-mail account linking crosses providers; `trusted_email_domains` on a self-service IdP = takeover (`oidc/service.rs:571-612`, `:353-354`, `:606`); D-118
## AUTH-9 (low, decision check) — owner-integrity floor can make the first signer owner when the mapping grants only member (`oidc/roles.rs` ensure_org ~170-190, promote_to_owner_floor ~214-270); D-139(d) vs D-165(a)
