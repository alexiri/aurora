#!/usr/bin/env bash
# M2 acceptance: real aurora-api + 2x aurora-gateway + Postgres + mock upstreams (TEST-012).
# The binaries hard-code ports 4101/4111, 4102/4103 and 4104 (hook host); the suite isolates itself by
# binding them on 127.0.0.31/.32/.33/.34. Tests run serially; each creates and drops its own database.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
# One run per host, across worktrees: the fixed addresses below are host-wide, and the port check
# alone misses a run that is still building (nothing bound yet). Waits for the other run.
exec 9>/tmp/aurora-e2e-gateway.lock   # fixed path: TMPDIR differs between sessions
if ! flock -n 9; then
  echo "another scripts/e2e/gateway.sh run holds the lock; waiting (up to 1 h)..." >&2
  flock -w 3600 9 || { echo "gave up waiting for the gateway e2e lock" >&2; exit 1; }
fi
for ip in 127.0.0.31 127.0.0.32 127.0.0.33 127.0.0.34; do
  for port in 4101 4102 4103 4104 4111; do
    if ss -ltn "sport = :$port" | grep -q "$ip:"; then echo "$ip:$port already in use" >&2; exit 1; fi
  done
done
cargo build -p aurora-api -p aurora-gateway -p aurora-hookhost || { echo "cargo build failed" >&2; exit 1; }

# P2-DBL-1: the database-lane scenario needs a PostgreSQL to connect *to* that is not the platform
# database (the dial guard refuses the platform database by address and port). A throw-away
# container, named after the worktree, on a random loopback port; set AURORA_E2E_DB_TARGET
# (host:port, user/password/db `aurora`/`aurora`/`postgres`) to use your own instead.
if [ -z "${AURORA_E2E_DB_TARGET:-}" ] && ! command -v podman >/dev/null 2>&1; then
  echo "podman not found and AURORA_E2E_DB_TARGET unset: the database-lane scenario will be skipped" >&2
elif [ -z "${AURORA_E2E_DB_TARGET:-}" ]; then
  PG="$(basename "$ROOT")-gwe2e-dbtarget"
  trap 'podman rm -f "$PG" >/dev/null 2>&1' EXIT
  podman run -d --name "$PG" -p 127.0.0.1::5432 \
    -e POSTGRES_USER=aurora -e POSTGRES_PASSWORD=aurora -e POSTGRES_DB=postgres \
    docker.io/library/postgres:16 -c fsync=off >/dev/null || { echo "cannot start $PG" >&2; exit 1; }
  for _ in $(seq 60); do
    podman exec "$PG" pg_isready -h 127.0.0.1 -U aurora >/dev/null 2>&1 && break
    sleep 1
  done
  AURORA_E2E_DB_TARGET="$(podman port "$PG" 5432/tcp | head -n1)"
  export AURORA_E2E_DB_TARGET
fi

cargo test -p aurora-e2e-gateway -- --test-threads=1 "$@"
rc=$?
exit $rc
