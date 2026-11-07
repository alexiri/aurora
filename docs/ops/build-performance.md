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
`just build-fast` (`AURORA_DEV_PROFILE=dev` switches back). One-line change in `aurora-api`: 7–14 s instead of
127–243 s. Incremental data only exists for workspace members, and it lives in `target/dev-fast`
(3.0 GB after building `aurora-api` plus its dependencies). `dev`, `test`, CI and worker lanes keep
`incremental = false` (I-24 stands for them). Run `just clean-stale` or delete `target/dev-fast` if it grows.

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
