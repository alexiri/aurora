# P5-SKLIB-1: Skill libraries read any git repository by URL; no app connection, no gateway (owner, 2026-11-21)

## Goal

Owner, 2026-11-21 (D-448): "Skill libraries need to support any git repo, not just things on github. It
shouldn't require a Github account to be configured, it should work the same as importing apps from
external repos … Drop the Github requirement and accept a git URL like for addons."

Today (P4-GSL, D-148/D-149) a library is `{ connection_id, repo, ref, path }`: an **org-scoped app
connection** of a package with a manifest `library` block (only the built-in `github` and `gitlab` have
one), and every read goes through the gateway's `POST /internal/v1/library-fetch`, which fills the
package's REST templates (`resolve_ref`, `list_tree`, `get_blob`). So a library needs a GitHub or GitLab
account connected to the organization, and a Gitea, Forgejo, Bitbucket, cgit or any self-hosted git
server cannot be used at all.

After this brief a library is `{ label, url, ref?, path?, token? }`: an https git URL (or the repository's
web page, as for addon sources), read by the **API** with the git smart-HTTP protocol, with an optional
read-only access token for private repositories. Everything above the reader stays: browse, add pinned
to a commit, read-only library skills, proposals approved by a person, budgets, the refresh job.

This is the backend brief. P5-SKLIB-2 adds instance-level libraries; P5-SKLIB-3 builds the dashboard.
This brief changes the web only as far as needed to keep it compiling and its tests green (§Part C).

## Decisions that bind this brief

- **D-448** (all of it): URL instead of connection; the API's own git reader on `http_instance`; optional
  token sealed with purpose `SkillLibraryToken`; the gateway route, the API's gateway client and the
  supersede move removed; the `library` manifest block parsed but ignored; the migration of existing
  libraries; the check route.
- **SEC-148** replaces SEC-84. **SEC-83** holds unchanged and its tests must pass against the new reader.
- Contracts **C-1122** (shape, create, patch), **C-1123** (address rules), **C-1124** (check, org route
  only; the admin one is P5-SKLIB-2's), **C-1125** (the reader), **C-1126** (token), **C-1127** (data,
  removals, limits). Migration **`20261201000270`**.
- Unchanged and binding: C-571..C-574, C-577's job semantics (lease, head epoch, throttling, budgets),
  D-154, D-178 Q-R5-3 / D-210 (recorded heads), FIX-096.

## Order and lanes

- One Rust lane. It edits `domains/skill_libraries/**`, `domains/skills/library.rs`,
  `repo/skill_libraries.rs`, the gateway's `main.rs`/`lib.rs`, `aurora-testkit`, `aurora-secrets`'
  `Purpose`, the two built-in manifests and `web/src/components/skills/libraries-page.tsx` (Part C).
- **P5-SKLIB-2 follows in the same lane** (same files). **P5-SKLIB-3** (web) starts after both merged.
- Size: L. Commit in this order, each commit green on its own targeted tests: (1) URL parser + git reader
  + testkit server with unit tests, (2) the switch of `skill_libraries` onto the reader + migration +
  token + check route + ITs moved over, (3) removals (gateway route, gateway client, supersede move,
  manifest blocks) + docs, (4) Part C web.
- **Security review (SEC-148) before merge**: tell the Orchestrator when commit (3) is in.

## Read first

- `docs/10-agents-conversations.md` §Git skill library (:900).
- `docs/05-control-plane-api.md` §Skill libraries (:905) and the two library job rows (:384, :385).
- `docs/06-gateway.md` §Library fetch (:23) (being removed) and §Destination guard (:685).
- `docs/09-app-plugins.md` §3.4 `library` (:737) and the linter list (:1039).
- `docs/15-deployment-operations.md` §Git skill library (:1330); `docs/operator/skill-libraries.md`.
- `docs/03-data-model.md` `skill_libraries` (:1147), `skills` library columns (:1117).
- `docs/17-security.md` SEC-83, SEC-84, **SEC-148**.
- `docs/API_CONTRACTS.md` C-570..C-579 (what exists), **C-1122..C-1127** (what to build).
- `docs/22-addon-repos.md` §The manifest (how addon sources accept a pasted repository URL).
- Code:
  - `services/aurora-api/src/domains/skill_libraries/content.rs`: `Reader` (:63) with `resolve` (:135),
    `tree` (:176), `blob` (:221). **This is the seam**: keep `resolve`/`tree`/`blob` and their
    `ReadError`s, replace what is under them. `assemble`, `load`, `finish`, hashing and validation stay.
  - `…/skill_libraries/gateway.rs` (deleted), `mod.rs` (`LibraryEnv`), `service.rs` (`create` :194,
    `patch` :344, `check_connection` :132, `read_ctx` :488, `listing` :512), `refresh.rs` (`compare`
    :256), `routes.rs`, `dto.rs`.
  - `services/aurora-api/src/domains/skills/library.rs` (add, approve, detach).
  - `services/aurora-api/src/domains/addon_sources/fetch.rs`: the guarded fetch pattern to copy.
  - `services/aurora-api/src/domains/connect/env.rs`: `Env::http_instance`, `allow`, `override_origin`.
  - `services/aurora-api/src/app.rs` :98, :207 (`HttpGateway::new`), :608 (`with_library_gateway`).
  - `crates/aurora-db/src/repo/skill_libraries.rs` (`LIB_COLS` :12, `insert_library` :58,
    `patch_library` :137, `connection_brief` :213).
  - `crates/aurora-secrets/src/store.rs` (`Purpose`, `Purpose::ALL`);
    `services/aurora-api/src/secrets_migrate.rs` (one source table per purpose).
  - `services/aurora-gateway/src/library_fetch.rs`, `src/main.rs` :398–440, `src/lib.rs`,
    `tests/it/library_fetch.rs`, `crates/aurora-db/src/repo/gateway.rs` :1066.
  - `services/aurora-api/src/domains/plugins/supersede.rs` (:32, :139),
    `crates/aurora-db/src/repo/plugins.rs` :687 (`move_skill_libraries`).
  - `crates/aurora-registry/src/library.rs` and the `library` validator;
    `plugins/builtin/github/plugin.yaml`, `plugins/builtin/gitlab/plugin.yaml`.
  - `crates/aurora-testkit/src/git_host.rs` (the fake GitHub REST host; replaced).

