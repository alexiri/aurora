#!/usr/bin/env bash
# VEL-002: the local quick gate — a fast pre-commit loop for the owner.
# Docs: docs/18-testing.md §"Quick gate (local loops)".
#
# A development-loop tool, NOT a merge gate: it runs a *subset* of what CI
# runs — fmt, clippy and tests for the crates a change touches, plus the web
# typecheck and the related vitest files. CI runs the full suite on every
# push; never cite a quick-gate pass as a merge-gate pass.
#
# Usage:
#   bash scripts/dev/quick-gate.sh                detect scope, run the matching subset
#   bash scripts/dev/quick-gate.sh --all          everything (the old full pre-commit suite)
#   bash scripts/dev/quick-gate.sh --clippy-only  fmt + clippy, no tests or web
#   bash scripts/dev/quick-gate.sh --tests-only   tests only (no fmt, no clippy, no tsc)
#   bash scripts/dev/quick-gate.sh --dry-run     print the plan and exit (runs nothing)
#   AURORA_QUICK_GATE_VERBOSE=1 ...               list every changed file before running
#
# VEL-005 scope rules: clippy covers the touched crates AND their workspace
# dependents (from `cargo metadata --no-deps`; the plan says why each is
# included), so a change in aurora-core/-db/-protocol can no longer "pass"
# while aurora-api or aurora-gateway no longer compile. Tests run on the
# touched crates only, plus aurora-db and aurora-api when a migration changed.
# Tests use cargo-nextest when installed (doctests are skipped, as always with
# nextest), else `cargo test`. Clippy omits --all-features unless a touched
# crate defines feature flags (--all and workspace-wide changes keep it).
# One line warns when target/ exceeds 60 GB (`just clean-stale`).
#
# Scope detection: changed paths from git diff (working tree vs HEAD, plus the
# index, plus untracked files); with a clean tree on a lane, the commits
# between the merge-base with origin/master and HEAD. Paths map to crates
# (crates/*, services/*, connectors/*, tests/conformance/connector), to web
# checks (web/* except web/e2e — Playwright is CI's job), or to the
# workspace-wide set (Cargo.lock, .cargo/, toolchain, lint config). The e2e
# crates (tests/e2e-*) are live-gated and CI-only: they only get a note. No
# scope at all (a docs-only change, or a clean tree on master) runs the core
# set — fmt + clippy on aurora-db and aurora-protocol — so the gate never
# silently does nothing.
#
# Host discipline (the box is shared with worker lanes): heavy commands run
# under `nice -n 10`; before the long phases the gate waits while the 1-minute
# load average exceeds 25 and notes 15–25. Tests and clippy find Postgres
# through the repo convention (I-05): `.env` at the repo root, else the
# environment, else the documented aurora-dev-pg URL is exported; sqlx::test
# creates and drops its own scratch databases, never the dev database itself.
#
# The gate never bypasses .cargo/config.toml: every cargo command runs from
# the repo root (no --manifest-path), so scripts/dev/linker.sh and the
# sccache rustc-wrapper stay in effect.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

usage() {
  echo "usage: scripts/dev/quick-gate.sh [--all | --clippy-only | --tests-only]"
  echo "  (default)      fmt + clippy + tests for the crates touched by the changes,"
  echo "                 plus web typecheck and related vitest when web/ changed"
  echo "  --all          the old full pre-commit suite: fmt, workspace clippy, all"
  echo "                 non-e2e cargo tests, web lint + typecheck + full vitest,"
  echo "                 architecture guards (cargo deny stays CI-only)"
  echo "  --clippy-only  fmt + clippy, no tests, no web"
  echo "  --tests-only   tests only (cargo tests; web vitest, no tsc)"
  echo "  --web          web checks only: tsc + the changed/vitest-related files"
  echo "  --dry-run      print the plan (scope, clippy dependents, test set) and exit"
  echo "  AURORA_QUICK_GATE_VERBOSE=1 lists every changed file before running"
  echo "  docs/18-testing.md §\"Quick gate (local loops)\": a dev-loop tool, NOT a merge gate"
}

