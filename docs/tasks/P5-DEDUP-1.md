# P5-DEDUP-1: find and remove duplicated Rust code

**Owner, 2026-11-18.** "There are probably many other things that are duplicated and could do with
shared primitives. Write briefs for searching for and fixing those duplications: one on the rust
side and another on the web side. These tasks should probably run in parallel, on their own fresh
lanes and with some parallelization, at least to identify duplicated code. The goal is to reduce
the overall lines of code." The owner's example that prompted this was FIX-173: nine hand-written
secret inputs, which now become one `SecretInput`. Decision: **D-376**. The web side is
**P5-DEDUP-2**. It runs at the same time in its own lane and shares no files with this one.

## Goal
Reduce the total lines of hand-written Rust by replacing duplicated code with shared helpers,
**without any change in behaviour**. That covers production code and tests in `crates/`,
`services/`, `connectors/` and `plugins/` (Rust only).

The measure is the net change in non-blank lines, reported per crate. A smaller codebase that
behaves differently is a failure, not a partial success.

## Context
- Size on 2026-11-18 (`wc -l`, with blank lines): about 470 k lines of Rust in total.
  - `services/` 349 k: `aurora-api` 227 k, `aurora-gateway` 52 k, `aurora-supervisor` 33 k,
    `aurora-runner` 16 k.
  - `crates/` 109 k.
  - `connectors/` 12 k.
  - Of these, about 176 k are under `tests/`.
- Shared homes that already exist. Use these before creating new ones:
  - `crates/aurora-core` (shared types and errors).
  - `crates/aurora-db` (persistence helpers).
  - `crates/aurora-protocol` (wire types).
  - `crates/aurora-testkit` (ephemeral Postgres, mock upstreams, fixtures).
  - Each crate's `tests/it/common/` (I-34: one integration-test binary per crate, shared helpers
    in `common/`).
- Generated or checked-in outputs are not targets:
  - `web/openapi.json`, `schemas/`, `docs/spec_schema.json`.
  - Anything under `target/`.
  - Migrations (`*.sql`). They are append-only; never edit a merged one.
- Layering rules are in `docs/01-architecture.md` (crate layout) and
  `docs/02-engineering-conventions.md`. `scripts/guards/dep-graph.sh` enforces the dependency
  graph. A shared helper must sit in a crate that every user may already depend on. Do not add a
  dependency edge just to share a helper.

## Scope

### Part A: discover duplication in parallel (read-only, no builds)
1. **Baseline.** On the lane's starting commit, record the non-blank line count per crate,
   split into `src` and `tests`:
   `git ls-files '<crate>/**/*.rs' | xargs grep -cv '^\s*$' | awk -F: '{s+=$2} END {print s}'`
   (or an equivalent the report shows verbatim). Record the commit sha.
2. **Clone scan.** Run a token-based clone detector over `crates services connectors`:
   - Use `jscpd` pinned to an exact version (e.g. `pnpm dlx jscpd@4.0.5 --format rust
     --min-tokens 70 --reporters json --output <scratch>`). Run it from a scratch directory.
   - Do **not** add it to any `package.json`, lockfile or CI.
   - Keep the JSON outside the repo. Summarise it in the report.
3. **Pattern sweeps, in parallel.** A token scanner misses code that has the same shape but
   different names. Run up to **five** read-only explorers (`haiku-explorer`, or `Explore` for
   the larger slices) at the same time. Each one gets one slice and the list of patterns below,
   and returns clusters, not file dumps.
   - **Slices:**
     1. `aurora-api/src/domains/` a–l.
     2. `aurora-api/src/domains/` m–z, plus the rest of `aurora-api/src`.
     3. `aurora-gateway` and `aurora-relay`.
     4. `aurora-supervisor`, `aurora-runner`, the other services, and `connectors/`.
     5. `crates/`, and every `tests/it/` (largely `aurora-api/tests/it`, 145 files).
   - **Patterns to look for:**
     - Handler boilerplate: extracting the principal, membership and role checks repeated inline,
       pagination parsing and cursor encoding, mapping errors to the API error body.
     - Repeated `sqlx` row-to-struct mapping, and copy-pasted queries that differ only in a
       filter.
     - Audit-row construction.
     - Validation of names, URLs, ids and lengths written again per domain.
     - Duplicated `From`/`Display` impls and enum↔string tables.
     - HTTP client setup and retry loops in the gateway, runner and supervisor.
     - Config parsing.
     - Tests: setup sequences (create org, workspace, agent, user, API key, login) and
       request-and-assert blocks repeated across files, when a `common/` or `aurora-testkit`
       helper would replace them.
     - Dead code: `#[allow(dead_code)]` items, and `pub` items with no reference outside their
       definition. Exclude the public API of `aurora-connector-sdk` and `aurora-plugin-sdk`,
       which outside code builds against.
   - **For each cluster, an explorer returns:**
     - the locations (`file:line`);
     - the number of copies and the lines per copy;
     - how the copies differ (for example a status code, an error message, or a permission);
     - the proposed shared home;
     - the estimated net saving;
     - a risk rating (low / medium / security-sensitive).
4. **Catalogue.** The lane lead merges the scan and the sweeps into one ranked catalogue and
   writes it into this brief's **Completion report** (Part A). Rank by net saving × safety.
   - Keep a cluster only if it has **3 or more copies**, or **2 copies with a net saving of at
     least 40 lines**, and the copies really are the same.
   - Copies that have drifted apart on purpose (different error codes or permissions) are
     listed as "not duplicates" with the reason. They are a finding, not a fix.
   - Commit the catalogue before starting Part B.

