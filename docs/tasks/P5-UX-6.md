# P5-UX-6: Administration › Overview, and defaults that carry the user (owner, 2026-11-15)

## Goal

In the first walkthrough, the best first run (one click from sign-in to a working chat) happened only
because the admin found and ticked "Give it to every new user's agent". Other defaults made things
worse:
- Without that tick, every person had to find an AI key on their own, and people who had signed in
  earlier never got it.
- A personal workspace was named after the sign-in provider's internal id ("dev-bob",
  "dev-dana@example.test").
- Nothing told the admin that people could not sign in to Linear or Notion until someone registered an
  OAuth client; three different pages were going to say it in three ways.
- The setup wizard stayed the first sidebar item for ever, and there was no health page.
- The kind dev image shipped no apps at all.

This task makes Administration's first page a setup checklist that turns into a health page, and changes
the defaults so that a new person's agent works from the first minute.

## Linked requirements

- **Design** (D-354): the canvas “Aurora — agents, accounts and access”, https://claude.ai/artifact/3UwmUgrhh6MLCsfqdhTFmQ, boards Administration › Overview, People › New users, Apps & accounts › Apps; Organization › Apps & accounts › Apps (the “can't sign in yet” notice). Read each board's note; where a board and the spec disagree, the spec wins.
- D-326, D-351, D-353; contracts **C-922, C-923, C-924**.
- Normative: `14 §Agents, accounts and access` §9 (Administration › Overview, defaults),
  `14 §Onboarding` §1 (the setup steps), `04 §New-user placement` (the name of a personal workspace,
  starter grants), `05` placement rows, `09 §10` (the builtin source).
- Unchanged: D-162 (the slug comes from `{sub}`), C-521, C-525 (starter grant rules), SEC-77.

## Wave and order