ALL=0 CLIPPY_ONLY=0 TESTS_ONLY=0 WEB_ONLY=0 DRY_RUN=0
for arg in "$@"; do
  case "$arg" in
    --dry-run)     DRY_RUN=1 ;;
    --all)         ALL=1 ;;
    --clippy-only) CLIPPY_ONLY=1 ;;
    --tests-only)  TESTS_ONLY=1 ;;
    -h|--help)     usage; exit 0 ;;
    --web)         WEB_ONLY=1 ;;
    *) echo "quick-gate: unknown flag '$arg'" >&2; usage >&2; exit 2 ;;
  esac
done

T0="$(date +%s)"
VERBOSE="${AURORA_QUICK_GATE_VERBOSE:-0}"

die() { echo "quick-gate: FAILED: $*" >&2; exit 1; }
load_1m() { cut -d' ' -f1 /proc/loadavg; }

# Host discipline: wait while the 1-minute load is above 25 (shared box),
# note 15–25, proceed below that. Give up after 20 min of waiting.
load_gate() {
  local l waits=0
  while :; do
    l="$(load_1m)"
    if awk -v l="$l" 'BEGIN { exit !(l > 25) }'; then
      waits=$((waits + 1))
      if [ "$waits" -gt 40 ]; then
        echo "quick-gate: 1-min load still > 25 after 20 min of waiting; retry when the box is quieter." >&2
        exit 1
      fi
      echo "load $l > 25 — waiting 30 s (host discipline: shared box)"
      sleep 30
    else
      awk -v l="$l" 'BEGIN { exit !(l >= 15) }' \
        && echo "note: 1-min load $l is elevated (15–25); proceeding under nice -10"
      break
    fi
  done
}

# phase "<label>" <cmd...> — run the command under nice -10, print wall time.
phase() {
  local label="$1"; shift
  local t0 t1 rc
  t0="$(date +%s)"
  echo "--- $label"
  if nice -n 10 "$@"; then rc=0; else rc="$?"; fi
  t1="$(date +%s)"
  echo "    [$((t1 - t0))s] $label"
  return "$rc"
}

# Package name of the crate in a workspace directory ("crates/aurora-db"), or
# "" when it has no Cargo.toml (deleted or not a crate directory).
cargo_name() {
  local toml="$1/Cargo.toml"
  [ -f "$toml" ] || { echo ""; return 0; }
  grep -m1 '^name = ' "$toml" | cut -d'"' -f2
}

# VEL-005: `scope_info <crate>...` reads `cargo metadata --no-deps` (workspace
# members only; fast, nothing cached) and prints one line per fact:
#   RDEP <dependent> <crate-it-depends-on>   workspace dependents, transitive
#   FEAT <crate>                             defines feature flags beyond `default`
# The e2e crates are CI-only and never listed as dependents.
scope_info() {
  cargo metadata --no-deps --format-version 1 | python3 -I -c '
import json, sys
pk = {p["name"]: p for p in json.load(sys.stdin)["packages"]}
touched = [t for t in sys.argv[1:] if t in pk]
rdeps = {}
for p in pk.values():
    for d in p["dependencies"]:
        if d["name"] in pk:
            rdeps.setdefault(d["name"], set()).add(p["name"])
seen = {}
for t in touched:
    stack = [t]
    while stack:
        c = stack.pop()
        for r in sorted(rdeps.get(c, ())):
            if r not in touched and r not in seen and not r.startswith("aurora-e2e-"):
                seen[r] = c
                stack.append(r)
for r, c in sorted(seen.items()):
    print("RDEP", r, c)
for t in touched:
    if any(f != "default" for f in pk[t]["features"]):
        print("FEAT", t)
' "$@"
}

have_nextest() { cargo nextest --version >/dev/null 2>&1; }

# One line, never deletes anything: target/ past 60 GB.
disk_warning() {
  local kb
  [ -d target ] || return 0
  kb="$(du -sk target 2>/dev/null | cut -f1)" || return 0
  if [ "${kb:-0}" -gt $((60 * 1024 * 1024)) ]; then
    echo "note: target/ is $((kb / 1024 / 1024)) GB (> 60 GB) — run 'just clean-stale' (prunes old incremental dirs and stale artefacts) when no build is running"
  fi
}

