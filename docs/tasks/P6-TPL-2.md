# P6-TPL-2: Agent templates — template sources from any git repository, the poll job, ingest (head read, package download, source comparison), review and approval, organization switches and the workspace catalogue (owner, 2026-11-21 and 2026-11-22, phase 6, D-435, D-436, D-456, D-457, C-1106..C-1110, C-1113, C-1142, SEC-146)

## Why
Instance admins add template repositories by their https git address, as for a skill library. Aurora reads the
tracked branch, downloads the package CI published for each new version, and checks that the package is exactly the
repository's source. It then keeps the version waiting until an admin has read and approved that exact content. The
previous version keeps working throughout. Organizations can turn templates off. Workspaces see what they can use,
which of their accounts fit, and which skill libraries the templates need (`docs/tasks/P6-TEMPLATES.md`).

This brief builds that API. Creating agents is P6-TPL-3.

Read first:
- `P6-TEMPLATES.md`;
- `API_CONTRACTS` **C-1106..C-1110** and C-1113, and **C-1142**'s resolution (all of them);
- **C-1122..C-1126** (skill libraries: address rules, check without saving, the git reader, token storage — you reuse
  all four);
- C-590..C-609 (addon sources: the routes, poll job and leases you mirror);
- C-917 (Account), C-504 (who may give an account to an agent), C-980 (levels, limits, `custom_only`), C-1022
  (attention items), C-1128 (instance libraries);
- `17` SEC-146, SEC-148, SEC-86;
- `07 §App availability`, `05 §Background jobs`.

## Scope

### 1. Migration and `03`
One migration. Take the next free number from PROGRESS's §Number allocation when you start, and update that line. It
holds:
- the four tables of C-1106, with their constraints and indexes: `agent_template_versions (agent_template_id, status)`
  and `agent_templates (source_id)`;
- the unique `(url, coalesce(ref, ''), path)` index on sources;
- `token_ref` / `token_updated_at` with their CHECK.

Move the rows from `03 §Planned changes` into real `03` sections in the same commit.
`python3 scripts/extract-schema.py --check` must pass.

### 2. Secrets purpose
Add `Purpose::TemplateSourceToken` to `aurora_secrets::store`, in `Purpose::ALL`, with source table
`agent_template_sources.token_ref`, so that `secrets migrate` and key rotation cover it. Do it exactly as
`SkillLibraryToken` was added by P5-SKLIB-1; find that commit and follow it, `08` included.

### 3. The repository reader, shared
Template sources use the **skill library's reader** (C-1125, in `services/aurora-api`; find it next to `content.rs`
of the skill-library domain). Do not write a second git client.
- Where it hard-codes library values, make them parameters of a small `ReadProfile` struct, with the library's values
  as its default: the user agent, the per-blob cap `LIBRARY_FILE_MAX`, the token's purpose and owner. Template reads
  pass `TEMPLATE_HELPER_FILE_MAX`, `aurora-templates/<version> git/2.0` and their own token.
- Skill-library behaviour must not change: its tests stay green, untouched.
- You also need to read every blob under one directory at a given commit. Use the existing `tree` and `blob` calls; if
  a helper that does "all files under a path" exists for adding a library skill, reuse it.

### 4. Sources (C-1107)
- The routes of C-1107.
- **Address handling is C-1123's parser** (in `aurora-core`), the same function skill libraries call.
- **Preview** reads the repository once, as C-1124 does for libraries (same rate limit, same `no-store`).
- Add and patch store and replace the sealed token as C-1122 and C-1126 do (re-read first; on failure the old token
  stays). The token is never in a response, log, audit row or `check_failed_reason`.
- Mirror the addon-source code for everything else: find it under `services/aurora-api`, the `addon_sources` routes
  and job. **Reuse its lease helpers and poll-job skeleton** by extracting shared functions where needed rather than
  copying them. Keep the addon behaviour unchanged: its tests must stay green, untouched.
- Differences from addons:
  - no `trusted_keys`, no key fingerprint and no `signing_key_changed` state;
  - git reads instead of a raw manifest URL;
  - the package download below;
  - `TEMPLATE_PACKAGE_RETRY` for sources that have a version waiting.
- Every route is IA-only and audited as C-1113 says, with `commit_critical` for the critical actions.