## Part A: the git reader (C-1123, C-1125, SEC-148)

1. **Address parser** in `aurora-core` (e.g. `aurora_core::git_url`): `parse(input) -> Result<GitAddress
   { url, repo_display, ref_hint, path_hint }, UrlError>`, exactly C-1123. Pure, no I/O. Unit table of at
   least 30 cases (C-1123 lists the families). The API maps `UrlError` to 422 `library_url_invalid` with
   `details.reason`.
2. **Reader** in `services/aurora-api/src/domains/skill_libraries/git/` (`pktline.rs`, `discover.rs`,
   `pack.rs`, `mod.rs`): the requests and rules of C-1125, on `Env::http_instance`. Implementation
   choice is yours within these limits, and the report says which:
   - **No `git` binary, no libgit2.** gitoxide's low-level crates (`gix-packetline`, `gix-pack`,
     `gix-object`, `gix-hash`, `gix-protocol`) may be used for parsing if they work **in memory** on our
     async reqwest bodies; otherwise write the small parts yourself (pkt-line, the pack entry header,
     zlib via `flate2` (already a workspace dependency), the two delta kinds, tree parsing). Any new crate:
     `cargo deny check` passes (licences, sources), pinned in the workspace `Cargo.toml`.
   - Nothing written to disk. Every cap of C-1125 enforced **while streaming** (stop reading at the cap,
     never buffer first and check after).
   - Every object id recomputed; a mismatch is a read failure with a fixed reason, never a partial result.
   - Protocol v2 first; v0/v1 when the discovery answer is a v0 advertisement. `filter blob:none` only when
     advertised. Blob wants in one request per read. Fallback to the whole depth-1 pack when blob wants are
     refused or `filter` is missing, under `LIBRARY_PACK_MAX`.
   - Errors become the existing `ReadError` variants with the fixed texts of C-1125 (no URL, no token, no
     server text in a reason; `last_error` stays ≤ 200 characters).
3. **The seam**: `Reader` keeps `resolve(ref) -> sha`, `tree(sha) -> Vec<TreeEntry>` (only entries under
   `path` are needed; keep `LIBRARY_TREE_MAX`), `blob(id) -> bytes`, plus a batch `blobs(&[id])` that
   browse, `load` and refresh use so one read is one fetch. Define `TreeEntry` in the API (it comes from
   `aurora_registry::library` today). `assemble` stops taking the manifest `Library`.
   Keep a trait (`RepoSource`) under `Reader` so unit tests of `content.rs`/`refresh.rs` can use an
   in-memory repository; ITs use the real reader against the testkit server.
4. **Testkit git server** (`crates/aurora-testkit/src/git_server.rs`, replacing `git_host.rs` once nothing
   uses it): fixture repositories built with the `git` CLI in a temp dir (commits, tags, branches, a
   subfolder library, symlink, submodule, executable bit, a 70 KiB file, 51 files), served by
   **`git http-backend`** (CGI) behind a small axum server, with switches: protocol v2 off (strip
   `Git-Protocol`), `uploadpack.allowFilter` off, `uploadpack.allowAnySHA1InWant` off, Basic auth
   required with a given token, inject a status (401/403/404/429 with `Retry-After`/503), redirect N
   times (same origin, other origin, to `http:`), slow answer, oversize answer, and a log of received
   requests and headers (to assert `Authorization` never crosses origins). The CI runners and dev boxes
   have `git`; the server **panics with a clear message** when `git` is missing (never skips). Point the
   API at it the way addon-source tests do (`override_origin`); if one override cannot express the
   cross-origin redirect test, extend the test hook to a map of origins (tests only).

## Part B: libraries by URL (C-1122, C-1124, C-1126, C-1127)

1. **Migration `20261201000270_skill_library_git_url.sql`** exactly as C-1127 and C-1126. The GitLab
   backfill reads the connection's instance base URL (find where `app_instances` keeps it); write it as
   one `UPDATE … FROM`, no Rust step. Columns that are not in a migration yet go only under `03 §Planned
   changes`; this one is migrated, so `03`'s live sections change.
2. **Create / patch / view / list** per C-1122: no connection check (`check_connection`,
   `connection_brief` callers removed), the address parsed by Part A, `ref`/`path` from the page URL only
   when the fields are absent, a read before saving, the token sealed (`Purpose::SkillLibraryToken`,
   C-1126) in the same transaction as the row (seal first; on a failed insert delete the sealed value),
   `token: null` deletes it, `url` patchable only while null, audit details as C-1122. The browse cache
   fingerprint (`service.rs:504`) must include `url` and the token's `token_updated_at` so a new token is
   used at once.
3. **Check route** `POST /v1/orgs/{org}/skill-libraries/check` (C-1124): the shared create path up to
   "read", then answer; 10/min per person through the existing rate-limit helper.
4. **Reads** (`read_ctx`, `listing`, `load_for_add`, `skill_content`, `refresh::compare`): open the token
   (if any) once per read, build the `Reader`, never keep the token in `LibraryEnv` or the cache.
5. **Status texts** (sanitized, fixed): "The repository needs an access token", "The access token was
   refused", "The repository was not found, or the token cannot read it", "The branch or tag was not
   found", "Enter the repository's git address", "SHA-256 repositories are not supported", the existing
   ones for throttling, tree too large and redirects.