### Part B: fix, in batches
- **One batch is one cluster family** (for example "pagination parsing in aurora-api"),
  committed on its own. Its commit message gives the net line delta.
- **Order:** highest-ranked low-risk batches first. Security-sensitive clusters go last, each in
  its own batch:
  - authorization and membership checks;
  - `aurora-crypto` and `aurora-secrets`;
  - the gateway's `decide/` path;
  - anything a SEC-nn names.
  
  Each of these batches gets a `sonnet-integration-reviewer` pass (or an Opus pass) before its
  commit. The review confirms that every call site keeps exactly the same check.
- **Before each batch**, list the files changed by every other live lane
  (`git diff --name-only master...alexiri/<lane>` for each running lane, the Orchestrator's
  worker list). Skip clusters that touch those files. Record each skipped cluster in the report,
  so it can be done after that lane merges.
- **Within the lane**, implementation is sequential: one Rust build directory, and `cargo`
  serialises on its lock anyway. Part A is where the parallelism goes. If the Orchestrator gives
  the lane a second worktree and disk allows it (see Constraints), a second worker may take a
  batch in a **different crate** at the same time.
- **Tell the Orchestrator after every ~5 batches.** It then merges the lane into master, which
  keeps the conflict window short for the other lanes.

### Out of scope
- Any change in behaviour, including:
  - routes, request or response bodies, status codes, error `code`s and messages, log fields;
  - OpenAPI (the drift test stays byte-identical without `UPDATE_OPENAPI`);
  - SQL semantics, migrations, settings keys, Helm values, metrics names.
- Performance work, new features, and renames for taste.
- Splitting crates (that was VEL-007).
- The web side (P5-DEDUP-2).

## Rules for the shared code
- **Prefer plain functions, generics and traits** over `macro_rules!`. Use a macro only when a
  function cannot do the job (for example generating several items), and give it a doc comment
  and a test.
- **Do not widen visibility** beyond what the callers need (`pub(crate)` inside a crate).
- **Each new shared item gets** a doc comment that says what it replaces, and a unit test if it
  has logic.
- **Tests:**
  - Never delete a test case or weaken an assertion to save lines. Only setup and repeated
    request plumbing move into helpers.
  - The number of tests reported by `cargo test` (or nextest) per crate must not drop. The one
    exception is dead tests of removed dead code: list each one.
- **Dead code** is removed only when the compiler, with `-D warnings` and every target, agrees it
  is unused. Nothing an SDK crate exports is removed.
- **Edit with the Edit/Write tools** (`git mv` is fine). A script may do a genuinely bulk
  mechanical rewrite. Its diff is reviewed.

## Constraints / file ownership
- **Owns:** `crates/**/*.rs`, `services/**/*.rs`, `connectors/**/*.rs`, and the `Cargo.toml` of
  a crate whose dependencies a batch must change (only toward existing edges), batch by batch.
  It does not own `web/**`, migrations, `docs/03`, `docs/05`/`API_CONTRACTS.md`, or Helm.
- **Other lanes:** this lane yields to them. Files in another lane's live diff are off limits
  until that lane merges.
- **Disk:** 51 GB was free on 2026-11-18 (94 % used), and a Rust lane needs about 15 GB.
  Before starting, check `df` and `scripts/dev/disk-report.sh`, and clean merged lanes with
  `scripts/dev/clean-lane.sh`. Use one worktree with its own `target/`. A second one only if at
  least 30 GB is still free after the first build.

## Acceptance criteria
1. **Part A catalogue** is committed in the Completion report. It contains:
   - the baseline sha and per-crate counts;
   - the `jscpd` summary;
   - the ranked clusters, each with locations, copies, differences, home, saving and risk;
   - the "not duplicates" list;
   - the clusters skipped because of other lanes.
2. **Every batch:**
   - `cargo fmt --check`;
   - `cargo clippy --all-targets -D warnings` on the touched crates and their dependents;
   - the targeted tests of the touched crates;
   - `bash scripts/guards/run-all.sh` and `python3 scripts/extract-schema.py --check`.
3. **Every ~5 batches and at the end of the chain**, the full gate:
   - every crate's tests;
   - `openapi_matches_checked_in` green without `UPDATE_OPENAPI`;
   - `scripts/e2e/gateway.sh` and `scripts/e2e/upgrade.sh` when the gateway or api changed.
   
   The test count per crate is no lower than at the baseline, apart from listed dead tests.
4. **Final report:**
   - the per-crate line counts before and after, and the net delta (src and tests separately);
   - the delta per batch;
   - the shared items added and where they live;
   - every security-sensitive batch, with its reviewer verdict;
   - what was left and why.
5. **No item in Out of scope changed.** The OpenAPI and schema outputs are byte-identical, and
   `git diff --stat` shows no migration, `docs/05` or Helm change.

## Dependencies
- None to start Part A. Part B rebases on master before each batch.
- Runs in parallel with P5-DEDUP-2 (no shared files).

## Suggested agent/model and reason
- **Part A:** up to five Haiku/Explore explorers in parallel for the sweeps. A Sonnet lane lead
  merges them into the catalogue. Ranking needs judgement, but the input is structured.
- **Part B:** `rust-developer` (Sonnet) per batch, plus `sonnet-integration-reviewer` for the
  security-sensitive batches. Escalate to Opus for any cluster where the copies differ in a way
  that might be a bug: that is a finding to report to the owner, not something to unify silently.

## Completion report
(Part A catalogue first, then the batches.)
