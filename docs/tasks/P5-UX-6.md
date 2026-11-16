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