- **Wave 3.** Starts after P5-IA-4 (New users under People; the pages this one links to), P5-UX-9 (app
  setup pages and the OAuth client form's prefill) and P5-UX-2 (the setup status function).

## Context

- **The wizard.** `web/src/components/admin/setup-view.tsx`, at `/admin` since P5-IA-1: `StarterSwitch`
  ~196–224 (only the organization's AI key; disabled unless placement is a shared organization with
  personal workspaces), and step 5 "Optional" ~323–335. Setup status and its reasons are C-533 and C-534.
- **Placement.** `GET/PUT /v1/admin/placement` (C-520..C-529); the form is
  `web/src/components/admin/placement-view.tsx`, under People › New users. A workspace's name and slug
  are set from the template when it is created at first sign-in.
- **Builtin apps.** In production they are active automatically when signed with the project key
  (`09 §10`). The kind dev image (`deploy/dev/thin.Containerfile`) copies only the binary, so a dev
  cluster has no apps. `docs/operator/plugins-and-connectors.md:19` and `09` ~1085 still say "Phase 0
  ships none". Eleven packages ship. `plugins/builtin/aws-role` and `github-app` are untracked leftovers;
  delete them from your worktree if present and never commit them.

## Scope

1. **Administration › Overview** (`/admin`).
   - Until setup is complete: the checklist (the wizard's steps), each step linking to the topic page
     that holds its settings (People › New users, AI models, Apps & accounts, Chat platforms, and
     Platform › Email as an optional step). The AI step adds an AI key for the default organization in
     place, with the form of the organization's Accounts › **Add an AI key**.
   - After: a health page from the same setup status:
     - runners, the gateway and AI endpoints;
     - **Apps people can't sign in to yet** (below);
     - e-mail, until it is set up: "E-mail is not set up. Aurora cannot send approval or invitation
       e-mails." linking to Platform › Email (setup status gains `email_not_configured`, C-924);
     - storage;
     - **Run setup again**.
2. **Apps people can't sign in to yet (C-924).**
   - Setup status gains the reasons `app_needs_oauth_client` and `app_token_only`, computed with the
     setup status function of P5-UX-2 for the default organization.
   - One component lists each app whose ways for a person to connect all need an OAuth client that does
     not exist: "People can't sign in to <App> yet", with **Register an OAuth client** (the OAuth clients
     tab with the app and client prefilled) and a link to the app's setup page. Apps a person can connect
     with a token get one informational line.
   - It replaces the wizard's step 5, and is also shown at the top of the organization's Apps tab for org
     admins. Mount it from the organization's Apps & accounts page, above the tab's content, not inside
     `org-apps-list.tsx` (P5-UX-11 edits that file beside this task).
3. **Starter by default (C-922).** Adding an AI key for the default organization in the checklist also
   adds it to the starter grants, whenever placement allows them. The switch shows **on**: "Every new
   person's agent can use this key from the first minute. Turn off to make people add their own."
4. **Backfill (C-922).** `POST /v1/admin/placement/starter-grants/backfill` (instance admin): idempotent,
   through the normal grant path with `actor_label = placement` and `backfill = true`; answer
   `{ granted, skipped }`. Setup status gains `personal_workspaces_without_starter`, and the step offers
   **Also give it to the <n> people who signed in before** when n > 0.
5. **A personal workspace's name (C-923).** A new personal workspace is named after the person: the
   given name, else the full name, else the local part of the e-mail. This is a fixed rule with no
   setting. The existing template keeps setting the slug; on People › New users its field is labelled
   "Workspace address", with the help text "The name people see is the person's own name." Nothing
   existing is renamed. The workspace selector shows "<name> (personal)". (Welcome's title is
   P5-UX-7's.)
6. **Docs and dev image.**
   - Fix the two "Phase 0 ships none" sentences.
   - `deploy/dev/kind-dev.sh` builds the builtin packages (`aurora plugin build`, unsigned) into the API
     image, at `plugins.builtin_dir`; `deploy/kind/values-dev.yaml` sets
     `config.plugins.allow_unsigned: true` (dev only, with a comment saying why). If unsigned builtins
     arrive pending approval, the script approves them through the API after install. Say in the report
     which happened. Mention it in `docs/operator/install-kind-dev.md`.
7. **Tests.**
   - API: backfill idempotence and audit; the name's fallback chain (given name, name, e-mail); the slug
     unchanged; the setup reasons for an app with no client and with an organization client, and for
     e-mail not set up.
   - Vitest: the Overview in both states; the starter switch on by default; the backfill offer; the "can't
     sign in yet" component; the e-mail line.
   - Manual, reported: after `just kind-up`, the workspace's Apps tab lists the builtin apps.

## Constraints / file ownership

- Owns: `setup-view.tsx` and the Overview page; `placement-view.tsx` (the field's label); the placement
  and setup-status API code; the function that names a workspace at creation; the "can't sign in yet"
  component and its place on the organization's Apps tab; `deploy/dev/kind-dev.sh`,
  `deploy/kind/values-dev.yaml`; `docs/operator/{plugins-and-connectors,install-kind-dev}.md`; the
  `09 §10` sentence.
- No migration: the placement document is stored where it is today.

## Acceptance criteria

1. On a fresh instance, the admin sets where new people land and adds an organization AI key, touching
   nothing else. A new person's agent answers with no key step.
2. People who signed in before get the key with **Also give it to …**, and running it again changes
   nothing.
3. A new personal workspace is named after the person ("Bob") with its slug unchanged, and no new
   setting exists for it.
4. With Linear and Notion active and no OAuth clients, the Overview lists both with a prefilled
   **Register an OAuth client**; with an organization client for Linear, Linear leaves the list.
5. After setup, `/admin` is a health page, and the checklist is one click away.
6. `just kind-up` gives a dev cluster whose Apps tab lists the builtin apps.
7. Rust and web gates; axe and narrow-route suites, both themes.

## Suggested agent/model and reason

Sonnet: backend, one page and a dev-script change that needs care with image builds. Haiku could take
scope 6 alone.

## Completion report (lane p5-agent, branch `alexiri/p5-ux6`; per-task gate only — the batch gate has NOT run)

Commits: `b5837c2f` (C-923), `7862afd3` (backfill + setup reasons), `a54bd2fc` (web), `f45e6593` (dev image + docs).

- **C-923** names a new personal workspace after the person (`template::person_name`: given name, else name, else
  e-mail local part); `tenancy::create_workspace_named_in_tx` + `SlugFrom::Text` keep the slug coming from the
  template. Four existing placement tests asserted the old name and were changed (they failed first with the new rule);
  new test `c923_personal_workspace_is_named_after_the_person` (given name / full name / e-mail).
- **C-922** `POST /v1/admin/placement/starter-grants/backfill` (`BackfillStarterGrants`/`BackfillResult`), one
  transaction and one audit row per grant (`actor_label = placement`, `backfill = true`), existing grants counted as
  skipped. Setup status: `personal_workspaces_without_starter`. Test `c922_backfill_…` (idempotence, audit, 400 for a
  non-starter id, 403 for a member). **Fail-first not observed** for it: the route did not exist.
- **C-924** `apps::setup::sign_in_gaps` (built on `setup_status`, default organization) → checks `app:<id>` with
  `app_needs_oauth_client` / `app_token_only` (`app_id`, `package`, `client_key`, `value` = app name) and `email` with
  `email_not_configured` in the `optional` step. Test `setup_status_lists_apps_people_cannot_sign_in_to_and_missing_email`
  (no client → listed; token alternative → informational; instance-bound and no-OAuth apps not listed; an organization
  client takes Linear off the list). The old `oauth_clients` check stays in the API (stable reason codes) and is hidden by
  the page.
- **Web**: `/admin` is the checklist until no step is `todo`, then a health page (needs-you, runners, gateway, AI models,
  new people, storage); **Run setup again** / **Back to the health page**. `SignInGaps` (one component) is on the
  Overview and on the organization's Apps tab (`OrgSignInGaps`, mounted in `org-topics.tsx` above `AvailabilityView`;
  for an organization it applies the same rule to each app's C-931 org view in TS, `gapFromSetup`, because
  `GET /v1/admin/setup` is instance-admin only). First AI key → starter grant automatically; switch text per the board;
  backfill offer. People › New users: **Workspace address**. Selector: "<name> (personal)" for the person's own personal
  workspace (from `/v1/me` memberships). Vitest: 36 onboarding (6 new), placement, context-switcher (1 new).
- **Dev image**: builder container builds the packages unsigned into `$WORK/plugins`; the API image copies them to
  `/var/lib/aurora/plugins`; `values-dev.yaml` `config.plugins.allow_unsigned: true`. **Deviation from the brief:**
  `plugins.builtin_dir` refuses unsigned packages even with `allow_unsigned` (`ingest.rs:287`), so they arrive as the
  `directory` source and **wait for approval**; the script does not approve them (it has no admin credential), the admin
  approves under Apps & accounts › Apps (documented in `install-kind-dev.md`). **Not run**: `just kind-up` / the manual
  Apps-tab check (disk below 50 GB on the box, and no cluster of this lane).
- Rule slips: two Rust appends (`placement/dto.rs`, `apps/setup.rs`) were written with a heredoc; content verified.
- Not done: storage line shows bytes held (no quota exists); health stat tiles of the board (People, Agents, …) are not
  built (no API for them); board "Needs you" is the spec's pending-panel, not pixel-matched.
- Gates run: `cargo fmt --check`, `cargo clippy -p aurora-api -p aurora-db --all-targets --all-features -D warnings`
  (ok), targeted `cargo test -p aurora-api --test it` (placement::, onboarding::, c922, setup_status_lists, openapi),
  `scripts/guards/run-all.sh` (exit 0), `extract-schema.py --check` (ok), `pnpm typecheck`, vitest on the touched files.
  **Not run:** the batch gate (full aurora-api it + lib, workspace clippy, web full vitest/lint/build, Playwright, axe,
  upgrade.sh — no migration was added).
- **Batch gate (run 2026-11-17, after `git merge master`)**: `cargo test -p aurora-db -p aurora-api` (all targets, fixtures
  built): api lib 141 ok, it 1382 ok / 1 failed → `permission_matrix::coverage_every_openapi_route_is_listed` (the
  backfill route was not in the matrix) — fixed (commit "the permission matrix lists …"), `permission_matrix` filter rerun
  5/5 ok. Workspace clippy all-targets/all-features `-D warnings` ok; guards exit 0; `extract-schema.py --check` ok;
  `gen:api` no drift (openapi test passes); web typecheck ok, lint 0 errors (5 warnings), vitest 132 files / 2202 ok,
  `pnpm build` ok. Playwright `mocked`: 1116 passed; the `real-stack` project (needs a live stack, not part of this gate)
  failed 4 specs; `stage4-routes` FIX-029 "Computer tab with a failed start (390 px)" failed in both themes and again alone:
  P5-UX-14's "1 account needs attention" chip made the agent header's pill row 1 px too wide — fixed (flex-wrap, commit
  "the agent header's pill row wraps"), stage4-routes + ia-navigation 260/260 ok after. Production project 11/11 ok.
  No migration → `upgrade.sh` not run.
