# Phase 4 exit verification, static half (AUDIT-004 steps 5, 6, 9, 11)

Part of AUDIT-004 (`docs/tasks/AUDIT-004.md`, Refresh 2026-11-14). The `audit-run` lane runs steps 1–4, 7, 8, 10
(`docs/audit/phase4-exit.md`); this file holds the contract, invariant, suite and spec checks. It verified; it did
not develop. Everything was read from, or run on, one commit.

**Audited commit:** `1f77f68f38d39c1322ef2b2ecccbdce6c85a9ff3` (local `master`, branch `alexiri/audit-static`,
2026-11-14). Host: shared box, other lanes active (load average 25–35 during my builds). Scratch database
`audit4s_mig` on `aurora-dev-pg`, created and dropped by me; the `aurora` database was only used as the connection
database of the sqlx test harness (which makes and drops its own scratch databases).

## Verdict for this half

**Steps 5 and 11: green with findings. Steps 6 and 9: statically complete, execution cross-check pending** — the run
lane's `passed-tests.txt` had not arrived when I committed (see [What is left](#what-is-left)).

- Step 5 is fully green: OpenAPI drift test passes, `spec_schema.json` is up to date, the permission matrix passes with
  `INCONCLUSIVE rows: 0`, all 67 migrations are unique, ordered and apply cleanly on an empty database (second run a
  no-op), and every contract C-400..C-720 (minus C-701..C-709) has an implementation I could find.
- Step 6/9 static: no phase-4 SEC row cites a missing test except one stale test name (fixed, F-S1) and one wrong name
  (fixed, F-S2). Six rows are phase 5 by D-138 and say so (SEC-63/64/65-remainder/74/75/76). The weak spot is
  **traceability of the REVIEW-5 reopenings**: most older SEC rows that a REVIEW-5 fix reopened do not cite that fix's
  tests (F-S3).
- Step 11 found stale phase labels and tracking rows (F-S4..F-S9) but no phase-4 rule that is silently missing.

No source code was changed. Doc-only trivial fixes are listed in [Fixes made](#fixes-made).

## Results

| # | Check | Command / method | Exit | Result |
|---|-------|------------------|------|--------|
| 5 | OpenAPI drift | `cargo test -p aurora-api --test it -- openapi_matches_checked_in permission_matrix --nocapture`, `UPDATE_OPENAPI` unset, `DATABASE_URL` set, fixtures built first | 0 | `foundation::openapi_matches_checked_in` ok (no drift) |
| 5 | Fixtures | `bash scripts/fixtures/build.sh` | 0 | `passed=57 failed=0 xfail=0` |
| 5 | Permission matrix | same cargo command (6 tests matched `permission_matrix`) | 0 | `test result: ok. 6 passed`, 290.75 s; `coverage_every_openapi_route_is_listed` ok; 303 `checked` lines; **`INCONCLUSIVE rows: 0 {}`** |
| 5 | Schema | `python3 scripts/extract-schema.py --check` | 0 | `spec_schema.json is up to date` |
| 5 | Planned changes | read `docs/03 §Planned changes` (from line 1484) | n/a | only phase-5 rows (`attachments` S3, `secrets` LLM, `approvals` park, `approval_push_claims`, `webpush_vapid_keys`, `push_subscriptions`); the phase-4 tables (`preview_*`, `relay_*`, `terminal_stream_grants`, groups, SCIM…) are real `###` sections |
| 5 | Migrations | 67 files in `crates/aurora-db/migrations`, `ls \| sort -c`, duplicate-version check; `target/debug/aurora-api migrate` (the sqlx migrator) on empty `audit4s_mig` twice | 0 / 0 | 67 unique ascending versions `20261105000000`..`20261201000110`; first run 67 rows, `bool_and(success)` true, 105 public tables (incl. `_sqlx_migrations`); second run no-op, still 67 rows; database dropped |
| 5 | C-numbers | `docs/API_CONTRACTS.md` blocks C-400..C-720 → routes vs `web/openapi.json` and handler source | n/a | table below: every route found; no missing implementation |
| 6 | SEC rows | `docs/tests/security-coverage.md` + `crosscheck.py` | n/a | table below |
| 9 | docs/18 suites | test files per suite, SCIM count | n/a | table below |
| 11 | Spec sweep, operator docs, FEATURES | grep + sampling | n/a | section below |

`sqlx-cli` is not installed here, so the migration check used the repo's own migrator (`aurora-api migrate`, which calls
`aurora_db::migrate`), the same path the Helm `migrate` job uses. "Strictly sequential" is read as: one version per
file, versions strictly ascending, no duplicates, no unparsable names. There are gaps in the numbering by design
(timestamps, not a counter).

