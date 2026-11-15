# Build performance and disk hygiene (OPS-016)

Scope: faster `cargo build` / `cargo test` for the developer loop and the parallel worker lanes, and no
build artefacts left behind when a lane ends. Decisions are I-34…I-39 in `90-decisions.md`.

## Results (before → after)

Host: 16 cores, shared with other lanes (load average 7–35 while measuring), `rustc 1.99.1`, shared dev
Postgres. "Before" is `phase1-2` at `48a3fb55`; "after" is the OPS-016 branch. Test set =
`cargo test --workspace --exclude aurora-e2e-gateway --exclude aurora-e2e-hosted`. All runs started from a
deleted `target/`.

| Measure | Before | After | Notes |
|---|---|---|---|
| Clean build, all test binaries (`cargo test … --no-run`) | 625 s | **491 s** | consolidation + lld |
| `target/` after that build | 12 GB | **4.5 GB** | 96 integration binaries → 22 |
| No-op rebuild | 1 s | 0 s | |
| One-line change in `aurora-api`, rebuild tests (`cargo test -p aurora-api --no-run`) | 317 s | **132–250 s** (150 s consolidation only) | the `aurora-api` library itself (~105 s, built twice: lib and lib-test) dominates; the test crate is 25 s |
| One-line change in `aurora-api`, developer build (`cargo build -p aurora-api`) | 127–243 s (`dev`) | **7–14 s** (`dev-fast`, incremental) | |
| Full `cargo test --workspace` (no e2e), incl. one `aurora-api` rebuild | 27 m 11 s | **14 m 33 s** | before and after each include the rebuild of the touched crate |
| `target/` after the full test run | 20 GB | **5.9 GB** | |
| Tests | 893 passed, 0 failed, 3 ignored (896) | 893 passed, 0 failed, 3 ignored (896) | same names, one `module::` level deeper |
| Test result lines (binaries + doctests) | 149 | 75 | |

Honest caveats:

- Timings on a shared host vary by ±50 % between runs (the three "after" one-line rebuilds were 132, 150 and
  250 s). The disk numbers and binary counts are exact; the times are indicative.
