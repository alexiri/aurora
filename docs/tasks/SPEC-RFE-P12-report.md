# SPEC-RFE-P12 report

Branch `alexiri/spec-rfe-p12`, HEAD `7b06b797`. Spec only (docs/), no code touched. 22 files, +1835/-168.

## Verification (all green)
- `python3 scripts/extract-schema.py docs/03-data-model.md`: extract unchanged (md5 63e7dd6a…, same before/after; fixture json not modified).
- `cargo test -p aurora-db --test schema_matches_spec`: 1 passed, 0 failed (needed the shared `.env` copied for DATABASE_URL; `.env` is gitignored).
- `bash scripts/guards/run-all.sh`: all guards passed (schema, no-integration-names, dep-graph, edge-routes Caddyfile + helm Ingress + HTTPRoute).
- Extra: markdown table column-count check over all changed docs: 0 irregular rows.

## Commits (one per RFE) and docs changed
Every commit also adds its decision row to 90 (D-43..D-58), its SEC invariant(s)/residual risk to 17, its proving tests to 18, and limits/config to 02/15 where new.
- RFE-001 `569df4db` D-43: 00, 01 (trust boundary 7), 03 planned (`config.verified_email_autolink`, `link_requests.platform_email_hint`), 05, 13 (§4.5, §5.1 step 4, §5.9, §6), 14, 17 (SEC-19/20, residual), 18, 20.
- RFE-002 `d683028a` D-44: 01, 02, 06 (§Listeners + new §Proxy listener TLS), 08 §10, 11 (boot doc), 12 (loopback), 15 (networking, config keys, compose, Helm), 16 (W1), 17 (SEC-40), 18.
- RFE-003 `7bdba39c` D-45: 03 planned (`policy_generations.open_lane_posture`, `gateway_credentials.capabilities`, org governance key), 05, 06 (admission, codes), 07 (new §CONNECT-time evaluation), 08 (§11.1/11.2/11.6/11.7, §7 capabilities), 11 (image label, boot `capabilities`), 12, 14, 15, 16 (`image_capabilities`), 17 (SEC-41, residual), 18.
- RFE-004 `9324db18` D-46: 02, 03 planned (`password_hash`, `email_tokens`, `sessions.reauth_at`), 04 (sign-up modes, instance admins/bootstrap-admin, Authentication, OIDC, re-auth, dev/e2e IdP, rate limits, deletion), 05 (removed routes), 14, 15, 17 (SEC-42, residual), 18.
- RFE-009 `6b3ec3b2` D-47: 04 (`can_see_conversation`, matrix), 05 (new §Conversation visibility), 07, 10 (new §Visibility), 13 (§5.5), 14, 17 (SEC-43, residual), 18.
- RFE-016 `3f180723` D-48: 01, 02, 05 (new §delta bus seam), 15 (§Sizing ceiling + config), 18.
- RFE-005 `669968f6` D-49: 03 planned (`gateway_credentials.speaker_user_id`, `request_log.speaker_user_id`), 05, 06, 07 (anatomy, warnings, simulate/effective), 08 §7, 14, 17 (SEC-44, residual), 18.
- RFE-006 `9239265a` D-50: 02, 03 planned (frozen_* on agents/workspaces/orgs, stop_reason, last_outcome), 04 (new §Freeze, matrix), 05 (due-work, invalidation, routes), 06, 07, 10, 11, 13 (§5.1), 14, 17 (SEC-45), 18.
- RFE-007 `51ebddf7` D-51: 02, 05, 06, 08 §4 (rings + rotation procedure), 13 (§4.4 `v2.` tokens), 15, 16, 17 (SEC-46, residual), 18.
- RFE-008 `c4f0ec4c` D-52: 03 (note + planned `audit_log.critical`), 04 §Audit (security-critical set), 05, 07, 14, 17 (SEC-32), 18.
- RFE-010 `f7c032c2` D-53: 02, 03 planned (archived_*, redacted_*, `session_purges`, org `settings.retention`), 04, 05, 10 (new section), 12 (§Session purge, `purge_session`), 14, 15, 16 (`session.purge`/`session.purged`), 17 (SEC-47, residual), 18.
- RFE-015 `844fe6c1` D-54: 02, 03 planned (`attachments.purged_at`, `org_storage_usage`), 05, 10 §Attachments, 13, 14, 15, 17 (SEC-48), 18.
- RFE-012 `3ca64834` D-55: 03 planned (`workspace_access.role`, `invitations.approver_workspace_ids`), 04 (`can_approve`, matrix, invitations, JIT), 05, 07, 13 (§5.5/5.6), 14, 17 (SEC-28), 18.
- RFE-011a `54d43177` D-56: 02, 03 planned (rule/grant timeouts, `approvals.resolved_at`, `approval_watchers`, `approval_notifications`, `users.approval_email_enabled`), 05, 06, 07 (new §Approval deadlines, stall exemption, e-mail), 10, 14, 15, 17 (SEC-49), 18.
- RFE-017 `aeba2c1c` D-57: 02, 04, 05 §Conventions (buckets, 429), 14, 15, 17 (SEC-50), 18.
- RFE-021 `948c4faa` D-58: 03 planned (`anchor`), 04 §Fire-time authority, 05, 10, 14, 17 (SEC-51), 18.
- Summary `7b06b797`: MASTER_SPEC bullet, 19 note.