## Step 5: C-number table

Method: `docs/API_CONTRACTS.md` blocks run C-400..C-720. I split the file into per-C blocks, extracted every
`METHOD /path` named in a block (332 route mentions in the in-scope blocks) and checked it against `web/openapi.json`
(method + path) and, for routes deliberately outside the dashboard document, against the route literals in
`services/` and `crates/`. For the 74 blocks that name no route (configuration, data, behaviour, audit, error codes)
I extracted every backticked snake_case/dotted identifier and checked it exists somewhere in code, SQL, chart, web
sources or schemas. This proves the contract's *surface* exists; the behaviour is proven by the tests in steps 6/9.

| Task (block) | C-numbers | Routes in block | In OpenAPI | Elsewhere (expected, host/internal plane) | Missing |
|--------------|-----------|----------------:|-----------:|-------------------------------------------|---------|
| P4-CNT | C-400..C-409 | 1 | 1 | — (config, trait, metric blocks: identifiers found; `redis.allow_plaintext`/`redis.ca_file` are nested serde keys, present) | none |
| P4-GRANTS | C-410..C-419 | 11 | 11 | — | none |
| P4-HAR | C-420..C-429 | 4 | 4 | — (`config.hosted.enabled_harnesses`, `config.runner.image_harnesses` are dotted doc paths; keys exist) | none |
| P4-MCON | C-450..C-459 | 8 | 5 | `POST /v1/connector-host/register`, `GET …/assignments`, `POST …/status` (host plane, source present) | none |
| P4-GRP-1 | C-460..C-469 | 15 | 14 | `PUT /dev/claims/{}` (dev IdP) | none |
| P4-GRP-2 (SCIM) | C-470..C-479 | 9 | 5 | `/scim/v2/*` Users, Groups, ServiceProviderConfig (SCIM wire, outside `/v1`) | none |
| FIX-019..021 | C-500..C-506 | 7 | 7 | — | none |
| P4-IDP-1 | C-510..C-519 | 5 | 4 | `PUT /dev/claims/{}` | none |
| P4-ONB-1 (placement) | C-520..C-529 | 4 | 4 | — | none |
| P4-ONB-2 | C-530..C-536 | 7 | 7 | — | none (C-539 is mentioned in a Rule line only, reserved) |
| P4-GSL | C-570..C-579 | 9 | 8 | `POST /internal/v1/library-fetch` (internal listener, SEC-84) | none |
| FIX-025 | C-580..C-589 | 2 | 2 | — | none |
| FIX-029 | C-610..C-615 | 5 | 5 | — | none |
| P4-TERM | C-616..C-626 | 3 | 1 | `GET /v1/agents/{agent}/terminal` is a WebSocket upgrade handler (`domains/terminal/routes.rs:18`), `POST /preview-streams/open` is the runner plane | none |
| FIX-050 / FIX-052 | C-630..C-633 | 1 | 1 | — | none |
| FIX-072 (relay limits) | C-636, C-637 | 3 | 0 | relay plane `/v1/relay/{report,resolve}`, `GET /_aurora/grant` (outside the dashboard OpenAPI, as the block states) | none |
| FIX-125 / FIX-132 | C-638, C-639 | 4 | 4 | — | none |
| FIX-099 | C-700 | 2 | 2 | — | none |
| FIX-120 / FIX-121 | C-690..C-695 | 5 | 5 | — | none |
| FIX-127 | C-710..C-715 | 7 | 7 | — | none |
| FIX-140 | C-720 | 2 | 2 | — | none |