- **The brief's premise that linking dominates did not hold at the current profile** (`debug =
  "line-tables-only"`, dependencies without debuginfo): linking the largest binary (`aurora-api` `it`) takes
  0.77 s with lld and 0.77–0.91 s with the system linker (`AURORA_LINK_LOG`, see below). The wins come from
  building and linking the dependency-heavy test crate once per crate instead of 96 times (compile ≈ 6–25 s of
  codegen each, plus hundreds of MB of output each).
- The full test run is bounded by `aurora-api`'s single `it` binary (352 tests, 566–652 s) because its tests
  now share one libtest thread pool of 16. That is the same work as before, in one process.

## What changed

### 1. Fewer test binaries (I-34)

`tests/it/main.rs` with one `mod` per file in `aurora-api`, `aurora-gateway`, `aurora-runner`,
`aurora-supervisor`, `aurora-db`, `aurora-llm`, `aurora-protocol`, `aurora-registry`, `aurora-channels`.
Stay separate: `aurora-api/tests/sec10_logs.rs` (a scoped tracing subscriber must not race other tests; the
test failed when merged, which is why it says "own test binary"), `aurora-supervisor/tests/conformance.rs`
(live-gated, called by `scripts/e2e/llm.sh`), the e2e crates, crates that already had one test file.
Test count equal (896 listed before and after; the multiset of leaf test names is identical).
Test ids are now `file::name`; `docs/tests/*-coverage.md` and the CI schema step were updated.

### 2. Linker (I-37)

`.cargo/config.toml` points `x86_64-unknown-linux-gnu` at `scripts/dev/linker.sh`, which runs
`clang -fuse-ld=lld` when both are installed and falls back to plain `cc` otherwise, so CI runners and any
machine without lld still build. The two Containerfiles `COPY . .` (their `rust:1-bookworm` base has no
lld), so `.cargo` is in `.dockerignore` / `.containerignore`.
Choose with `AURORA_LD=lld|mold|system`; `AURORA_LINK_LOG=/tmp/links.txt` appends the seconds each link took.
Measured effect today: none that can be told from noise (see caveats); it is kept because it is free, passed the
whole suite, and matters when debuginfo is raised. **Optional for the owner**: `sudo pacman -S mold`, then
`export AURORA_LD=mold` (not measured here, mold was not installed).

### 3. Test runner: cargo-nextest (I-38, not adopted as default)

Installed in a scratch dir for the measurement (cargo-nextest 0.9.151), same host, same binaries:

| Run | Result | Time (test phase only) |
|---|---|---|
| `cargo test` (libtest) | 893 passed | ≈ 14 min including a rebuild; `aurora-api` `it` alone 566 s |
| nextest, 16 threads | 890 passed, **2 failed** | 534 s |
| nextest, 8 threads (`.config/nextest.toml`) | 892 passed, 1 skipped | 1014 s (host load 34 at the time) |

The two failures were `too many clients already` from the shared Postgres: process-per-test opens one
connection pool per test, and 16 parallel processes exceeded `max_connections`. Capped at 8 threads it passes but
the host was busier, so no net win could be shown, and nextest skips doctests and lists 893 tests against
cargo's 896 (not investigated). Therefore `just test` stays `cargo test`; `just test-nextest` and
`.config/nextest.toml` (retries 0, no fail-fast, 60 s slow warning, 10 min kill, 8 threads) are there for whoever
wants it. Install: `cargo install cargo-nextest --locked`. Revisit if the Postgres `max_connections` is raised.

### 4. Shared compilation cache: sccache (I-39, optional)

Measured with sccache 0.19.0 installed in a scratch dir, same test set, `target/` deleted before each run:

| Run | Time | Compile requests | Cache |
|---|---|---|---|
| no sccache | 491 s | – | – |
| sccache, empty cache (cold) | 546 s (+11 %) | 999 cacheable misses | 616 MB on disk |
| sccache, warm cache | **244 s (−55 %)** | 999 of 999 cacheable hits | unchanged |

The remaining 244 s is what sccache cannot cache (179 non-cacheable calls: linking, build scripts, proc-macro
hosts) plus cargo itself. So a lane that starts from an empty `target/` but a warm cache builds in about half the
time and writes the same 4.5 GB of `target/`. Not measured: hits across *different worktree paths*. Registry
dependencies compile from the same `~/.cargo/registry` sources and flags and should hit; workspace crates embed
their path in the crate hash and will not. sccache disables incremental compilation, which is already off for
`dev`/`test` (I-24).

Optional setup, nothing in the repo depends on it:

```sh
cargo install sccache --locked
export RUSTC_WRAPPER=sccache          # shell profile, or [build] rustc-wrapper in ~/.cargo/config.toml
export SCCACHE_CACHE_SIZE=20G         # cap (default 10 GB); location ~/.cache/sccache, override with SCCACHE_DIR
sccache --show-stats                  # also printed by scripts/dev/disk-report.sh
```

Do not put `rustc-wrapper` in the repo's `.cargo/config.toml`: cargo fails where the binary is missing.
Use it in lanes by exporting the variables in the Orca worker environment, not in the worker spec.

### 5. Developer profile: `dev-fast` (I-36)

`[profile.dev-fast]` (inherits `dev`, `incremental = true`) is used by `just dev`, `just migrate`,
`just build-fast` (`AURORA_DEV_PROFILE=dev` switches back). **Since VEL-004 (I-45) `dev` itself is incremental
and `dev-fast` is only an alias** (`inherits = "dev"`); see §6. CI keeps `CARGO_INCREMENTAL=0`.

### 6. Incremental `dev`/`test` (VEL-004, I-45)

`[profile.dev] incremental = true` (test inherits). Workspace members only; registry deps stay non-incremental and
sccache-cached. `scripts/dev/rustc-wrapper.sh` passes `-C incremental=…` rustc calls straight through and unsets
`CARGO_INCREMENTAL` before invoking sccache (sccache aborts every call, even `rustc -vV`, with
"incremental compilation is prohibited" when that variable is set — reproduced). Do not export
`CARGO_INCREMENTAL=1`: it makes registry crates incremental too.

CI: `ci.yml` workflow env `CARGO_INCREMENTAL: "0"` (line 26, applies to every job); `nightly.yml` had none and now sets
the same at workflow level. `just clean-stale` also prunes `target/*/incremental/<crate>-<hash>` untouched for N days
(demo: 5 dirs aged 20 days → 11 GB → 7.4 GB).

Measurement: one-line comment added at the top of one source file, then `cargo test <pkgs> --no-run`; A = non-incremental
(`CARGO_PROFILE_DEV_INCREMENTAL` unset = old profile), B = incremental, separate target dirs, interleaved A,B,
sccache on. **The machine was never idle** (other lanes compiling; load 5–50, listed per row), so read ratios, not seconds.

| edit in | round | A non-incr. | B incr. | ratio | load at start of A→B |
|---|---|---|---|---|---|
| aurora-api (`domains/me/mod.rs`) | 1 | 250.6 s | 700.8 s | 0.36× (outlier) | 5.5 → 9.6, up to 28 during B |
| | 2 | 252.6 s | 35.8 s | 7.1× | 28.5 → 15.0 |
| | 3 | 325.8 s | 105.6 s | 3.1× | 17.3 → 30.8 (B ran right after a cache re-prime) |
| aurora-gateway (`src/lib.rs`) | 1 | 149.5 s | 105.5 s | 1.4× | 22.4 → 16.3 |
| | 2 | 38.5 s | 6.2 s | 6.2× | 9.0 → 8.8 |
| aurora-db (`src/lib.rs`; builds db+api+gateway tests) | 1 | 438.6 s | 178.1 s | 2.5× | 8.3 → 21.5 |
| | 2 | 436.8 s | 50.3 s | 8.7× | 23.2 → 13.3 |

Round 1 of each pair was the first edit after the warm build and ran under heavy load (the api B run hit load 28; its
cause was not isolated — contention, not a measured property of incremental). Steady-state edits are 6–9×;
the worst loaded pair is 3.1×. Pending: re-run api on an idle machine.
`du -sh target/debug`: non-incremental 5.0 GB (api) / 5.8 GB (gateway, db); incremental 12 GB / 14 GB / 16 GB (11 GB of it
`incremental/`, i.e. the trade is ~2.5–3× disk for ~6× speed; bounded by I-34/I-35/clean-stale).

sccache with incremental on (aurora-crypto, fresh target dir, warm cache): requests 14034 → 14086 (+52: 32 hits, 18 misses,
workspace crates bypass sccache); with `CARGO_INCREMENTAL=1` env (wrapper unsets it): +52 requests, 43 hits, 7 misses, 0 errors.
Registry crates still hit the cache.

### 7. Quick gate, test pool, thread count (VEL-005)

Measured on the shared host (other lanes compiling; load recorded per row — read as indicative only).

**Test pool size.** `TestDb` keeps a pool of 5 (now `AURORA_TEST_POOL_SIZE`-overridable, default 5). The brief's
hoped-for 2 does **not** work: the `aurora-api` harness hands `db.pool()` to the app under test (workers, SSE, LISTEN,
long polls), and with 2 connections `cargo nextest run -p aurora-api -j 8` failed 20 of 899 tests (500s / timeouts at 40–127 s:
`channels_inbox::file_fetch_*`, `channels_mgmt::*`, `grants_*`, `runner_plane::*`, `schedules::*`, …; 2390 s run, load 15–20).
The same 20 tests pass with `AURORA_TEST_POOL_SIZE=5` (20/20, 195 s). Full suite at pool 5, 8 threads: 899/899 passed,
1272 s wall (load 7–14, ~16 CPU-minutes of user time, i.e. dominated by waiting on a loaded host).

**Thread-count sweep (8 / 12 / 16): pending an idle machine — the coordinator runs it.** Load stayed 7–30 the whole lane, so no
meaningful sweep was possible; `test-threads = 8` is unchanged and `max_connections` of aurora-dev-pg was not touched
(unmeasured estimate: pool 5 × threads connections plus app-side pools, so 16 threads may need a raised
`max_connections` first — watch for "too many clients"). Command: `for j in 8 12 16; do /usr/bin/time cargo nextest run -p aurora-api -j $j; done`
(optionally with `AURORA_TEST_POOL_SIZE=3|4`), then pick the fastest run without "too many clients" and set
`[profile.default] test-threads` in `.config/nextest.toml`.

**Quick gate on a one-file `aurora-gateway` change** (incremental profile, interleaved, load 3–15): the old command set
(`clippy -p aurora-gateway --all-targets --all-features` + `cargo test -p aurora-gateway`) took 87 s and 193 s (after a
416 s warm-up); the new one (clippy without `--all-features` — gateway defines no feature flags, so the artefacts are identical —
+ `cargo nextest run -p aurora-gateway`) took 197 s and 225 s (warm-up 271 s). **No speed-up measured**: the gateway suite
is database-bound, libtest ran its binaries with 16 threads where nextest is capped at 8, and host load swung the numbers more
than the tools did. The gate's gain here is correctness (dependents) not speed.

**Template provenance.** The stamp is `COMMENT ON DATABASE` on the template (`hash=… migrations=53`); the
`aurora_template_meta` table is gone and the hash is a stable SHA-256 prefix. A testkit test asserts a clone's relation list
equals the `AURORA_TEST_TEMPLATE_DB=0` path's and that the stamp table is absent.

**Leaked test databases and the reap lock (FIX-103).** `aurora_test_<uuid>` databases left by killed runs are reaped (no
connection, older than 2 h by the creation time in the v7 uuid) by the first `TestDb` of any test process; their
`aurora_test_gw_*` roles with them. Clones take the advisory lock `aurora_test_reap` shared, reaps exclusive (skipped after a
10 s wait), so a reap of another migration set's template never races a clone. The gate's migration scope is the
reverse-dependency closure of `aurora-testkit`, not just `aurora-db` and `aurora-api`.

## aurora-api split (VEL-007 Part A)

Question: with incremental `dev` (§6) merged, does moving part of `aurora-api` (75.5 k LOC, 311 files) into its own
crate still pay? Measured at `96707ff7` in a lane worktree (own `target/`, sccache on, `rustc 1.99.1`).
**The host was never idle** (other lanes compiling, 1-minute load 9–25 during the timed runs, recorded per row), so
read the seconds as loaded numbers. **The coordinator should confirm the absolute numbers on an idle machine.**

### Measurements

Each edit appends, changes or removes a `// comment` line at the end of one file (Edit tool), then runs the command;
three edits per row, median reported. Load = 1-minute load at start → end of the runs.

