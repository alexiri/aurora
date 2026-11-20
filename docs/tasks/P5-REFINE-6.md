# P5-REFINE-6: Apps, chat platforms, invitations and organizations backend: send-only levels, one readiness, invitation links, a Test for sign-in apps (owner, 2026-11-20)

## Goal

UX review 5 found four places where the API cannot give the dashboard what an honest screen needs. This brief is the **Rust side only**. It adds the web `lib/api`
client and types for every new field and no UI (P5-REFINE-13, -17 and -18 build the screens).

- **Read only on an outgoing webhook fails (access-1, priority 1).** The Outgoing webhook has no read tool. The connect journey offers "Read only (recommended)", expands it to an
  empty custom grant and the grant service refuses it: "A custom grant needs at least one tool; detach the connection instead." The Done step then says "Dani can now read Outgoing webhook".
  **D-419**: send-only apps have their own levels, "Ask before each send" and "Send without asking"; Read only is never offered for them.
- **Chat platforms: two pages, two answers (org-1, admin-5, priority 1).** The organization's Chat platforms page says "Mattermost is added, but its connector isn't running" while Administration
  shows its host Running; nothing can be connected, so Carol's request dead-ends. Administration says "Waiting for the connector host" and never says that the next step is an organization owner
  connecting it. `connector-availability` and the admin list compute the state in two places.
- **Invitations need a mail server (org-4).** "Invitation links are written to the server log" and a pending row has only Cancel. **D-418**: organization owners and admins can copy an invitation link
  in the UI (**SEC-140**).
- **Sign-in apps are accepted and marked Ready with no check (org-10).** One name ("sign-in app"), a Test that runs against the provider, "Ready (not tested)" until it is used or tested.
- **Package upload errors are code (admin-12):** "The package is invalid. archive_corrupt invalid gzip header".
- **Storage lists database tables (admin-14):** `transcript_events`, `request_log`. People think in conversations, files and activity.
- **"New organization" (org-5):** the API already creates a second organization; the switcher needs to know when it may be offered.

Decisions that bind this brief: **D-405**, **D-407** ("sign-in app", roles not names), **D-418** (invitation link), **D-419** (send-only levels). **SEC-140** is this brief's.

## Linked requirements

- **The review:** <https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt>. Workers cannot open artifacts; read the local copy `/home/alex/orca/workspaces/aurora/design-ux-review-5/` (written `UXR5/` below).
  `UXR5/findings.md` is the authoritative text; `UXR5/priorities-themes.md` priorities 1, 9.