## Choices made (defaults I picked; please review)
1. RFE-002: the CA-issued proxy leaf (30 d, renewed < 10 d) is the default; SANs = advertised host, sandbox-facing address, k8s service names, compose `10.89.250.2`, `gateway.proxy_tls.names`. Operator cert optional. The client→loopback-forwarder hop inside a sandbox stays plain `http://127.0.0.1:3128` (credential-free, never leaves the sandbox): the only exception to "https everywhere". I also introduced `aurora run --forward` as the workaround for clients that cannot speak to an HTTPS proxy.
2. RFE-003: open lane is admitted only for `aub_` boot credentials minted with the `browser` capability (BYO `aug_` and `auc_` never). Capability source = sandbox image label `org.aurora.capabilities` (runner reports `image_capabilities`) ∩ effective posture. `allowlist_only` checks each level that sets it against its own list (instance baseline, org list ∪ baseline, workspace list). Path-scoped allow rules and allow-list entries do not admit tunnels. No per-agent switch (workspace/org/instance only).
3. RFE-004: `require_email_verified` removed (always true); phase-1 migration clears `password_hash`, later release drops the column and `email_tokens`; account deletion needs OIDC re-auth (`reauth=1`, `sessions.reauth_at`); dev IdP = `aurora-dev-idp` packaged from `aurora_testkit::oidc`, refused on a non-loopback https URL; `bootstrap-admin` pre-provisions a user bound at first OIDC sign-in.
4. RFE-009: owners get read + abort/send-now on others' direct conversations but cannot post (403 `direct_conversation_forbidden`); approvals naming an invisible direct conversation are hidden from non-participants (decider set = approvers ∩ visibility); request-log rows of invisible direct conversations are omitted (not just redacted); owner reads write a best-effort `conversation.owner_view` audit row.
5. RFE-005: BYO context credentials never carry a speaker (the agent mints them itself). Documented residual: speaker binding is attribution, not isolation, within one sandbox.
6. RFE-006: field name `frozen_*`; `frozen_authority` ordering `workspace_owner < org_admin < instance_admin`; org freeze by org owner or instance admin; gateway answers 403 `frozen` (not 407); credentials not revoked; sandbox `stop_reason = frozen`; schedules skipped not disabled; API 423 `frozen`.
7. RFE-007: rings use the existing variable names plus `*_ACTIVE_KEY`; bare value = ring id `v0`; ≤4 entries; action tokens `v2.<key_id>.…`; internal calls add `X-Aurora-Key-Id`; runner session key and gateway CA explicitly out of scope.
8. RFE-008: critical set enumerated in 04 §Audit (includes secrets/connections, runner/connector tokens, user links, erasure and reassignment); audit failure ⇒ 503 `audit_unavailable`; `audit_log.critical` column planned.
9. RFE-010: delete also purges the harness session files in the agent's home (`session.purge`, `session_purges` table, executed before any turn after a stopped sandbox boots); redaction can reset the session; retention bounds 7..3650 days; export capped at 256 MiB/200k events.
10. RFE-015: default org attachment quota 10 GiB, instance ceiling 50 GiB; retention purges bytes and keeps the row (410 on download).
11. RFE-011a: "stall exemption" interpreted as: a turn with a `pending` approval is not failed `turn_stalled` (clock restarts at `resolved_at`); `TURN_CEILING` still applies; instance max timeout default 3600 s (≤ 14400, < ceiling − warning); effective timeout = minimum of set values; "live surface" = delivered channel card or a fresh watcher (`approval_watchers`); e-mails carry title + dashboard link only, never decision links; browser notification is opt-in and tab-bound (Web Push stays RFE-011b).
12. RFE-012: members lose approval authority at upgrade (migration leaves roles unchanged; no silent privilege). Plain members do not see the approvals feed. No separation-of-duties rule.
13. RFE-017: buckets per principal (user shared across sessions, key, BYO agent) + per-org bucket + stream cap; default numbers in 05 §Conventions; machine planes and webhooks exempt.
14. RFE-021: scope = schedules and webhooks (peer tasks/reach unchanged); `reassign_to` on member removal/suspension and binding removal; agents can never create workspace-anchored automations.

## Open questions / risks for the coordinator
- RFE-001 residual: a malicious connector can still name already-linked users as speaker/clicker (inherent to the contract); documented in 17, not changed.
- RFE-012 upgrade: existing plain members lose approval ability; owners must promote approvers. Confirm no opt-in migration helper is wanted.
- RFE-002: confirm the sandbox loopback hop staying `http://127.0.0.1:3128` is acceptable against the hint "`HTTPS_PROXY=https://…` everywhere".
- RFE-009: hiding request-log rows (rather than redacting) reduces member visibility of agent activity; confirm.
- New implementation surface not named by the decisions: `aurora-dev-idp`, `aurora run --forward`, `session_purges`, `approval_watchers`, `org_storage_usage` (all planned, none in phase-0 schema).
- Phase-tag numbering: new SEC-40..SEC-51 are appended by RFE processing order (not by phase); 90 rows D-43..D-58 likewise.