| Edit | Command | Runs (s) | Median | Load |
|---|---|---|---|---|
| leaf domain `domains/me/service.rs` | `cargo test -p aurora-api --no-run` | 31.8, 44.0, 46.7 | **44.0 s** | 9.6–21 |
| same, second series | same | 42.5, 32.3, 61.6 | **42.5 s** | 9.5–20 |
| big domain `domains/scim/resource_users.rs` | same | 68.0, 47.1, 48.3 | **48.3 s** | 18.7–24.9 |
| `aurora-supervisor/src/harvest.rs`, **before** the dependency fix | same | 52.5, 43.3, 44.6 | **44.6 s** | 13.9–22.7 |
| same, **after** the fix (below) | same | 16.2, 22.3, 20.7 | **20.7 s** | 10.7–11.5 |
| `domains/me/service.rs` | `cargo test -p aurora-api --test it --no-run` (the lane gate's build) | 32.1, 35.9, 30.5 | **32.1 s** | 14.6–17 |

`cargo build -p aurora-api --tests --timings`, per unit (one run each; frontend/codegen split from the HTML, which
shows it for the lib only):

| Edit | `aurora-api` lib (frontend + codegen) | lib-test (`cfg(test)` copy) | `it` test crate | Wall | Load |
|---|---|---|---|---|---|
| `domains/me/service.rs` | 29.3 s (23.6 + 5.6) | 29.3 s, **in parallel** with the lib | 11.7 s, after the lib | 42.6 s | 14 |
| supervisor, before the fix | 13.9 s (10.2 + 3.7) | 14.1 s | 11.4 s | 26.8 s | 20 |
| supervisor, after the fix | **not rebuilt** | 16.3 s | 9.9 s (starts at once) | 20.0 s | 12 |
| `aurora-db/src/repo/scim.rs` (proxy for "edit inside an extracted seam") | 21.5 s (16.9 + 4.7) | 22.8 s | 17.5 s | 41.5 s | 10–12.5 |
| same file, revert | 31.0 s (23.0 + 8.0) | 32.2 s | 25.1 s | 60.9 s | 12.7–15.7 |

Where an incremental rebuild of the api lib goes (`RUSTC_BOOTSTRAP=1 cargo rustc -p aurora-api --lib -- -Z time-passes`
after a comment edit in `me/service.rs`, load 16–19; a one-off diagnostic — setting `RUSTC_BOOTSTRAP` rebuilds every
dependency once): total **21.5 s**: `macro_expand_crate` **9.4 s (44 %)**, dep-graph and result-cache persistence 3.3 s,
`generate_crate_metadata` 2.2 s, monomorphization collection 1.3 s, name resolution 1.0 s, symbol partitioning 0.7 s,
type check 0.6 s, coherence 0.5 s, borrowck 0.25 s, link 0.3 s, parse ≈ 0. The same for the `it` test crate (63 k LOC,
92 files) after an api edit: total **14.9 s**, macro expansion **6.1 s**, dep-graph persistence 3.4 s, link 1.8 s, codegen 1.1 s.

`-Z macro-stats` on the api lib: 13.7 MB of expanded code per build, half of it from attribute/derive proc macros.
Largest: `#[derive(Deserialize)]` 231 uses / 2.66 MB, `#[utoipa::path]` 315 / 1.77 MB, `json!` (479 calls, 7 543
recursive `json_internal!`) 1.69 MB, `#[derive(ToSchema)]` 479 / 1.24 MB, `utoipa_axum::routes!` 0.82 MB, `tracing`
macros ≈ 3 MB together, `#[derive(Serialize)]` 408 / 0.62 MB. The 42 `dto.rs` files (7.1 k LOC) carry 240 of the 330
`Deserialize`, 417 of 483 `Serialize`, 490 of 528 `ToSchema` and 47 of 62 `IntoParams` mentions. `it`: 2 082 `json!`,
862 `#[tokio::test]`.

What the numbers say:

1. **The cost of an api edit is a per-rebuild floor, not the edit.** Incremental reuses type checking and codegen, but
   macro expansion (proc macros run again on every build), metadata and dep-graph persistence are paid over the whole
   crate every time. A change in an *unused* upstream crate (supervisor, before the fix) still rebuilt the api lib in
   13.9 s, and the 63 k-LOC `it` crate adds 10–12 s after it, on the critical path of every api edit.
2. **lib-test is not on the critical path.** The lib and its `cfg(test)` copy compile in parallel and `it` waits only
   for the lib. On a loaded host the second copy competes for CPU, which is why `--test it` (no lib-test) measured
   32.1 s against 42.5–44.0 s.
3. **An edit in an upstream crate the api uses was not cheaper end to end** (aurora-db proxy: 41.5 s vs 42.6 s for an
   in-api edit). The api lib dropped to 21.5 s from 29.3 s, but `it` (which also uses aurora-db directly) grew. An edit
   *inside* an extracted seam behaves like this: seam crate + api lib (upstream changed) + `it`.
4. **An edit outside a seam saves only the seam's share of the floor.** The floor scales with the expanded code, so a
   4.7 k-LOC domain (6 % of the crate) saves ≈ 1–1.5 s of a ≈ 21–29 s lib rebuild — inside the ±50 % noise of this host.

### Why `aurora-api` depended on `aurora-supervisor`

`[dependencies] aurora-supervisor` was **unused**: there is no `aurora_supervisor` path in `services/aurora-api/src`
(lib or bin). It came in with FIX-005 (`212f75b1`) together with the dev-dependency. The only users are two `it` tests:
`tests/it/home_render_inputs.rs` (`aurora_supervisor::instructions::{RenderInputs, render}`: the supervisor renders what
the API sends) and `tests/it/home_checksum.rs` (`harvest::{Harvester, TRAILER_PREFIX, strip_trailer, verify}`,
`home::{HomeConfig, HomeProjector, PartOutcome}`: real code of both crates around the memory checksum trailer). These are
supervisor *behaviour*, not shared types, so moving them to `aurora-protocol`/`aurora-core` is not right; only
`RenderInputs`, a wire shape, could go to `aurora-protocol` (PROGRESS FUP-M4), and that would not remove the
dev-dependency because the test calls `render`. The normal dependency also broke the "`services/*` depend on `crates/*`"
rule (`docs/02-engineering-conventions.md`).

**Done** (commit `VEL-007: aurora-api takes aurora-supervisor as a dev-dependency only`): the normal dependency is
removed, the dev-dependency stays, `Cargo.lock` is unchanged. A supervisor edit no longer rebuilds the api lib and bin:
**44.6 s → 20.7 s** (medians above). Left: lib-test and the test crates still rebuild on supervisor edits, because
dev-dependencies apply to every test target. Removing that too needs the two tests in a separate crate (with a copy of,
or a shared crate for, `tests/it/common`); not worth it for two tests.

### Seam candidates

Inbound = references to the seam from the rest of `aurora-api` (`grep 'domains::<x>'` outside its directory);
outbound = what the seam uses from the rest of the crate. Savings are per api lib copy, estimated from point 4 above.

| Seam | LOC | Inbound | Outbound | Prerequisite | Edit outside the seam | Edit inside the seam |
|---|---|---|---|---|---|---|
| `domains/scim` | 4 705 (13 files) | 7 refs in 3 files: `app.rs` routers; `scim::gates::{admission_gate, audit_denied, Admission}` from `oidc/service.rs` and `invitations/service.rs` | `app::AppState` (7), `audit` (20), `http::{auth, counters, client_ip, apilimit}`, `hosted::lifecycle::{finish, flush_deltas}`, `conversations::service::append_event`, `groups::service::notify_group_deleted`, `me::service::REAUTH_WINDOW_SECS` | kernel crate (below), and those four domain calls moved down or injected | ≈ −1–1.5 s | ≈ 0 (point 3) |
| `skills` + `skill_libraries` (they reference each other, so they move together) | 4 489 | 11 refs: `app.rs` (`AppState.library: LibraryEnv`, gateway), `main.rs` refresh task, `tools/skills.rs` | `AppState`, `audit`, `http::{auth, json, error, pagination}`, `plugins` | kernel crate, with `LibraryEnv` + gateway in it | ≈ −1–1.5 s | ≈ 0 |
| `relay` + `previews` + `terminal` | 2 701 | 7 refs: `app.rs`, `main.rs` (`bootstrap_relay_token`), `runners_admin/routes.rs`, `runner_plane/preview_streams.rs` | `runner_plane` (9), `freeze` (4), `conversations` (3), `oidc`, `AppState`, `audit`, `http::{auth, json, ratelimit, csrf, error}` | kernel crate, plus `runner_plane` (1.6 k) or traits | ≈ −0.5–1 s | ≈ 0 |
| OpenAPI/utoipa layer | — | `#[utoipa::path]` sits on 313 handlers and `ToSchema` on the DTOs; `openapi.rs` is 56 lines | — | not separable from the handlers; its DTO half is the next row | — | — |
| **DTO layer** → `crates/aurora-api-types` | 7 147 (42 `dto.rs`) | every domain, through `super::dto` | 6 references: `memory/dto.rs:22` `super::service::index_line`; `skills/dto.rs:1` `service::{self, SkillView}`; `agents/dto.rs:31,61` `byo::ByoView`, `avatar::public_path`; `tenancy/dto.rs:169` `agents::byo::ByoPostureView`; `groups/dto.rs:309` `http::pagination::Page` | none (only lower crates: `aurora-db` ×27 files, `aurora-registry`, `aurora-llm`) | **≈ −3–4.5 s** (≈ 26 % of expanded bytes, ≈ 52 % of the proc-macro output) | equal or worse: a derive-dense crate (est. 5–8 s; expansion is not incremental) + api lib (upstream changed) + `it` |

The kernel prerequisite: every domain uses `crate::app::AppState`, `crate::audit` and `crate::http::*`, and `AppState`
holds types from eight domains (`approvals::{bus, notify}`, `conversations::bus`, `connectors::bus`, `oidc::OidcClients`,
`connect::Env` + `hooks`, `channels::card_requests::CardRequests`, `skill_libraries::{LibraryEnv, gateway}`,
`auth::mail::Mailer`; `try_new` also calls `secrets::gov::instance_from_config` and builds `channels::cards::requests::CardsImpl`).
A domain crate below `aurora-api` therefore first needs an `aurora-api-kernel` crate: `AppState` + `http/` (2.7 k) +
`audit.rs` + those components (≈ 2 k), ≈ 5–6 k LOC, with router assembly and `CardsImpl` construction staying in
`aurora-api`. That is two new crates for one domain seam, and every kernel edit still rebuilds everything above it.

### Decision

- **Brief's rule** (one-line api edit → `cargo test -p aurora-api --no-run` ≤ 30 s ⇒ no split): under load the medians are
  42.5–48.3 s and the best single runs 31.8 s and 32.3 s (load ≈ 10). An idle host will probably land near the threshold,
  so the rule's outcome is **pending idle confirmation** (rerun rows 1 and 2 of the first table on an idle host).
- **Recommendation regardless of that number: do not run Part B with a domain seam.** The measured mechanism is a floor
  over the whole crate (macro expansion, metadata, dep graph) plus the `it` crate. One domain seam removes ≈ 6 % of the floor
  for edits outside it and nothing for edits inside it, and it needs a kernel crate first; the expected benefit is
  below this host's noise.
- If a Part B is still wanted, the **DTO crate is the best single seam**: no kernel needed, and it takes most of the
  proc-macro output out of every api rebuild. Expected ≈ 3–4.5 s off each lib copy for edits that don't touch DTOs (≈ 10 % of
  an edit→test cycle); edits that change an API shape (DTO + handler) get no faster, possibly slightly slower. Plan below.
