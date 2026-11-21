# P5-SKLIB-2: Instance-level skill libraries (owner, 2026-11-21)

## Goal

Owner, 2026-11-21 (D-449): "It should also be possible to configure it at the Instance level, not just at
the org level."

After P5-SKLIB-1 a library is an organization's `{ label, url, ref, path, token }`. This brief lets an
**instance admin** set up a library for the whole installation: every organization sees it beside its
own libraries, browses it and adds skills from it, with exactly the authority it needs for any skill of
that tier. The instance admin curates the catalogue; each organization still decides, skill by skill,
what its agents are told, and approves its own updates.

This is a backend brief. P5-SKLIB-3 builds the dashboard for both levels. Web changes here are limited to
keeping the web compiling and its tests green.

## Decisions that bind this brief

- **D-449** (all of it): `org_id` null = instance library; instance-admin management; every organization
  sees the active instance libraries; browse with an organization context; per-organization counts,
  pending updates and open-proposal budget; one refresh for everyone; delete detaches everywhere with
  audit rows in each organization; the token is the instance admin's. No per-organization opt-out
  (open question Q-SKLIB-1; build without it).
- **SEC-149**. SEC-56, SEC-83 and SEC-148 hold unchanged.
- Contract **C-1128** (and C-1124's admin route, C-1122's shape with `scope`). Migration
  **`20261201000271`**.

## Order and lanes

- Same Rust lane as P5-SKLIB-1, **after it has merged** (same files). Size M–L.
- P5-SKLIB-3 (web) starts when this has merged.
- **Security review (SEC-149) before merge.** Tell the Orchestrator when the commit is in.

## Read first

- `docs/tasks/P5-SKLIB-1.md` and its completion report (the reader, the token, the check route).
- `docs/10-agents-conversations.md` §Git skill library; `docs/04-identity-tenancy.md` the permission
  matrix rows :696–697 and the audit list :1453; `docs/05-control-plane-api.md` §Skill libraries and the
  library job rows (:384, :385); the addon-source admin row (:974) for the admin route conventions.