changed_files() {
  {
    git diff --name-only HEAD --
    git diff --name-only --cached --
    git ls-files --others --exclude-standard --
  } | sort -u
}

# Repo convention (I-05): sqlx macros and sqlx::test resolve the dev Postgres
# from .env at the repo root, else DATABASE_URL/AURORA_TEST_DATABASE_URL. When
# none of those exist, export the documented aurora-dev-pg URL so the gate
# does not fall back to starting a throwaway podman container per run.
ensure_db_url() {
  if [ -n "${AURORA_TEST_DATABASE_URL:-}" ] || [ -n "${DATABASE_URL:-}" ] || [ -f "$ROOT/.env" ]; then
    return 0
  fi
  export DATABASE_URL="postgres://aurora:aurora@127.0.0.1:55432/aurora"
  echo "env: no .env and no DATABASE_URL — exporting the repo-convention dev Postgres (aurora-dev-pg on 127.0.0.1:55432; sqlx::test uses scratch databases only)"
}

fixtures_needed=0
# CI builds the fixture packages before its test shards (rust-build, e2e and
# the secrets job run scripts/fixtures/build.sh); plugins/fixtures/dist is
# gitignored, so a fresh lane does not have them and the aurora-api connect::
# tests fail on a missing file. Build them once when the test scope includes
# aurora-api (or is workspace-wide) and the packages are absent.
plan_fixtures() {
  fixtures_needed=0
  [ "$CLIPPY_ONLY" -eq 0 ] || return 0
  if [ "$ALL" -eq 1 ] || [ "$workspace_wide" -eq 1 ]; then
    fixtures_needed=1
  else
    for c in ${test_crates[@]+"${test_crates[@]}"}; do
      [ "$c" = aurora-api ] && fixtures_needed=1
    done
  fi
  [ "$fixtures_needed" -eq 1 ] || return 0
  [ -f plugins/fixtures/dist/fixture-hooks.aurora-plugin ] && fixtures_needed=0
}

run_fixtures() {
  [ "$fixtures_needed" -eq 1 ] || return 0
  echo "note: plugins/fixtures/dist is missing (gitignored; CI builds it before the test shards) — building the fixture packages once"
  if [ -x target/debug/aurora ] && [ -x target/debug/aurora-gateway ]; then
    phase "fixture packages: bash scripts/fixtures/build.sh (SKIP_CARGO=1; target binaries exist)" \
      env SKIP_CARGO=1 bash scripts/fixtures/build.sh
  else
    phase "fixture packages: bash scripts/fixtures/build.sh" bash scripts/fixtures/build.sh
  fi
}

# Web checks. tsc is the typecheck; vitest `related` is a subcommand in this
# vitest (4.x), not a --related flag: pass the changed source files and it
# runs the test files that import them. Changed test files run directly.
run_web() {
  [ "$CLIPPY_ONLY" -eq 1 ] && return 0
  [ "$web_changed" -eq 1 ] || return 0
  command -v npx >/dev/null 2>&1 || die "npx not found (the web checks need Node)"
  [ -x web/node_modules/.bin/tsc ] || die "web dependencies missing: run 'cd web && pnpm install --frozen-lockfile' first"
  if [ "$TESTS_ONLY" -eq 0 ]; then
    phase "web: npx tsc --noEmit" bash -c 'cd web && exec npx tsc --noEmit'
  fi
  if [ "${#web_tests[@]}" -gt 0 ]; then
    phase "web: npx vitest run (changed test files)" \
      bash -c 'cd web && exec npx vitest run "$@"' _ "${web_tests[@]}"
  fi
  if [ "${#web_src[@]}" -gt 0 ]; then
    phase "web: npx vitest related (tests importing the changed sources)" \
      bash -c 'cd web && exec npx vitest related "$@"' _ "${web_src[@]}"
  fi
}

# --- scope detection ---------------------------------------------------------