- **Cheaper levers, by measured yield:**
  1. Supervisor dependency dev-only — done, 44.6 → 20.7 s for supervisor edits.
  2. Inner loop `cargo test -p aurora-api --test it [filter]` instead of `cargo test -p aurora-api`: skips lib-test,
     32.1 s vs 42.5–44.0 s median under load (≈ no gain on an idle host, where lib-test runs in parallel). The lane gate
     already uses `--test it`; the 67 lib unit tests (24 modules) can run once before the final commit.
  3. The `it` crate (10–17 s after every api edit, 6.1 s of it macro expansion, on the critical path): splitting it into
     2–3 binaries that compile in parallel would cut that part roughly proportionally, at the cost of I-34 (more links and
     disk; its link is 1.8 s). Needs an owner decision on I-34; not measured.

### Part B plan (DTO seam), if chosen

- New crate `crates/aurora-api-types` (lib, no tests of its own) depending on `aurora-core`, `aurora-db`,
  `aurora-registry`, `aurora-llm`, `serde`, `serde_json`, `utoipa`, `uuid`, `chrono` — whatever the moved files import.
  In `crates/` because it depends on crates only. Add it to the workspace members and `[workspace.dependencies]`, and
  to the crate lists in `docs/02-engineering-conventions.md` and `docs/01-architecture.md`.
