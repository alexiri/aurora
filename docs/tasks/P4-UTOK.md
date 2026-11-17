# P4-UTOK: user-token features (send-as-me through a per-person token)

## Goal
A person can give the agents they choose a token of their own Mattermost account, so that an agent can post as that person, and only that way: only in the person's own turns, only through a grant the person made, and only after the person approves the text. Aurora seals the token, delivers a copy to the connector for one operation, and revokes everything on any end of the person's standing. Read-as-user is not in this brief (Q-UTOK-1).

## Linked requirements
- D-38 (read-as-user and send-as-me dropped from phase 0, "requires `user_tokens`"); D-225 (moved to phase 5, needs a spec task).
- D-281 (send-as-me only, pasted personal access token matched to the caller's link); D-282 (sealed custody, per-op copy); D-283 (who may act: owner's own turns, grants, one approval per send, footer); D-284 (revocation cascades, audit, limits).
- Contracts C-853 (routes), C-854 (`user_token.verify` op), C-855 (`UserToken`, grants), C-856 (`send_as_me` and approval), C-857 (`message.post` with `sender.user_token`), C-858 (audit, revocation, limits), C-859 (tests, dashboard card) in `docs/API_CONTRACTS.md` §Phase 5 — P4-NARR / P4-UTOK.
- SEC-112 (custody), SEC-113 (confused deputy, scope, revocation, audit) in `docs/17-security.md`. Security-critical: an Opus review is required before merge.
- Spec sections: `docs/13-channels-connectors.md` §4.2 (ops), §4.5 (`user_tokens`), §5.10 (`send_as_me`), §5.11 (this feature), §5.9 (links); `docs/20-mattermost-connector.md` §Operations, §Rendering (footer); `docs/08-secrets-credentials.md` §1 (purposes `user_token`, `channel_op_user_token`); `docs/07-policy-approvals.md` §Action approvals; `docs/03-data-model.md` §Planned changes (`user_tokens`, `user_token_agents`, migrations `20261201000232`, `…000233`); `docs/02` §Limits (`USER_TOKENS_PER_PERSON`, `USER_TOKEN_PENDING_MAX`); `docs/18-testing.md` §User tokens; `docs/14-web-dashboard.md` §Account (Send as me).

## Context
- Link and identity: `crates/aurora-db/src/repo/user_links.rs` (`platform_user_ref` per integration); the link flow `services/aurora-api/src/domains/channels/links.rs`.
- Connector ops, claim and copies: `services/aurora-api/src/domains/connectors/ops.rs` (`claim`, the delivery point for `credentials` on the deprovision op); the deprovision copy pattern to mirror is `services/aurora-api/src/domains/channels/deprovision.rs` (sealed copy in `payload`, discarded at terminal state by the sweep); `crates/aurora-db/src/repo/channel_ops.rs` (`insert`, `by_dedupe_key`).
- Approvals: `services/aurora-api/src/domains/action_approvals/handlers.rs` (action keys, frozen payload, decide, expiry) and `crates/aurora-db/src/repo/action_approvals.rs`; decide rights in `crates/aurora-db/src/authz.rs`. The new key `send_as_me` must decide by owner only, not by `can_approve`.
- Agent tools: `services/aurora-api/src/domains/tools/messaging.rs` (`send_message` is the model to follow for platform resolution and the proactive mode; `send_as_me` is not a proactive mode and is never `allow`-able). The turn's speaker and door: `domains/channels/inbox/speaker.rs` and `inbox/message.rs`.
- Capability: `crates/aurora-channels/src/capability.rs` (`UserTokens = "user_tokens"`, line ~23 and the note at ~130 that absence hides read/send-as-user features).
- Revocation triggers: user delete (`domains/me` or `domains/auth` user lifecycle; verify), SCIM deprovision (`services/aurora-api/src/domains/scim/deprovision.rs`), agent delete (`domains/agents/service.rs`), integration delete (`domains/channels/integrations.rs`), link removal (`DELETE /v1/me/channel-links/{id}`).
- Secrets: `crates/aurora-secrets` (seal, open, discard with purpose and owner binding; SEC-11).
- Mattermost: `connectors/mattermost/src/{ops.rs, api.rs, render.rs, convert.rs, managed.rs}`; the connector's HTTP client for a person's token must be a separate client type that can reach only the endpoints of C-857 (no shared client with the bot token).
- Dashboard: Account page under `web/src/…` (locate the chat-platform links section; verify). Shared primitives only.

