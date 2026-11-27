#!/usr/bin/env bash
# THE way to run tests on the shared development machine (FIX-255, D-491). It does the safe thing
# by itself: one shared test Postgres (test-pg.sh), a heavy slot per run (heavy.sh), capped test
# threads, one crate at a time, and it waits while the machine is loaded. CI does not use it.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HEAVY="$ROOT/scripts/dev/heavy.sh"
TEST_PG="$ROOT/scripts/dev/test-pg.sh"
HEAVY_DIR=${AURORA_HEAVY_DIR:-/home/alex/orca/workspaces/aurora}

DEFAULT_THREADS=4
MAX_THREADS=8
VITEST_WORKERS=4
PLAYWRIGHT_WORKERS=2
MAX_CONTAINERS=10
POLL_SECS=30
MAX_POLLS=60     # 30 minutes
# Never part of --all-crates: they bind fixed addresses, start containers and are not env-gated.
# Run them with scripts/e2e/gateway.sh and scripts/e2e/hosted.sh.
E2E_CRATES="aurora-e2e-gateway aurora-e2e-hosted"

usage() {
  cat <<'EOF'
usage:
  scripts/dev/test.sh <lane> rust -p <crate> [nextest filters/args...]      one crate
  scripts/dev/test.sh <lane> rust --crates <a,b,c> [--keep-going] [args...]  one after another
  scripts/dev/test.sh <lane> rust --all-crates [--keep-going] [args...]      every workspace
                                       member except the e2e-gateway/e2e-hosted crates, one at a time
  scripts/dev/test.sh <lane> web vitest [files...]
  scripts/dev/test.sh <lane> web playwright <spec> --project=mocked

<lane> is your lane's name (it shows in the heavy slot's status file). The e2e scripts
(scripts/e2e/*.sh) are not run by this script: run them through scripts/dev/heavy.sh.
Rust tests use the shared
test Postgres (scripts/dev/test-pg.sh) and AURORA_TEST_THREADS test threads (default 4, max 8).
Each run takes its own heavy slot (do not wrap this script in heavy.sh) and waits while the
1-minute load is above 2 x nproc or more than 10 podman containers are running.
Exit: 0 all passed; 1 a run failed; 2 refused; 3 gave up waiting for the machine.
EOF
}

refuse() { echo "test.sh: refused: $*" >&2; exit 2; }

# Anything that names port 55432: a URL (…:55432) or a key=value DSN (port=55432).
points_at_dev_db() {
  printf '%s' "$1" | grep -Eq '(://[^[:space:]]*:55432([^0-9]|$))|(port=55432([^0-9]|$))'
}

check_no_dev_db() {
  local a name
  for a in "$@"; do
    ! points_at_dev_db "$a" || refuse "an argument points at port 55432, the owner's dev database; tests use the shared server of scripts/dev/test-pg.sh."
  done
  while IFS= read -r name; do
    ! points_at_dev_db "${!name-}" || refuse "\$$name points at port 55432, the owner's dev database; unset it (scripts/dev/test.sh sets the test database URL itself)."
  done < <(compgen -e)
  [ "${PGPORT-}" != 55432 ] || refuse "\$PGPORT is 55432, the owner's dev database; unset it."
}

# --- waiting -----------------------------------------------------------------

load_1m() { LC_ALL=C uptime | sed -n 's/.*load average: *\([0-9.]*\),.*/\1/p'; }

# Wait until the machine has room; exit 3 after 30 minutes.
wait_for_room() {
  local polls=0 cores load containers why ids
  cores="$(nproc)"
  while :; do
    why=""
    load="$(load_1m)"
    [ -n "$load" ] || refuse "cannot read the load average from uptime."
    if command -v podman >/dev/null 2>&1; then
      ids="$(podman ps -q)" || refuse "podman ps failed, so the number of running containers is unknown."
      containers="$(printf '%s' "$ids" | grep -c . || true)"
    else
      containers=0
    fi
    if awk -v l="$load" -v m="$((cores * 2))" 'BEGIN { exit !(l > m) }'; then
      why="1-minute load $load is above 2 x $cores cores"
    elif [ "$containers" -gt "$MAX_CONTAINERS" ]; then
      why="$containers podman containers are running (more than $MAX_CONTAINERS)"
    fi
    [ -n "$why" ] || return 0
    if [ "$polls" -ge "$MAX_POLLS" ]; then
      echo "test.sh: gave up after 30 minutes: $why." >&2
      return 3
    fi
    echo "test.sh: waiting ${POLL_SECS}s: $why." >&2
    polls=$((polls + 1))
    sleep "$POLL_SECS"
  done
}

# --- running and reporting ---------------------------------------------------

summary=()
failed=0

# run_one <label> <command...>: wait for room, run in a heavy slot, record the exit code.
# Returns the command's exit code (3 when the machine never had room).
run_one() {
  local label="$1" rc=0 t0
  shift
  wait_for_room || { summary+=("$label: not run (the machine stayed loaded), exit 3"); return 3; }
  echo "test.sh: running $label: $*"
  t0="$(date +%s)"
  "$HEAVY" "$LANE" "$@" || rc=$?
  summary+=("$label: exit $rc, $(($(date +%s) - t0))s")
  [ "$rc" -eq 0 ] || failed=1
  return "$rc"
}

report() {
  echo "test.sh: summary (lane $LANE)"
  printf '  %s\n' "${summary[@]}"
}

# --- rust --------------------------------------------------------------------

workspace_crates() {
  local json
  json="$(cd "$ROOT" && cargo metadata --no-deps --format-version 1)" || return 1
  printf '%s' "$json" | python3 -I -c '
import json, sys
skip = set(sys.argv[1].split())
for p in json.load(sys.stdin)["packages"]:
    if p["name"] not in skip:
        print(p["name"])
' "$E2E_CRATES"
}

# A plain crate name: nextest's -p also takes globs ("aurora-*" is the whole workspace).
valid_crate() {
  case "$1" in
    ''|-*|*[!A-Za-z0-9_-]*) return 1 ;;
  esac
}