- `git mv services/aurora-api/src/domains/<d>/dto.rs crates/aurora-api-types/src/<d>.rs` for all 42; the new `lib.rs`
  declares `pub mod <d>;`. In `aurora-api` each `domains/<d>/mod.rs` replaces `pub mod dto;` with
  `pub use aurora_api_types::<d> as dto;` (the three private `mod dto;` become `use aurora_api_types::<d> as dto;`), so
  every `super::dto::X` and `crate::domains::<d>::dto::X` path keeps compiling unchanged. Inside the new crate,
  `crate::domains::<x>::dto::` becomes `crate::<x>::`.
- Also move: `http::pagination::Page` (the response type and its `ToSchema`; the query extractor stays),
  `http::error::{ErrorEnvelope, ErrorBody}` only if `openapi.rs` cannot reach them otherwise, and the view structs
  `agents::byo::{ByoView, ByoPostureView}`.
- The outbound references: `memory/dto.rs` `index_line` and `agents/dto.rs` `avatar::public_path` are pure helpers — move
  them along (or pass the value in). `skills/dto.rs` builds from `service::SkillView`: keep that conversion in `aurora-api` as
  `impl From<SkillView> for Dto` (the orphan rule allows it: the local type is the trait parameter) or as a free function.
  Any inherent `impl Dto` that needs api-internal items becomes a free function in the domain.
