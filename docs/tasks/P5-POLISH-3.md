# P5-POLISH-3: The server side of the fixes: app readiness, package text, builtin approval, honest health, endpoint role warnings (owner, 2026-11-19)

## Goal

Give the dashboard the facts it needs to stop showing things that aren't true, which UX review 4 found
in several places:

1. **The journey said every app was "Ready to connect."** The catalogue (`GET /v1/apps`) has no
   readiness. The journey tried to work it out from a per-tile plan fetched on focus, so every tile
   started out "ready". Google Calendar, Drive, Gmail, Microsoft 365, Notion and Linear then dead-ended
   on the Sign in step.
2. **The app cards cut their descriptions mid-word.** The token step didn't say which permissions to
   tick, and Done had nothing to try. The manifest has no short text for people.
3. **Twelve builtin apps waited for approval in the dev image** before anyone could connect anything.
   The owner decided that apps shipped with Aurora, and their updates, arrive approved (D-381).
4. **A health line said "Works" from a passed test or a setting.** The owner: "There's nothing worse
   than getting an error and see the interface telling you that it works." Real use and tests share one
   `last_check_*` pair today.
5. **Every agent page showed "Large/Medium/Small is not set" warnings.** A new person can't act on
   them; the admin who can set roles never sees them (D-385).

## Linked requirements

- **Design reference (D-380).** Proposals **B1**, **B4**, **B5**, **C1**, **C3**, **D2**, **E5** and
  **F2** of “Aurora UX fixes”, <https://claude.ai/artifact/WXbcpcrkbCLuxMSurpkuQW>. They show the
  screens this data feeds.
  - Local copy: `/home/alex/orca/workspaces/aurora/design-ux-fixes/` (`README.txt`, `after/b1.png`,
    `after/b4.png`, `after/b5.png`, `after/d2.png`, `after/e5.png`, `after/f2.png`).
  - The review: <https://claude.ai/artifact/33oHaXQpCU3oSVRVmgtY5Z> (screens 024, 050, 105, 210, 240,
    250, 257).
- **Decisions:** D-381, D-383, D-385, D-387, D-388.
- **Contracts:** **C-1029**, **C-1031**, **C-1032**, **C-1033** and **C-1034** in
  `docs/API_CONTRACTS.md` › "Phase 5 — P5-POLISH". C-1030 is P5-POLISH-4's.
- **SEC-136** (`docs/17-security.md`). Migration **`20261201000254`** (planned rows in
  `docs/03-data-model.md` › Planned changes › "real use and tests apart"; move them into the live
  sections in this change).
- **Normative docs:** `05` rows for `/v1/apps`, `/v1/apps/{app}`, `/v1/admin/plugins` and
  `/v1/admin/llm-endpoint-warnings`. `09 §3` (manifest) and `§10` (approval) need the new fields and
  rules written in this change.

## Wave and order

- **First, in parallel with P5-POLISH-1 and P5-POLISH-8.** No dependency.
- Lane B: **P5-POLISH-3** → P5-POLISH-4 → P5-POLISH-5.
- Unblocks P5-POLISH-5 (journey), -6 (agent pages), -7 (workspace), -9 (organization) and -10
  (Administration).
- `web/openapi.json` and `web/src/lib/api/schema.d.ts` are regenerated in this change.

## Context

- **Catalogue:**
  - `services/aurora-api/src/domains/apps/routes.rs:107` (`listApps`, `GET /v1/apps?workspace_id=`);
  - DTO `apps/dto.rs:29` `AppSummary` (`available`, `connection_count`, …);
  - availability `apps/service.rs:14–52` (`Avail`, from `gateway::app_availability`).
- **Readiness logic:**
  - the connect plan `GET /v1/apps/{app}/connect-plan` (`routes.rs:87`) is a projection of
    `apps/setup.rs` `setup_status`;
  - `SetupMethod` (setup.rs:66) carries `ready`, `needs: [oauth_client|instance|admin_material]`,
    `who_can`, `scope_presets`; `ConnectPlan` (setup.rs:157) adds `available_who_can`;
  - OAuth clients come from `connections::usable_clients` (workspace → org → instance);
  - `WhoCan`: `services/aurora-api/src/http/who_can.rs:42` (C-981);
  - the organization-wide gap list is `SignInGap` (setup.rs:477/491, C-924).
- **Manifest:**
  - `crates/aurora-registry/src/manifest/model.rs`: `Manifest` :14, `App` :165, `About` :300,
    `Connect` :359, `AccessLevel` :522 (`id`, `label`, `authorize_params`), `ScopePresets` :548;
  - docs `09 §3` (line ~70, `about` ~129, `connect[]` ~169);
  - builtins in `plugins/builtin/`: atlassian, aws-keys, caldav, github, gitlab, google-workspace,
    ics-calendar, indico, linear, microsoft-365, notion, postgresql, sentry.