### 5. Ingest and approval (C-1108)
- **The check path of C-1108, steps 1–7, for both triggers** (scheduled and manual), per source, under its lease.
  Use `aurora_template::parse_manifest`, `parse_meta`, `render_package_url`, `read_archive`, `read_tree` and
  `content_sha256`. Never parse or compare by hand.
- **The package download**:
  - It goes through `http_instance` (the destination guard at dial time), https only.
  - At most 3 redirects, each https and guarded.
  - The source's token goes as HTTP Basic only on the repository URL's origin (scheme, host, port), and never on a
    request after a redirect to another origin. Reuse C-1125's origin check.
  - The body is streamed under `TEMPLATE_ARCHIVE_MAX`, with a `TEMPLATE_FETCH_TIMEOUT` total.
  - Record `archive_sha256` and `size_bytes`.
- **`awaiting_package`** handling as C-1108 step 5: retried on every check of the source; the source due again within
  `TEMPLATE_PACKAGE_RETRY`; expired to `invalid` after `TEMPLATE_PACKAGE_WAIT`.
- **The source comparison** (step 6): the package's content hash must equal the tree's at the row's `commit_sha`.
  Read the tree **at that commit**, not at a newer head.
- The avatar is re-encoded to a 256×256 PNG with the existing avatar encoder before storing.
- Then the rest of C-1108:
  - the admin list (with `needs_libraries`: is a matching library added for at least one organization, or as an
    instance library?);
  - the version detail (`TemplateContentView`: every text in full, the README first; `FileView.text` for valid UTF-8 up to 256 KiB);
  - the avatar route;
  - approve (`content_sha256` echo, 409 `template_content_changed`), reject, withdraw and offer.
- **The offered version** is computed in one function (newest `approved` while `offered`). Every reader uses it: the
  catalogue here, creation and updates in P6-TPL-3. Nothing reads "the newest version" of a template in any other way.
- Attention items, both kinds of C-1108, in `GET /v1/admin/attention`. Build them like the existing
  `package_approval` items.
- Instance settings `agents.templates.enabled` and `agents.templates.update_interval`, registered like
  `plugins.addon_sources.update_interval`. Find where that key is registered and shown in the settings registry.

### 6. Organization switches (C-1109) and the workspace catalogue (C-1110)
- The two organization routes (org owner/admin) and `org_disabled_agent_templates`.
- `listWorkspaceTemplates` and `getWorkspaceTemplatePlan` (`can_use`), always for the **offered** version. The card
  carries the full `readme` (D-458), so the gallery's details view needs no second request:
  - availability from the active snapshot (app installed and active) and the organization's app availability
    (`07 §App availability`);
  - `level` capped for `custom_only`;
  - `custom` filtered against the current catalogue, with `dropped_tools`;
  - `accounts` = the app's accounts of this workspace and of its organization that the caller may give to an agent
    here. **Call the C-504 check function**; do not re-derive it. Use C-917's shape, with `limit`.
  - The workspace avatar route.
- **Skill references (C-1142 resolution)** in one function, `resolve_skill_refs(org, workspace, refs)`, which P6-TPL-3
  calls too:
  - library candidates come from the org's libraries and the instance libraries, through the existing library
    queries, matched on the stored `url`, `ref` and `path`;
  - the skill's validity at the head comes from the library browse cache;
  - `content` comes from the library's existing content read at that commit;
  - organization references come from the effective-skills query of that workspace;
  - plus the agent-tier library budget check (`over_budget`).
  Never call a library's network read more than once per library per plan request.
- The kill switch: `404 templates_disabled` on the organization and workspace routes when it is off.

### 7. OpenAPI, `05` and types
Every route goes in the OpenAPI description, with the operation ids of the contracts, and in `05`'s route tables
(the instance administration, organization and workspace sections), with their auth columns. Regenerate
`schema.d.ts`.

## Not in this brief
`POST …/agents/from-template`, provenance and updates (P6-TPL-3); helper files on agents (P6-TPL-6); every screen
(P6-TPL-4).