- Visibility: no `pub(crate)` exists in any `dto.rs`, so the types need no change; only moved helpers go `pub(crate)` → `pub`.
- Risks: (1) OpenAPI drift — utoipa names schemas by type ident, so the document should stay byte-identical;
  `openapi_matches_checked_in` must pass without `UPDATE_OPENAPI`. (2) No `#[cfg(test)]` module lives in a `dto.rs`, so no
  unit tests move. (3) Nearly every api lane edits `dto.rs` files: run it alone among api lanes, right after a rebase.
  (4) The saving is an estimate from expansion byte shares: re-measure the `me/service.rs` row and the `--timings` row after
  the move, record them here, and revert if the lib saving is below ≈ 2 s.

## Disk hygiene

- `bash scripts/dev/disk-report.sh [dir…]` (or `just disk-report`): free space, `target/` of every
  `git worktree list` entry (largest first), sccache stats, podman images / containers / volumes by prefix,
  kind clusters. Read-only. Note it runs `du` over every `target/`; a 60 GB tree takes a minute.
- `bash scripts/dev/clean-lane.sh [--dry-run] [--prefix P]… [--keep-target] <worktree>` (or `just clean-lane <worktree>`):
  1. kills processes whose `/proc/PID/cwd` is inside the worktree (never itself or its ancestors), SIGTERM then SIGKILL;
  2. removes podman containers, pods, networks and volumes whose **name starts with** the lane prefix
     (default `<worktree-basename>` and `aurora-<worktree-basename>`) or that carry the label `aurora.lane=<basename>`;
     the shared `aurora-dev-pg` is never touched, and prefixes shorter than 4 characters or `aurora`/`test`/`e2e`/`dev` are refused;
  3. removes `<worktree>/target`.
  It refuses anything that is not a linked worktree (`.git` must be a file), `/` and `$HOME`. Lanes that start
  podman objects must name them with the lane prefix (or label them `aurora.lane=<basename>`) to be cleaned.
  Tested on a throwaway worktree: stray `sleep` with cwd inside and the shell that started it killed,
  containers `throwaway-lane-c1` and `aurora-throwaway-lane-c2`, network, two volumes (one found by label) and a
  48 MB `target/` removed, an unrelated container and `aurora-dev-pg` untouched; `--dry-run` printed the same
  plan and changed nothing.
- `just clean-stale [days=14]`: `cargo sweep --time <days>` when cargo-sweep is installed
  (`cargo install cargo-sweep --locked`), otherwise it prints the fallback: `cargo clean` while no build is running.

### Text for the Orca worker spec template (the coordinator owns it)

