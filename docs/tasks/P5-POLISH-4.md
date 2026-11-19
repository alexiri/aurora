# P5-POLISH-4: Ask an admin to set up an app (owner, 2026-11-19)

## Goal

Replace the connect journey's dead end with a request. Today, when Bob picks Google Calendar, chooses
an access level and reaches Sign in, he reads: "Google Calendar cannot be connected yet. Only
organization owners and admins can register oauth client. Ask alice." There is nothing to click. A
disabled Connect stays on screen, and Alice never hears about it.

After this brief, Bob presses **Ask Alice**. The request lands in the **Needs you** card of the
workspace Overview for every owner and admin of the organization, and on the app's row in
Organization › Apps. It also arrives as an e-mail and a push notice. Once the app is ready, the request
closes by itself, Bob is told, and his journey offers the app.

This brief builds the server side, the notifications and the admin's **Needs you** row. The person's
**Ask** buttons are added by P5-POLISH-5 (journey) and P5-POLISH-7 (Apps tab); the organization's Apps
row by P5-POLISH-9.

## Linked requirements

- **Design reference (D-380).** Proposal **B2** of “Aurora UX fixes”,
  <https://claude.ai/artifact/WXbcpcrkbCLuxMSurpkuQW#b2>.
  - Local copy `/home/alex/orca/workspaces/aurora/design-ux-fixes/`: `after/b2.png` shows both
    sides, the person in the journey and Alice on her Overview; `before/b2.png` shows today.
    `after/b1.png` shows the "Needs an admin first" group with **Ask Alice**.
  - The review: <https://claude.ai/artifact/33oHaXQpCU3oSVRVmgtY5Z> (screen 257).
- **D-386**, **C-1030**, **SEC-135**; migration **`20261201000255`** (`docs/03-data-model.md` ›
  Planned changes › `app_setup_requests`; move the rows into the live sections in this change).
- Uses **C-1029**'s readiness (P5-POLISH-3) to decide "ready", and fills its `readiness.requested`.
- Patterns to copy:
  - P5-UX-17's agent access requests: a request record resolved by a real change
    (`crates/aurora-db/migrations/20261201000250_agent_access_requests.sql`,
    `services/aurora-api/src/domains/access_requests/`);
  - the RFE-011a notifier, with claims for idempotent mail (`domains/approvals/notify.rs`,
    `approval_notifications`, C-265);
  - Web Push (`domains/push/deliver.rs`).

## Wave and order

- After **P5-POLISH-3** in lane B: P5-POLISH-3 → **P5-POLISH-4** → P5-POLISH-5.
- Needed by P5-POLISH-5 (the journey's **Ask**), -7 (the Apps tab's **Ask**) and -9 (organization
  Apps rows).

## Context

- Readiness and who can fix it: `domains/apps/setup.rs` (factored by P5-POLISH-3) and
  `http/who_can.rs` (C-981).
- **Organization roles:** an active owner or admin of the organization is the audience for listing and
  declining. To ask, use the same "may connect apps here" check the connect plan uses (`can_own` on the
  workspace).
- **Mail:**
  - `domains/auth/mail.rs:100` (`Mailer`);
  - the folding and claims pattern in `approvals/notify.rs`;
  - `approvals.email_notifications` switches approval mail off, and the same switch applies here;
  - with `email.smtp_url` unset, mail is logged only.
- **Push:** `domains/push/deliver.rs`, `push/routes.rs`. Add types `app_setup_request`,
  `app_setup_resolved` and `app_setup_declined` to the push payload enum and the service worker's
  notification text map (`web/public/sw.js` or wherever push texts live).
- **Where readiness changes**, so open requests must be re-evaluated:
  - OAuth client create and delete (`connections` / OAuth client routes at workspace, organization and
    instance level);
  - app instance add and remove;
  - availability changes (`/v1/orgs/{org}/app-availability`);
  - package approve or activate (`plugins/lifecycle`).

  Call one function `app_setup_requests::reevaluate(org_id | all)` from each, after commit.
- **Overview:** `web/src/components/overview/needs-you.tsx:17` (`NeedsYou`, one card: approvals and
  accounts needing attention, C-927). `overview-view.tsx:81` renders it.

## Scope

1. **Migration `20261201000255`:** `app_setup_requests`, `app_setup_request_askers` and
   `app_setup_request_notices` as planned in `03`.
   - Partial unique index: one `open` row per `(org_id, app_id)`.
   - Foreign keys cascade on organization, user and workspace deletion.
   - Closed rows are purged after 90 days by the retention sweep (add them to it).