## Scope
### In scope
**Part A — API (Rust, Sonnet)**
- Migrations `20261201000232_user_tokens.sql` and `20261201000233_user_token_agents.sql` (columns and partial unique index per `docs/03` §Planned changes); repo modules; the rows move into `docs/03` real sections in the same change.
- Routes C-853, C-855 under a new domain module (e.g. `services/aurora-api/src/domains/user_tokens/`); verify flow: seal, `user_token.verify` correlated op, compare with the caller's link, discard on refusal (C-853 table). Request-log omission of the token body.
- Claim path (`domains/connectors/ops.rs`): for an op with `sender.user_token`, in the claim transaction take the copy only when the token is `active`, its grant is active and the link exists; deliver `user_credential`; strip the stored copy; the terminal-state sweep discards it (C-857).
- `send_as_me` tool (C-856) in `domains/tools/messaging.rs`: eligibility at the call (D-283(a)); creates an action approval `send_as_me` with the frozen payload and the platform-composed summary; `USER_TOKEN_PENDING_MAX`; the decide handler accepts only the token owner (`not_token_owner` otherwise); on approve, enqueue the `message.post` with the sender; on expiry or reject, no op.
- Revocation cascade (C-855, C-858, D-284(a)) in one transaction per trigger, including discard of `token_ref` and cancellation of pending approvals (`rejected`, `credential_revoked`); copies at connectors refused at their next claim.
- Audit rows (C-858) with no text, token or platform user id by value.
- Limits `USER_TOKENS_PER_PERSON` (5) and `USER_TOKEN_PENDING_MAX` (5) in `aurora-core::limits`; remove their `doc-only` lines from `scripts/guards/limits-gaps.allow`.
- The `user_token.verify` op variant and the `sender` field in `aurora-channels`/the connector wire types (C-854, C-857), without changing any other op.

**Part B — connector (Rust, Sonnet)**
- Mattermost: `user_tokens: true` in the manifest (`docs/20`); a person-token client limited to `GET /api/v4/users/me` (verify) and `POST /api/v4/posts` (send); `401` → `credential_dead`; the footer of C-851 appended at send.
- Conformance scenario for `user_token.verify` and a `sender.user_token` post in `tests/conformance/connector/`.

**Part C — dashboard (Next.js, nextjs-developer)**
- Account › chat platform links › **Send as me** (`docs/14`): token table, Add token dialog (masked input, help text as in `docs/14`), grant dialog, revoke `ConfirmDialog`, error copy for 422/409/503, empty state, 390 px stacking. The approval page shows the full text of a `send_as_me` approval to its owner only.

**Part D — docs**: keep `docs/03`, `05`, `08`, `13`, `17`, `18`, `20`, `02`, `14`, `API_CONTRACTS` matching what lands.