C-701..C-709 skipped (phase 5). Numbers that are referenced but have no block of their own (C-539, C-619, C-629,
C-631, C-632, C-635: reserved tails of a block's range) need no implementation. Previews and the relay have no C-number
block at all: their wire is specified in `docs/16 §W18` and D-144..D-147 (observation, not a gap).

Items that did not match literally, triaged by reading the code:
- **C-711** (`POST /v1/orgs/{org}/channel-integrations:probe`) names error codes `connector_not_found`, `org_not_found`
  and `validation_error`; none exists as an identifier. The handler (`channels/integrations.rs:225`) answers an unknown or
  inactive connector with the standard 422 invalid-field error on `/connector_id`, and an unknown org through
  `c.org(...)`. Behaviour is reasonable; the **contract text and the code disagree on the codes** (F-S10, low).
- **C-616** is a WebSocket upgrade, so it is intentionally absent from OpenAPI; `docs/05` should say so (no finding).

## Step 6: SEC invariants

Source of truth for tests: `docs/tests/security-coverage.md` (rows SEC-62..SEC-87 plus the older rows REVIEW-5 reopened);
status per `docs/17-security.md`. "Cited" is the number of backticked identifiers (≥ 9 chars) in the row's test cell;
"fns found" is how many of them are a test function (`fn` in a `.rs` file or a `test()`/`it()` title) somewhere in the
repository. The rest are file basenames, error codes or words (all 150-odd unresolved identifiers were read; the only
real misses are F-S1, F-S2, both fixed). The check is by name across the repo, not by cited file, so a test moved to
another file would still count. **Ran-and-passed: pending `passed-tests.txt`** (column filled by
my scratch script `crosscheck.py <passed-tests.txt>`, kept outside the repo per the lane's Owns).

| Row | Task | docs/17 status | coverage status | Cited | fns found | Ran and passed | Remark |
|-----|------|----------------|-----------------|------:|----------:|----------------|--------|
| SEC-62 | P4-CNT, FIX-087 | phase 4 | proven | 22 | 21 | pending | Redis cases gated `AURORA_TEST_REDIS=1`: need the `gated-suites` evidence. No FIX-087 test cited (F-S3) |
| SEC-63 | P4-S3 | phase 5 (D-138) | pending (P4-S3) | 5 | 0 | n/a | phase 5; coverage row still says "(phase 4" (F-S5) |
| SEC-64 | P4-LLM | phase 5 (D-138) | pending (P4-LLM) | 2 | 0 | n/a | phase 5, same label drift |
| SEC-65 / 65a | P4-LLM / FIX-105 | 65a phase 4 (litellm clause), rest phase 5 | passing | 3 | 2 | pending | 65a counted; remainder phase 5. Status word "passing" vs "proven" elsewhere (cosmetic) |
| SEC-66 | P4-HAR, FIX-095 | phase 4 | proven | 23 | 18 | pending | FIX-095's tests live outside `*/tests/` (supervisor adapter fixtures) |
| SEC-67 | P4-GRANTS, FIX-084/085 | phase 4 | proven | 22 | 16 | pending | two-gateway activation test is an e2e (`gateway.sh`, run lane step 8) |
| SEC-68 | P4-GRANTS, FIX-085 | phase 4 | proven | 30 | 23 | pending | the corpus yaml cases (`p4g_*`) are not `fn`s; run by `aurora-policy` `corpus.rs` |
| SEC-69 | P4-MCON | phase 4 | proven (API half) | 7 | 6 | pending | connector-host half rests on kind/conformance (run lane) |
| SEC-70 | P4-MCON | phase 4 | proven | 8 | 6 | pending | |
| SEC-71 | P4-GRP-1, FIX-083 | phase 4 | proven | 31 | 27 | pending | cited test `fix063_a_suspended_members_idp_rows_survive…` no longer exists, replaced by FIX-083's test (**F-S1, fixed**) |
| SEC-72 | P4-GRP-2 | phase 4 | proven | 18 | 15 | pending | `scim_auth`, `scim_scope`, `scim_idp_replay` are files; 89 SCIM tests in total (step 9) |
| SEC-73 | P4-GRP-2, FIX-080/082 | phase 4 | proven | 20 | 18 | pending | |
| SEC-74 | RFE-011b | phase 5 (D-138) | pending | 1 | 0 | n/a | no tests by design; coverage row says "(phase 4" (F-S5) |
| SEC-75 | RFE-011b | phase 5 (D-138) | pending | 0 | 0 | n/a | cites no test (phase 5) |
| SEC-76 | RFE-011b | phase 5 (D-138) | pending | 0 | 0 | n/a | cites no test (phase 5) |
| SEC-77 | FIX-020 | phase 4 | proven (shared path) | 12 | 8 | pending | |
| SEC-78 | P4-IDP-1 | phase 4 | proven | 14 | 13 | pending | |
| SEC-79 | P4-ONB-1 | phase 4 | proven | 8 | 6 | pending | |
| SEC-80 | P4-PRV-1, FIX-097 | phase 4 | proven | 8 | 7 | pending | no FIX-097 test cited (F-S3) |
| SEC-81 | P4-PRV-2, FIX-088/097 | phase 4 | proven | 19 | 16 | pending | |
| SEC-82 | P4-PRV-2, FIX-088/090/091 | phase 4 | proven | 27 | 24 | pending | cites 17 of the fixes' 65 new tests |
| SEC-83 | P4-GSL, FIX-096 | phase 4 | proven | 22 | 18 | pending | no FIX-096 test cited (F-S3) |
| SEC-84 | P4-GSL, FIX-096 | phase 4 | proven | 31 | 24 | pending | |
| SEC-85 | P4-PRV-2, FIX-088/090 | phase 4 | proven | 14 | 12 | pending | no FIX-088/090 test cited (F-S3) |
| SEC-86 | — | — | **no row** | — | — | — | the number is unused in `docs/17` and the coverage file (not a gap: nothing defines it) |
| SEC-87 | P4-TERM, FIX-091 | phase 4 | proven | 43 | 38 | pending | `pty_env_probe`, `pty_leak`, `stream_listener`, `leg_limits` are files |

Older rows a REVIEW-5 brief reopened (`docs/FEATURES.md` rows `P4/R5§*`, "reopens SEC-xx"). Each must be proven again
by the fix's own tests, not only by the row's original citations:

| Row | Reopened by | Row cites | Fix's new tests (`fn`s added under `*/tests/*`) | Fix tests cited in the row | Ran and passed |
|-----|-------------|----------:|-----------------------------------------------:|---------------------------:|----------------|
| SEC-01 | FIX-104 | 5 | 9 | 0 | pending |
| SEC-02 | FIX-094 | 5 | 13 | 1 | pending |
| SEC-04 | FIX-104 | 4 | 9 | 0 | pending |
| SEC-15 | FIX-107 | 12 | 18 | 2 | pending |
| SEC-17 | FIX-104, FIX-107 | 4 | 27 | 0 | pending |
| SEC-19 | FIX-112 | 14 | 11 | 0 | pending |
| SEC-24 | FIX-110 | 4 | 9 | 0 | pending |
| SEC-25 | FIX-094 | 8 | 13 | 3 | pending |
| SEC-45 | FIX-109 | 21 | 2 (rest via corpus/unit) | 0 | pending |
| SEC-47 | FIX-097 | 22 | 16 | 1 | pending |
| SEC-49 | FIX-118 | 29 | 10 | 1 | pending |
| SEC-57 | FIX-107 | 10 | 18 | 2 | pending; a cited name was wrong (**F-S2, fixed**) |
| SEC-60 | FIX-118 | 23 | 10 | 1 | pending |

The same cross-check for SEC-62, 66, 67, 68, 71, 73, 80..85 and 87 is in `crosscheck.py` (section "REVIEW-5 fixes",
counts per fix). The per-fix lists are derived by the script from `git log --grep '^FIX-<n>'`, so they are
reproducible; the run lane's list decides whether they ran.

## Step 9: docs/18 phase-4 suites

`docs/18` describes most phase-4 suites in prose and cites test *files*; I mapped each suite to its files and counted
test attributes (`#[test]`, `#[tokio::test]`, `#[sqlx::test]`) per file. Ran-and-passed is checked per file/module
against `passed-tests.txt` (pending).

| Suite (docs/18) | Test modules (count of test attributes) |
|-----------------|-------------------------------------------|
| Managed connectors (P4-MCON) | `api it/connectors_managed.rs`, `connectors_host_tokens.rs`, `connector_ops.rs`, `managed_support.rs` (helper); connector-host crate tests; `channels.sh`, `kind.sh` scenarios (run lane) |
| Shared counters (P4-CNT) | `aurora-counters tests/it/conformance.rs` (12), `guard.rs`; `api it/counters.rs`; `gw it/pipeline.rs`; `scim_counters.rs` (3); Redis cases gated by `AURORA_TEST_REDIS=1` |
| Harness adapters (P4-HAR) | `api it/harness_selection.rs`; supervisor adapter tests; `model_readiness.rs`; `fix_029.rs` (start retry/log) |
| Custom grants / parameter matching | `api it/grants_custom_only.rs`, `grants_bulk.rs`, `grants_connections.rs`; `aurora-policy` `corpus.rs` + `tests/corpus/*.yaml`; `params_matching.rs` (6) |
| Groups (P4-GRP-1) | `api it/groups.rs` (26), `groups_idp.rs`, `authz_groups.rs` (15, in the gateway `it`) |
| **SCIM (P4-GRP-2)** | **89 test functions in `services/aurora-api/tests/it/scim_*.rs`** (all `#[tokio::test]`): `scim_protocol` 13, `scim_auth` 10, `scim_scope` 9, `scim_link` 19, `scim_deprovision` 20, `scim_groups` 10, `scim_idp_replay` 5, `scim_counters` 3 (`scim_common.rs` is a helper, 0 tests). FIX-074 counted 65; the 24 added since are REVIEW-5 fixes (FIX-080..083, 087). Fixtures `tests/fixtures/scim/{entra,okta,keycloak}/*.json` exist. No external conformance client (deviation recorded by the run lane). |
| Port previews (non-relay) | `api it/previews.rs` (26), `preview_auth.rs` (26); relay `tests/limits.rs` (11), `behaviour.rs` (16), `big_body.rs` (1); `preview_streams_flow.rs` (supervisor); hosted e2e `s*` previews, `kind.sh` k6 (run lane) |
| Web terminal (P4-TERM) | `api it/terminal.rs` (24); runner `stream_listener.rs` (2); supervisor `pty_env_probe.rs`, `pty_leak.rs`, `streams.rs`, `leg_limits.rs` (9); web Vitest/Playwright terminal specs; `kind.sh` |
| Git skill library (P4-GSL) | `api it/skill_library.rs` (34), `skills_library.rs`; registry `library.rs` (14), `library_fetch.rs` (12) |
| Parking / push | **phase 5 (D-138)** — `docs/18` section is tagged so; nothing to run |
| Terminator | **phase 5 (D-138)** — nothing to run |

Checks done: every test file cited in these sections of `docs/18` exists (the one apparent miss, `preview.sh`, is the
audit-row pattern `preview.share.*`, not a file). Suite-to-test mapping is by file because the document does not name
functions.

## Step 11: spec sweep and docs

**Sweep.** `grep -rn "(phase 4" docs/*.md` excluding `80-*`, `PROGRESS.md`, `FEATURES.md` → **411 lines in 24 files**
(`15` 54, `05` 49, `02` 48, `03` 46, `04` 34, `17` 29, `14` 22, `90` 21, `16` 18 …). Sampling:
1. All 411 lines were parsed for the task tag in their `(phase 4, …)` group. 380 carry a task or decision tag (P4-*,
   FIX-*, D-*, SEC-*); 31 are untagged and I read each.
2. Every line that mentions a feature moved to phase 5 by D-138 (SSH terminator, park-and-resume, Web Push, S3, extra
   LLM providers, evals) was listed (16 lines) and checked for a phase-5 marker.
3. Per task family I spot-checked one rule per document against code or tests (counters in `07`/`06`, groups and SCIM
   in `04`, placement, IdP roles, harnesses in `12`, previews and terminal in `11`/`16`, start diagnostics in `10`/`11`,
   library in `10`/`21`). All found.
Result: **no phase-4 rule is silently missing**; the stale labels below are the discrepancies.

| Finding | Where | State |
|---------|-------|-------|
| SSH terminator still labelled "(phase 4)" although D-138 moved it to phase 5 | `00:127` (`aut_` token), `01:17`, `01:47`, `11:333` | **fixed** (F-S4) |
| Phase-5 alerts labelled "(phase 4)" and referring to metrics that do not exist (`aurora_blob_store_up`, `aurora_approvals_parked_pending`, `aurora_push_sent_total`: 0 hits in code; `aurora_counters_*` exist) | `docs/operator/outage-and-alerts.md` rows "Blob store down", "Parked approvals", "Push" | **fixed** (F-S4) |
| `docs/05:782` AdminOverview line mentions `blobs: { backend, healthy }` after "(phase 4)" (it is marked phase 5 inline; the field is not in OpenAPI) | `05:782` | ok as written |
| Coverage rows SEC-63/64/65/74/75/76 still say "(phase 4, …)" and `pending (P4-…)` | `docs/tests/security-coverage.md` | open, F-S5 |

**Operator docs per phase-4 feature** (`docs/operator/`): SCIM → `scim-provisioning.md`; previews and relay →
`previews.md`; terminal → `terminal.md`; harnesses → `harnesses.md`; skill libraries → `skill-libraries.md`; managed
connectors / connector host → `plugins-and-connectors.md`, `install-helm.md#connector-host`; roles and groups from the
IdP, placement → `sign-in.md`; model readiness → `llm-endpoints.md`; counters/Redis → `limits-and-approvals.md`,
`outage-and-alerts.md`; runner start diagnostics, token rotation grace → `runners.md`; paired apps → `apps-and-catalogue.md`.
Missing or thin (F-S6, low): **connector-host token management** (C-690..C-692, the admin routes) has no operator text
(only the bootstrap token in `install-helm.md`); the **instance audit log** (C-638, FIX-125) has no operator page;
**onboarding / setup wizard** (C-530..C-536) is documented only in `docs/14`; **`grants.custom_only`** is documented for
package authors in `docs/09`, not under `docs/operator/`.

**FEATURES.md phase-4 rows.** 100 rows with phase `P4` are `done`; 5 are `P5 (moved, D-138)` (`ssh-ca`,
`ssh-terminator`, `evals`, `s3`, `llm-providers`); `P4+/21§narration`, `§user-tokens` are phase 5 (D-225). Exceptions:
- `P4/R5§approval-recheck` (FIX-084) and `P4/R5§params-doubt` (FIX-085) are still `planned`, though both merged
  (`169a57d7`) and FIX-104/109 build on them. Their SEC-67/68 reopenings are therefore "unproven" in the tracker (F-S7).
- `P4/12§terminal follow-up` (`sup/12§pty-ctty`) is still `reopened (owner, 2026-11-13)`; FIX-091 item 7 (O_NOCTTY, merged
  `bf5b69be`) fixed it: `rustix::pty::OpenptFlags::NOCTTY` and the `O_NOCTTY` open exist in `supervisor/src/pty.rs:579,594`
  and `pty_env_probe.rs` covers it (F-S7).
- `RFE-011b§park` and `§webpush` are `todo` under phase "P5 (moved, D-138)": consistent, but their descriptions still say
  "(RFE-011b, phase 4)".
- `P4/FB§instance-governance` is "not built (owner decision)" (D-195): an owner-deferred row, as the audit allows.
Dependabot (`.github/dependabot.yml`) is outside my four steps and is OPS-021, phase 5 (D-138).

## Findings (each a candidate fix task)

| ID | Sev | Finding | Proposed task |
|----|-----|---------|---------------|
| F-S3 | medium | **REVIEW-5 reopenings are not traceable in `docs/tests/security-coverage.md`**: SEC-01, 04, 17 (FIX-104), 19 (FIX-112), 24 (FIX-110), 45 (FIX-109), 62 (FIX-087), 68 (FIX-085), 80 (FIX-097), 83 (FIX-096), 85 (FIX-088/090) cite none of the fix's tests; SEC-02/15/25/47/49/57/60/71/73/81/84 cite 1–3. The tests exist; the rows would let a future reader believe the invariant rests on pre-review tests only. | Doc task: add each fix's invariant tests to its rows, after the passed-list check shows they ran |
| F-S7 | medium | Tracker is stale: FEATURES rows `P4/R5§approval-recheck`, `§params-doubt` `planned` (merged), `sup/12§pty-ctty` `reopened` (fixed), RFE-011b rows say "phase 4" | Coordinator edit of FEATURES/PROGRESS (not in my ownership) |
| F-S5 | low | `security-coverage.md` rows SEC-63/64/65-remainder/74/75/76 still labelled "(phase 4 …)" / `pending (P4-…)`; their docs/17 rows say phase 5 | Doc relabel |
| F-S6 | low | Operator docs missing for connector-host tokens (C-690..692), instance audit log (C-638), onboarding/setup wizard, `custom_only` (operator view) | Doc task |
| F-S8 | low | `docs/05`/`API_CONTRACTS` do not say that `GET /v1/agents/{agent}/terminal` (C-616) and `/scim/v2/*` are outside OpenAPI by design (the drift test can't catch changes) | Doc note |
| F-S9 | low | Eight C-number blocks reserve numbers they never use (C-539, C-619, C-629, C-631, C-632, C-635…): harmless, but "every C-number has an implementation" cannot be mechanical until the reserved-unused ones are marked | Doc convention |
| F-S10 | low | C-711 names error codes (`connector_not_found`, `org_not_found`, `validation_error`) that the implementation does not emit (standard invalid-field 422 / `not_found`) | Align contract text or code |
| F-S11 | info | Redis-gated cases of SEC-62 and the Vault/Mattermost/podman-gated suites only count when their gates were on; the cross-check can't see that from a name list | Run lane to confirm gates (CI `gated-suites` evidence) |

F-S1 and F-S2 and F-S4 were fixed (below).

## Fixes made

All doc-only, one commit.

- **F-S1** `docs/tests/security-coverage.md` SEC-71: replaced the missing test
  `fix063_a_suspended_members_idp_rows_survive_a_sign_in_and_reactivation_restores_them` with the test that FIX-083
  put in its place, `fix083_suspension_ends_idp_group_access_and_reactivation_does_not_restore_it`
  (`services/aurora-api/tests/it/groups_idp.rs:478`). The row's prose was not re-read against the new semantics
  (suspension now ends IdP group access): **owner/coordinator to confirm the wording**.
- **F-S2** `docs/tests/security-coverage.md` SEC-57: `added_required_field_flips_and_minor_renames_never_do` →
  `added_required_field_flips_and_so_does_a_minor_rename` (`plugins_upgrade.rs:259`).
- **F-S4** phase labels: `docs/00-overview.md:127`, `docs/01-architecture.md:17,47`, `docs/11-runner-sandboxes.md:333`
  ("(phase 4)" → phase 5, D-138 for SSH terminator items); `docs/operator/outage-and-alerts.md` three alert rows
  relabelled phase 5 with "the metric does not exist yet".

## What is left

1. **`passed-tests.txt`** from `audit-run`: run
   `python3 -I <scratchpad>/crosscheck.py <path to passed-tests.txt>` from this worktree's root
   (`/tmp/claude-1000/-home-alex-orca-workspaces-aurora-audit-static/5329c86e-c40e-400d-8630-3a5609c9d8ef/scratchpad/crosscheck.py`). It prints,
   per SEC row and per reopening fix, `ran-and-passed=n/m` and the names not in the list. Names that are in
   the repository but not in the list are "not executed" (candidates: Redis/Vault/podman-gated cases, `#[ignore]`d
   tests) — each is then a finding or an owner-accepted gap. Suites in step 9 are checked the same way, by module.
2. The step-9 e2e members of the suites (hosted, kind k1..k6, gateway.sh s19, channels, conformance) are evidenced
   by the run lane's steps 7/8, not here.
3. Nothing was run on the Vault/Redis/podman gates; I did not run any e2e script, kind, or `scripts/perf/*`.

## Leftovers

Created and removed: database `audit4s_mig`. Left: this worktree's `target/` (a cold build, ~20 GB) and
`plugins/fixtures/dist` (gitignored); no kind clusters, containers or images. Scratch scripts are in my session
scratchpad only (not committed). Method for each is described in the step sections. I could not copy `.env` from
the owner's checkout (the harness denied it), and no step needed it.