- `docs/API_CONTRACTS.md` C-570..C-577, **C-1122, C-1124, C-1128**; `docs/17-security.md` **SEC-149**.
- Code:
  - `services/aurora-api/src/domains/skill_libraries/{service,routes,refresh,dto}.rs` as P5-SKLIB-1 left
    them; `domains/skills/library.rs` (add path: the library's org is checked against the tier's org
    today; find it and allow `org_id IS NULL`).
  - `crates/aurora-db/src/repo/skill_libraries.rs`: `list_libraries` :104, `count_libraries` :116,
    `skill_counts` :226, `pending_count` :241, `pending_bytes_org` :529, `pending_for_org` :677,
    `claim_due` :717, `finish_refresh` :756, `detach_all` :402.
  - `services/aurora-api/src/http/auth.rs` `InstanceAdmin`; `domains/addon_sources/routes.rs` for an admin
    route set; `domains/audit` for instance rows (`org_id` null, C-638's instance audit log).
  - `services/aurora-api/src/domains/skill_libraries/refresh.rs` :128–145 (the org lock and the
    `over_budget` decision you generalize).

## Work

1. **Migration `20261201000271_skill_library_instance.sql`** (C-1128): `org_id` nullable; the label index
   on `(coalesce(org_id, '00000000-0000-0000-0000-000000000000'::uuid), label)`; the url index likewise;
   `skill_library_org_state`; move `over_budget` rows into it. `skills.library_id`'s FK is unchanged.
2. **Authority helper**: one function that, given a library row and a caller, answers `Manage` /
   `ReadAs(org)` / `None`: organization library → its owner/admin manage, `uses_any_workspace` members
   read; instance library → instance admins (session only, never an API key) manage, and a member of
   org X reads it **as X** only with `?org=X` and `uses_any_workspace(X)`. Every library route goes
   through it; an instance library the caller cannot manage or read answers 404 (never 403, so ids don't
   leak). API keys of an organization can never manage an instance library.
3. **Admin routes** `GET/POST /v1/admin/skill-libraries`, `POST /v1/admin/skill-libraries/check`
   (C-1124, C-1128): `InstanceAdmin`, session only, `LIBRARIES_PER_INSTANCE`; `orgs_using` and totals.
   `PATCH/DELETE/refresh` on `/v1/skill-libraries/{id}` use the helper.
4. **Organization list** `GET /v1/orgs/{org}/skill-libraries`: append active instance libraries with
   `scope: "instance"`, fields per C-1128, counts and pending updates **of this org only**, `status`
   `over_budget` from `skill_library_org_state`. Members get `id`, `label`, `status`, `scope`.
5. **Browse and content** with `?org=` (C-1128): `added` filtered to that org's skills the caller can
   see; the browse cache stays per library (the listing is the same for every org), the `added` chips
   are computed per request, never cached.
6. **Add** from an instance library at any tier of any org (C-572's checks otherwise unchanged).
   `skill_library_updates` and the pending lists already key on the skill; check every query that joins
   `skill_libraries` on `org_id` and make it use the **skill's** org.
7. **Refresh** (C-1128): proposals per skill as today; the budget sum and insert per skill's org under that
   org's advisory lock (FIX-096); an org over budget gets its `skill_library_org_state` row and no
   proposal; under budget again → row deleted; a library with any state row skips the unchanged-head
   short-circuit. The library's own `status` no longer becomes `over_budget`; `error`, `paused` and the
   backoff stay library-wide.
8. **Delete** of an instance library: `detach_all` (every org), then the instance audit row and one
   `skill_library.delete` row per affected org with `{ library_label, repo, skills_detached }`, in one
   transaction; the sealed token deleted.
9. **Kill switch and paused**: `skills.library.enabled` false hides instance libraries too; a paused
   instance library is listed to organizations as Paused, browse answers 409 `library_paused` as today.
10. **Web, only to stay green**: the regenerated `schema.d.ts` adds `scope` and `orgs_using`; make
    `web/src/lib/api/skill-libraries.ts` and the libraries page tolerate instance rows (show them read-only
    in the same table with a "Shared with every organization" badge, no actions) and keep the mocks and
    tests green. P5-SKLIB-3 redesigns it.

## Tests move with the code (owner, 2026-11-20 — binding)

In **every commit**: find every test that sees what you change (`rg -n "over_budget|skill-libraries|org_id" services/aurora-api/tests/it/skill_library.rs web/tests web/e2e`), update it in the same commit, never `.skip`, `fixme`, delete or loosen a test. Known: `tests/it/skill_library.rs` (the `over_budget` scenarios now assert the org state and the org-view `status`), `tests/it/permission_matrix.rs` (new rows), `tests/it/groups.rs` (:1360, the org list now carries instance rows), `web/tests/skill-library.test.tsx`, `web/e2e/support/mock-api.ts`, `web/e2e/support/skill-libraries.ts`, `web/e2e/skill-libraries.spec.ts`.

## Acceptance

1. **Matrix** (`permission_matrix.rs`): admin routes instance-admin only (org owner 403, API key 401/403
   as the admin guard answers); PATCH/DELETE/refresh of an instance library by an org owner → 404; browse
   of an instance library without `?org=` → 400, with another org's id → 404, with own org → 200.
2. **Two organizations** A and B, one instance library: both list it; A adds skill `x` at org tier, B at an
   agent tier; A's list shows `skill_counts.org = 1` and nothing of B, B's the reverse; browse `added`
   shows only the caller's org; the admin list shows `orgs_using = 2` and totals, no skill names.
3. **Refresh**: a push changes `x`: one refresh pass writes one proposal for A's skill and one for B's; A
   approves (its tier authority, hash quoted), B's stays pending; A over its open-proposal budget gets
   none and its view says `over_budget` while B gets its proposal; freeing A's space clears the state at
   the next pass.
4. **Delete**: the instance admin deletes it: both skills detached with content unchanged, one instance
   audit row and one row in A and one in B; the sealed token is gone.
5. **Organization libraries unchanged**: every existing `skill_library.rs` scenario passes; the migrated
   `over_budget` row shows `over_budget` in its org's view.
6. **Docs in the same commit**: `10 §Git skill library` (instance libraries: who manages, who sees, one
   refresh, budgets per org, delete), `03` (`org_id` nullable, `skill_library_org_state`), `04` (matrix
   rows and audit list), `05` (admin rows, the `?org=` parameter, the org list's `scope`, the job row's
   budget rule), `15` and `docs/operator/skill-libraries.md` (instance libraries), `docs/tests/security-coverage.md`
   (SEC-149 with its proving tests), `web/openapi.json` and `schema.d.ts` regenerated;
   `extract-schema.py --check` and `scripts/guards/run-all.sh` pass.
7. **Gates** (targeted, through `heavy.sh`): `cargo fmt --check`;
   `cargo clippy -p aurora-api -p aurora-db --all-targets -- -D warnings`;
   `cargo test -p aurora-api --test it -- skill_library skills_library permission_matrix groups audit`;
   web vitest on the touched files, `tsc`, lint.

## Out of scope

- An organization hiding an instance library (Q-SKLIB-1), an instance-wide skill tier, notifying
  organizations when an instance library changes or is removed, the dashboard (P5-SKLIB-3).

## Completion report

Files changed, test commands and results, and anything left undecided. Always name:

- every query you changed from the library's org to the skill's org;
- how the org list is ordered (org libraries first, then instance, each by label, unless you found a
  reason not to);
- whether any existing audit action needed a new metadata key.
