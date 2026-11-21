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

### Completion report (lane A, Sonnet, 2026-11-21; same lane and commits as P5-SKLIB-1, after it)

Everything in the brief is done (items 1–10 and acceptance 1–7); nothing waits. P5-SKLIB-1 is in the same lane
and not yet merged, so this brief did not wait for a merge: it was built on top of it.

**What was built.**
- Migration `20261201000271_skill_library_instance.sql`: `org_id` nullable, the label and address unique indexes on
  `(coalesce(org_id, nil uuid), …)`, `skill_library_org_state`, `over_budget` rows moved into it (status back to `active`).
- `domains/skill_libraries/access.rs`: **the one authority helper**: `manage(caller, state, row)` (an organization's library: its owner/admin, 403 for a
  member as before; an instance library: an instance admin's **session**, else 404) and `read(caller, state, row, ?org)`
  (organization library: the browse audience of its organization, `?org` optional and equal to the library's organization;
  instance library: `?org` required, 400 without it, the caller must use a workspace of it, else 404; members get 404 for a non-active library).
  Every library route goes through them (`admin_of`, `browse_of`); the org-scoped list, create and check routes use the organization's own checks.
- Routes: `GET/POST /v1/admin/skill-libraries`, `POST /v1/admin/skill-libraries/check` (`InstanceAdmin`: session only, the check limited to 10 a minute per person);
  `GET /v1/skill-libraries/{id}` takes `?org=` (an organization's owner/admin gets that organization's view of an instance library, an instance admin its
  totals and `orgs_using`); browse and content take `?org=`; the organization list is its own libraries by label, then the instance ones by label.
- `SkillLibrary` gains `orgs_using`; `View::{Picker, Org(org), Manage}` decides what a caller sees; `status` is `over_budget` in an organization's view
  while its `skill_library_org_state` row exists and the library is otherwise `active`; `token_updated_at` is hidden from organizations.
- Refresh: one pass, proposals per skill, the budget sum and insert under the lock of **the skill's organization**; an organization over budget gets its state row and
  no proposal while the others get theirs; rows are set from scratch at the end of a full pass (added, or deleted when the organization fits again or no longer uses the library);
  a library with any row skips the unchanged-head short-circuit; the library's own status never becomes `over_budget` any more.
- Delete: one transaction detaches everywhere, writes the instance row and one `skill_library.delete` row per affected organization (`commit_critical_many`), and discards the sealed token.
- Web (only to stay green): instance rows show read-only in the same table with "Shared with every organization" and no actions; Add from library browses as the organization (`?org=`).

**Files changed** (beyond P5-SKLIB-1's): `crates/aurora-db/migrations/20261201000271_skill_library_instance.sql`, `crates/aurora-db/src/repo/skill_libraries.rs`,
`services/aurora-api/src/domains/skill_libraries/{access.rs (new), dto.rs, mod.rs, refresh.rs, routes.rs, service.rs}`, `services/aurora-api/src/domains/skills/library.rs`;
docs 03, 04, 05, 10, 15, 18, `API_CONTRACTS.md`, operator page, security coverage; generated `web/openapi.json`, `schema.d.ts`, `spec_schema.json`;
web `libraries-page.tsx`, `lib/api/skill-libraries.ts`, `skills/library.tsx`; tests `services/aurora-api/tests/it/{skill_library/instance.rs (new), skill_library.rs, permission_matrix.rs}`,
`crates/aurora-db/tests/it/{skill_library_instance_migration.rs (new), main.rs}`, `web/tests/skill-library.test.tsx`, `web/e2e/support/skill-libraries.ts` (an instance row in the mocked list; the production Playwright projects cannot run here: `skill-libraries.spec.ts` for the nightly).

**Commands run and results:** the same gate set as P5-SKLIB-1's report, run after this brief (clippy `-D warnings` and `cargo fmt --check` clean; the IT run `skill_library skills_library permission_matrix plugins_supersede groups secrets audit foundation`: 231 passed, the only 2 failures need the fixture package build, see that report; `skill_library::instance`: 7 passed; db `skill_library_instance_migration` and `schema_matches_spec` passed; `extract-schema.py --check`, `scripts/guards/run-all.sh` (the `limits-readers` guard needed this brief: `LIBRARIES_PER_INSTANCE` has a reader now), `tsc`, eslint and the vitest file: ok).
The `over_budget` scenarios of `skill_library.rs` were rewritten for the new rule (state row, organization-view status, head recorded, the next pass compares again and clears the row); the other `over_budget` assertions pass unchanged through the organization view.

**Always name:**
- *Queries that moved from the library's org to the skill's org:* the refresh's per-skill budget lock key (`skill_library_pending:<skill org>`) and `pending_bytes_org(<skill org>)` (it used `lib.org_id`); the `skill.library.update_proposed` audit row (`.org(skill org)`); `library_skills(library, org)` for the browse chips (filtered by `skills.org_id`); `skill_counts(library, org)` and `pending_count(library, org)` (`skills.org_id`); the add path's check (`lib.org_id.is_some_and(|o| o != org)`). Approve, reject, detach, `pending_for_org/workspace/agent` and `pending_bytes_org` already keyed on the skill (`s.org_id` / the skill row) and are unchanged; the new `affected_orgs(library)` groups by `skills.org_id`. None of these joins a people table (the gateway role has no grant on these tables at all).
- *Org list order:* the organization's own libraries by label, then the instance libraries by label (an IT asserts it).
- *New audit metadata key:* `library_label` on `skill_library.delete` rows (the brief's `{ library_label, repo, skills_detached }`); the existing `label` is kept beside it. Instance rows (create, update, delete) have `org_id` null and actor kind `instance_admin`; nothing else changed.

**Decisions and differences to review:**
- Item 4 says the organization list appends the *active* instance libraries, item 9 says a paused one is listed as Paused. Built: **owners/admins see every status** (so Paused shows), members only `active`: the rule for an organization's own libraries. docs/05, C-1128 and docs/10 say so. Browse of a paused library behaves as for organization libraries today (owners/admins can browse it, members get 404); the brief's "409 `library_paused` as today" matches nothing in the code today (that code is only on check-now and approve), so it was not invented.
- `added` chips: the library's skills **of the organization** the caller can see: owners/admins see all of them, other people the org-tier skills and those of the workspaces they can use. (For an organization's own library members used to see every tier's chips; this narrows that to what they can see.)
- `GET /v1/skill-libraries/{id}` by an instance admin with `?org=` answers that organization's view (no membership needed); without `?org=` the totals.
- A `PATCH` by a manager of an organization library keeps today's 403 for a member of the organization and 404 for a stranger; an instance library is always 404 for non-managers.
- The `over_budget` value stays allowed in the status CHECK for one release (C-1128); nothing writes it.

**Left undecided / for the owner:** Q-SKLIB-1..3 are built on the recommended defaults (no organization hiding, a token after the backfill for private repositories, https only). `PROGRESS.md` and `FEATURES.md` are the Orchestrator's.

#### Security notes (SEC-149, for the review before merge)

Where to look: `domains/skill_libraries/access.rs` (the authority), `routes.rs` (every handler's first lines), `service.rs` (`view`, `delete`), `refresh.rs` (`compare`), `domains/skills/library.rs` (`add`, `added_by_path`), `repo/skill_libraries.rs`.
- **Authority per tier of caller.** Manage: an instance library only by `c.is_instance_admin && c.is_session()` (org owners/admins, members, strangers and **API keys, even an instance admin's, get 404**; the admin routes use `InstanceAdmin`, which is session-only, and answer 401/403 to a key). 404 rather than 403 so ids do not leak. Read: only *as* an organization the caller can use a workspace of; `?org=` is required (400) and anything else about the organization is a 404, so the answer for "another organization", "not a member" and "no workspace" is the same. Tests: `only_instance_admins_manage_an_instance_library_and_never_by_key`, `two_organizations_use_one_instance_library_each_with_its_own_numbers` (browse as B from A's admin: 404; unbound member: 404), permission-matrix rows for the three admin routes. Mutations checked by hand: allowing an instance admin's API key to manage fails the first; listing every organization's skills in the chips fails the second.
- **Nothing moves between organizations.** Counts, pending updates, `status` and the chips are filtered by `skills.org_id` for an organization's view; the instance admin's list has totals and `orgs_using` only (no skill names, ids or tiers; the IT asserts a skill name is absent). `token_updated_at` is never shown to an organization. The browse cache is per library and holds only the repository listing; the chips are computed per request and never cached.
- **Budgets.** One organization's open-proposal budget is taken under that organization's advisory lock and never stops another's proposals (IT with two organizations, one over budget). A concurrent refresh of two libraries of one organization still serialises on that lock.
- **Add and approve keep their authority.** `add` still needs the tier's write authority (the route's check, unchanged) and now only relaxes "the library is the organization's own" for `org_id` null; approvals decide by the skill's tier authority and its own organization (A's admin got 404 on B's skill).
- **Delete.** One transaction: affected organizations are read under the library's row lock, skills detached, library and sealed token deleted, instance and per-organization audit rows written atomically (`commit_critical_many`); an organization's row carries only that organization's count.
- **Things I would look at twice:** (1) `GET /v1/skill-libraries/{id}?org=X` by an instance admin does not require membership of X (they can see totals anyway); (2) an instance library's `last_error` text (for example "The repository needs an access token") is shown to every organization's admins: fixed reasons only, never the URL or token; (3) `skill_library_org_state` rows are replaced at the end of a *completed* pass only, so a pass that errors leaves the previous states; (4) an organization admin sees an instance library's address (`url`, `repo`): intended (they browse it), but it is the instance admin's choice what to publish there.

#### Security review send-back (SEC-149; lane A, Sonnet, 2026-11-21; commits on top of `b99fedec`)

5. **Stale row in delete/patch: fixed.** `delete` now has no earlier `get`: `token_ref`, `label`, `repo` and `org_id` come from the row
   `get_library_for_update` returns inside the transaction. `patch` keeps its earlier read for validation but decides what to discard from the locked row
   (`locked.token_ref`, when the patch changes the reference and it differs from the new one) and only after the commit. Test:
   IT `instance.rs::deleting_an_instance_library_discards_its_sealed_token_from_the_store` (the default store is a `MemoryStore`, so the sealed value is **counted**:
   create 1 → replace by PATCH 1 → delete 0; a second library: `token: null` 0). A deterministic race test is not practical (the window is between two
   statements of one request); the replace-then-delete sequence is covered, and the delete assertion fails if `discard_token` is removed.
6. **Errors as 404: fixed.** `access::hide_denial` turns only 404/403 into the library's 404; a database or other internal error passes through (5xx).
   Used by `read` (the `?org=` audience) and the organization branch of `view_as`.
7. **One authority helper: done.** `access::view_as(caller, state, row, org) -> View` replaces the inline logic of `GET /v1/skill-libraries/{id}`
   (the handler is now two lines). Behaviour kept: an instance admin's session may view any organization's per-tier counts of an instance library (no names;
   D-449(c)); **new: the `?org=` must exist** (`orgs::name_of`), else 404. Noted in the SEC-149 row of docs/17 and security-coverage. Test: IT
   `the_view_as_an_organization_needs_that_organization_to_exist` (existing org 200 with the organization's view, unknown org 404).
8. **Shared refresh fairness: done.** At the top of the per-skill loop an organization already `over` is skipped before `content::load`; each changed directory is
   loaded once per pass (`BTreeMap<dir, Loaded>`) and reused across the skills of every organization (a read error still fails the pass; an invalid result is cached too).
   The test hook is `LibraryEnv.loads` (a per-replica counter, so parallel ITs do not disturb each other). Test: IT
   `a_refresh_pass_loads_a_directory_once_and_skips_an_over_budget_organization` (A over budget with two skills, B with one of the same directory: **one** load for the pass
   instead of two, A without a proposal, B with its proposal; it fails without either the skip or the cache).
9. **Tests that would not fail: added.** Sealed-value deletion: see 5. `added_by_path`'s `can_use`: IT `added_chips_leave_out_workspaces_the_caller_cannot_use`
   (a skill added to a second workspace of the organization is absent from a member's `added` of another workspace and present for the admin).
   Organization library with another organization's `?org=`: IT `the_view_as_an_organization_needs_that_organization_to_exist` (browse and skill content → 404; its own organization 200).

**Not changed, as told:** the error reasons, and `http_instance` without `.no_proxy()`. **For lane B / API shape:** none of the routes, fields or error codes changed;
the new `502 library_unavailable` and `429 rate_limited` on create/patch use existing codes (a create that used to answer 201 can now answer 429 after ten reads a minute by one person).
Same commands and results as in the SEC-148 send-back under P5-SKLIB-1.
