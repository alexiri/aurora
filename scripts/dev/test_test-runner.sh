#!/usr/bin/env bash
# Shell test of the test scripts (FIX-255): test.sh, test-pg.sh and heavy.sh. No root, no real
# podman or cargo: stubs for podman, cargo, npx, nproc, uptime and sleep come first on PATH, and
# the heavy slots and the test-pg lock live in a temporary directory.
# Run: bash scripts/dev/test_test-runner.sh   (part of scripts/guards/run-all.sh)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TEST_SH="$ROOT/scripts/dev/test.sh"
TEST_PG="$ROOT/scripts/dev/test-pg.sh"
HEAVY="$ROOT/scripts/dev/heavy.sh"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
STUBS="$TMP/bin"
STATE="$TMP/state"
mkdir -p "$STUBS" "$STATE"

# --- stubs -------------------------------------------------------------------

# A one-container podman: state/container holds "running" or "stopped".
cat >"$STUBS/podman" <<'EOF'
#!/usr/bin/env bash
echo "$*" >>"$STUB_STATE/podman.log"
c="$STUB_STATE/container"
case "$1" in
  container) [ -f "$c" ] ;;
  inspect) [ "$(cat "$c" 2>/dev/null)" = running ] && echo true || echo false ;;
  exec) [ ! -f "$STUB_STATE/unhealthy" ] && [ "$(cat "$c" 2>/dev/null)" = running ] ;;
  port) echo "127.0.0.1:$(cat "$STUB_STATE/port")" ;;
  run)
    [ ! -f "$c" ] || { echo "name in use" >&2; exit 125; }
    /bin/sleep 0.3
    echo "$*" | sed -E 's/.*-p 127\.0\.0\.1:([0-9]+):5432.*/\1/' >"$STUB_STATE/port"
    echo running >"$c"
    rm -f "$STUB_STATE/unhealthy"
    echo run >>"$STUB_STATE/runs" ;;
  rm) rm -f "$c" ;;
  ps)
    n="$(cat "$STUB_STATE/others" 2>/dev/null || echo 0)"
    for ((i = 0; i < n; i++)); do echo "id$i"; done ;;
  *) echo "stub podman: unexpected $*" >&2; exit 99 ;;
esac
EOF

# cargo logs its arguments and both URLs, notes any overlap with another cargo, and fails for the
# crate named in STUB_FAIL.
cat >"$STUBS/cargo" <<'EOF'
#!/usr/bin/env bash
if [ "$1" = metadata ]; then
  echo '{"packages":[{"name":"x-one"},{"name":"aurora-e2e-gateway"},{"name":"x-two"},{"name":"aurora-e2e-hosted"}]}'
  exit 0
fi
mkdir "$STUB_STATE/running" 2>/dev/null || echo overlap >>"$STUB_STATE/overlap"
crate=""
prev=""
for a in "$@"; do [ "$prev" = -p ] && crate="$a"; prev="$a"; done
echo "start $crate" >>"$STUB_STATE/order"
echo "$*" >>"$STUB_STATE/cargo.log"
echo "AURORA_TEST_DATABASE_URL=${AURORA_TEST_DATABASE_URL-} DATABASE_URL=${DATABASE_URL-}" >>"$STUB_STATE/urls"
echo "held=${AURORA_HEAVY_HELD-} status=$(cat "$AURORA_HEAVY_DIR"/.heavy.status 2>/dev/null)" >>"$STUB_STATE/slot"
/bin/sleep 0.2
echo "end $crate" >>"$STUB_STATE/order"
rmdir "$STUB_STATE/running"
[ "${STUB_FAIL-}" != "$crate" ] || exit 100
EOF

cat >"$STUBS/npx" <<'EOF'
#!/usr/bin/env bash
echo "$(basename "$PWD"): $*" >>"$STUB_STATE/npx.log"
EOF

cat >"$STUBS/nproc" <<'EOF'
#!/usr/bin/env bash
echo 4
EOF