- **Package ingest:** `services/aurora-api/src/domains/plugins/`.
  - Scans: `scan.rs:20` `scan_once` (`plugins.dir`, `Source::Directory`) and `:31`
    `scan_builtin_once` (`plugins.builtin_dir`, default `/usr/share/aurora/plugins`,
    `crates/aurora-config/src/raw.rs:1123`).
  - `ingest.rs:200–206`: Builtin + project signer activates at first install; otherwise
    `pending("first_install")`.
  - Pending reasons at :192 and :211–228 (`claim_conflict`, `publisher_changed`,
    `builtin_id_reserved`, `version_downgrade`, `permissions_widened`).
  - Builtin fails closed at :283–292. `Source` enum (`builtin|directory|upload|addon`) at :21–36.
  - Signer recorded at :309–313. DTO `plugins/dto.rs:238–242`.
  - Approve route `plugins/routes.rs:267`.
- **Images:**
  - `deploy/dev/thin.Containerfile:7–11` copies the packages **unsigned into `/var/lib/aurora/plugins`**
    (`plugins.dir`), with `allow_unsigned` in `deploy/kind/values-dev.yaml:41`. That is why the dev
    image showed 12 pending apps.
  - The release image ships signed archives to `/usr/share/aurora/plugins`
    (`images/services/Containerfile:152`).
  - Related decisions: D-75, D-326(d), D-142, D-184.
- **Health:**
  - migration `20261201000248_account_health.sql` (`last_check_at`, `last_check_result` on
    `app_connections` and `secrets`);
  - `domains/accounts/health.rs:10` `state_of`, `:42` `health()` (adds `last_used_at`;
    `accounts/service.rs:310,386`);
  - Test: `accounts/routes.rs:26`, `accounts/test.rs` (C-943);
  - gateway recording `services/aurora-gateway/src/account_health.rs` (C-985), called from
    `decide.rs:319` (401 → rejected, 2xx/3xx → ok, at most one write a minute).
- **Role warnings:** `domains/hosted/model.rs:543` `resolve_roles` (`role_unset` :590,
  `role_not_allowed`, `role_not_served` D-372); `GET /v1/agents/{agent}/model-readiness` at
  `hosted/routes.rs:512`.

## Scope

1. **C-1029, readiness in the catalogue.**
   - Compute `readiness` for every app in one pass:
     - load the usable OAuth clients (workspace, organization, instance) and the instances once;
     - evaluate each app's methods with the same function `setup_status` uses (factor it so the
       per-app plan and the catalogue cannot disagree; a unit test asserts they agree for every
       builtin fixture);
     - `needs` and `who_can` follow C-1029.
   - Omit apps that are not `available` for callers who cannot change availability.
   - Add `accounts` (from the accounts query of C-917, one query for all apps) and `summary`.
   - Keep `connection_count`.
   - Measure the route with 20 apps before and after. Report p50/p95 in the completion report; it must
     not regress by more than 20 %.
2. **C-1031, package text.**
   - Add `about.summary`, `about.first_prompt` and `access_levels[].permission_hints` to the manifest
     model with validation (lengths as C-1031). Expose them on `GET /v1/apps/{app}`, on the connect
     plan's methods and on `AppSummary.summary`.
   - Treat changes to these fields alone as non-widening: they never set `permissions_widened`.
   - **Write the content for all 13 builtin packages.** For each package:
     - one plain `summary` of at most 100 characters, saying what an agent can do with it (see the
       wording in `after/d2.png`: "Repositories, issues, pull requests and Actions on github.com.");
     - one `first_prompt` a person could send to try it ("What's on my calendar this week?");
     - for each token method and each access level, the `permission_hints` a person must tick in the
       app's token screen, in the app's own words. For GitHub fine-grained tokens with Read only, as in
       `after/b4.png`: "Contents · Read", "Issues · Read", "Pull requests · Read", "Metadata · Read".
       Check each service's current token UI documentation before writing them, and cite the source
       URL in the package's `docs/setup.md` next to the steps.
   - Bump each package's version.
3. **C-1032 and SEC-136, builtin approval.**
   - In `ingest.rs`: a project-signed package from `plugins.builtin_dir` is activated at first install
     (today) **and** on every later version or digest change, including `permissions_widened`. Write
     audit `plugin.auto_approved { version, previous_version, reason: "builtin_release" }`.
   - With `plugins.allow_unsigned = true`, an unsigned package in `plugins.builtin_dir` is treated the
     same way (`reason: "builtin_dev_unsigned"`). Otherwise it fails closed as today.
   - The pending reasons `claim_conflict`, `publisher_changed`, `builtin_id_reserved` and
     `version_downgrade` still apply to builtins. Only the approval of a newer project version changes.
   - Change `deploy/dev/thin.Containerfile` (and any kind or dev script that copies packages) to
     install the packages into the builtin directory, so a fresh dev or kind install shows them active.
   - Add `origin`, `summary` and `reaches` to `GET /v1/admin/plugins` items.
   - Update `09 §10` with the rule.