- **Findings** (artifact anchor `https://claude.ai/artifact/1YJcRA7Q84864pkim44Qnt#f-<key>`; After `UXR5/after/<key>.png` and `<key>-lt.png`; markup `UXR5/after/src/<key>.html`; Before `UXR5/before/<key>-b1.png`; screens `UXR5/screens/<id>.png`):
  - `access-1` (send-only levels; Done says "Dani can now send to Ops alerts, and asks you first.") · After `access-1.png`, `access-1-lt.png`, `src/access-1.html` · Before `access-1-b1.png`, `access-1-b2.png` · Screens `B109-webhook-step1.png`, `B121-webhook-done-step.png`, `B128-level-readonly.png`.
  - `org-1` (Chat platforms uses the same readiness as Administration: a running host means Connect is offered) · After `org-1.png`, `org-1-lt.png` · Before `org-1-b1.png`, `org-1-b2.png` · Screens `A131-org-chat.png`, `A073-add-mattermost.png`, `A193-connect-chat-from-ask.png`.
  - `admin-5` (status from the host; "Next: an organization owner connects Mattermost") · After `admin-5.png`, `admin-5-lt.png` · Before `admin-5-b1.png`, `admin-5-b2.png` · Screens `A072-add-connector.png`, `A073-add-mattermost.png`, `A076-chat-after-mm.png`.
  - `org-4` (Copy invitation link always, Resend when e-mail works; D-418) · After `org-4.png`, `org-4-lt.png` · Before `org-4-b1.png` · Screens `A114-org-people.png`, `A123-invite-dialog.png`, `A125-invited.png`.
  - `org-5` (New organization in the switcher; the API half is `can_create_org`) · After `org-5.png`, `org-5-lt.png` · Before `org-5-b1.png` · Screens `A150-switcher.png`, `A220-root-home.png`.
  - `org-10` (one name, a Test button, "Ready (not tested)") · After `org-10.png`, `org-10-lt.png` · Before `org-10-b1.png`, `org-10-b2.png` · Screens `A138-org-apps-tab.png`, `A140-org-google-added.png`, `A141-org-apps-after-google.png`.
  - `admin-12` (a bad package: "That isn't an app package (.tar.gz from `aurora plugin pack`)") · After `admin-12.png`, `admin-12-lt.png` · Before `admin-12-b1.png` · Screens `A228-upload-bad-package.png`.
  - `admin-14` (Storage by category; the designer's note: it needs a backend mapping) · After `admin-14.png`, `admin-14-lt.png` · Before `admin-14-b1.png`, `admin-14-b2.png` · Screens `A080-data-storage.png`, `A092-platform-limits.png`.
  - Not in this brief: `org-2`, `org-3` (web), `org-6..9`, `org-11..14` (web, P5-REFINE-17), `admin-1` (P5-REFINE-5), `admin-13` (names: P5-REFINE-5 and the UI).
- **Layer-above:** rec 9 (Administration/organization: invitations, one name), rec 8 ("Manage Acme"): `UXR5/layer-above.md` anchor `…#above`.
- **Decisions:** D-405, D-407, D-418, D-419. **Contracts:** **C-1067..C-1072** and the additions **C-1086..C-1090**
  (all in `docs/API_CONTRACTS.md` › “Phase 5 — P5-REFINE”, merged with the briefs 2026-11-20; correct an entry there first
  if the code needs a different shape).
  **Migration:** **`20261201000265`** (assigned by the coordinator, 2026-11-20): `oauth_clients.last_test_at`, `last_test_result`.
- **Additions for the area briefs (coordinator, 2026-11-20)** — small read fields the web briefs need, built in this brief:
  - **C-1086** `AccessBinding.signed_in` (P5-REFINE-16 workspace-4, "Not signed in yet");
  - **C-1087** `PolicyGenerationSummary.changes` and `actor` (P5-REFINE-17 org-14, "Alice added ‘Block tracker’");
  - **C-1088** `include_system` / `hidden_system` on the audit lists (P5-REFINE-18 admin-10);
  - **C-1089** `GET /v1/me/chat-platforms` (P5-REFINE-16 account-3: the platforms a person can link and the bot to message);
  - **C-1090** `GET /v1/orgs/{org}/agents` (P5-REFINE-17 org-6: the organization's agents, owners and admins).
  Each gets a targeted API test (shape, authorization: who gets 403/404) and its `web/src/lib/api` type and hook.
- **Security: SEC-140** (the draft row, "Security" in the same draft file; add it to `docs/17-security.md` and a coverage row to `docs/tests/security-coverage.md`).
- **Normative docs:** `05` (invitations, oauth-clients, plugins, storage, connectors, `me`), `04 §Invitations`, `09` (grants, apps), `13 §Connectors` (managed hosts), `docs/operator/*` (mail).

## Wave and order

- Wave 2, **lane F** after brief 5: 5 → **6** → 8 → 10. No code dependency on 5; the lane runs one brief at a time.
- Disjointness (lane F): `services/aurora-api/src/domains/{invitations,oauth_clients,connectors,plugins,attachments,tenancy,me}/**`, `domains/grants/levels.rs` and the small hooks in `domains/{accounts,apps,connect,access_requests}/**`
  (brief 5 also edits `accounts/service.rs` — rebase), `crates/aurora-registry` (none expected), `crates/aurora-db/src/repo/{invitations,oauth_clients,connectors}.rs`, the new migration, `web/src/lib/api/{apps,accounts,connectors,org,plugins,oauth*,account}.ts`.
- `web/openapi.json` and `schema.d.ts` regenerated. P5-REFINE-13 (access, Done step), -17 (organization) and -18 (Administration) wait for this.

## Context

Paths from the repository root; line numbers are master `c0fe7d10`.

### Levels and send-only apps (Scope 1)

- `services/aurora-api/src/domains/grants/levels.rs`: `Level` (`Read`/`Ask`/`Act`), `is_read` :23 (tool `group == "read"`), `expand` :34 (`Read` → `allow: every read tool`, **empty for a send-only app**),
  `levels_for` :194 (offers `Read, Ask, Act` whenever the app has **any** write tool; drops `Act` for `custom_only` and above the limit), `classify` :262, `lowered_grant` (limits).
- `services/aurora-api/src/domains/grants/service.rs`: the grant writer `grant_connection_in_tx`; the `custom_only` check :877; **`empty_custom` :922** ("A custom grant needs at least one tool; detach the connection instead.") is where the Read grant dies.
- `services/aurora-api/src/domains/connect/service.rs`: `apply_grant` :709 (`levels::expand(intent.grant.level, &tools)` :723); request validation :1013 (`full_grant_forbidden` for `Act` on a `custom_only` app) and :1047 (the limit).
- `services/aurora-api/src/domains/accounts/service.rs` :387 (`levels: levels::levels_for(&tools, custom_only, limit)`), `accounts/dto.rs` `AccountApp` :34; `apps/dto.rs` `AppSummary` :30.
- `services/aurora-api/src/domains/access_requests/service.rs` :37–51 (the Done-style sentences: "can now read {app}").
- The Outgoing webhook: `plugins/builtin/webhook/plugin.yaml` (one write tool `send`, `approval: ask`; no read tool; `secret_link`).
- Web: `web/src/lib/api/apps.ts`, `web/src/components/apps/connect-journey/*` (level step, Done), `web/src/components/accounts/*` (level select).

### Connector readiness (Scope 2)

- `services/aurora-api/src/domains/connectors/admin.rs`: `availability` :288 builds `ConnectorAvailability` from the live host catalogue (`managed::live_catalogue` :82) and the rows; its `state_of` :291 maps **any** other status (including
  `awaiting_registration`) to `WaitingForHost`. The admin list (`admin.rs` :47 `host_status: mv.host_status`, `managed.rs` :637/:670) and the web's `connectorStatus` (`web/src/components/connectors/shared.tsx` :54–65) decide separately.
- `services/aurora-api/src/domains/connectors/dto.rs`: `ConnectorAdmin` :194, `ConnectorSetupState` :278, `ConnectorAvailability` :287.
- `crates/aurora-db/src/repo/connectors.rs`: `list_managed_enabled` :105 (hands `active` **and** `awaiting_registration` rows to the host); `awaiting_registration` is set at insert (:62, :169, :189); a registration flips it to `active` through the approval update :314.
- Web: `web/src/components/channels/connector-blocked.tsx` ("… is added, but its connector isn't running" :54), `web/src/components/connectors/shared.tsx` :61 ("Waiting for the connector host").
- Creating an integration requires `connectors.status = 'active'` (`channels/integrations.rs` :441, `presences.rs` :572 → `503 connector_unavailable`).

### Invitations (Scope 3)

- `services/aurora-api/src/domains/invitations/routes.rs`: `create_invitation` :63 (owner/admin), `cancel_invitation`, `list_invitations`; `service.rs` `create` :46 (token `TokenPrefix::Invitation`; link built at :124; mailed with `mail::send_detached` :130);
  `crates/aurora-db/src/repo/invitations.rs` `upsert_pending` :151 (`ON CONFLICT … DO UPDATE SET token_hash = EXCLUDED.token_hash, expires_at = now() + interval '7 days'`: **re-inviting a pending e-mail already rotates the token**); `dto.rs` `Invitation` :60 ("The token is never part of any response").
- `invitations.token_hash` is unique and hashed (`crates/aurora-db/migrations/20261105000001_identity.sql` :46). A link cannot be read back: Copy link must **re-issue**.
- Mail: `services/aurora-api/src/domains/auth/mail.rs` (`LogMailer` vs `SmtpMailer`); `GET /v1/instance` `email_delivery: "smtp" | "log"` (`domains/instance/service.rs` :60).
- Web: `useInvitations`, `useCancelInvitation` in `web/src/lib/api/queries.ts`; `web/src/components/org/people*` (the pending table and `InviteDialog`).

### Sign-in apps (Scope 4)

- `services/aurora-api/src/domains/oauth_clients/routes.rs` (`patch_client` :196, `writable(&caller, &state, id)` is the caller rule to reuse), `service.rs` (`create` :142, `patch` :214), `dto.rs` `OAuthClient` :10;
  `crates/aurora-db/src/repo/oauth_clients.rs`.
- The manifest's OAuth block: `crates/aurora-registry/src/manifest/model.rs` `OauthClient` :107 (`authorize_url`, `token_url`, `pkce`, `client_auth`, `body_format`, `fields`).
- The code exchange that already knows how to call a token endpoint with the client's credentials: `services/aurora-api/src/domains/connect/service.rs` :1711 (`("grant_type", "authorization_code")`): reuse its request building and its destination guard; do not write a second HTTP client path.
- Web: `web/src/components/org/*sign-in*`/`oauth-clients*` (the "OAuth client" and "sign-in app" words are P5-REFINE-1/17's).

### Package errors and storage (Scopes 5, 6)

- `services/aurora-api/src/domains/plugins/ingest.rs`: `reject` :58 (`422 package_invalid`, "The package is invalid.", `details.violations` from `violations_json` :49; 413 for `package_too_large`), `reject_public` :67 (the add-on source path);
  `crates/aurora-registry/src/lib.rs` `Violation` :34 (`code`, `path`, `message`); the codes: `crates/aurora-registry/src/package.rs` (`archive_corrupt` :139/:145/:201, `archive_entry_type`, `archive_link`, `file_missing`, `icon_not_png`, `icon_not_square`, `path_*`, `guide_not_utf8` :312) and the manifest validators.
  Route `plugins/routes.rs` `upload_plugin` :75.
- `services/aurora-api/src/domains/attachments/storage.rs`: `AdminStorage` :90 (`tables: Vec<TableSize>`), `BIG_TABLES` :111, `compute_admin` :169; `OrgStorage` :40; `snapshot` (the cached JSON in `attachments::snapshot_get/put`).
- `services/aurora-api/src/domains/tenancy/routes.rs` `create_org` :91 (`403 org_creation_forbidden` when `org_creation = instance_admins` and not an instance admin); `me/dto.rs` `Me` :55.

## Scope

1. **Send-only levels (C-1067; access-1).** Anchor `…#f-access-1`. After `access-1.png`.
   - `levels.rs`: `fn send_only(tools) -> bool` (no tool with `is_read`, at least one tool). `levels_for` returns `[Ask, Act]` for a send-only app (still dropping `Act` for `custom_only` and above the limit; `Read` never). `AppSummary` and `AccountApp` gain `send_only`.
   - `connect/service.rs` :1013/:1047 and the grant PUT path: a `Read` level on a send-only app → `422 level_not_offered` (`details { app_id, offered }`) **before** `expand`; `expand` for `Ask` on a send-only app yields `allow: [], ask: [every write tool]`.
     `grants/service.rs` :922 keeps `empty_custom` for hand-built lists. Add a unit test that `classify` returns `Ask`/`Act` for the Outgoing webhook's saved grants (and never `Read`).
   - `access_requests/service.rs` :37–51: the send-only sentences of C-1067 ("can now send to <Account>, and asks you first." / "… without asking."). The Done step's own sentence is the web's (P5-REFINE-13) and reads `send_only` and the account name.
   - Web client types: `send_only` on the app and account types in `lib/api/apps.ts`/`accounts.ts`; a `level_not_offered` message in `lib/api/errors.ts`.
2. **One readiness for a chat platform (C-1068; org-1, admin-5).** Anchors `…#f-org-1`, `…#f-admin-5`. After `org-1.png`, `admin-5.png`.
   - **Investigate first (an hour, not a day), then fix the cause.** On a throwaway cluster or in an integration test with the connector-host test rig (`services/aurora-api/tests/it/connectors*.rs`): add a managed connector whose type a live host announces;
     watch `connectors.status` and `host_status`. Find why the row can stay `awaiting_registration` while the host reports running (candidates: the assignment revision bump `bump_revision` is missing on add; `list_managed_enabled` hands the row out but the host's registration
     does not run the approval update :314 when the manifest digest is already known; the row is `awaiting_registration` until a **first poll with its connector token**). Write what you found in the completion report. The expected behaviour (C-1068): a running host that announces the type
     makes the row `active` within one host poll; the availability item says `can_connect: true`.
   - Add `connector_readiness(row, live_types, host_status) -> { setup_state, host_running, next_step }` (new `connectors/readiness.rs`) and use it from **both** `availability` (:288) and the admin list (`admin.rs` :47). `ConnectorSetupState::Registering` for "announced by a live host, first
     registration pending" (it must resolve; add a test that it does). `next_step` per C-1068. Existing values and their meaning are unchanged for old clients (`waiting_for_host` is now only "no live host announces the type").
   - Web client: `lib/api/connectors.ts` types for `host_running`, `can_connect`, `next_step`, `registering`; the web's `connectorStatus` is **not** changed here (P5-REFINE-17/18).
3. **Invitation links (C-1069, SEC-140; org-4, D-418).** Anchor `…#f-org-4`. After `org-4.png`.
   - `create` (service :46) returns the link in `Invitation.link` (add the optional field to the DTO; `skip_serializing_if` none; present only from `create`). Keep the mail send. A **new** `invitations::service::reissue(state, org, id, actor) -> (link, expires_at)`:
     new token, `UPDATE invitations SET token_hash, expires_at = now() + interval '7 days' WHERE id AND status = 'pending'`, audit `invitation.link` in the same transaction (`audit::commit_critical`), `Cache-Control: no-store` on the response.
   - Routes `POST /v1/invitations/{id}/link` and `POST /v1/invitations/{id}/resend` in `invitations/routes.rs` (owner/admin of the invitation's organization via `c.org(&s, org).await?.require_admin()?`; load the invitation first and answer **404** for another organization's, as `cancel_invitation` does), the `management` rate class plus a per-organization hourly counter of 20
     (use the existing `http::counters` pattern, `check_webhook` is an example). `resend` is `409 mail_disabled` when the mailer is the `LogMailer` (expose `Mailer::is_log_only()` or read `live.smtp` as `instance_info` does).
   - **SEC-140 tests** as in the draft row; add the coverage row. Make sure no logging middleware prints the response body of these routes.
   - Web client: `lib/api/queries.ts` hooks `useInvitationLink`, `useResendInvitation`; `Invitation.link?` on the type.
4. **Test a sign-in app (C-1070; org-10).** Anchor `…#f-org-10`. After `org-10.png`.
   - Migration `20261201000265_oauth_client_test.sql` (number to be confirmed): `ALTER TABLE oauth_clients ADD COLUMN last_test_at timestamptz, ADD COLUMN last_test_result text CHECK (last_test_result IN ('ok','rejected','unreachable','unexpected'));`
     (`not_testable` is never stored). Update `docs/03` and run `extract-schema.py --check`.
   - `oauth_clients/test.rs`: `POST /v1/oauth-clients/{id}/test` as C-1070. Build the token request with the **same helper the code exchange uses** (`connect/service.rs` :1711) so `client_auth` and `body_format` are honoured, with a fake `code`; send it through the destination guard (never a raw `reqwest::Client`).
     Map the answer: `400`/`401` + `error: invalid_grant` → `ok`; `invalid_client`/`unauthorized_client` → `rejected`; transport error, timeout, guard refusal → `unreachable`; anything else → `unexpected`; no `token_url` or disabled client → `not_testable`.
     Read at most 8 KiB of the response; return nothing of it (host and HTTP status only). Audit `oauth_client.test`. Rate class `check`. Caller rule = `writable(...)` of `patch_client`.
   - `OAuthClient` gains `test` and `state` (`ready_not_tested` / `ready` / `test_failed` / `disabled`) computed in the DTO from the row and `connection_count`.
   - Tests use a mock token endpoint (`wiremock`) answering the four ways; plus "a connection exists → `ready` without a Test".
   - Web client: `lib/api/*oauth*` hook `useTestOauthClient`, types. (Words "sign-in app" are the UI briefs'.)
5. **A bad package in words (C-1071; admin-12).** Anchor `…#f-admin-12`. After `admin-12.png`.
   - One function `plugins/sentences.rs::violation_sentence(code, path, message) -> String` with the table of C-1071 (a `match` on the code; unknown codes get the generic sentence; **no** raw error text such as "invalid gzip header" in the sentence).
     `reject` (:58) puts the first sentence (or "That app package has N problems. The first: …") into `message` and every `sentence` into `details.violations[]`; `package_too_large` says "That package is too large (limit N MB)." (read the limit from config).
     `reject_public` and the add-on source path use the same function. The raw `code` and `message` stay in `details.violations[]`.
   - Tests: a corrupt gzip, an archive with a missing manifest, an oversize one; assert the sentence and that the raw code is still there.
   - Web client: `lib/api/plugins.ts` type `violations[].sentence`; no UI.
6. **Storage by category; who may create an organization (C-1072; admin-14, org-5).** Anchors `…#f-admin-14`, `…#f-org-5`. After `admin-14.png`, `org-5.png`.
   - `storage.rs`: `categories` from the same table sizes (`tables`) with the mapping of C-1072 (add `transcript_search` to the sizes read: `BIG_TABLES` becomes seven names; keep `tables` limited to the five existing ones for compatibility, or include all with a `category` field — pick one and write it in the report).
     `#[serde(default)]` on the new field so cached snapshot JSON still deserializes (`AdminStorage`/`OrgStorage` are stored as JSON by `snapshot_put`). Per-organization storage: the organization's own attachment bytes are `files`, its conversations and activity are estimated from its rows if the code has them; if not, say
     "files only" in the report and return the three ids with `null` bytes for the missing ones (the dashboard hides a null category). Do not invent a per-organization size you cannot compute.
   - `Me.can_create_org` (`me/service.rs`): `is_instance_admin || auth().org_creation == Anyone`. Test: `POST /v1/orgs` by a person who already belongs to an organization succeeds with `org_creation = anyone` and is `403 org_creation_forbidden` otherwise; `can_create_org` agrees in both cases.
   - Web client: `lib/api/account.ts` (`Me`), `lib/api/queries.ts` storage types.

## Out of scope

- All UI: the level select and Done sentence, the Chat platforms page, the Administration connector rows and the "Next:" line, the invitation table and buttons, the sign-in app rows and Test button, the upload error, Storage, the switcher (P5-REFINE-13, -17, -18).
- Org-level request lists and "Ask an admin" with a reason (P5-REFINE-8).
- Making connectors register faster in general, or a connector host health page.
- OAuth **sign-in providers** (`settings/sign_in.rs` discovery) — the "sign-in app" here is the OAuth **client of an app** (Google Drive, Atlassian…); do not conflate them.
- Mail delivery itself (SMTP setup is `setup-6` / P5-REFINE-18).

## Constraints / file ownership

- Everything under "Wave and order". The migration is `20261201000265` (assigned).
- Every new route is in the permission matrix (`tests/it/permission_matrix.rs`) and has an OpenAPI annotation. Never log tokens, links or client secrets.
- Never touch `localhost:4000` or the aurora-dev cluster. Throwaway Postgres. Heavy commands only through `heavy.sh`. Fast targeted tests; GitHub CI is the full gate.

## Acceptance criteria

1. **Send-only.** For the Outgoing webhook: `GET …/accounts` lists `levels: ["ask","act"]` and `send_only: true`; the connect journey with `grant.level: "read"` is `422 level_not_offered`; with `ask` it saves `allow: [], ask: [send]` and the account reads back `ask`; with `act` it reads back `act`;
   GitHub (read and write tools) is unchanged (`read, ask, act`); the access notice reads "can now send to Ops alerts, and asks you first.".
2. **One readiness.** A managed connector added while a live host announces its type becomes `active` within one host poll in the rig; both `GET /v1/connector-availability` and `GET /v1/admin/connectors` report `host_running: true` and the same `setup_state` for it (a test asserts equality for every state in the matrix:
   not added, disabled, pending approval, registering, active, no live host). `next_step` is `owner_connects` for an instance admin and `connect` for an organization admin of an `active` type; `start_host` only with no live host. The investigation's finding is in the completion report.
3. **Invitation links (SEC-140).** The `201` of `POST /v1/orgs/{org}/invitations` carries `link`; no list/`GET` carries it; `POST …/link` by an org admin returns a new link (old link fails `accept`, new works), `Cache-Control: no-store`; a member, a workspace owner who is not an org admin, and another organization's admin are refused (403/404);
   an accepted/cancelled/expired invitation is 409; the 21st link in an hour is 429; `resend` is 202 with SMTP and 409 `mail_disabled` with the log mailer; no audit row, log capture or error body contains the token or link.
4. **Sign-in app Test.** The four mock answers map to `ok` / `rejected` / `unreachable` / `unexpected`; no upstream body in the response; `OAuthClient.state` is `ready_not_tested` for a new client, `ready` after an `ok` Test or with a connection, `test_failed` after a failed Test, `disabled` when off; the caller rule equals `PATCH` (a member of the workspace who is not an owner gets 403); the row is audited.
5. **Package errors.** The corrupt gzip upload answers `message: "That isn't an app package. Upload the .tar.gz that `aurora plugin pack` makes."`, `details.violations[0].code: "archive_corrupt"` and a `sentence`; the oversize one says the limit; no sentence contains the words "gzip header", "archive_corrupt" or a Rust error string.
6. **Storage and `can_create_org`.** `GET /v1/admin/storage` returns the three categories summing the right tables; an old cached snapshot JSON (without `categories`) still deserializes; `Me.can_create_org` matches `POST /v1/orgs` in all four combinations (admin/non-admin × `instance_admins`/`anyone`); a person already in an organization can create another.
7. **Contracts, schema, docs.** C-1067..C-1072 and SEC-140 landed first; `docs/03` (oauth_clients columns), `04 §Invitations`, `05` rows (invitations ×2, oauth test, `me`, storage, connector availability), `09` (send-only levels), `13` (readiness rule), `docs/tests/security-coverage.md`; `extract-schema.py --check` passes; `web/openapi.json` and `schema.d.ts` regenerated; web types and hooks added.
8. **Gates (targeted):** `cargo fmt --check`; `cargo clippy -p aurora-api -p aurora-db -p aurora-registry --all-targets -- -D warnings`; `cargo test -p aurora-api --test it -- grants connect accounts access_requests connectors invitations oauth plugins storage tenancy me permission_matrix` (names as they exist);
   `cargo test -p aurora-api --lib levels`; `scripts/guards/run-all.sh`; `python3 scripts/extract-schema.py --check`; OpenAPI check and `pnpm gen:api` with no diff; web `tsc`. Report counts.

## Tests

- **Unit:** `send_only`, `levels_for` for the Outgoing webhook / GitHub / a custom-only app; `classify` of send-only grants; `violation_sentence` for every known code and an unknown one; the `connector_readiness` matrix; the OAuth result mapping; the storage category mapping and the old-snapshot deserialization.
- **API integration (`services/aurora-api/tests/it/`):** a new `send_only_levels.rs` (journey and grant PUT), `connectors_readiness.rs` (the rig; equality of the two outputs), `invitations.rs` (SEC-140 cases), `oauth_clients_test.rs` (wiremock token endpoint), `plugins.rs` (error text), `storage.rs`, `tenancy.rs`/`me.rs` (`can_create_org`), `permission_matrix.rs` rows.
- **Migration:** `…265` up on a copy with existing `oauth_clients` rows.

## Tests move with the UI (owner, 2026-11-20 — binding)

The owner: "Make sure the briefs not only change the website, but also the tests that test it! I don't want a ton of
broken tests because a button was renamed and the tests look for the old label." (P5-REFINE-1 merged with old labels
in tests, and master's CI broke until the CI Watcher fixed it in `ee7bfa0d`.) So, in **every commit** of this brief:

1. **Find every test that sees what you change.** Before committing a change to visible text, a label, an
   `aria-label`, a role, a heading, a route or `?tab=` id, a page's structure, an API field, an error code or message,
   search all test trees for the **old** strings, selectors and paths, and for the components you touched:
   - `web/tests/**` (vitest + Testing Library);
   - `web/e2e/**` (Playwright: the mocked default project **and** the `production` project that `scripts/e2e/*.sh`
     run against a real stack — `first-run.sh`, `kind.sh`, `files.sh`, `ssh.sh`, `channels.sh`, `approvals-park.sh`…);
   - `scripts/e2e/*.sh` themselves (they grep pages and call routes);
   - Rust tests that assert a message, an error code or a response shape (`services/*/tests/**`, `crates/*/src/**`
     `#[cfg(test)]`, `services/aurora-api/tests/it/**`);
   - fixtures and mocks (`web/e2e/fixtures/**`, `web/tests/**/mocks*`, MSW handlers, `web/openapi.json` snapshots).
   A plain `rg -n "<old text>|<old selector>" web/tests web/e2e scripts/e2e services crates` per change is the minimum.
2. **Update them in the same commit as the change.** New expectations use the new words exactly as the mockup shows
   them. Prefer `getByRole(…, { name })` or a `data-testid` over raw text where the text is not the point of the test;
   where the words are the point (they are the fix), assert the new words. Never loosen an assertion so that both the
   old and the new label pass, never `.skip`/`test.fixme`/delete a test to get green; a test is removed only when the
   feature it covers is removed, and the completion report names it and why.
3. **Run what you touched, plus everything that references it.** Through `heavy.sh`: every vitest file you changed
   **and** every vitest file that renders a component you changed; every Playwright spec (mocked project) that visits a
   page you changed. **Exception to the targeted-tests rule for this set (owner, 2026-11-20):** a commit that renames
   or restructures shared UI (a primitive, the app shell, the rail, the agent page, a label used on several pages) runs
   the **whole** web vitest suite once (`cd web && /home/alex/orca/workspaces/aurora/heavy.sh <lane> pnpm exec vitest run` (not `pnpm --dir web …`, which fails here)) before handing over. Production
   specs (`--project production`) can't run in a lane without a real stack: update them by reading, and list them in
   the completion report so the nightly / P5-REFINE-20 runs them.
4. **New behaviour gets new tests** (the brief's Tests section); old tests are **kept and updated**, not replaced.
5. **The completion report lists** every test file changed (and why), every test removed (and the removed feature),
   the commands run with their results, and the production specs that only the nightly will run.
6. A merge that turns master's CI red because a test still expects old UI is a send-back to this lane, not the CI
   Watcher's job.

## Docs to update

- `docs/API_CONTRACTS.md` (C-1067..C-1072 first), `docs/17-security.md` (SEC-140), `docs/tests/security-coverage.md`, `docs/03-data-model.md`, `docs/04-*` §Invitations, `docs/05-control-plane-api.md`, `docs/09-*` (send-only levels), `docs/13-channels-connectors.md` (one readiness; `registering`).
- `docs/PROGRESS.md` and `docs/FEATURES.md` rows for P5-REFINE-6. `docs/90-decisions.md` is the Orchestrator's.

## Suggested agent/model and reason

Sonnet (`rust-developer`). Six slices over seven domains, a migration, one security surface (invitation links) and one outbound HTTP call (the Test) that must go through the destination guard. The connector investigation (Scope 2) is open-ended: escalate to Opus if the cause is in the host protocol rather than the API.

## Completion report

(Fill in; P5-REFINE-20 reads it.)

- **Items done** (Scope 1–6 and the additions C-1086..C-1090): all, in lane F (`alexiri/p5-agent`): WIP commit `30efcc5c` (Scopes 1–6, C-1086, C-1087) and the final commit (C-1088..C-1090, the `ClientRow` fix, critical audit actions, docs, web client, OpenAPI). Nothing in this brief waits on another brief.
- **Contracts C-1067..C-1072, C-1086..C-1090**: built as written, with "(Built: …)" notes in `docs/API_CONTRACTS.md` where the code is more precise or differs (listed under Deviations). **SEC-140**: the `docs/17` row was already in place; the coverage row is added to `docs/tests/security-coverage.md`. **Migration** `20261201000265_oauth_client_test.sql` (number assigned by the coordinator in the brief and the PROGRESS next-free line).
- **Send-only (C-1067):** `levels::send_only` (tools present, none in group `read`). Outgoing webhook: `levels: ["ask","act"]`, `send_only: true` (on `AppSummary`, `AppDetail` and `AccountApp`); GitHub-like apps (reads and writes) unchanged `["read","ask","act"]`; custom-only drops `act` as before. `read` on a send-only app is refused `422 level_not_offered` (`details { app_id, offered }`) **before** `expand` by the connect journey's `grant.level` and `limit` and by the level `PUT` of a grant (`levels::require_offered`); `expand(Ask)` = `allow: [], ask: [send]`. `classify` unit tests pin `ask`/`act` for the webhook's grants and never `read` (a grant blocking every send, which only a lowered limit can leave, reads `custom` with base `ask`). Notice: "Dani can now send to Ops alerts, and asks you first." / "… without asking." (named by the account; restart turn "(Ask before each send)" / "(Send without asking)"); other apps unchanged.
- **Chat platform readiness — the cause found:** a development connector-host build embeds an **empty** manifest signature (`connectors/builtin/build.rs`: no `AURORA_CONNECTOR_SIGNATURES_DIR` → empty `.sig`), and `deploy/kind/values-dev.yaml` sets `plugins.allow_unsigned`. The host's own `POST /v1/connector-host/register` accepts the empty signature under that flag (D-142, `signed: false`), so the type is announced and the admin can add it; but the managed connector's own `POST /v1/connector/register` (`plane::do_register`) always called `verify_manifest` → 422 → the SDK runtime treats 422 as fatal and stops, the host's instance becomes `Error`, the next heartbeat's `reconcile` restarts it (`!cur.finished()`), and `report` runs right after the restart, so the host reports `running` almost all the time while the row never leaves `awaiting_registration`. That is exactly UX review 5's cluster (A073 "Waiting for the connector host" next to "Running"; A131 still waiting much later). **Fix:** `do_register` applies D-142 the same way as the host: only with `plugins.allow_unsigned`, only an **empty** signature, only a **managed** connector (a present signature is always verified; without the flag it is still 422 — test `an_unsigned_registration_is_refused_without_allow_unsigned_and_a_bad_one_always`). With signed release builds the path already worked. **One function:** `connectors/readiness.rs` `connector_readiness(row, facts)` + `next_step(state, viewer)`, used by `availability` and the admin `view`; `registering` = `awaiting_registration` and a live host announces the type; `waiting_for_host` only when none does. Tests: `it/connectors_readiness.rs` (an unsigned dev host's connector goes `registering` → assigned in one poll → registers → `active`/`can_connect`; the equality of `setup_state` and `host_running` on both pages for registering, active, disabled, pending approval, no live host, plus not-added on availability) and the readiness unit matrix. Not changed (out of my files, noted for the platform lane): the host reports a just-restarted instance as `running` before its registration succeeded; readiness no longer depends on that report alone.
- **Invitation links (C-1069, SEC-140):** the `201` of create carries `link` (only there); `POST /v1/invitations/{id}/link` (200 `{ link, expires_at }`, `Cache-Control: no-store`) and `POST /v1/invitations/{id}/resend` (202 `Invitation` without link, `409 mail_disabled` when `GET /v1/instance` says `email_delivery: log`), both `invitations::service::reissue`: new token, `UPDATE … WHERE id AND status='pending' AND expires_at > now()` (409 `invitation_not_pending` otherwise), audit `invitation.link` / `invitation.resend` with id and e-mail only, **security-critical** (added to `SECURITY_CRITICAL_ACTIONS`, written in the token's transaction). Rate limit: the `write` (management) bucket plus a per-organization hourly key charge `invitation-link:org:<org>` (20, `api_limiter.take` like `by-email`; `http::counters::check_webhook` is a no-op with the default memory backend, so it could not enforce a per-org cap in a single replica). **Who can see a link, for how long, and why nobody else can:** only an active owner or admin of the invitation's organization (`c.org(..).require_admin()`; a member or workspace owner 403, another organization's admin and an unknown id 404), session or API key confined to that organization; the link exists in plaintext only in that one response (`no-store`), the token is stored only as its SHA-256 hash, a re-issue replaces the hash so every earlier link dies at once, and a link is valid 7 days at most; it never appears in a list, `GET`, audit row, error body or log line (test-asserted), and even a leaked link only works for a signed-in person whose verified e-mail equals the invitation's. No logging middleware prints response bodies (the only body-reading middleware, `who_can`, rewrites 403/422 bodies and logs nothing). Tests: `it/invitations.rs` `sec140_*` (4 tests) and the SEC-10 log capture extended (`tests/sec10_logs.rs`: the create and re-issue tokens never reach a TRACE log).
- **Sign-in app Test (C-1070):** the code exchange's request building is now one function, `connect::service::token_request` (grant type, code, redirect URI, PKCE verifier, `client_auth` Basic/Body, `body_format` Form/JSON), used by both `token_exchange` and `probe_token_endpoint`; it is sent through the connect environment's guarded client (the instance client for a client bound to a self-hosted instance), 10 s timeout, at most 8 KiB read, only the OAuth `error` code kept. Mapping: `invalid_grant` (and GitHub's `bad_verification_code`) → `ok`; `invalid_client`/`unauthorized_client` (and GitHub's `incorrect_client_credentials`) → `rejected` (HTTP 200/400/401); transport error, timeout or guard refusal → `unreachable`; else `unexpected`; templated tenant token URL, no OAuth entry or disabled → `not_testable` (never stored). `state`: `disabled` when off; `ready` after an `ok` Test, or with a connection and no failed Test, or a connection made after the failed Test; `test_failed`; else `ready_not_tested`. Audited `oauth_client.test` (critical). Tests: `it/oauth_clients_test.rs` (wiremock token endpoint: the four answers, no body canary in response/audit/row, the request's form fields; the caller rule = PATCH (a workspace member 403), disabled → `not_testable` and nothing sent; a client used by a real OAuth connect is `ready` untested) and the `classify` unit test.
- **Package errors (C-1071):** `plugins/sentences.rs` `violation_sentence`: `archive_corrupt`/`archive_entry_type`/`archive_link` → "That isn't an app package. Upload the .tar.gz that `aurora plugin pack` makes."; `file_missing` → "The package is missing <path>."; `icon_not_png`/`icon_not_square`/`icon_too_small` → "The icon must be a square PNG."; `icon_too_large`; `path_*` → "The package has a file with an unsafe name: <path>."; `guide_not_utf8` → "The setup guide must be plain UTF-8 text."; `guide_too_large`, `manifest_not_utf8`, `manifest_too_large`; signature codes → "The package isn't signed by a key this server trusts."; other (manifest) codes: the validator's message without the code word; empty → "The package has a problem: <path>.". Example: `422 { error: { code: "package_invalid", message: "That isn't an app package. Upload the .tar.gz that `aurora plugin pack` makes.", details: { violations: [{ code: "archive_corrupt", path: "", message: "invalid gzip header", sentence: "That isn't an app package. …" }] } } }`; 413 "That package is too large (limit 20 MB)." Tests: unit table and `it/plugins.rs a_bad_package_says_so_in_words` (not gzip, no manifest, oversize).
- **Storage categories (C-1072):** chosen: `tables` lists **all** read tables (now incl. `transcript_search`), each with `category`; `categories` (always conversations, files, activity) sums them; `#[serde(default)]` on both new fields (an old snapshot reads, unit-tested). Per organization: **files only** (`files` = the live `used_bytes`; conversations and activity `null`), no invented size. **`can_create_org`** on `GET /v1/me`: instance admin or `org_creation = anyone`; `it/me.rs` checks all four combinations against `POST /v1/orgs` by a person already in an organization.
- **Additions:** C-1086 `AccessBinding.signed_in` (`it/workspace_suspend.rs`); C-1087 `changes`/`more`/`actor` on the rule history (`it/policy.rs`); C-1088 `include_system`/`hidden_system` on the org, workspace and instance audit lists (system = `actor_kind = 'system'`; `it/audit.rs`); C-1089 `GET /v1/me/chat-platforms` (`it/channels_mgmt.rs`); C-1090 `GET /v1/orgs/{org}/agents` (`it/agents.rs`). **D-425 (owner, 2026-11-20, arrived mid-lane):** C-1090's `state` is real: the agent header's state in P5-REFINE-11's precedence (`agentStateOf`) over P5-REFINE-4's computer words (`hosted::dto::computer_word`, merged from master for this): `paused` → `needs_key` (the dispatcher's readiness via `hosted::answer::reason_of`, the same `decide_reason` as C-1062) → `failed` → `working` (a live turn or a busy process) → `starting` → `asleep` → `ready`; `last_active_at` is the newest turn's time; `workspace` gains `id` and `personal_of` for grouping. Unit test `org_agent_state_tests` (the whole precedence) and the IT (needs_key, paused, working with `last_active_at`, ready). Permission-matrix rows for the five new routes.
- **Deviations and why:** (1) C-1090 answers the house page `{ items, next_cursor }` instead of `{ agents }` ("paged like other org lists"); `state` is `active`/`paused` (frozen). (2) The 413 keeps the code `payload_too_large` (the raw codes stay the contract); only the message changed. (3) The table is `app_oauth_clients`, not `oauth_clients`. (4) Storage category `bytes` is nullable (per-organization). (5) The per-org link cap uses `api_limiter` (see above), not `http::counters`. (6) GitHub's OAuth error words are mapped too. (7) C-1089: an API key sees its own organization only. (8) `invitation.link`, `invitation.resend` and `oauth_client.test` are security-critical audit actions (the audit writer's debug assertion requires it for `commit_critical`). All written into the contracts as "(Built: …)".
- **Screenshots compared:** none (no UI); the UI briefs compare `access-1`, `org-1`, `org-4`, `org-10`, `admin-12`, `admin-14`.
- **Tests run** (all through `heavy.sh p5-agent`, throwaway Postgres from the testkit):
  - `cargo test -p aurora-api --test it -- send_only_levels connect:: connect_access accounts:: connectors_readiness connectors_managed channels_mgmt invitations plugins:: attachment_storage me:: agents::an_organizations audit:: policy::draft_validation workspace_suspend permission_matrix audit_atomic` → 242 passed, 1 failed (`permission_matrix::coverage_every_openapi_route_is_listed`, because `web/openapi.json` was not regenerated yet); after the regeneration `-- permission_matrix openapi_matches_checked_in plugins::` → 17 passed. An earlier run had found the bug of a second `ClientRow` query (`connections::usable_clients`) without the new columns (17 OAuth connect tests) and the missing critical-set entries (the audit writer's debug assertion); both fixed before these runs.
  - After merging master and D-425: `UPDATE_OPENAPI=1 cargo test -p aurora-api --test it -- openapi_matches_checked_in agents::an_organizations permission_matrix` → 7 passed.
  - `cargo test -p aurora-api --lib -- org_agent_state levels readiness sentences storage oauth_clients access_requests` → 20 passed (and earlier with `apilimit`: 28 passed); `cargo test -p aurora-api --test sec10_logs` → 1 passed; `cargo test -p aurora-db --test it schema_matches` → 2 passed; `cargo test -p aurora-core --lib audit` → 2 passed.
  - `cargo fmt --check` ok; `cargo clippy -p aurora-api -p aurora-db -p aurora-core --all-targets -- -D warnings` clean; `python3 scripts/extract-schema.py --check` up to date (regenerated for migration 000265); `scripts/guards/run-all.sh` all passed (it first caught integration names in my comments and test data, renamed); `pnpm gen:api` regenerated `schema.d.ts`; `pnpm typecheck` clean; `eslint` on the five touched `lib/api` files clean.
  - Not run (CI is the gate): the full `it` suite, the web vitest suite (no component or test touched), Playwright. **Test files changed:** `it/connect_access.rs` (the forced-failure case now uses a tool-less app; `read` on a send-only app is `level_not_offered`), `it/invitations.rs` (the `201` carries `link`; 4 `sec140_*` tests), `it/plugins.rs`, `it/attachment_storage.rs`, `it/me.rs`, `it/policy.rs`, `it/workspace_suspend.rs`, `it/audit.rs`, `it/channels_mgmt.rs`, `it/agents.rs`, `it/permission_matrix.rs` (rows), `it/connect.rs` (two child modules), `it/main.rs`, `tests/sec10_logs.rs`; new `it/send_only_levels.rs`, `it/connectors_readiness.rs`, `it/oauth_clients_test.rs`. **Removed:** none. No web test or e2e spec asserts anything this brief changed (the "can now read …" Done sentences are the web's own, P5-REFINE-13's); no production spec to hand to the nightly.
- **Open questions:** none blocking. For the platform lane: the connector host could report an instance `running` only after its `register` succeeded (`services/aurora-connector-host/src/host.rs` `start`/`report`).