## Security review (before merge)
A separate reviewer (`sonnet-integration-reviewer`, or the Orchestrator's choice) reviews against SEC-146:
- every route's authority: IA-only admin routes, org owner/admin, `can_use`;
- the git reads and the package download: https; a public host after every redirect; token origin-bound and never
  after a cross-origin redirect; never logged or returned; size caps and the timeout;
- that the package is accepted only when its content equals the source at the recorded commit;
- that `TemplateContentView` is complete and untruncated, helpers and skill references included (the admin approves
  what agents get);
- that approve is bound to `content_sha256`;
- that the offered version cannot be changed by anything but approve, withdraw and offer;
- the critical audit rows;
- that no route here writes a grant, rule, secret, skill library or agent.

## Acceptance criteria
- [ ] API tests (`services/aurora-api/tests/it`). The fixtures are a local **git smart-HTTP fixture** (reuse the one
  the skill-library reader tests use) that serves a repository built from `templates/examples/`, plus a local HTTP
  server for packages. Cover:
  - preview without storing (and no audit row);
  - add: one transaction, the audit row, the token sealed;
  - preview errors (each code), and the 20-source limit;
  - a private fixture repository: the token is sent to its origin, not to a package host on another origin, and not
    after a cross-origin redirect;
  - check outcomes (`updated`, `up_to_date` on an unchanged head, `failed` kept as a result);
  - the poll job claiming due sources once under concurrency; `never`; backoff; a source with a waiting version due
    again within 15 min.
- [ ] Ingest:
  - the first check takes the head version;
  - a head bump creates `awaiting_package`;
  - the package appearing later moves it to `pending_approval`;
  - a package that never appears expires to `invalid` (a test clock or a settable wait);
  - `source_mismatch` (package with an extra or changed file);
  - `identity_mismatch`; `too_large`;
  - a broken package stored `invalid` once and never retried;
  - `head_state = invalid` keeps stored versions;
  - `version_not_newer`; `id_taken` by another source; `min_core` (`needs_newer_aurora`, re-checked);
  - `removed_upstream` set and cleared.
- [ ] Approval: approve (and 409 with a stale hash); reject (never ingested again); withdraw and offer.
- [ ] **The previous version stays offered**, and its plan unchanged, while a newer version is `awaiting_package`,
  `pending_approval`, `rejected` or `invalid`, while the head is invalid, and after a failed check. After approve the
  newer one is offered.
- [ ] Attention items appear and disappear.
- [ ] Organization switch and its effect on the workspace list.
- [ ] The catalogue's availability reasons: not installed; not available here; library not added; skill not in the
  library; library paused; organization skill missing; over budget. Also the `custom_only` cap; dropped tools;
  `accounts` limited to what the caller may give (a member of another workspace sees none of its accounts); the kill
  switch.
- [ ] Permission-matrix rows for every new route (`18`'s matrix test), and audit coverage of the critical actions.
- [ ] Addon-source and skill-library tests unchanged and green.
- [ ] `cargo fmt --check`; clippy for `aurora-api` and touched crates; targeted `cargo nextest run -p aurora-api`
  filters for the new tests, the addon tests and the library tests; `extract-schema.py --check`;
  `scripts/guards/run-all.sh`; the OpenAPI check. All through `heavy.sh`. CI is the full gate.

## Ownership and sequencing
- **Owns:**
  - the new migration;
  - `services/aurora-api` template-source, ingest, approval, organization and catalogue modules;
  - the `ReadProfile` parameters of the library reader (no behaviour change there);
  - the shared lease and poll helpers you extract from the addon code (no behaviour change there);
  - the secrets purpose;
  - the settings registry entries;
  - the `03` sections;
  - OpenAPI and `web/src/lib/api/schema.d.ts` (regenerated only).
- Starts after P6-TPL-1 is merged. P6-TPL-3 starts after this and P6-TPL-6 have merged.
- One Rust lane. Nothing else may edit the addon-source module or the skill-library reader while this runs. P6-TPL-6
  may run at the same time in another lane: it touches the supervisor, the home-sync builder and a new agents module,
  never these.

## Numbers
D-435, D-436, D-456, D-457, D-458, C-1106..C-1110, C-1113, C-1142 (resolution), SEC-146. Migration: next free when
placed.

## Suggested agent/model and reason
`rust-developer` (Sonnet). It is a multi-module API feature built from two existing ones (addon sources for the admin
shape and the job, skill libraries for the git reads). The risk is in reuse without regressions, in the
package-versus-source check, and in exact authority checks, which the security review covers.