4. **C-1033, real use and tests apart (migration `000254`).**
   - Add the five columns to `app_connections` and `secrets`, with CHECKs. Backfill from
     `last_check_*` in the migration.
   - **Gateway** (`account_health.rs`): write `last_ok_at` or `last_error_at`/`last_error` for agent
     calls only, keeping the one-write-a-minute limit per account and outcome. A failure always writes,
     even inside the minute after a success, so a failure is never hidden.
   - **Test:** writes only `last_test_*`.
   - **`AccountHealth`** (`health.rs`): the new fields and the `state` values of C-1033. Keep
     `reauth_required` → `sign_in_expired` and `disabled` → `disabled`.
   - Update the gateway's DB role grants if it needs `UPDATE` on the new columns
     (`crates/aurora-db` role grants and the security coverage row of SEC-129).
   - Keep writing nothing to `last_check_*` from now on, but keep the columns.
   - Update `03` (move the planned rows into the live sections) and `docs/tests/security-coverage.md`.
5. **C-1034, endpoint role warnings.**
   - New route `GET /v1/admin/llm-endpoint-warnings` (instance admin) using `resolve_roles` with each
     endpoint's defaults. Add it to the permission matrix
     (`services/aurora-api/tests/it/permission_matrix.rs`).
   - `model-readiness` is unchanged.
6. **OpenAPI and generated types:** regenerate `web/openapi.json` and `web/src/lib/api/schema.d.ts`.
   The web build must still pass (fields are additive). Don't change web components here; the page
   briefs do.

## Tests

- **Unit:**
  - readiness agrees with `setup_status` for each builtin fixture (ready, needs an OAuth client,
    needs an instance, needs admin material);
  - an app switched off is omitted for a member and present (`available: false`) for an owner;
  - manifest validation of the new fields (too long, too many hints);
  - summary and hint changes alone are not a widening.
- **API integration** (`services/aurora-api/tests/it/`):
  - the catalogue's `readiness`, `accounts` and `summary` for a member and an owner;
  - SEC-136's cases (a project-signed upgrade with widened permissions activates with
    `plugin.auto_approved`; the same package in `plugins.dir`, uploaded, or from an addon source is
    pending; an unsigned builtin with `allow_unsigned = false` fails closed);
  - health: an agent call success writes `last_ok_at`, a 401 writes `last_error_at`/`rejected`, a Test
    writes only `last_test_*`, and `state` follows;
  - `llm-endpoint-warnings` lists an endpoint without roles and is 403 for a non-admin.
- **Gateway tests** for the health writes, including a failure written inside the minute after a
  success.
- **Migration:** `20261201000254` up on a copy with existing `last_check_*` rows backfills correctly;
  `scripts/extract-schema.py --check` after moving the `03` rows.

## Out of scope

- All web components: P5-POLISH-5/6/7/9/10 consume these fields.
- App setup requests (C-1030, P5-POLISH-4).

## Constraints / file ownership

- **Owns:**
  - `domains/apps/{routes,dto,service,setup}.rs` (readiness factoring);
  - `crates/aurora-registry/src/manifest/*`;
  - `plugins/builtin/*/` (manifest text and `docs/setup.md` sources);
  - `domains/plugins/{ingest,dto,routes}.rs`;
  - `deploy/dev/thin.Containerfile` and the dev/kind package copy;
  - `domains/accounts/{health,test}.rs`;
  - `services/aurora-gateway/src/account_health.rs`;
  - migration `20261201000254`;
  - the new endpoint-warnings route;
  - the regenerated OpenAPI and schema.
- Serialize with other `crates/aurora-registry` work (memory: FIX-159 → P5-UX-2 → P5-UX-13 were
  serialized the same way). Check with the Orchestrator that no running lane edits the manifest model.
- Shared files: append to `docs/tests/security-coverage.md` and `permission_matrix.rs`.

## Acceptance criteria

1. `GET /v1/apps` returns `readiness`, `accounts` and `summary`. In the dev cluster without OAuth
   clients, Google Calendar, Drive, Gmail, Microsoft 365, Notion and Linear are `needs_admin` with
   `needs: ["oauth_client"]` and the organization's owners in `who_can`; GitHub, GitLab, Calendar link,
   Sentry and AWS are `ready`.
2. Every builtin package has a `summary` of at most 100 characters, a `first_prompt`, and
   `permission_hints` per access level for its token methods; `GET /v1/apps/github` shows them.
3. A fresh dev or kind install shows all builtin apps active with `plugin.auto_approved` audit rows;
   SEC-136's tests pass; a release upgrade that widens a builtin's permissions keeps it active.
4. A failed agent call shows `state: "failing"` until a later success; a passed Test never makes
   `state` `working`.
5. Admin endpoint warnings list the dev mock endpoints that lack roles.
6. Gates: `cargo fmt --check`; clippy for the workspace `-D warnings`; targeted `cargo test` for
   aurora-api, aurora-gateway, aurora-registry and aurora-db; `scripts/guards/run-all.sh`;
   `python3 scripts/extract-schema.py --check`; OpenAPI check (`openapi_matches_checked_in`) and
   `pnpm gen:api` with no diff; web typecheck and build. Report the counts.

## Suggested agent/model and reason

Sonnet (`rust-developer`). It spans the API, gateway, registry and a migration, and has security
weight (SEC-136, SEC-129). Not Haiku.