> **Disk hygiene.** Build only the crates your brief names, in this worktree's own `target/`; do not set
> `CARGO_TARGET_DIR`. Run commands in the foreground with a timeout. Name any podman container, network or
> volume you create with the prefix `<worktree-basename>-` (or label it `aurora.lane=<worktree-basename>`).
> After `worker_done` do nothing: the coordinator cleans up.
>
> **Coordinator, on merge:** `bash scripts/dev/clean-lane.sh <worktree>` (add `--dry-run` first if unsure),
> then `git worktree remove <worktree>`. Run `bash scripts/dev/disk-report.sh` before starting a new wave and
> stop when less than 15 GB is free.

## Owner actions (nothing is required; everything works without these)

| Optional install | Command | Gain |
|---|---|---|
| mold | `sudo pacman -S mold` then `export AURORA_LD=mold` | untested here; links are already < 1 s |
| sccache | `cargo install sccache --locked` then `export RUSTC_WRAPPER=sccache SCCACHE_CACHE_SIZE=20G` | see §4 |
| cargo-nextest | `cargo install cargo-nextest --locked`, then `just test-nextest` | none shown (§3) |
| cargo-sweep | `cargo install cargo-sweep --locked` | enables `just clean-stale` |

---

<!-- CI section (OPS-019) -->

(OPS-016 owns the local-build sections; this file currently holds only the CI section from OPS-019.)

## CI (OPS-019)

Layout of `.github/workflows/ci.yml`:

| Job | Purpose |
|---|---|
| `rust-lint` | fmt + clippy (own cache `rust-lint`, own profile) |
| `rust-deny` | cargo-deny |
| `rust-build` | compiles once: fixture packages, `cargo nextest archive`, doctests; uploads the archive |
| `rust-test` (×4) | `cargo nextest run --archive-file … --partition count:i/4`, no compile |
| `web` | pnpm lint/typecheck/test/build/API drift |
| `guards` | `scripts/guards/run-all.sh`, no compile |

Changes and why:
- All actions pinned by commit SHA, all on Node 24 (`runs.using` verified per action; `cargo-deny-action` is Docker, `install-action`/`rust-toolchain` are composite). `ubuntu-24.04` pinned instead of `ubuntu-latest`.
- sqlx-cli binary cached with `actions/cache` (was `cargo install` in three jobs); nextest via `taiki-e/install-action`.
- lld via `RUSTFLAGS=-C link-arg=-fuse-ld=lld`, `CARGO_PROFILE_DEV_DEBUG=0` (disk). Align with OPS-016's `.cargo/config.toml` once merged.
- `rust-cache` is saved by `rust-build` only (key `rust-test`), `rust-lint` has its own key.
- **Coverage fix:** `connect.rs` (12 tests) failed in CI because `plugins/fixtures/dist` is gitignored and never built; `rust-build` now runs `scripts/fixtures/build.sh` and ships the packages to the shards.
- `aurora-e2e-gateway`/`aurora-e2e-hosted` are excluded from the archive (not env-gated, fixed IPs/ports, podman, serial) and run as separate jobs `e2e-gateway` / `e2e-hosted` via their scripts; the hosted sandbox image is cached as a `podman save` tarball in `actions/cache`.
- `schema_matches_spec` now runs in the shards instead of the `guards` job (same test, same migrated DB).

### Timings

| | rust-test (cold) | rust-test (warm) |
|---|---|---|
| Before (run 42367335980) | 13m47s, failed (linker SIGBUS) | n/a |
| After | TO BE RECORDED after the first push | TO BE RECORDED |
| e2e-gateway | TO BE RECORDED after first push | |
| e2e-hosted (image cache miss / hit) | TO BE RECORDED | |

(Those two rows are not before/after pairs: the jobs are new.) Not measured locally: nextest could not be installed in the worker sandbox.

## Container images (OPS-020)

`images/services/Containerfile` and the supervisor stage of `images/sandbox/Containerfile` use cargo-chef
(`planner` → `cargo chef cook` dependency layer → workspace build) with lld, and all six service images are
copies of **one** `builder` stage compile. In CI the `images-builder` job (final jobs of `ci.yml`, after every
check job) builds that stage once into the gha cache scope `services`; the per-image jobs import it.
`target/` is deliberately kept in image layers, not a `--mount=type=cache`: `cache-to type=gha` exports layers
only, so a cache-mounted `target/` would be lost between CI runs. The cargo registry/git use cache mounts.

Local measurements (podman, shared 16-core host, cold layer cache for the first row; `localhost/ops-020-*`):

| Image(s) | Wall time | Notes |
|---|---|---|
| `api` (first service target) | 1580 s | chef install + dependency cook + workspace compile of all 6 binaries |
| `gateway`, `hookhost`, `runner`, `connector-fake` | 8 s each | `COPY` of one binary out of the cached builder |
| `connector-mattermost` | 24 s | |
| `web` | 89 s | |
| `sandbox-min` | 310 s | supervisor compile + AlmaLinux OS layer |
| `sandbox` (after `sandbox-min`) | 425 s | base tool set (dnf, Chromium, Node), claude-code npm install |

CI timings (`ghcr.io` publish, cold and warm gha cache) are recorded here after the first push to `master`.

## Image builds: sccache and the kind dev path (OPS-023, I-44)

### What changed