2. **Routes (C-1030)** in a new domain `services/aurora-api/src/domains/app_setup_requests/`:
   - **Ask:** `POST …/setup-request`.
     - Checks: membership and `can_own` on `ws`; the app is visible to the caller; readiness is not
       `ready` (409 `app_ready`); rate limit 20 per person per hour (429 `rate_limited`, a new rate
       class `setup_request` in `05`'s table).
     - Joins or opens the organization's request for the app and records `needs`. Idempotent.
     - Audit `app_setup_request.ask`.
   - **Withdraw:** `DELETE …/setup-request`. Removes the caller's asker row and closes the request as
     `withdrawn` when no asker is left.
   - **List:** `GET /v1/orgs/{org}/app-setup-requests?status=open`, for owners and admins (404 for
     others, as other organization admin routes do).
   - **Decline:** `POST …/{id}/decline`, for owners and admins. Sets `closed_by` and notifies the
     askers.
   - **Catalogue:** `AppSummary.readiness.requested = { by_me, count }` from the open request
     (P5-POLISH-3 left the field absent).
3. **Resolution:**
   - `reevaluate` closes, as `resolved`, every open request whose app is now `ready` for at least one
     of its askers (evaluated for each asker's workspace; askers for whom it is still not ready keep a
     new open request with only them).
   - Then it notifies each resolved asker.
   - It runs after commit of the triggering change, never inside the admin's transaction, so a failure
     does not undo the admin's change.
   - A periodic sweep (every 10 minutes, like the approval notifier) also re-evaluates, so a missed
     trigger heals.
4. **Notices:**
   - **To owners and admins:**
     - one e-mail per request: subject "<Name> wants <App> in <Org>"; body: the askers, the app, what
       is needed in one sentence ("A Google sign-in app needs to be registered once; Calendar, Drive
       and Gmail then work for everyone."), and a link to Organization › Apps & accounts › Apps
       filtered to the app;
     - later asks join the same request and are folded into one digest per hour;
     - one push of type `app_setup_request`.
   - **To askers:** `app_setup_resolved` ("<App> is ready. Give it to <Agent> from its Access tab.",
     with a link to the journey) or `app_setup_declined` ("<Name> isn't setting up <App> now.").
   - Claims in `app_setup_request_notices` keep mail and push exactly-once across replicas.
   - Bodies carry only the fields SEC-135 allows.
5. **Web, the admin's side only:**
   - API hooks in a new `web/src/lib/api/app-setup-requests.ts`: `useAskSetup`, `useWithdrawSetup`,
     `useOrgSetupRequests`, `useDeclineSetup`. Each invalidates `["apps"]` and the Overview queries.
   - **`NeedsYou`** gains rows of kind `app_setup_request` for viewers who are owners or admins of the
     workspace's organization, as in `after/b2.png` (bottom):
     - avatar of the first asker;
     - "Bob Builder wants Google Calendar" (or "Bob and Carol want …");
     - one line on what is needed;
     - **Set up** (opens Organization › Apps & accounts › Apps at the app, or the OAuth-client form
       when that is what is needed) and **Not now** (decline, with a confirmation that names who will
       be told).
     - Use P5-POLISH-2's `AttentionChip`, or the existing pending row style when P5-POLISH-2 has not
       merged.
   - Push texts for the three new types.
6. **Docs:** add the new rows in `05` (already listed) with the rate class; write the request in
   `14 §Agents, accounts and access` (one paragraph, from D-386); update `03`, `17` (SEC-135 is
   written) and `docs/tests/security-coverage.md`.

## Tests

- **API integration:**
  - ask, then a second asker joins one request; idempotent re-ask;
  - withdraw by the last asker closes it;
  - list and decline by an owner, and 404 for a member and for another organization's owner;
  - 409 for a ready app; 429 on the 21st ask;
  - registering the missing OAuth client resolves the request and notifies each asker once;
  - a decline notifies once;
  - every SEC-135 case;
  - mail and push bodies hold only the allowed fields;
  - claims stop duplicates across two notifier runs.
- **Permission matrix** rows for the four routes.
- **Vitest:**
  - the `NeedsYou` row in one-asker and two-asker forms, and only for owners and admins;
  - **Not now** confirms and declines.
- **Playwright** (mocked): an owner's Overview shows the request; **Set up** opens the app's place in
  Organization › Apps & accounts.

## Out of scope

- The **Ask** buttons in the journey (P5-POLISH-5) and the Apps tab (P5-POLISH-7).
- The organization's Apps table row "Bob asked" (P5-POLISH-9).

## Constraints / file ownership

- **Owns:**
  - `domains/app_setup_requests/*` (new);
  - migration `000255`;
  - the push type additions;
  - the `reevaluate` calls in the OAuth client, instance, availability and plugin lifecycle code (one
    line each);
  - `web/src/lib/api/app-setup-requests.ts` (new);
  - `components/overview/needs-you.tsx` (the new row kind only).
- Regenerate `web/openapi.json` and `schema.d.ts`.
- Shared files: append to `permission_matrix.rs` and `security-coverage.md`.

## Acceptance criteria

1. On the dev stack without a Google sign-in app, a member asks for Google Calendar. The
   organization's owners see "<Member> wants Google Calendar" in Needs you, get one e-mail (logged when
   SMTP is unset) and one push, and can decline it.
2. Registering a Google OAuth client for the organization resolves the request; the asker is notified
   once, and the catalogue then reports Google Calendar `ready`.
3. SEC-135's tests pass. No route reveals a request to someone outside the organization's owners and
   admins, except the asker's own `requested.by_me`.
4. Gates:
   - `cargo fmt --check`, clippy `-D warnings`, targeted `cargo test` (aurora-api, aurora-db), guards,
     `extract-schema.py --check`, the OpenAPI check;
   - web typecheck, lint, vitest and build; the Playwright mocked suites for the Overview.
   - Report the counts.

## Suggested agent/model and reason

Sonnet (`rust-developer`). A new domain with a migration, notifications across replicas and a security
invariant; a small web part at the end.