mode="working tree (+ index + untracked) vs HEAD"
files=()
mapfile -t files < <(changed_files)
if [ "${#files[@]}" -eq 0 ]; then
  base="$(git merge-base HEAD origin/master 2>/dev/null || true)"
  if [ -n "$base" ] && [ "$base" != "$(git rev-parse HEAD)" ]; then
    mode="clean lane: commits since merge-base $(git rev-parse --short "$base") with origin/master"
    mapfile -t files < <(git diff --name-only "$base" HEAD)
  fi
fi

crates=() web_src=() web_tests=()
web_changed=0 workspace_wide=0 e2e_note=0 schema_check=0 migrations_changed=0
for f in ${files[@]+"${files[@]}"}; do
  case "$f" in
    crates/aurora-db/migrations/*|docs/03-data-model.md) schema_check=1 ;;
  esac
  case "$f" in
    crates/aurora-db/migrations/*) migrations_changed=1 ;;
  esac
  case "$f" in
    crates/*|services/*|connectors/*)
      top="${f%%/*}"; rest="${f#*/}"
      name="$(cargo_name "$top/${rest%%/*}")"
      if [ -n "$name" ]; then
        crates+=("$name")
      else
        echo "note: '$f' does not map to a crate (Cargo.toml missing?)" >&2
      fi
      ;;
    web/*)
      case "$f" in
        web/e2e/*)   e2e_note=1 ;;
        web/src/*)   web_changed=1; web_src+=("${f#web/}") ;;
        web/tests/*) web_changed=1; web_tests+=("${f#web/}") ;;
        *)           web_changed=1 ;;
      esac
      ;;
    Cargo.toml|Cargo.lock|rust-toolchain.toml|rustfmt.toml|clippy.toml|deny.toml|.cargo/*|.config/*)
      workspace_wide=1
      ;;
    tests/conformance/connector/*)
      name="$(cargo_name tests/conformance/connector)"
      [ -n "$name" ] && crates+=("$name")
      ;;
    tests/e2e-gateway/*|tests/e2e-hosted/*) e2e_note=1 ;;
    *) ;; # docs, scripts, deploy, plugins, images, schemas, justfile, .github, …
  esac
done
if [ "${#crates[@]}" -gt 0 ]; then
  mapfile -t crates < <(printf '%s\n' "${crates[@]}" | sort -u)
fi

# VEL-005: tests run on the touched crates (+ aurora-db and aurora-api when a
# migration changed: migrations change behaviour everywhere); clippy runs on
# the touched crates plus their workspace dependents.
test_crates=(${crates[@]+"${crates[@]}"})
test_notes=()
if [ "$migrations_changed" -eq 1 ]; then
  test_crates+=(aurora-db aurora-api)
  test_notes+=("aurora-db/migrations changed → also test aurora-db and aurora-api")
fi
if [ "${#test_crates[@]}" -gt 0 ]; then
  mapfile -t test_crates < <(printf '%s\n' "${test_crates[@]}" | sort -u)
fi
clippy_crates=(${crates[@]+"${crates[@]}"})
rdep_notes=() feat_crates=()
if [ "${#crates[@]}" -gt 0 ] && [ "$workspace_wide" -eq 0 ]; then
  scope_out="$(scope_info "${crates[@]}")" || die "cargo metadata failed — cannot expand the clippy scope to dependents"
  while read -r kind a b; do
    case "$kind" in
      RDEP) clippy_crates+=("$a"); rdep_notes+=("$a (depends on $b)") ;;
      FEAT) feat_crates+=("$a") ;;
    esac
  done <<<"$scope_out"
fi
# Inner loop: --all-features only when a touched crate has gating features.
# The workspace-wide set and --all keep it (CI parity).
clippy_features=()
clippy_feature_note="no --all-features: touched crates define no feature flags"
if [ "${#feat_crates[@]}" -gt 0 ]; then
  clippy_features=(--all-features)
  clippy_feature_note="--all-features: ${feat_crates[*]} define feature flags"
fi
use_nextest=0
have_nextest && use_nextest=1

# --- plan --------------------------------------------------------------------

echo "== quick gate (VEL-002) — development-loop tool, NOT a merge gate (CI runs the full suite)"
echo "branch $(git branch --show-current) @ $(git rev-parse --short HEAD); 1-min load $(load_1m); scope: $mode"
if [ "${#files[@]}" -eq 0 ]; then
  echo "changed files: none detected"
elif [ "$VERBOSE" = "1" ]; then
  printf 'changed files (%d):\n' "${#files[@]}"
  printf '  %s\n' ${files[@]+"${files[@]}"}
else
  echo "changed files: ${#files[@]} (AURORA_QUICK_GATE_VERBOSE=1 to list them)"
fi
[ "$e2e_note" -eq 1 ] && echo "note: e2e/Playwright paths changed — live-gated, CI runs them; the quick gate does not"

echo "plan:"
if [ "$ALL" -eq 1 ]; then
  echo "  --all: the old full pre-commit suite (fmt, workspace clippy, all non-e2e cargo tests,"
  echo "         web lint + typecheck + full vitest, architecture guards)"
elif [ "$WEB_ONLY" -eq 1 ]; then
  echo "  --web: tsc + vitest (changed/related files only)"
else
  [ "$TESTS_ONLY" -eq 0 ] && echo "  cargo fmt --all --check"
  [ "$schema_check" -eq 1 ] && echo "  python3 scripts/extract-schema.py --check   (data model touched)"
  if [ "$TESTS_ONLY" -eq 0 ]; then
    if [ "$workspace_wide" -eq 1 ]; then
      echo "  workspace-wide files changed → cargo clippy --workspace --all-targets --all-features -- -D warnings"
    elif [ "${#crates[@]}" -gt 0 ]; then
      echo "  cargo clippy ${clippy_crates[*]} --all-targets ${clippy_features[*]:-} -- -D warnings"
      echo "    touched: ${crates[*]}"
      [ "${#rdep_notes[@]}" -gt 0 ] && printf '    + dependent: %s\n' "${rdep_notes[@]}"
      echo "    ($clippy_feature_note; CI runs --all-features on the whole workspace)"
    else
      echo "  no crates mapped → core set: cargo clippy -p aurora-db -p aurora-protocol --all-targets -- -D warnings (no --all-features)"
    fi
  fi
  if [ "$CLIPPY_ONLY" -eq 0 ]; then
    plan_fixtures
    [ "$fixtures_needed" -eq 1 ] && echo "  fixture packages: bash scripts/fixtures/build.sh (plugins/fixtures/dist is gitignored)"
    if [ "$workspace_wide" -eq 1 ]; then
      echo "  cargo test --workspace --exclude aurora-e2e-gateway --exclude aurora-e2e-hosted"
    elif [ "${#test_crates[@]}" -gt 0 ]; then
      if [ "$use_nextest" -eq 1 ]; then
        echo "  cargo nextest run ${test_crates[*]}   (doctests are skipped by nextest; 'cargo test --doc -p <crate>' or 'just test' runs them)"
      else
        echo "  cargo test ${test_crates[*]}   (cargo-nextest not installed)"
      fi
      for n in ${test_notes[@]+"${test_notes[@]}"}; do echo "    $n"; done
    fi
  fi
  if [ "$web_changed" -eq 1 ] && [ "$CLIPPY_ONLY" -eq 0 ]; then
    [ "$TESTS_ONLY" -eq 0 ] && echo "  web: npx tsc --noEmit"
    [ "${#web_tests[@]}" -gt 0 ] && echo "  web: npx vitest run ${web_tests[*]}"
    [ "${#web_src[@]}" -gt 0 ] && echo "  web: npx vitest related ${web_src[*]}"
  fi
  if [ "$TESTS_ONLY" -eq 1 ] && [ "$workspace_wide" -eq 0 ] && [ "${#crates[@]}" -eq 0 ]; then
    echo "  (nothing to test: no crate or workspace-wide file in the changed set; the full suite remains CI's job)"
  fi
fi

if [ "$DRY_RUN" -eq 1 ]; then
  echo "(--dry-run: nothing executed)"
  exit 0
fi
disk_warning

# --- execute -----------------------------------------------------------------

if [ "$ALL" -eq 1 ]; then
  load_gate
  phase "cargo fmt --all --check" cargo fmt --all --check
  if [ "$TESTS_ONLY" -eq 0 ]; then
    phase "cargo clippy --workspace --all-targets --all-features -- -D warnings" \
      cargo clippy --workspace --all-targets --all-features -- -D warnings
  fi
  if [ "$CLIPPY_ONLY" -eq 0 ]; then
    plan_fixtures
    run_fixtures
    ensure_db_url
    phase "cargo test --workspace --exclude aurora-e2e-gateway --exclude aurora-e2e-hosted (8 test threads; shared dev Postgres)" \
      cargo test --workspace --exclude aurora-e2e-gateway --exclude aurora-e2e-hosted -- --test-threads=8
    [ -x web/node_modules/.bin/eslint ] || die "web dependencies missing: run 'cd web && pnpm install --frozen-lockfile' first"
    phase "web: npx eslint ." bash -c 'cd web && exec npx eslint .'
    if [ "$TESTS_ONLY" -eq 0 ]; then
      phase "web: npx tsc --noEmit" bash -c 'cd web && exec npx tsc --noEmit'
    fi
    phase "web: npx vitest run" bash -c 'cd web && exec npx vitest run'
  fi
  phase "architecture guards (bash scripts/guards/run-all.sh)" bash scripts/guards/run-all.sh
elif [ "$WEB_ONLY" -eq 1 ]; then
  load_gate
  run_web
else
  load_gate
  if [ "$TESTS_ONLY" -eq 0 ]; then
    phase "cargo fmt --all --check" cargo fmt --all --check
  fi
  if [ "$schema_check" -eq 1 ]; then
    phase "schema guard: python3 scripts/extract-schema.py --check" python3 scripts/extract-schema.py --check
  fi
  if [ "$TESTS_ONLY" -eq 0 ]; then
    if [ "$workspace_wide" -eq 1 ]; then
      phase "cargo clippy --workspace --all-targets --all-features -- -D warnings" \
        cargo clippy --workspace --all-targets --all-features -- -D warnings
    elif [ "${#crates[@]}" -gt 0 ]; then
      clippy_args=()
      for c in "${clippy_crates[@]}"; do clippy_args+=(-p "$c"); done
      phase "cargo clippy ${clippy_crates[*]} --all-targets ${clippy_features[*]:-} -- -D warnings" \
        cargo clippy "${clippy_args[@]}" --all-targets ${clippy_features[@]+"${clippy_features[@]}"} -- -D warnings
    else
      phase "core set: cargo clippy -p aurora-db -p aurora-protocol --all-targets -- -D warnings" \
        cargo clippy -p aurora-db -p aurora-protocol --all-targets -- -D warnings
    fi
  fi
  if [ "$CLIPPY_ONLY" -eq 0 ]; then
    if [ "$workspace_wide" -eq 1 ]; then
      plan_fixtures
      run_fixtures
      ensure_db_url
      phase "cargo test --workspace --exclude aurora-e2e-gateway --exclude aurora-e2e-hosted (8 test threads; shared dev Postgres)" \
        cargo test --workspace --exclude aurora-e2e-gateway --exclude aurora-e2e-hosted -- --test-threads=8
    elif [ "${#test_crates[@]}" -gt 0 ]; then
      plan_fixtures
      run_fixtures
      ensure_db_url
      test_args=()
      for c in "${test_crates[@]}"; do test_args+=(-p "$c"); done
      if [ "$use_nextest" -eq 1 ]; then
        phase "cargo nextest run ${test_crates[*]} (doctests skipped)" \
          cargo nextest run "${test_args[@]}" --no-tests=pass
      else
        phase "cargo test ${test_crates[*]}" cargo test "${test_args[@]}"
      fi
    fi
    run_web
  fi
fi

echo "quick gate passed in $(( $(date +%s) - T0 ))s (target ≤ 3 min for a single-crate change; the full suite remains CI's job)"
