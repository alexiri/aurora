# P5-UX-6: Admin defaults that carry the user (owner walkthrough, 2026-11-15)

## Goal

In the walkthrough, the best first run (one click from sign-in to a working chat) happened only because
the admin found and ticked "Give it to every new user's agent". Other defaults made things worse:

- Without that tick, every user had to find an LLM key on their own.
- Users who signed in earlier never got the key.
- The workspace was named after the IdP subject (`dev-bob`).
- Nothing told the admin that people could not sign in to Linear or Notion until someone registered an
  OAuth client.
- The kind dev image shipped no app packages at all.

This task changes those defaults.

## Linked requirements

- D-326; contracts **C-922, C-923, C-924**.
- Normative: `04 §New-user placement` (display name template, starter grants), `14 §Onboarding` §1 (setup
  wizard) and `14 §First run and connecting` §4, `05` placement rows, `09 §10` (builtin source; doc fix).
- Unchanged: D-162 (slug from `{sub}`), C-521, C-525 (starter grant rules), SEC-77.

## Context

- **Wizard.** `web/src/components/admin/setup-view.tsx`: `StarterSwitch` ~196–224 (only the org LLM
  secret; disabled unless placement is `shared_org` with personal workspaces), and step 5 "Optional"
  ~323–335. Setup status and reasons are C-533/C-534 (the API's setup-status route).
- **Placement.** `GET/PUT /v1/admin/placement` (C-520..C-529); the placement form is
  `web/src/components/admin/placement-view.tsx`. Workspace creation at first sign-in sets name and slug
  from the template; find the function in the API's placement code.
- **Builtins.** In production they activate automatically when signed with the project key
  (`09 §10`; `ingest.rs` ~202).
  - The kind dev image (`deploy/dev/thin.Containerfile`) copies only the binary, so a dev cluster has no
    apps.
  - `docs/operator/plugins-and-connectors.md:19` and `09` ~1085 still say "Phase 0 ships none".
  - 11 packages ship (`docs/operator/apps-and-catalogue.md`). `plugins/builtin/aws-role` and
    `github-app` are untracked leftovers (only `hooks/target`); delete them from your worktree if
    present, never commit them.

## Scope

1. **Starter by default (C-922 a).** In the wizard's LLM step, adding a default-org LLM key also adds it to
   `placement.starter_grants` (one PUT after the key's POST, or one API call if the lane prefers a combined
   route; say which in the report) whenever placement allows starter grants. The switch shows **on** with
   the C-922 text, and turning it off removes the key from the starter grants.
2. **Backfill (C-922 b, c).**
   - Add `POST /v1/admin/placement/starter-grants/backfill` (instance admin). It is idempotent, grants
     through the normal grant path with `actor_label = placement` and `backfill = true`, and answers
     `{ granted, skipped }`.
   - Setup status gains `personal_workspaces_without_starter`.
   - The wizard step offers **Also give it to the <n> existing personal workspaces' agents** when n > 0.
3. **Display name template (C-923).**
   - Add `personal_workspace.display_name_template` (default `{given_name}|{name}|{email_local}`) to the
     placement document. It is validated like the name template (same variables, same fallback syntax,
     length limit), shown in the placement form with its own preview, and used for the workspace
     **name** at creation; the existing template keeps setting the slug.
   - Nothing existing is renamed (D-162(e)).
   - The Welcome title for a personal workspace is "Welcome, <display name>"; the workspace selector
     shows "<name> (personal)".
4. **Wizard Apps step (C-924).**
   - Add setup reasons `app_needs_oauth_client` and `app_token_only` (C-534), computed with the P5-UX-2
     plan function for the default org. If P5-UX-2 has not merged, compute readiness with the same
     OAuth-client query and leave a note.
   - Replace step 5 with **Apps**:
     - each active app whose person methods all need a missing client says "People can't sign in to
       <App> yet", with **Register an OAuth client** linking to Admin › OAuth clients with `package` and
       `client_key` prefilled (the form reads them from the query);
     - token-only apps get an informational line;
     - Connectors and Plugins keep their links.
5. **Docs and dev image (D-326 d).**
   - Fix the two "Phase 0 ships none" sentences.
   - `deploy/dev/kind-dev.sh` builds the builtin archives (`aurora plugin build`, unsigned) into the API
     image or a mounted volume at `plugins.builtin_dir`.
   - `deploy/kind/values-dev.yaml` sets `config.plugins.allow_unsigned: true` (dev only; comment why).
   - Unsigned builtins in dev arrive `pending approval` unless the builtin-source rule activates them;
     if they are pending, the dev script approves them through the API after install. Say which happened.
   - `docs/operator/install-kind-dev.md` mentions it.
6. **Tests.**
   - API ITs: backfill idempotence and audit; the template default and the fallback chain; the slug
     unchanged; setup reasons for an OAuth-only app with no client and with an org client.
   - Vitest: wizard switch default on; the backfill offer; the Apps step rendering.
   - The kind dev check: after `just kind-up`, `GET /v1/apps` lists the builtins (manual, reported).

## Constraints / file ownership

- Owns: `setup-view.tsx`, `placement-view.tsx`, the placement and setup-status API code, the
  workspace-creation naming function, `deploy/dev/kind-dev.sh`, `deploy/kind/values-dev.yaml`,
  `docs/operator/{plugins-and-connectors,install-kind-dev}.md`, the `09 §10` sentence.
- Admin OAuth clients form: reading `package` and `client_key` from the query only.
- No migration (the placement document is stored where it is today; if it is a typed column set, report
  and add the key the way C-521's keys were added).

## Acceptance criteria

1. Fresh instance: the admin saves placement and adds an org LLM key in the wizard, touching nothing else.
   A new user's agent answers with no key step (Welcome shows steps 1–2 done).
2. An existing user's agent gets the key after **Also give it to … existing**, and running it again
   grants nothing new.
3. A new personal workspace is titled from the display template ("Bob") with the slug unchanged
   (`dev-bob`).
4. With Linear and Notion active and no OAuth clients, the wizard's Apps step lists both with a prefilled
   **Register an OAuth client**; with an org client for Linear, Linear leaves the list.
5. `just kind-up` gives a dev cluster whose Apps tab lists the builtin apps.
6. Rust and web gates as usual; axe and narrow-route suites, both themes.

## Dependencies

P5-UX-2 for the shared readiness function (soft, see Scope 4). Not beside P5-UX-5 if both change the
sign-in placement code: P5-UX-5 first.

## Suggested agent/model and reason

Sonnet: backend plus wizard, plus a dev-script change that needs care with image builds. Haiku could take
Scope 5 alone.