check_threads() {
  case "$1" in
    ''|*[!0-9]*) refuse "the number of test threads must be a whole number from 1 to $MAX_THREADS, got '$1'." ;;
  esac
  if ! { [ "${#1}" -le 3 ] && [ "$1" -ge 1 ] && [ "$1" -le "$MAX_THREADS" ]; }; then
    refuse "$1 test threads is outside 1 to $MAX_THREADS, the cap on this machine."
  fi
}

run_rust() {
  local selector="" list="" keep_going=0 user_threads=0 arg value
  local -a passthrough=() crates=()

  while [ "$#" -gt 0 ]; do
    arg="$1"; shift
    case "$arg" in
      --workspace|--workspace=*|--all)
        refuse "a workspace-wide run is not allowed; name crates with -p or --crates, or use --all-crates (one crate at a time)." ;;
      -p|--package|--crates)
        [ -z "$selector" ] || refuse "give one of -p, --crates or --all-crates, once; for several crates use --crates a,b,c."
        [ "$#" -gt 0 ] || refuse "$arg needs a crate name."
        selector="$arg"; list="$1"; shift ;;
      --package=*|--crates=*)
        [ -z "$selector" ] || refuse "give one of -p, --crates or --all-crates, once; for several crates use --crates a,b,c."
        selector="${arg%%=*}"; list="${arg#*=}" ;;
      -p?*)
        [ -z "$selector" ] || refuse "give one of -p, --crates or --all-crates, once; for several crates use --crates a,b,c."
        selector=-p; list="${arg#-p}"; list="${list#=}" ;;
      --all-crates)
        [ -z "$selector" ] || refuse "give one of -p, --crates or --all-crates, once; for several crates use --crates a,b,c."
        selector="$arg" ;;
      --keep-going) keep_going=1 ;;
      -j|--test-threads|--jobs)
        [ "$#" -gt 0 ] || refuse "$arg needs a number."
        check_threads "$1"
        user_threads=1; passthrough+=("$arg" "$1"); shift ;;
      -j?*|--test-threads=*|--jobs=*)
        case "$arg" in
          --*) value="${arg#*=}" ;;
          *) value="${arg#-j}"; value="${value#=}" ;;
        esac
        check_threads "$value"
        user_threads=1; passthrough+=("$arg") ;;
      *) passthrough+=("$arg") ;;
    esac
  done

  case "$selector" in
    "") refuse "no crate given; use -p <crate>, --crates <a,b,c> or --all-crates." ;;
    -p|--package) crates=("$list") ;;
    --crates)
      local IFS=,
      read -r -a crates <<<"$list"
      unset IFS ;;
    --all-crates)
      local names
      names="$(workspace_crates)" || refuse "cargo metadata failed, so the list of workspace crates is unknown."
      mapfile -t crates <<<"$names" ;;
  esac
  [ "${#crates[@]}" -gt 0 ] || refuse "the crate list is empty."
  local c
  for c in "${crates[@]}"; do
    valid_crate "$c" || refuse "'$c' is not a plain crate name (no globs, no empty entries)."
  done
  # Only reachable by naming one: --all-crates leaves them out.
  for c in "${crates[@]}"; do
    case " $E2E_CRATES " in
      *" $c "*) echo "test.sh: warning: $c starts its own stack (containers, fixed addresses; scripts/e2e/ is its usual runner); it runs in one heavy slot like any crate." >&2 ;;
    esac
  done

  local -a threads=()
  if [ "$user_threads" -eq 0 ]; then
    local n="${AURORA_TEST_THREADS:-$DEFAULT_THREADS}"
    check_threads "$n"
    threads=(--test-threads "$n")
  fi
  # With several crates, a crate without tests (or without a match for the filter) is not a failure.
  local -a no_tests=()
  case "$selector" in
    --crates|--all-crates) no_tests=(--no-tests=pass) ;;
  esac

  cd "$ROOT"
  local url rc i=0
  for c in "${crates[@]}"; do
    i=$((i + 1))
    "$TEST_PG" up || { echo "test.sh: the shared test Postgres is not available." >&2; summary+=("$c: not run (no test database)"); failed=1; break; }
    url="$("$TEST_PG" url)"
    rc=0
    # Both URLs on the command itself, so they win over any .env.
    run_one "$c" env "AURORA_TEST_DATABASE_URL=$url" "DATABASE_URL=$url" \
      cargo nextest run -p "$c" \
      ${threads[@]+"${threads[@]}"} ${no_tests[@]+"${no_tests[@]}"} \
      ${passthrough[@]+"${passthrough[@]}"} || rc=$?
    # 3: no room; 130/143: interrupted. Neither continues, --keep-going or not.
    if [ "$rc" -eq 3 ] || [ "$rc" -eq 130 ] || [ "$rc" -eq 143 ] || { [ "$rc" -ne 0 ] && [ "$keep_going" -eq 0 ]; }; then
      local skipped
      for skipped in "${crates[@]:$i}"; do summary+=("$skipped: not run"); done
      report
      [ "$rc" -eq 3 ] && exit 3
      exit 1
    fi
  done
  report
  exit "$failed"
}