6. **Removals** (C-1127): gateway route, module, IT file and wiring; `skill_libraries/gateway.rs`;
   `AppState::with_library_gateway`; `repo::gateway` library connection lookup if unused;
   `move_skill_libraries` and the supersede report field (its JSON key `skill_libraries` disappears from
   the supersede audit metadata; say so in the report); the `library` blocks of the two built-in
   manifests with a patch version bump each (follow the builtin version-bump rules in `09 §10` and
   whatever `scripts/guards` check builtin versions); the registry linter warns `library_ignored` on a
   manifest that still has the block (parsing unchanged, contract 1 is additive); the `raw.rs:624` comment.
   `api.gateway_internal_url` stays (accounts use it).
7. **Limits** in `aurora_core::limits`: `LIBRARY_PACK_MAX`, `LIBRARY_INFLATE_MAX`, `LIBRARY_WANTS_MAX`,
   `LIBRARIES_PER_INSTANCE` (C-1127), each with a doc comment naming `02 §Limits`.
8. **Rolling upgrade**: a new API never calls the gateway route; an old API replica talking to a new
   gateway gets 404 and records a transient refresh error that the next pass clears. Write that in
   `docs/operator/upgrades.md` with the private-repository note ("libraries of private repositories show
   'The repository needs an access token' after the upgrade until an organization admin adds one; pinned
   skills keep working").

## Part C: web, only what keeps it compiling and true

The regenerated `schema.d.ts` drops `connection` and `app_id`. In `web/src/components/skills/libraries-page.tsx`
replace the Account / Repository fields of the existing dialog with **Repository address**, Branch or tag,
Folder and **Access token** (`SecretInput`, optional) and send C-1122's body; drop the read-only-token
connection notice and the Reconnect link; show `repo` in the table. No new layout: P5-SKLIB-3 redesigns
the page. Update `web/src/lib/api/skill-libraries.ts`, `web/src/lib/api/errors.ts` (new codes), the mocks
(`web/e2e/support/mock-api.ts`, `web/e2e/support/skill-libraries.ts`) and the tests that see it
(`web/tests/skill-library.test.tsx`, `web/e2e/skill-libraries.spec.ts`).

## Tests move with the code (owner, 2026-11-20 — binding)

In **every commit**: find every test that sees what you change (`rg -n "connection_id|app_without_library|connection_not_org_scoped|library-fetch|LibraryGateway|git_host|Connect a repository|Account" services crates web/tests web/e2e scripts/e2e`), update it in the same commit, never `.skip`, `fixme`, delete or loosen a test to get green. A test is removed only with the feature it covers (the gateway route's tests go with the route; list them in the report). Known callers:

- `services/aurora-api/tests/it/skill_library.rs` (the whole suite moves onto the testkit git server;
  every behaviour it covers today keeps a test: browse, add, budgets, read-only, detach, proposals,
  approve/reject, superseded, recorded heads, throttling, single-flight, retention),
  `tests/it/skills_library.rs`, `tests/it/permission_matrix.rs` (:1055; add the check route),
  `tests/it/plugins_supersede.rs` (:214, :354: the library move is removed; the test keeps its other
  assertions), `tests/it/groups.rs` (:1360).
- `services/aurora-gateway/tests/it/library_fetch.rs` and `tests/it/main.rs` (removed with the route).
- `crates/aurora-registry` linter tests (`library_ignored`).
- Web files of Part C.

## Acceptance

1. **Address parser**: the C-1123 table passes; every refused form gives its `reason`.
2. **Reader against `git http-backend`** (API ITs): a public repository with v2 + filter, v2 without
   filter, v0 only, blob wants refused; a branch, a tag (annotated, peeled), a full commit id, the default
   branch (symref `HEAD`); a subfolder `path`; a private repository: no token → "needs an access token",
   wrong token → "refused", right token → reads; a 3-hop redirect works, a 4th, an `http:` hop and a hop
   to a refused address fail; `Authorization` absent from every request to another origin (server log);
   429 with `Retry-After: 120` → `Throttled(120)`; pack over the cap and inflate over the cap fail with
   fixed reasons; a corrupted object id fails the read; SHA-256 refused.
3. **Unit/property tests** of `pktline` and `pack` (truncated input, bad lengths, flush/delim handling,
   bad zlib, delta offsets out of range, chain depth 51, object count over cap) never panic.
4. **Same behaviour above the seam**: every existing `skill_library.rs` scenario passes on the new reader;
   blob ids of a skill added before the switch compare equal after it (seed a row with the ids
   `git http-backend` serves and run a refresh: no proposal).
5. **API**: create with a page URL fills ref/path; 409 `library_exists` on the same url+ref+path; patch
   token set/replace/remove (the old token kept on a failed re-read); `url` patch refused unless null;
   check route returns C-1124's shape, stores nothing (no row, no head, no audit), 429 after 10/min;
   no route ever returns a token; audit rows carry `token: set|replaced|removed` only.
6. **Migration** on a database with a GitHub, a GitLab (with instance base URL) and a third-app library:
   urls backfilled, third one `error` with the reason, unique index created; `extract-schema.py --check`.
7. **Removals**: the gateway has no `/internal/v1/library-fetch`; `rg -n "library-fetch|LibraryGateway|move_skill_libraries"`
   finds only history and docs that say it was removed; built-in manifests bumped without `library`.
8. **Web (Part C)**: vitest and the mocked Playwright spec for the libraries page pass with the new fields.
9. **Docs in the same commits**: `10 §Git skill library` (Opt-out switch, Connecting, Refresh: no
   connection, the reader, the token), `03`, `05` (rows of C-1122/C-1124, removed errors), `06` (§Library
   fetch removed; the :25 note), `09 §3.4 library` (ignored, linter warning), `02 §Limits`, `15 §Git skill
   library` and `docs/operator/skill-libraries.md` (how to add one, tokens per host: GitHub fine-grained
   "Contents: read", GitLab `read_repository` (project or personal), Gitea/Forgejo read scope, Bitbucket
   repository access token with user `x-token-auth`; `gateway.allowed_private` for a server on a private
   network), `docs/operator/upgrades.md`, `18 §Git skill library`, `docs/tests/security-coverage.md`
   (SEC-148 row with its proving tests; SEC-84 marked replaced), `web/openapi.json` and `schema.d.ts`
   regenerated. `python3 scripts/extract-schema.py --check` and `bash scripts/guards/run-all.sh` pass.
10. **Gates** (targeted, through `/home/alex/orca/workspaces/aurora/heavy.sh <lane> …`; CI is the full gate):
    - `cargo fmt --check`;
    - `cargo clippy -p aurora-api -p aurora-db -p aurora-core -p aurora-secrets -p aurora-testkit -p aurora-gateway -p aurora-registry --all-targets -- -D warnings`;
    - `cargo test -p aurora-core git_url`, `cargo test -p aurora-api --lib skill_libraries`;
    - `cargo test -p aurora-api --test it -- skill_library skills_library permission_matrix plugins_supersede groups secrets`;
    - `cargo test -p aurora-gateway --test it` (compiles without the route);
    - `cargo test -p aurora-registry`; `cargo deny check` if a crate was added;
    - web: vitest on the touched files, `tsc`, lint.

## Out of scope

- Instance libraries (P5-SKLIB-2) and the new dashboard (P5-SKLIB-3).
- ssh URLs, `http:`, git LFS (an LFS pointer file is stored as the small text file it is), submodules
  (still refused), following a branch automatically (still no auto-update).
- Dropping the unused `connection_id` / `app_id` columns (a later cleanup).

## Completion report

Files changed, test commands and results, and anything left undecided. Always name:

- the gitoxide crates used, or why the parser is our own, and the `cargo deny` result;
- how the testkit server is started and what it needs on the machine;
- what the migration did on the dev database (counts per app);
- every test removed with the gateway route;
- the supersede audit metadata key that disappeared.

### Handover (lane A, Opus → Sonnet, 2026-11-21, WIP commit 8bbb909a)

**State of the tree.** `cargo check -p aurora-api --lib` and `cargo check -p aurora-testkit` passed
just before the last edits; `aurora-core` `git_url` unit tests passed (2/2). The API unit tests
(`cargo test -p aurora-api --lib skill_libraries`) were **never run to the end** (stopped on
handover). Nothing else was built or run. The workspace as a whole does **not** compile yet: the
gateway still imports `aurora_testkit::git_host` (module unexported, file still on disk) and the
`skill_library.rs` IT is half moved.

**Done (in 8bbb909a).**
- Contract: `docs/05` §Skill libraries rows for C-1122/C-1124/C-1128 (org, check, admin, `?org=`),
  the refresh and retention job rows; SEC-148/SEC-149 rows in `docs/tests/security-coverage.md`
  as `pending` with planned test names (fill in the real ones).
- `aurora_core::git_url` (C-1123) with a ≥30-case table; `limits`: `LIBRARY_PACK_MAX`,
  `LIBRARY_INFLATE_MAX`, `LIBRARY_WANTS_MAX`, `LIBRARIES_PER_INSTANCE`.
- Reader `services/aurora-api/src/domains/skill_libraries/git/`: `pktline.rs`, `discover.rs` (v2 caps,
  v0 advertisement, `ls-refs`, fetch requests, side-band demux), `pack.rs` (in-memory pack parser:
  caps on objects/inflate/delta depth 50, ids recomputed, tree/commit parsing, proptests),
  `mod.rs` (`GitRepo` on `connect.http_instance`: discovery redirects ≤3 https+guarded, token as
  Basic only to the library origin, fixed reasons in `git::reason`, blob-wants-refused → whole
  depth-1 pack, streaming body cap). **Our own parser, no gitoxide; no new crate** (flate2 is a
  workspace dep, SHA-1 from aws-lc-rs; `proptest` added as API dev-dep) → no `cargo deny` change.
- Seam: `content.rs` `RepoSource` trait + `Reader::new(src, path)` with `prefetch`/`blob` cache,
  own `TreeEntry`/`Resolved`/`RefKind`; `assemble(path, entries)` (no manifest block); an in-memory
  `MemRepo` unit test.
- Migration `20261201000270_skill_library_git_url.sql` (url/token_ref/token_updated_at, backfill,
  `error` rows, `skill_libraries_url_uq`). `Purpose::SkillLibraryToken` (`skill_library.token`) in
  `Purpose::ALL` + secrets-migrate `SOURCES` + `tests/it/secrets_migrate.rs` seed/REF_SQL.
- `repo/skill_libraries.rs` (new columns, `NewLibrary.id`, `url_taken`, patch url/token, retention
  no longer marks `connection_deleted`, `connection_brief` removed, redacted `Debug`);
  `service.rs` (address/token input, seal-first create with discard on failure, `check`, patch with
  token set/replace/remove + `library_url_fixed`, delete discards the token, reads open the token per
  read, cache fingerprint has url + `token_updated_at`); `dto.rs` (C-1122/C-1124 shapes);
  `routes.rs` (`POST /orgs/{org}/skill-libraries/check`, 10/min/person via `rate_limiter`);
  `refresh.rs` (reader, one blob prefetch per pass); `mod.rs` (`LibraryEnv::new()`,
  `fetch_timeout`); `app.rs` (`with_library_gateway` → `with_library_fetch_timeout`);
  `domains/skill_libraries/gateway.rs` deleted; supersede `libraries` count and
  `repo::plugins::move_skill_libraries` removed (**the supersede audit key `skill_libraries` is
  gone**).
- Testkit `crates/aurora-testkit/src/git_server.rs` (`GitServer::start()`, bare repos via git
  plumbing, `git http-backend` CGI, switches: v2, allowFilter, allowAnySHA1InWant, `require_token`,
  `override_status`, `redirect`, `slow`, `oversize`, `wrong_pack`, `add_repo_sha256`, `tag`,
  request log with Authorization/Git-Protocol/body). Needs `git` ≥ 2.31 (uses `GIT_CONFIG_COUNT`);
  panics with a message if git is missing.
- IT `services/aurora-api/tests/it/skill_library.rs`: header and `world_with` replaced (GitServer +
  `with_connect_override` + 5 s fetch timeout; `url()`/`ORIGIN` helpers, `W` fields `pub(crate)`
  for reuse by an instance-library IT); `library_at`, `create_list_patch_delete_…` and
  `create_refusals_address_reach_and_limit` rewritten.

**Left, in order.**
1. Finish moving `skill_library.rs`: every remaining test still uses `w.conn`/`w.lab`/`connection(…)`
   or GitHub-REST request counts (`/commits/`, `/trees/`, `/git/blobs/`, `set_page_size`,
   `set_truncated`, `expect_authorization`, `override_status(needle, status)` now takes a third
   `Retry-After` arg). Map REST counts to git ones (`count("/info/refs")`, `fetches()`), replace
   pagination/truncation with a tree over `LIBRARY_TREE_MAX` and the 3-redirect rules, the
   connection-revoked block in `refresh_errors_back_off…` with token/404 cases, and
   `refresh_reads_the_host_with_the_host_credential_only_through_the_stand_in` with a token test
   (Authorization only to the library origin, never in responses/`last_error`/audit).
   Kill-switch test body still sends `connection_id` (harmless but update). Also
   `skills_library.rs`, `permission_matrix.rs` (:1055, add the check route), `plugins_supersede.rs`
   (:214, :354), `groups.rs` (:1360).
2. New IT (planned name `skill_library_git.rs`, register in `tests/it/main.rs`) for C-1125/SEC-148
   acceptance 2 and 5: v2+filter, v2 no filter, v0, v0 with allowAnySHA1InWant off (fallback),
   branch/annotated tag/commit/default, subfolder, private repo none/wrong/right token,
   3 redirects ok / 4th / `http:` / IP literal, cross-origin without Authorization, 429
   `Retry-After: 120` → `Throttled(120)`, `oversize(Some(LIBRARY_PACK_MAX+1))`, a 65 MiB zero file
   with filter off for the inflate cap, `wrong_pack`, `add_repo_sha256`; check route shape, stores
   nothing, 429 after 10; patch token set/replace/remove (old kept on failed read), `url` patch
   refused unless null; token absent from responses/audit/`last_error`; blob ids pinned before the
   switch compare equal (seed a skill row with `git.blob_id(...)` and refresh: no proposal);
   a static scan that `Purpose::SkillLibraryToken` is opened only in `skill_libraries/service.rs`
   `open_token`.
3. Migration IT on a database with a GitHub, a GitLab-with-instance and a third-app library
   (`schema_matches_spec` too); docs/03 live sections for the new columns; `extract-schema --check`.
4. Removals: gateway `library_fetch.rs`, `main.rs` :398–440 wiring, `lib.rs`, `tests/it/library_fetch.rs`
   + `tests/it/main.rs` entry, `repo/gateway.rs` :1066 lookup if unused; delete
   `crates/aurora-testkit/src/git_host.rs`; registry linter warning `library_ignored`; drop the
   `library` blocks of `plugins/builtin/github` (2.4.1 → 2.4.2) and `gitlab` (**take the next patch
   after FIX-216's 1.4.2 on master, i.e. 1.4.3; keep FIX-216's numeric-id tools and ambiguous_path
   fixtures**); `raw.rs:624` comment; registry builtin tests that assert the blocks.
5. Docs: 10, 02 §Limits, 06 (§Library fetch removed), 09 §3.4, 15, 18, `docs/operator/skill-libraries.md`,
   `docs/operator/upgrades.md`, SEC-84 coverage row marked replaced; openapi + `schema.d.ts` regenerated.
6. Part C web, then P5-SKLIB-2 (nothing started).

**Decisions not in the brief (review them).**
- Backfill host: a GitHub/GitLab connection with a registered instance uses `app_instances.host` +
  `base_path` (GitHub Enterprise too); without one, github.com / gitlab.com; a GitLab library whose
  connection is gone stays null. Duplicate url+ref+path after backfill: only the oldest gets the url,
  the others stay null/`error` (else the unique index would fail the migration).
- `repo` CHECK widened to 1..500 (the display form `host/path` exceeds the old 200).
- Field errors (`token`, `token_username` without `token`, ref, path) answer **400**
  `invalid_request` like every `invalid_request` in this API, not 422 as C-1122 says; docs/05 row I
  wrote also says 422 — align one of them.
- An IP-literal host refused by the guard answers 422 `library_unreachable` ("The repository's
  address is not allowed"), not a new `library_url_invalid` reason.
- A successful re-read on PATCH (token added, ref/path changed) sets `status` back to `active` unless
  paused.
- 403 without a token is "The repository needs an access token" (C-1125 only names 401).
- Check route `found` counts every skill directory; `valid` counts those of the first
  `LIBRARY_BROWSE_MAX` whose `SKILL.md` parses.

**Known failing.** The whole workspace build (see State); every not-yet-moved test in
`skill_library.rs`; gateway ITs (`git_host` import). No test was observed failing on its own merits.

### Completion report (lane A, Sonnet, 2026-11-21; commits on `alexiri/p5-polish-a` after `cc82ece0`)

Everything in the brief is done, in the order of the handover; nothing waits. The earlier code (the WIP commit)
was kept as written; where it was wrong the fix is listed under "Found in the earlier code".

**Commits** (on top of the WIP history, nothing rewritten): the gateway route and `git_host` removed and the
gateway limiter moved to `aurora_gateway::limiter` (the account Test still uses it); the `skill_library.rs` IT
moved onto the git server (34 tests); secrets-migrate order and counts, permission matrix, supersede and
`skills_library` ITs; the migration IT and docs/03 columns, `spec_schema.json`, `web/openapi.json` and
`schema.d.ts` regenerated; `skill_library/git.rs` IT (SEC-148); built-in manifests, `library_ignored`, registry
tests; docs, web and the pack-test fix.

**Files changed** (by area):
- Rust: `crates/aurora-core` (`git_url`, limits), `crates/aurora-db` (migration `20261201000270`, `repo/skill_libraries.rs`, `repo/plugins.rs`),
  `crates/aurora-secrets` (`Purpose::SkillLibraryToken`), `crates/aurora-registry` (`library_ignored` warning),
  `crates/aurora-testkit` (`git_server.rs` new, `git_host.rs` removed), `crates/aurora-config` (a comment),
  `services/aurora-api` (`domains/skill_libraries/**` incl. the `git/` reader, `domains/skills/library.rs`,
  `secrets_migrate.rs`, `app.rs`), `services/aurora-gateway` (`library_fetch.rs` removed, `limiter.rs` new, `main.rs`, `lib.rs`).
- Manifests: `plugins/builtin/github` 2.4.1 → **2.4.2**, `plugins/builtin/gitlab` 1.4.2 → **1.4.3** (library blocks dropped; FIX-216's numeric-id tools and `ambiguous_path` fixtures untouched).
- Docs: 01, 02, 03, 05, 06, 09, 10, 15, 16, 17, 18, 21, `API_CONTRACTS.md`, `docs/operator/skill-libraries.md`, `docs/operator/upgrades.md`, `docs/tests/security-coverage.md`.
- Generated: `web/openapi.json`, `web/src/lib/api/schema.d.ts`, `crates/aurora-db/tests/fixtures/spec_schema.json`.
- Web: `libraries-page.tsx`, `lib/api/errors.ts`; tests `web/tests/skill-library.test.tsx`, `web/e2e/support/skill-libraries.ts`.
- Guards: `scripts/guards/integration-names.allow` (the git-address parser's test table and the one-off migration name the forges; `git_host.rs` entry removed).

**Test files changed:** `services/aurora-api/tests/it/{skill_library.rs, skill_library/git.rs (new), skills_library.rs, permission_matrix.rs, plugins_supersede.rs, secrets_migrate.rs}`,
`crates/aurora-db/tests/it/{skill_library_git_url_migration.rs (new), main.rs}`, `crates/aurora-registry/tests/it/{library.rs, builtin_versions.rs}`,
`services/aurora-gateway/tests/it/{account_test.rs, main.rs}`, `web/tests/skill-library.test.tsx`, `web/e2e/support/skill-libraries.ts`.
`web/e2e/skill-libraries.spec.ts` needed no change (its screens are driven through the support file, whose connect screen waits for "Repository address" now); **the production Playwright projects cannot run here: list `skill-libraries.spec.ts` for the nightly.**

**Commands run and results** (all through `heavy.sh p5-polish-a`, a throwaway Postgres `pg-p5-polish-a`):
- `cargo fmt --check` ok; `cargo clippy -p aurora-api -p aurora-db -p aurora-core -p aurora-secrets -p aurora-testkit -p aurora-gateway -p aurora-registry -p aurora-config --all-targets -- -D warnings` ok.
- `cargo test -p aurora-core git_url`: 2 passed (the table has 30+ cases). `cargo test -p aurora-api --lib skill_libraries`: 22 passed (3 failed first, see below).
- `cargo test -p aurora-api --test it -- skill_library skills_library permission_matrix plugins_supersede groups secrets audit foundation`: **231 passed, 2 failed**: `connect::account_test::a_working_account_is_ok…` and `connect::a_tls_verification_opt_out…` (they matched the `audit` filter and read `plugins/fixtures/dist/fixture-hooks.aurora-plugin`, which needs `scripts/fixtures/build.sh`; nothing to do with this change).
- `cargo test -p aurora-db --test it -- skill_library schema_matches_spec`: 4 passed (migration IT + both schema tests). `python3 scripts/extract-schema.py --check` ok.
- `cargo test -p aurora-gateway --test it`: 381 passed, 1 failed (`apps::wire_framing::wire_log_shows_framing_and_never_a_value`, it passes alone; a log-capture race under parallel runs, not touched).
- `cargo test -p aurora-registry`: 25 + 175 passed. `bash scripts/plugins/check-builtin.sh github gitlab`: ok both. `cargo deny check`: ok (no crate added).
- `bash scripts/guards/run-all.sh`: all guards passed. Web: `vitest run tests/skill-library.test.tsx` 18 passed, `tsc --noEmit` and eslint on the touched files clean.

**Answers to "Always name":**
- *gitoxide crates:* none. The parser is our own (pkt-line, pack entry headers, both delta kinds, tree and commit parsing) on `flate2` and aws-lc-rs SHA-1, both already workspace dependencies; `proptest` is added as an API dev-dependency (already a workspace dependency). `cargo deny check`: advisories, bans, licences, sources ok.
- *Testkit server:* `GitServer::start()` builds bare repositories with `git` plumbing in a temp directory and serves them with `git http-backend` (CGI) behind an axum server on 127.0.0.1; it needs `git` ≥ 2.31 (`GIT_CONFIG_COUNT`) and panics with a message when `git` is missing. The API reaches it through `with_connect_override(&git.base())` (every https origin maps to it; the reader still decides by the logical URL which origin gets the token). Switches: `set_v2`, `set_allow_filter`, `set_allow_any_sha1`, `require_token`, `override_status` (with `Retry-After`), `redirect`, `slow`, `oversize`, `wrong_pack`, `add_repo_sha256`, `tag`, `set_default_branch`, `Change::Fill` (a 20 000+ entry tree from one blob), request log with `Authorization`, `Git-Protocol` and body, `count`, `fetches`, `ls_refs`.
- *Migration on the dev database:* **not run there** (the aurora-dev stack and its DB are off limits for lanes). The migration IT builds a database at version 269, seeds a GitHub library, a GitHub Enterprise one (registered instance), a GitLab one with an instance and base path, a gitlab.com one, a duplicate of the first (same repo, ref, folder), a GitLab one whose connection is gone and a third-app one, and migrates: github.com, the GHES host, `git.example.org/gl/...` and gitlab.com addresses are backfilled; the duplicate, the connection-less GitLab library and the third-app library become `error` with "Enter the repository's git address"; the unique index refuses a duplicate; the token columns refuse half a pair. Counts on a real database come from the Orchestrator's run (`SELECT app_id, url IS NULL, count(*) FROM skill_libraries GROUP BY 1, 2` after).
- *Tests removed with the gateway route* (`services/aurora-gateway/tests/it/library_fetch.rs`, the whole file): `each_step_is_one_get_built_from_the_template_with_the_injected_credential`, `sec_84_parameter_fuzzing_is_refused_before_anything_is_sent`, `sec_84_connection_checks_other_org_workspace_scoped_disabled_other_app`, `sec_84_the_route_needs_the_internal_secret`, `sec_84_redirects_are_not_followed_and_the_location_is_not_returned`, `sec_84_oversize_responses_are_cut_and_the_credential_never_appears`, `a_body_cut_short_by_the_upstream_is_502_library_upstream_not_a_partial_success`, `an_instance_bound_app_reads_the_apps_default_host`, `a_registered_instance_with_a_base_path_is_read_under_it`, `a_host_rule_with_rewrite_or_a_foreign_claim_cannot_serve_a_library`, `an_expired_oauth_token_is_refreshed_before_the_read`, `a_revoked_oauth_grant_is_409_reauth_required_and_sets_the_flag`, and the wire half of `the_route_is_rate_limited_per_organization` (its limiter half is now the unit test `limiter::tests::rate_limited_per_organization_and_per_gateway`, since the account Test still uses the limiter). Also: registry `builtin_github_and_gitlab_declare_a_valid_library` (replaced by `…carry_no_library_block_and_stay_valid`); `gitlab_shape_…` keeps running against a literal block; API IT `refresh_reads_the_host_with_the_host_credential_only_through_the_stand_in` (replaced by the token test), `browse_follows_pagination_and_a_truncated_tree_is_refused` (replaced by the tree-limit test: the git protocol has no pagination) and the connection-revoked/reauth/package-without-library block of `refresh_errors_back_off…` (replaced by the 404 and 401 cases); the library assertions of `plugins_supersede`.
- *Supersede audit metadata key that disappeared:* `skill_libraries` (the count of moved libraries) in the `plugin.supersede` row.

**Decisions of the handover, reviewed and kept:** the backfill host rules; `repo` widened to 500; field errors answer **400 `invalid_request`** (C-1122 and docs/05 now say so instead of 422; the reader's failures are 422); an IP-literal host the guard refuses is 422 `library_unreachable` ("The repository's address is not allowed"); a successful re-read on PATCH sets `status` back to `active` unless paused; 403 without a token reads "The repository needs an access token"; the check route counts every skill directory in `found` and the first 200 in `valid`.

**Found in the earlier code (reported, fixed where small):**
- The unit tests of `pack.rs` had never run: three failed because the **test builder** was wrong (a delta's header carries the delta data's size, and `Ofs(0, …)` pointed at its own offset), not the parser. Fixed in the builder; 22 unit tests pass.
- The secrets-migrate source for the library token sat first in `SOURCES`; the test `old_ref_stays_readable_until_discard` waits for the fourth purpose and ran out of time. Moved to the end; three hard-coded counts in `secrets_migrate.rs` went from 10/11 to 11/12.
- A skill library's list reads only `SKILL.md` blobs (`filter blob:none`), so an oversize *other* file of a skill is no longer marked invalid in the list (the old REST tree had sizes); the content request and the add still refuse it (tested). Documented in docs/10 and the operator page.

**Left undecided / for the owner:** nothing blocks. The `connection_id` and `app_id` columns stay unused (a later cleanup, as the brief says). `PROGRESS.md` and `FEATURES.md` are not touched by this lane (the Orchestrator owns them): P5-SKLIB-1's rows can say done once SEC-148 is reviewed.

#### Security notes (SEC-148, for the review before merge)

Where to look: `services/aurora-api/src/domains/skill_libraries/git/mod.rs` (the reader), `…/git/{pktline,discover,pack}.rs`, `…/service.rs` (`seal_token`, `open_token`, `create`, `patch`, `check`), `crates/aurora-core/src/git_url.rs`.
- **Token.** Sealed with `Purpose::SkillLibraryToken` before the row is inserted (`seal_token`; a failed insert discards it), opened only by `open_token` (a source-scan test fails if another file mentions the purpose or calls `open_token` outside the library domain), held in a `Secret` and never in `LibraryEnv` or the browse cache (the cache fingerprint carries `token_updated_at`, not the token). Sent as Basic **only when `u.origin() == self.origin`** (`git/mod.rs`, `send`): a redirect to another origin gets no `Authorization` (IT with the server's request log; mutation checked: sending it everywhere fails the test). Never returned (`token_set` and, for managers, `token_updated_at`), `LibraryRow` and `Token` have a redacted `Debug` (unit test), audit rows carry `token: set|replaced|removed` only, `last_error` is built from the fixed reasons of `git::reason` (the IT asserts a canary token appears in no response, audit row or `last_error`). The check route's token is used for that read and dropped (it has no row to hold it).
- **Only https, destination guard.** `GitRepo::allowed` refuses `http:`, credentials in the URL and an IP literal the guard refuses (`Env::allow`); names are checked by the resolver at dial time (`http_instance`); a redirect hop gets the same checks; redirects only on discovery, at most 3, and the target must end in the discovery path (IT: `http:`, 10/8, loopback, link-local metadata, IPv6 loopback, credentials and a non-git target all refused with the fixed reason; mutation checked: accepting `http:` fails the test). Private addresses need `gateway.allowed_private` (the guard's rule, unchanged).
- **Size and time.** The body is read as a stream and stops at `LIBRARY_PACK_MAX`; inflate is capped at `LIBRARY_INFLATE_MAX`, object count, blob size and delta depth (50); every object id is recomputed and compared with what was asked or listed (`wrong_pack` IT); a 65 MiB zero file is refused only when the filter is off (IT); a total timeout per request and per refresh pass (`LIBRARY_REFRESH_BUDGET` below the lease); SHA-256 repositories refused.
- **Things I would look at twice:** (1) `Reader::prefetch`/`blobs` ask for at most `LIBRARY_WANTS_MAX` ids per request (a request with more is split, not refused); (2) the fallback to a depth-1 whole pack when blob wants are refused is bounded by the same caps but costs up to 32 MiB per read; (3) `PATCH` of `token: null` reads the repository without it first, so removing the token of a private repository is refused with "needs an access token" until the repository is public or the library deleted: intended, but a person may find it surprising; (4) the discovery answer's `Content-Type` is checked, its body is not trusted beyond the parser; (5) no log-capture test exists for the token (only the redacted `Debug` and the source scan).
- SEC-83 holds unchanged: every scenario of `skill_library.rs` that proves it runs on the new reader (34 tests).

#### Security review send-back (SEC-148; lane A, Sonnet, 2026-11-21; commits on top of `b99fedec`)

1. **Memory exhaustion in `walk` (blocker): fixed.** `limits::LIBRARY_TREE_NAME_MAX` (255 bytes, refused in `pack::tree_entries` as `Malformed`),
   `LIBRARY_TREE_DEPTH_MAX` (**5**: a skill's own path depth is 4, plus one) and `LIBRARY_TREE_PATH_BYTES_MAX` (4 MiB, counting a folder's path twice because
   the stack holds a copy). Over the depth or the bytes the walk answers `TreeTooLarge` (**refuse, not skip**: skipping a deeper folder would silently drop files of
   a skill that is then listed as valid; the answer is `library_tree_too_large`, narrow `path`). Tests: unit `a_deeply_nested_tree_is_refused_fast`
   (2 000 nested trees of 255-byte names, under 2 s; exactly at the cap passes, one over refuses), `the_total_path_bytes_are_capped` (a wide tree),
   `a_tree_entry_name_over_255_bytes_is_malformed` (256 → malformed, 255 fine; 1 KiB names cannot be built any more: the name cap refuses them first);
   IT `skill_library/git.rs::a_repository_nested_too_deep_is_refused` (a repository one level too deep over the testkit git server → 422
   `library_tree_too_large`; the shallow folder reads). Limits in `limits.rs`, docs/02 §Limits, SEC-148 rows of docs/17 and security-coverage.
2. **Concurrency and rate: fixed.** `LibraryEnv.reads: ReadSlots` (`LIBRARY_READS_CONCURRENT` = 4 permits per replica) is taken by `probe` (create, patch),
   `check`, browse's cold listing, `load_for_add`, `skill_content` and every refresh pass (`refresh_library_within`, before its budget starts).
   Interactive reads wait for a permit **inside** `LIBRARY_READ_DEADLINE` (90 s, browse keeps its `BROWSE_DEADLINE`) and answer the existing
   **502 `library_unavailable`** (the message says too many reads or too slow); a refresh waits without a deadline. `check_rate` is now also called by
   `service::create` (organization and instance routes) and by a `patch` that re-reads (same bucket as the check: 10 a minute per person). The OpenAPI
   operations of create (both) and patch list 429 explicitly (it was already emitted generically; only a description line changed in `web/openapi.json`,
   `schema.d.ts` regenerated, no shape change for lane B). Tests: unit `skill_libraries::tests::reads_are_capped_and_a_full_replica_answers_unavailable`
   (peak concurrency 2 of 8 tasks, the deadline answer, permits returned after a timeout); IT `create_and_a_rereading_patch_share_the_checks_rate`
   (create 429 after 10–11, then a check by the same person 429, a re-reading patch 429, a rename not limited). `create_refusals_address_reach_and_limit`
   now creates its ten libraries with two admins (the shared allowance).
3. **Docs: done.** `docs/operator/upgrades.md`: library reads use the API's own guarded client, not `gateway.egress_proxy` (D-448(b)); migrated
   GitHub Enterprise / self-managed libraries whose base path was an API prefix may need the address entered again. docs/05 notes the 429 and 502 on create and patch.
4. **Missing tests: added** (unit, in `git/mod.rs`): `disguised_ip_literals_are_refused_like_the_plain_ones` (`https://2130706433/a`, `https://0x7f.1/a`,
   `https://[::ffff:127.0.0.1]/a`, plus the plain one: refused with the address reason before anything is dialled) and
   `a_redirect_to_a_name_that_resolves_privately_is_refused` (a canned 302 to `https://localhost:9/…` through the override; the new test-only
   `Env.override_bypass` makes the reader dial that one host for real, so the **guarded resolver** refuses it with `ADDRESS_REFUSED`; without the bypass the same
   redirect is followed to the override). The brief's "testkit resolver override" did not exist (the override replaces every dial, resolver included); the bypass is the smallest addition.

**Commands** (all through `heavy.sh p5-polish-a`, throwaway `pg-p5-polish-a`): `cargo fmt --check` ok; `cargo clippy -p aurora-api -p aurora-core --all-targets -- -D warnings` clean;
`cargo test -p aurora-api --lib skill_libraries` 28 passed; `cargo test -p aurora-api --test it -- skill_library openapi_matches permission_matrix skills_library` 66 passed;
`python3 scripts/extract-schema.py --check` ok; `bash scripts/guards/run-all.sh` all passed. Not run: web tests (no web source changed beyond the regenerated `schema.d.ts`).
