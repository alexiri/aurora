#!/usr/bin/env bash
# The ONE shared test Postgres of this machine (FIX-255, D-491). Every lane, run and crate uses it;
# sqlx::test and aurora-testkit create a scratch database per test, so they do not collide.
#
#   scripts/dev/test-pg.sh up       start it if needed (idempotent, safe to call concurrently)
#   scripts/dev/test-pg.sh url      print its URL
#   scripts/dev/test-pg.sh status   print its state; exit 0 only when it accepts connections
#   scripts/dev/test-pg.sh down     remove it (kills the test runs that are using it)
#
# Port: AURORA_TEST_PG_PORT (default 55990). It applies when the container is created; a server
# that is already running keeps its port, and `url` prints the real one.
# Data lives on a tmpfs mounted over the image's PGDATA, which the image declares as a VOLUME:
# without the mount podman creates an anonymous volume per container (about 1,900 leaked in two
# days in Nov 2026, which exhausted podman's lock table). This container has no volume of any kind.
# The script only ever touches the container named below.
set -euo pipefail

NAME=aurora-test-pg
IMAGE=docker.io/library/postgres:16
PGDATA=/var/lib/postgresql/data
MAX_CONNECTIONS=300
PORT=${AURORA_TEST_PG_PORT:-55990}
# One lock per user, at a fixed path: rootless podman's containers are per user too.
# (AURORA_TEST_PG_LOCK exists for the shell test only.)
LOCK=${AURORA_TEST_PG_LOCK:-/tmp/aurora-test-pg.$(id -u).lock}
LOCK_WAIT_SECS=300
READY_TRIES=60   # one per second

die() { echo "test-pg: $*" >&2; exit 1; }
usage() { echo "usage: scripts/dev/test-pg.sh up|url|status|down" >&2; }

case "$PORT" in
  ''|*[!0-9]*) die "AURORA_TEST_PG_PORT must be a port number, got '$PORT'" ;;
esac
[ "$PORT" -ne 55432 ] || { echo "test-pg: port 55432 is the owner's dev database; pick another AURORA_TEST_PG_PORT" >&2; exit 2; }

# Descriptor 7 is the lock. Every podman call closes it: conmon outlives this script and would
# otherwise keep the lock held for as long as the container runs (FIX-057).
pm() { podman "$@" 7>&-; }

exists() { pm container exists "$NAME"; }
running() { [ "$(pm inspect -f '{{.State.Running}}' "$NAME" 2>/dev/null)" = true ]; }
# -h 127.0.0.1: while initdb runs, the image's temporary server listens on the unix socket only,
# so a TCP probe cannot report "ready" too early.
ready() { pm exec "$NAME" pg_isready -h 127.0.0.1 -U aurora -d aurora >/dev/null 2>&1; }

wait_ready() {
  local i
  for ((i = 0; i < READY_TRIES; i++)); do
    ready && return 0
    running || return 1
    sleep 1
  done
  return 1
}

actual_port() {
  pm port "$NAME" 5432/tcp 2>/dev/null | sed -n '1s/.*://p'
}

url() {
  local p=""
  if exists && running; then p="$(actual_port)"; fi
  echo "postgres://aurora:aurora@127.0.0.1:${p:-$PORT}/aurora"
}

lock() {
  exec 7>"$LOCK"
  flock -w "$LOCK_WAIT_SECS" 7 || die "gave up after ${LOCK_WAIT_SECS}s waiting for the lock $LOCK"
}

up() {
  command -v podman >/dev/null 2>&1 || die "podman is not installed"
  lock
  if exists; then
    # A running server gets the whole bounded wait before it is judged unhealthy: other lanes'
    # tests may be using it, so one slow probe must never recreate it.
    if running && wait_ready; then return 0; fi
    echo "test-pg: $NAME exists but is stopped or unhealthy; recreating it" >&2
    pm rm -f -t 1 "$NAME" >/dev/null || die "could not remove the old $NAME"
  fi
  echo "test-pg: starting $NAME on 127.0.0.1:$PORT" >&2
  if ! pm run -d --name "$NAME" \
      --tmpfs "$PGDATA" \
      -p "127.0.0.1:$PORT:5432" \
      -e POSTGRES_USER=aurora -e POSTGRES_PASSWORD=aurora -e POSTGRES_DB=aurora \
      "$IMAGE" -c "max_connections=$MAX_CONNECTIONS" </dev/null >/dev/null; then
    # `podman run` leaves a created, never-started container behind (e.g. the port is taken).
    pm rm -f -t 1 "$NAME" >/dev/null 2>&1 || true
    die "podman run failed (is port $PORT in use? set AURORA_TEST_PG_PORT)"
  fi
  wait_ready || die "$NAME did not become ready within ${READY_TRIES}s (podman logs $NAME)"
}

status() {
  if ! exists; then echo "$NAME: absent"; return 1; fi
  if ! running; then echo "$NAME: stopped"; return 1; fi
  if ! ready; then echo "$NAME: running but not accepting connections"; return 1; fi
  echo "$NAME: ready at $(url)"
}

down() {
  lock
  if exists; then
    pm rm -f -t 1 "$NAME" >/dev/null || die "could not remove $NAME"
    echo "test-pg: removed $NAME" >&2
  else
    echo "test-pg: $NAME is not there" >&2
  fi
}

[ "$#" -eq 1 ] || { usage; exit 2; }
case "$1" in
  up) up ;;
  url) url ;;
  status) status ;;
  down) down ;;
  -h|--help) usage ;;
  *) usage; exit 2 ;;
esac