# --- web ---------------------------------------------------------------------

run_web() {
  local suite="${1:-}" arg rc=0
  [ "$#" -gt 0 ] && shift
  case "$suite" in
    vitest|playwright) ;;
    *) refuse "web needs 'vitest [files...]' or 'playwright <spec> --project=mocked'." ;;
  esac
  for arg in "$@"; do
    case "$arg" in
      --maxWorkers*|--minWorkers*|--max-workers*|--min-workers*|--workers*|-j*|--fully-parallel|--poolOptions*)
        refuse "'$arg': this script sets the number of workers (vitest $VITEST_WORKERS, Playwright $PLAYWRIGHT_WORKERS)." ;;
      --watch|--watch=*|-w|--ui|--ui=*|--ui-*|--debug|--headed)
        refuse "'$arg' would keep a heavy slot open; run only runs that end by themselves." ;;
    esac
  done
  if [ "$suite" = playwright ]; then
    case "${1:-}" in
      ''|-*) refuse "playwright needs a spec first (then the flags, e.g. --project=mocked)." ;;
    esac
  fi
  [ -d "$ROOT/web" ] || refuse "there is no web/ directory."
  if [ "$suite" = vitest ]; then
    run_one "web vitest ${*:-(all files)}" env -C "$ROOT/web" npx vitest run "--maxWorkers=$VITEST_WORKERS" "$@" || rc=$?
  else
    run_one "web playwright $*" env -C "$ROOT/web" npx playwright test "$@" "--workers=$PLAYWRIGHT_WORKERS" || rc=$?
  fi
  report
  [ "$rc" -eq 3 ] && exit 3
  exit "$failed"
}

# --- main --------------------------------------------------------------------

case "${1:-}" in
  -h|--help) usage; exit 0 ;;
esac
[ "$#" -ge 2 ] || { usage >&2; exit 2; }
LANE="$1"; KIND="$2"; shift 2
case "$LANE" in
  -*|rust|web) usage >&2; refuse "the first argument is your lane's name, got '$LANE'." ;;
esac
case "$KIND" in
  rust|web) ;;
  *) usage >&2; refuse "the second argument must be 'rust' or 'web', got '$KIND'." ;;
esac
case "${1:-}" in
  -h|--help) usage; exit 0 ;;
esac
[ "${AURORA_HEAVY_HELD:-}" != "$HEAVY_DIR" ] \
  || refuse "this is already running inside a heavy slot; call scripts/dev/test.sh directly (it takes its own slot, and waiting for a second one can deadlock)."
check_no_dev_db "$@"

if [ "$KIND" = rust ]; then run_rust "$@"; else run_web "$@"; fi