1. **sccache in the release builder.** `images/services/Containerfile` installs sccache 0.10.0 (pinned, SHA-256
   verified per architecture) in the `chef` stage; the cargo-chef `cook` and the release `build` run through
   `with-sccache`, which sets `RUSTC_WRAPPER=sccache` and prints `sccache --show-stats` at the end of the step.
   - Local / plain `podman build`: `--mount=type=cache,target=/root/.cache/sccache`.
   - CI: **removed after measuring** (see "CI measurement" below). What follows describes the mechanism as it
     was built, kept for reference: GitHub Actions cache backend (`SCCACHE_GHA_ENABLED`). `crazy-max/ghaction-github-runtime` exposes
     `ACTIONS_RESULTS_URL` / `ACTIONS_RUNTIME_TOKEN`, which `docker/build-push-action` passes as BuildKit
     **secrets** (`actions_results_url`, `actions_runtime_token`, `images-builder` and `images-services`). They are
     mounted as files for those two RUN steps only. Not build args, so not in a layer, the `type=gha` layer cache or the
     provenance. Checked: `podman history --no-trunc` and `podman inspect` of a build with secrets contain neither
     value, only the secret *ids* in the RUN text. The build also succeeds with no secrets (checked) and falls back to
     the local directory, then to no sccache, if the backend does not start.
   - Keys: the build path is always `/build`, no `.cargo/config.toml` (no worktree linker path, no
     `CARGO_TARGET_DIR`), `RUSTFLAGS`/`CARGO_INCREMENTAL` fixed in the file. The second build below hit 19 of 20
     crates, so nothing per-build leaks into the keys.
2. **kind e2e on the dev path.** `scripts/e2e/kind.sh` sources `deploy/dev/kind-dev.sh` (in a subshell) and uses its
   `build_rust` / `thin_image`: debug build in the builder container with a persistent target volume
   (`<lane>-kind-dev-target`), thin distroless images tagged `localhost/aurora-e2e/aurora-<svc>:e2e`.
   `--release-images` (or `AURORA_E2E_RELEASE_IMAGES=1`) builds the release `Containerfile` as before. CI does not run
   `kind.sh` and keeps the release build.

### Local measurements (this host, shared with other lanes; indicative, ±50 %)

| Step | Time | sccache |
|---|---|---|
| Release builder, cold sccache (= no sccache), `cargo build --release` step | 12 m 13 s (whole `podman build` 24 m 8 s incl. cook) | 0 hits / 20 misses (cook: 0 / 616 Rust) |
| Same, after a one-line change in `aurora-api` (cook layer reused; whole `podman build` 9 m 4 s) | **8 m 32 s** | **19 hits / 1 miss** (95 %); 6 binaries not cacheable (`crate-type`) |
| `kind.sh` image preparation, release path, one-line `aurora-api` change (before) | ≈ 9–12 min (the builder step above plus ~1 min of copies) | |
| `kind.sh` image preparation, dev path, same change (after, warm target volume) | **60 s** (cargo 57 s) | n/a |
| `kind.sh --fresh` end to end on the dev images (cold target volume) | 18 m 0 s, k8s e2e 4 passed, 0 failed | |

**Finding: sccache removes the rlib compiles, not the link step.** With 19 of 20 crates cached the release build
still took 8.5 min, because the six service binaries (never cacheable: `crate-type`) are each linked with
`lto = "thin"` over the whole dependency graph. Any source change in a crate that every binary depends on, such as
`aurora-db`, therefore costs the LTO of six binaries plus the rebuild of its dependents. The profile is part of the release
contract and was not changed. If the CI numbers confirm that linking dominates, the next options are
`lto = false` for the connector/fake binaries or building the six binaries in parallel jobs; neither is done here.

### CI measurement

| Run | `images-builder` | cook step (deps) | build step (workspace) | sccache hits |
|---|---|---|---|---|
| 42400978710 (baseline, before OPS-023) | 15.5 min | layer cached | 782 s | — |
| 42417975590 (first run with the GHA backend) | 32.2 min | 10 m 01 s, recompiled | 17 m 10 s | 0 / 909 (cook), 0 / 20 (build) |
| 42422594408 (next push) | 32.0 min | 10 m 19 s, recompiled again | 17 m 39 s | 0 / 909, 0 / 20 |

**Result: removed from CI** (2026-11-08). The backend never hit across consecutive runs, and the cook layer that
the BuildKit `type=gha` cache had kept before (~44 s) was rebuilt every time. The repository's Actions cache is
capped at 10 GB and was already at ~10.8 GB (BuildKit layers 6.9 GB, the `rust-build` job's Rust cache 3.9 GB), so
sccache's entries and the layer cache evicted each other. `ci.yml` no longer exposes the Actions runtime or passes
the secrets; `with-sccache` then uses its local directory (empty in CI, negligible cost). Local builds keep sccache
through the cache mount. If CI image time matters again: shrink what competes for the 10 GB (e.g. `mode=min` for
the layer cache, or a smaller Rust cache), or move the release link out of the critical path (see the finding above).

Original checklist for a CI run with the backend (kept for reference): jobs `images-builder` and `images-services (*)`; in the step
"build the shared Rust builder stage (no push)" the log lines `with-sccache: backend = GitHub Actions cache` (twice:
cook and build) and the `sccache --show-stats` blocks (`Cache hits`, `Cache misses`, `Cache write errors`,
`Cache location  ... GitHub Actions`). Cold run: all misses, writes without errors. Second push: hits on every
workspace library crate. Also confirm the "expose the Actions cache runtime" step succeeds, the log shows no secret
values (GitHub masks the secrets), all six service images publish, and that a fork PR run (read-only or no token) still builds; if sccache cannot use the backend the log shows
`backend did not start; retrying on the local directory`.