### Out of scope
Read-as-user (`history.read` or `read_conversation` with a person's token), any other platform, an OAuth app flow, token expiry on Aurora's side, an `allow` mode, files or ephemeral posts as a person, scheduled or webhook sends, a per-channel grant narrower than per agent, any use of a person's token by the gateway, a runner or a sandbox.

## Constraints / file ownership
- Owns: `services/aurora-api/src/domains/user_tokens/**` (new), `domains/connectors/ops.rs` (the claim branch for `sender.user_token` only), `domains/tools/messaging.rs` (`send_as_me` only), `domains/action_approvals/handlers.rs` (the `send_as_me` key only), `domains/channels/deprovision.rs` (no change to the bot path), migrations `…000232`, `…000233`, `crates/aurora-db/src/repo/{user_tokens.rs,user_token_agents.rs}` (new), `crates/aurora-core` (limits), `crates/aurora-channels/src/capability.rs` (no semantic change), `connectors/mattermost/src/{ops.rs (person-token client), render.rs (footer), managed.rs (manifest value only)}`, `tests/conformance/connector/**`, the Account component, `scripts/guards/limits-gaps.allow` (`USER_TOKEN*` lines only), `docs/03`, `docs/05`, `docs/08` §1, `docs/API_CONTRACTS.md`, `docs/18` (User tokens section), `docs/17` only to correct a number.
- Must not change: the bot's presence credentials path, the deprovision op, the approval rules of other keys, the gateway, the runner, the sandbox.
- Security rules (non-negotiable): the token is never logged, never in an error or metric label, never in a response; a person's token is never passed to any code path other than the claim and the verify op; the person-token client has no method other than the two of C-854/C-857; `send_as_me` has no `allow` path, and a test asserts that no route, enum or config value creates one.
- Opus reviews Parts A's eligibility and revocation code and the connector's person-token client before merge.

## Acceptance criteria (failing-first: each new test fails before its code)
1. Add and verify (`api/it/user_tokens.rs`, mocked connector): own link → 201 `active`; another platform user → 422 `user_token_mismatch` with the sealed value discarded; rejected → 422 `user_token_invalid`; sixth token → 409 `user_token_limit`; store down → 503 with nothing stored; the canary token absent from responses, audit rows, captured logs and the request-log body.
2. Custody (SEC-112): the per-op copy exists from claim to terminal state, then `null` with no store version left; a claim after revoke finds no copy and fails `credential_revoked`; a static check (or grep test) shows `user_token` and `channel_op_user_token` opened only from the claim and verify paths; boot documents and sandbox env snapshots carry no token.
3. Eligibility matrix (SEC-113): another speaker in a shared channel, scheduled, webhook, watch, peer, system, dashboard and API turns, a grant of another agent, an inactive token, an inactive link → each refused with its C-856 code and no approval row; the owner's own DM and own thread reach the approval step.
4. Approval: a decision by another approver, even with `can_approve`, → 403 `not_token_owner`; no `allow` route or enum value exists (route-table and enum test); expiry sends nothing; the sixth pending send → `pending_limit`; revoke during a pending approval → `rejected`, `credential_revoked`, and no `message.post` reaches the connector.
5. Connector (`connectors/mattermost` tests, mock platform): the only requests carrying a person's token are `GET /api/v4/users/me` and `POST /api/v4/posts`; the footer is the last line; no `ephemeral` or file; `401` → `credential_dead`, the token `invalid`, the presence untouched.
6. Revocation cascades (D-284(a)): each trigger (owner delete, grant removal, link removal, SCIM deprovision, owner deletion, agent deletion, integration deletion, platform `401`) ends the token in one transaction with its discard and its approval cancellations; one audit row each.
7. Audit: one `user_token.send` row per attempted send, no text, no token, no platform user id by value; `user_token.revoke` is critical.
8. Web: vitest for the token table, add dialog errors, grant switches, revoke confirm; Playwright + axe at 390 px and both themes; the approval page shows the full text to the owner only.
9. Live (gated, `scripts/e2e/` with the pinned Mattermost dev server of `docs/20`): add a token, grant an agent, owner's DM turn asks for a post in a channel, owner approves in the dashboard, the post appears under the owner's account with the footer; a second person's turn in that channel cannot trigger it; after revoke the same request is refused.
10. `bash scripts/guards/run-all.sh` passes, with the `USER_TOKEN*` lines removed from `limits-gaps.allow`.

## Verification
Per-task gates of `docs/tasks/LANE_RULES.md`. Rust: `cargo fmt --check`; `cargo nextest run -p aurora-api -p aurora-connector-mattermost -p aurora-db`; `cargo clippy --workspace --all-targets --all-features -- -D warnings` at the batch gate. Web: lint, typecheck, vitest, build, Playwright. Guards: `bash scripts/guards/run-all.sh`, `python3 scripts/extract-schema.py --check` (the new tables move from `docs/03` planned to real sections). The live check runs only when the machine is idle.

## Dependencies
- Blocked by: none technical (link, approvals, channel ops, secrets are merged). Schedule after P4-NARR's Part A if both touch `domains/connectors/ops.rs` or the Mattermost render path (they should not, but check `PHASE5-PLAN.md`).
- Blocked on the owner: Q-UTOK-1 to Q-UTOK-8 (recommended answers are the spec as written; the owner may flip any before merge).
- Unblocks: read-as-user, if the owner answers Q-UTOK-1 with it (a new brief).

## Suggested agent/model and reason
`rust-developer` (Sonnet) for Parts A and B (multi-module, security-sensitive: claim, approvals, revocation); `nextjs-developer` (Sonnet) for Part C. Opus reviews the claim branch, the eligibility checks, the revocation cascades and the person-token client before merge (SEC-112, SEC-113). Size **L**: a new domain, two tables, a connector client, an approval key and six revocation triggers. If the lane finds it larger, split at the approval boundary: `P4-UTOK-1` (custody, verify, routes, revocation, audit, dashboard) and `P4-UTOK-2` (`send_as_me`, approvals, connector send, live check); both must merge before any send is offered.

## Open questions for the owner
**Answered (owner, 2026-11-15, D-288): every recommendation below accepted.** The spec text follows them.

- **Q-UTOK-1 — Which features.** Recommended: send-as-me only in phase 5; read-as-user stays hidden and gets its own brief later, because a wider read reaches private channels the bot cannot see, and the answers can then be posted elsewhere. Alternative: read-as-user in phase 5 as well (needs its own confused-deputy rules before build).
- **Q-UTOK-2 — How a person grants a token.** Recommended: paste a Mattermost personal access token in Account, verified against the caller's own link. Alternative: a Mattermost OAuth2 app per server (the server admin creates it; more setup, and the token's scope is the app's, not the person's choice).
- **Q-UTOK-3 — Who may use a token.** Recommended: only the owner's own turns, and only for agents the owner granted one by one. Alternative: a grant that also lets the agent act for the owner in turns other people start (not recommended: confused deputy).
- **Q-UTOK-4 — Approval for each send.** Recommended: every send is approved by the owner in the dashboard or chat; no `allow` mode in phase 5. Alternative: an `allow` mode per grant for trusted agents (risk: a prompt-injected document could post as the person unseen).
- **Q-UTOK-5 — Shared channels.** Recommended: allowed for the owner's own turns, each send approved, with the footer. Alternative: direct messages only, which removes the audience of a shared channel from the risk.
- **Q-UTOK-6 — Expiry and standing.** Recommended: no Aurora-side expiry (personal tokens have none); a platform `401` marks the token `invalid`; the owner may revoke at any time, and the revocations of D-284(a) apply. Alternative: a maximum age with re-entry, e.g. 90 days.
- **Q-UTOK-7 — Who sees the audit rows.** Recommended: the owner sees their own `user_token.*` rows, and instance admins see them in the instance audit log, without text or token values. Alternative: owners only.
- **Q-UTOK-8 — Footer on every send.** Recommended: every post a token sends ends with `_Sent by <agent> for <owner>_`, added by the connector and not waivable, so recipients can tell that an agent wrote it. Alternative: a per-token option to drop it (not recommended: it hides the agent's authorship from the people who read the post).

## Completion report
(lane `p5-agent`, 2026-11-17; commits `864fe700` Parts A/B, `79e473bd` connector tests + conformance, `968f0f10` Part C, then the docs commit.)

- **Brief references checked against master and what was adapted.** FIX-160 (sqlx 0.9): no runtime-built SQL needed
  `AssertSqlSafe` beyond the usual `format!` of the repo's `SELECT` constant; tests that read best-effort audit rows poll
  (`wait_audit`). P5-IA-1: no new nav entry; the Account section sits in `AccountChannelsView` (route `/account/channels`);
  the approvals text sits on the existing workspace Approvals page (Pending tab). SET-5, P5-UX-1/2/13, P5-MODELS: no overlap
  (the `Purpose` enum gained two variants after `InstanceSetting`; `ActionContext` gained a `state` field). `ResourceTable`
  does not exist in the codebase: cards (the page's own pattern) and `DataTable` are the primitives; the token list is cards so
  rows stack at 390 px. `docs/14`'s "Account page" is `/account/channels`.
- **Files changed (grouped).** Migrations `20261201000232_user_tokens.sql`, `…000233_user_token_agents.sql`; `aurora-db`
  `repo/user_tokens.rs` (the brief's two repo modules are one file), `repo/channel_ops.rs` (claim returns the internal keys,
  finished-copy sweep), `repo/action_approvals.rs` (owner list); `aurora-secrets` (purposes `user_token`,
  `channel_op_user_token`); `aurora-core` (limits, `user_token.revoke` critical); `aurora-protocol` (`OpKind::UserTokenVerify`,
  `Sender::UserToken`, `Operation.user_credential`, `MessagePostOp.user_footer`, verify op/result, schemas);
  `aurora-api` `domains/user_tokens/{custody,claim,end,send,service,routes,dto,events}.rs`, the claim and result paths of
  `domains/connectors/ops.rs`, the decide rule of `domains/action_approvals`, `domains/tools/messaging.rs` (+ `pub(crate)`
  helpers in `channels/messaging.rs`), the revocation hooks in `channels/{links,commands,integrations}.rs`,
  `agents/service.rs`, `me/service.rs`, `tenancy/service.rs`, `scim/{deprovision,resource_users}.rs`, `secrets_migrate.rs`, the
  custody sweep (`jobs.rs`, `main.rs`); `connectors/mattermost/src/{person,ops,render,api,connector}.rs`;
  `connectors/fake` (person tokens), `tests/conformance/connector` (driver trait, fake core, scenario s21); web
  `lib/api/user-tokens.ts`, `components/account/send-as-me.tsx`, `components/approvals/{own-send-approvals,approvals-view}.tsx`,
  `components/channels/account-channels-view.tsx`; `web/openapi.json` and `schema.d.ts` regenerated; docs 03, 04, 05, 08, 12, 13,
  14, 17, 18, 20, API_CONTRACTS, operator/vault; `limits-gaps.allow` (the two `USER_TOKEN*` lines removed).
- **Tests/checks that ran (exact commands, results).** `cargo test -p aurora-api --test it -- user_tokens` 24/24;
  `cargo test -p aurora-connector-mattermost` 86 lib + 5 + 2 + 7 (`tests/user_tokens.rs`) ok; `cargo test -p
  aurora-conformance-connector` 21/21 (s21 against the fake connector); `cargo test -p aurora-db` 8 + 40 ok;
  `cargo test -p aurora-protocol` (`UPDATE_SCHEMAS=1`) 33 ok; `cargo clippy --workspace --all-targets --all-features -- -D
  warnings` clean; `bash scripts/guards/run-all.sh` exit 0; `python3 scripts/extract-schema.py --check` up to date;
  `UPDATE_OPENAPI=1 … openapi_matches_checked_in` + `pnpm -C web gen:api`; web `typecheck`, eslint on the touched files,
  `vitest run` 125 files / 2083 tests, Playwright `e2e/send-as-me.spec.ts` 16/16 (both themes, 1280 and 390 px, axe, no
  horizontal scroll).
- **Batch gate (run after the last code change).** `bash scripts/fixtures/build.sh` 57/0; `cargo test -p aurora-api --locked --
  --test-threads=8` (lib 128 ok; it 1353 of 1354, the one failure being `channels_inbox::a_channel_event_turn_belongs_to_the_inbox_job_trace`,
  the known load flake: it passes alone); `cargo test -p aurora-core -p aurora-secrets -p aurora-connector-fake -p
  aurora-connector-host -p aurora-protocol -p aurora-db -p aurora-connector-sdk -p aurora-channels` all ok; `cargo test -p
  aurora-connector-mattermost` and `-p aurora-conformance-connector` ok; workspace clippy `-D warnings` clean; `cargo fmt --all
  --check` ok; guards exit 0; `extract-schema.py --check` up to date; OpenAPI regenerated and `gen:api` run; web `typecheck`, `lint` (0
  errors, 4 old warnings in other files), `vitest` 125 files / 2083 tests, `pnpm build`, Playwright (`AURORA_E2E_WEB_PORT=4183
  AURORA_E2E_WEB_PROD_PORT=4184`, all 1045: the 1039 `mocked` tests pass, stage4-axe included; the 4 failures are `real-stack`
  specs that need a running stack). **The first full api run found two defects of mine that no targeted test touched**, both
  fixed in the gate commit: the claim's `user_op` column was NULL for an op without a `sender` (500 on every such claim) and
  `payload - key` fails on a non-object payload; `secrets_migrate` ITs counted 8 purposes (now 10) and the permission matrix
  lacked the new routes. **Not run:** `bash scripts/e2e/upgrade.sh` (two builds and images, about 15 GB, on a box with 63 GB
  free and a 50 GB floor; the migrations are additive tables and apply on a fresh database in the `aurora-db` tests and every IT),
  `scripts/e2e/mattermost.sh` (podman netns suite; the box load was 7 to 23 throughout), `hosted.sh`, `gateway.sh` (nothing in
  this task touches the gateway or the runner).
- **Fail-before (new tests that fail without the code).** The owner-only decision (`only_the_owner_decides…`): with the
  `send_as_me` branch of `action_approvals::service::decide` disabled the approver's decision answers 200 instead of 403
  (`left: 200, right: 403`). The claim check (`a_claim_after_revoke_finds_no_copy…`): with the token/grant/link check at
  the claim bypassed the claim hands the op out (`left: 1, right: 0`). Both mutations were reverted.
- **Decisions taken on open questions (all recommendations, D-288) and where the spec was silent.**
  (1) Grant removal and the deletion of an agent or workspace end *that agent's standing* (pending approvals rejected
  `credential_revoked`, unclaimed sends failed, one `user_token.ungrant` row) and leave the token; the owner's `DELETE`, link
  removal, SCIM suspend/remove, owner deletion, integration/org deletion end the **token** (`user_token.revoke`, critical); a
  platform `401` makes it `invalid` (acceptance 6 and D-284(a) say "ends the token" for all; I read grant removal as the
  agent's standing, because revoking the whole token on one switch would be harsher than the spec's own `ungrant` audit
  action implies — please confirm). (2) `token_ref` stays set on an ended token until its sealed value is discarded (right
  after the commit, else by the one-minute custody sweep): SCIM deprovisioning runs in four large transactions whose callers
  return no value to run after commit, so they leave the discard to the sweep. (3) The per-op copy is made at approval
  execution (`custody::copy_for_send` opens the stored token and seals the copy under the op id); it is opened only by the
  claim. `Purpose::UserToken`/`ChannelOpUserToken` appear only in `custody.rs` and `secrets_migrate.rs` (a static test).
  (4) `send_as_me` posts only to a channel or thread (a person target would need `POST /channels/direct`, a third
  endpoint), on the token's integration, and keeps `send_message`'s reach rule (own thread, bound space, approved grant).
  (5) The owner's text is served on a new `GET /v1/me/user-token-approvals` (the workspace list is `can_approve`-only and
  must not carry the text); new refusals `422 user_link_required`, `409 user_token_exists`, `409 token_inactive` (grant of a
  non-active token); `UserToken.platform_user_label` needs a column `platform_user_label` (not in the planned table). (6) A
  decision by an approver on an approval in the owner's *direct* conversation is 404 (RFE-009: the approver cannot see it);
  in a shared channel thread it is 403 `not_token_owner`. (7) `maybe_sent` on a person's post: the bot's token searches the
  marker; if it cannot tell, the post is not repeated. (8) Numbers: no new C-/D-/SEC- numbers used; the new route and codes
  are written into C-853/C-855/C-856 and docs/05.
- **Remaining risks / not done.** Acceptance 9 (the live Mattermost journey: dashboard approval, post under the owner's
  account) is **not implemented or run**: it needs the podman netns suite (`scripts/e2e/mattermost.sh`) on an idle box; the
  conformance scenario s21 has Mattermost-driver support (`person_token`, `revoke_person_token`) but only ran against the
  fake connector. The Opus review of the claim branch, the eligibility checks, the revocation cascades and the person-token
  client is still to do. The `user_token.*` rows are visible in the org audit log (actor = the owner); there is no
  owner-only audit view (Q-UTOK-7's "owner sees their own rows" needs a per-person audit route that no contract defines).
  `user_token.send` rows are written best-effort after the terminal state, so a crash between the result and the row loses
  one row. A send claimed before its revocation still completes (the connector holds the value); documented in `13 §5.11`.