# The load is the first line of state/loads, consumed while more than one line is left.
cat >"$STUBS/uptime" <<'EOF'
#!/usr/bin/env bash
f="$STUB_STATE/loads"
l="$(head -n1 "$f" 2>/dev/null || echo 0.50)"
if [ "$(wc -l <"$f" 2>/dev/null || echo 0)" -gt 1 ]; then sed 1d "$f" >"$f.new" && mv "$f.new" "$f"; fi
echo " 10:00:00 up 1 day,  1 user,  load average: $l, 1.00, 1.00"
EOF

cat >"$STUBS/sleep" <<'EOF'
#!/usr/bin/env bash
echo "$1" >>"$STUB_STATE/sleeps"
EOF
chmod +x "$STUBS"/*

export PATH="$STUBS:$PATH"
export STUB_STATE="$STATE"
export AURORA_HEAVY_DIR="$TMP/heavy"
export AURORA_TEST_PG_LOCK="$TMP/test-pg.lock"
mkdir -p "$AURORA_HEAVY_DIR"
# The caller's environment must not change what is tested.
unset AURORA_TEST_DATABASE_URL DATABASE_URL AURORA_DATABASE_URL PGPORT STUB_FAIL
unset AURORA_TEST_THREADS AURORA_TEST_PG_PORT AURORA_HEAVY_HELD

# --- helpers -----------------------------------------------------------------

passed=0
failures=0
ok() { passed=$((passed + 1)); echo "ok   $1"; }
bad() { failures=$((failures + 1)); echo "FAIL $1"; sed 's/^/       /' "$TMP/out" 2>/dev/null || true; }
check() { local name="$1"; shift; if "$@"; then ok "$name"; else bad "$name"; fi; }

reset() { rm -rf "$STATE"; mkdir -p "$STATE"; }
rc=0
run() { rc=0; "$@" >"$TMP/out" 2>&1 || rc=$?; }
out_has() { grep -qF -- "$1" "$TMP/out"; }
file_has() { grep -qF -- "$2" "$STATE/$1" 2>/dev/null; }
no_file() { [ ! -e "$STATE/$1" ]; }
count() { if [ -f "$STATE/$1" ]; then grep -c -- "$2" "$STATE/$1" || true; else echo 0; fi; }

# refused <name> <message part> <command...>: exit 2, one sentence on one line, nothing started.
refused() {
  local name="$1" want="$2"
  shift 2
  reset
  run "$@"
  if [ "$rc" -eq 2 ] && out_has "$want" && [ "$(grep -c 'refused:' "$TMP/out")" -eq 1 ] \
      && no_file cargo.log && no_file npx.log && no_file runs; then
    ok "refuses $name"
    grep -F 'refused:' "$TMP/out" | sed 's/^/       /'
  else
    bad "refuses $name (exit $rc)"
  fi
}

URL=postgres://aurora:aurora@127.0.0.1:55990/aurora
DEV=postgres://aurora:aurora@127.0.0.1:55432/aurora

# --- refusals ----------------------------------------------------------------

refused "rust without a crate" "no crate given" "$TEST_SH" lane rust
refused "a filter without a crate" "no crate given" "$TEST_SH" lane rust some_test
refused "a cargo-style argument list without a crate" "no crate given" "$TEST_SH" lane rust cargo nextest run
refused "--workspace" "workspace-wide" "$TEST_SH" lane rust --workspace
refused "--workspace next to a crate" "workspace-wide" "$TEST_SH" lane rust -p x-one --workspace
refused "--all" "workspace-wide" "$TEST_SH" lane rust -p x-one --all
refused "a crate glob" "not a plain crate name" "$TEST_SH" lane rust -p 'aurora-*'
refused "a second -p" "once" "$TEST_SH" lane rust -p x-one -p x-two
refused "an attached second -p" "once" "$TEST_SH" lane rust -p x-one -px-two
refused "an empty entry in --crates" "not a plain crate name" "$TEST_SH" lane rust --crates x-one,,x-two
refused "-p without a name" "needs a crate name" "$TEST_SH" lane rust -p
for flag in "--test-threads 9" "--test-threads=64" "-j 16" "-j16" "-j=9" "--jobs=12" "-j num-cpus" "-j -1" "-j 0"; do
  # shellcheck disable=SC2086 # the flag and its value are split on purpose
  refused "$flag" "test threads" "$TEST_SH" lane rust -p x-one $flag
done
refused "AURORA_TEST_THREADS above the cap" "test threads" env AURORA_TEST_THREADS=32 "$TEST_SH" lane rust -p x-one
refused "DATABASE_URL on 55432" "55432" env "DATABASE_URL=$DEV" "$TEST_SH" lane rust -p x-one
refused "AURORA_TEST_DATABASE_URL on 55432" "55432" env "AURORA_TEST_DATABASE_URL=postgres://u:p@localhost:55432/x" "$TEST_SH" lane rust -p x-one
refused "any other URL variable on 55432" "55432" env "SOME_URL=postgres://u:p@127.0.0.1:55432" "$TEST_SH" lane rust -p x-one
refused "a port=55432 DSN" "55432" env "SOME_DSN=host=localhost port=55432" "$TEST_SH" lane rust -p x-one
refused "PGPORT=55432" "55432" env PGPORT=55432 "$TEST_SH" lane rust -p x-one
refused "a 55432 URL in the arguments" "55432" "$TEST_SH" lane rust -p x-one "$DEV"
refused "a 55432 URL for web" "55432" env "DATABASE_URL=$DEV" "$TEST_SH" lane web vitest
refused "a run inside a heavy slot" "already running inside a heavy slot" "$HEAVY" outer "$TEST_SH" lane rust -p x-one
refused "a missing lane name" "lane" "$TEST_SH" rust -p x-one
refused "an unknown kind" "rust' or 'web" "$TEST_SH" lane go ./...
refused "vitest with its own worker count" "number of workers" "$TEST_SH" lane web vitest --maxWorkers=16
refused "Playwright with its own worker count" "number of workers" "$TEST_SH" lane web playwright e2e/a.spec.ts --workers 8
refused "vitest watch mode" "heavy slot open" "$TEST_SH" lane web vitest --watch
refused "Playwright without a spec" "needs a spec first" "$TEST_SH" lane web playwright --project=mocked

reset; run env AURORA_TEST_PG_PORT=55432 "$TEST_PG" up
check "test-pg.sh refuses port 55432" test "$rc" -eq 2
check "test-pg.sh started nothing on 55432" no_file runs

# --- one crate ---------------------------------------------------------------

reset; run "$TEST_SH" my-lane rust -p x-one some_filter
check "one crate passes" test "$rc" -eq 0
check "nextest gets the crate, 4 threads and the filter" file_has cargo.log "nextest run -p x-one --test-threads 4 some_filter"
check "both URLs are the shared server's" file_has urls "AURORA_TEST_DATABASE_URL=$URL DATABASE_URL=$URL"
check "the run holds a heavy slot through heavy.sh" file_has slot "held=$AURORA_HEAVY_DIR status=HOLDER slot 0"
check "the slot status names the lane" file_has slot "my-lane pid="
check "the URLs are on the command given to heavy.sh" file_has slot "env AURORA_TEST_DATABASE_URL"
check "the summary names crate and exit code" out_has "x-one: exit 0,"

reset
run env DATABASE_URL=postgres://u:p@db.example:5432/x "$TEST_SH" lane rust -p x-one
check "the shared URL overrides a URL from the environment" file_has urls "DATABASE_URL=$URL"

reset; run env AURORA_TEST_THREADS=8 "$TEST_SH" lane rust -p x-one
check "AURORA_TEST_THREADS=8 is honoured" file_has cargo.log "--test-threads 8"

reset; run "$TEST_SH" lane rust -p x-one -j 2
check "-j 2 is passed through" file_has cargo.log "-j 2"
check "and no second thread flag is added" test "$(count cargo.log -- '--test-threads')" -eq 0

reset; STUB_FAIL=x-one run "$TEST_SH" lane rust -p x-one
check "a failing run exits 1" test "$rc" -eq 1
check "the summary shows nextest's exit code" out_has "x-one: exit 100,"

# --- several crates ----------------------------------------------------------

reset; run "$TEST_SH" lane rust --crates x-a,x-b
check "--crates passes" test "$rc" -eq 0
check "--crates runs a then b, one after the other" \
  test "$(tr '\n' ';' <"$STATE/order")" = "start x-a;end x-a;start x-b;end x-b;"
check "--crates never runs two at once" no_file overlap
check "each crate is its own nextest run" test "$(count cargo.log 'nextest run -p ')" -eq 2
check "several crates tolerate a crate without tests" file_has cargo.log "--no-tests=pass"

reset; STUB_FAIL=x-a run "$TEST_SH" lane rust --crates x-a,x-b
check "the first failure stops the run" test "$rc" -eq 1 -a "$(count order 'start ')" -eq 1
check "and the rest is reported as not run" out_has "x-b: not run"

reset; STUB_FAIL=x-a run "$TEST_SH" lane rust --crates x-a,x-b --keep-going
check "--keep-going runs the rest and still exits 1" test "$rc" -eq 1 -a "$(count order 'start ')" -eq 2

reset; run "$TEST_SH" lane rust --all-crates
check "--all-crates runs each member except the e2e crates, one at a time" \
  test "$(tr '\n' ';' <"$STATE/order")" = "start x-one;end x-one;start x-two;end x-two;"

check "--all-crates warns about nothing" test "$(grep -c 'warning:' "$TMP/out" || true)" -eq 0

reset; run "$TEST_SH" lane rust -p aurora-e2e-gateway
check "a named e2e crate runs, in a heavy slot" test "$rc" -eq 0 -a "$(count slot 'HOLDER')" -eq 1
check "with one warning line that it starts its own stack" \
  test "$(grep -c 'warning: aurora-e2e-gateway starts its own stack' "$TMP/out" || true)" -eq 1
reset; run "$TEST_SH" lane rust --crates x-a,aurora-e2e-hosted
check "the same inside --crates" out_has "warning: aurora-e2e-hosted starts its own stack"

# Two lanes at once share one server.
reset
"$TEST_SH" lane-a rust -p x-a >"$TMP/out-a" 2>&1 &
a=$!
"$TEST_SH" lane-b rust -p x-b >"$TMP/out" 2>&1 &
b=$!
ra=0; wait "$a" || ra=$?
rb=0; wait "$b" || rb=$?
check "two lanes at the same time both pass" test "$ra" -eq 0 -a "$rb" -eq 0
check "and start one server" test "$(count runs run)" -eq 1

# --- test-pg.sh --------------------------------------------------------------

reset
run "$TEST_PG" up
run "$TEST_PG" up
check "up twice starts one container" test "$(count runs run)" -eq 1
check "the data directory is a tmpfs" file_has podman.log "--tmpfs /var/lib/postgresql/data"
check "no volume is mounted" test "$(grep -cE -- ' (-v|--volume|--mount) ' "$STATE/podman.log" || true)" -eq 0
check "it binds the fixed port on loopback" file_has podman.log "-p 127.0.0.1:55990:5432"
check "max_connections is raised" file_has podman.log "max_connections=300"
run "$TEST_PG" url
check "url prints the shared URL" out_has "$URL"
run env AURORA_TEST_PG_PORT=56001 "$TEST_PG" url
check "a running server keeps its port" out_has "$URL"
run "$TEST_PG" status
check "status is 0 when ready" test "$rc" -eq 0

reset
"$TEST_PG" up >"$TMP/out-a" 2>&1 &
a=$!
"$TEST_PG" up >"$TMP/out" 2>&1 &
b=$!
ra=0; wait "$a" || ra=$?
rb=0; wait "$b" || rb=$?
check "two concurrent ups both succeed" test "$ra" -eq 0 -a "$rb" -eq 0
check "two concurrent ups start one container" test "$(count runs run)" -eq 1

echo stopped >"$STATE/container"
run "$TEST_PG" up
check "a stopped container is recreated" test "$rc" -eq 0 -a "$(count runs run)" -eq 2
touch "$STATE/unhealthy"
run "$TEST_PG" up
check "an unhealthy container is recreated after the bounded wait" test "$rc" -eq 0 -a "$(count runs run)" -eq 3
check "the wait was bounded at 60 probes" test "$(count sleeps '^1$')" -eq 60
check "only the test server is ever removed" \
  test "$(grep '^rm ' "$STATE/podman.log" | grep -vc ' aurora-test-pg$' || true)" -eq 0
check "no rm removes volumes" test "$(grep -cE '^(rm .*(-v|--volumes)( |$)|volume )' "$STATE/podman.log" || true)" -eq 0
run "$TEST_PG" down
check "down removes it" test "$rc" -eq 0 -a ! -e "$STATE/container"
run "$TEST_PG" status
check "status is 1 when absent" test "$rc" -eq 1

# --- waiting -----------------------------------------------------------------

reset; printf '9.00\n12.50\n1.00\n' >"$STATE/loads"
run "$TEST_SH" lane rust -p x-one
check "high load: it waits, then runs" test "$rc" -eq 0 -a "$(count sleeps '^30$')" -eq 2
check "and says why" out_has "load 9.00 is above 2 x 4 cores"

reset; echo 8.00 >"$STATE/loads"
run "$TEST_SH" lane rust -p x-one
check "a load of exactly 2 x nproc does not wait" test "$rc" -eq 0 -a "$(count sleeps '^30$')" -eq 0

reset; echo 50.00 >"$STATE/loads"
run "$TEST_SH" lane rust --crates x-a,x-b --keep-going
check "load that stays high: exit 3 after 30 minutes" test "$rc" -eq 3 -a "$(count sleeps '^30$')" -eq 60
check "and nothing ran" no_file cargo.log

reset; echo 11 >"$STATE/others"
run "$TEST_SH" lane rust -p x-one
check "more than 10 containers: exit 3, nothing runs" test "$rc" -eq 3 -a ! -e "$STATE/cargo.log"
check "and says why" out_has "11 podman containers are running"
reset; echo 10 >"$STATE/others"
run "$TEST_SH" lane rust -p x-one
check "10 containers is fine" test "$rc" -eq 0

# --- web ---------------------------------------------------------------------

reset; run "$TEST_SH" lane web vitest tests/a.test.ts
check "vitest runs in web/ with 4 workers" file_has npx.log "web: vitest run --maxWorkers=4 tests/a.test.ts"
reset; run "$TEST_SH" lane web playwright e2e/a.spec.ts --project=mocked
check "Playwright gets the spec before the flags and 2 workers" \
  file_has npx.log "web: playwright test e2e/a.spec.ts --project=mocked --workers=2"
reset; echo 50.00 >"$STATE/loads"; run "$TEST_SH" lane web vitest
check "web waits for the machine too" test "$rc" -eq 3 -a ! -e "$STATE/npx.log"

# --- heavy.sh ----------------------------------------------------------------

reset
# shellcheck disable=SC2016 # expanded by the inner shell, under heavy.sh
run "$HEAVY" lane sh -c 'echo "$AURORA_HEAVY_HELD"; exit 7'
check "heavy.sh returns the command's exit code" test "$rc" -eq 7
check "heavy.sh uses AURORA_HEAVY_DIR" out_has "$AURORA_HEAVY_DIR"
check "and records the last run there" grep -qF "last: lane rc=7" "$AURORA_HEAVY_DIR/.heavy.status"
run "$HEAVY" lane sh -c '[ ! -e /proc/$$/fd/9 ]'
check "the slot descriptor is closed for the command" test "$rc" -eq 0

echo
echo "test-runner shell test: $passed passed, $failures failed"
[ "$failures" -eq 0 ]
