#!/usr/bin/env bash
# M2 acceptance: real aurora-api + 2x aurora-gateway + Postgres + mock upstreams (TEST-012).
# The binaries hard-code ports 4101/4111, 4102/4103 and 4104 (hook host); the suite isolates itself by
# binding them on 127.0.0.31/.32/.33/.34. Tests run serially; each creates and drops its own database.
# TEST-021 tier 2 (frontdoor.rs): FRONTDOOR=1 also starts strict front-door containers (pinned
# images, --network=host) between the gateway and the mock upstream — the production topology that
# escaped FIX-052 — and exports AURORA_E2E_FRONTDOOR{,_HAPROXY}; without them those tests skip.
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
FD_RUN=""
PG=""
cleanup() {
  [ -n "$PG" ] && podman rm -f "$PG" >/dev/null 2>&1
  podman rm -f aurora-e2e-fd-nginx aurora-e2e-fd-haproxy >/dev/null 2>&1
  [ -n "$FD_RUN" ] && rm -rf "$FD_RUN"
}
trap cleanup EXIT
cargo build -p aurora-api -p aurora-gateway -p aurora-hookhost || { echo "cargo build failed" >&2; exit 1; }

# P2-DBL-1: the database-lane scenario needs a PostgreSQL to connect *to* that is not the platform
# database (the dial guard refuses the platform database by address and port). A throw-away
# container, named after the worktree, on a random loopback port; set AURORA_E2E_DB_TARGET
# (host:port, user/password/db `aurora`/`aurora`/`postgres`) to use your own instead.
if [ -z "${AURORA_E2E_DB_TARGET:-}" ] && ! command -v podman >/dev/null 2>&1; then
  echo "podman not found and AURORA_E2E_DB_TARGET unset: the database-lane scenario will be skipped" >&2
elif [ -z "${AURORA_E2E_DB_TARGET:-}" ]; then
  PG="$(basename "$ROOT")-gwe2e-dbtarget"
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

# TEST-021 tier 2: strict front doors (nginx, and haproxy as a second peer) in front of the
# mock upstream, the production topology (docs/tasks/TEST-021.md req 4). CI-only machinery:
# opt in with FRONTDOOR=1; everything below is skipped otherwise and the frontdoor.rs tests
# just print SKIPPED. The doors are pinned `@sha256:` images like scripts/e2e/llm.sh; they
# run `--network=host` so they can reach the fixed loopback addresses below, and the suite
# reaches them the same way (127.0.0.2 is inside the gateway's allowed_private and is not
# one of the gateways' own listeners, so the destination guard lets it through).
if [ "${FRONTDOOR:-}" = "1" ]; then
  NGINX_IMAGE="docker.io/library/nginx:1.29-alpine@sha256:5616878291a2eed594aee8db4dade5878cf7edcb475e59193904b198d9b830de"
  HAPROXY_IMAGE="docker.io/library/haproxy:3.2-alpine@sha256:1c3568e520453cc8ba98c39501febcf450429c80e6cc27ecad779961d76e597c"
  FD_CONF="$ROOT/deploy/dev/gateway-e2e"
  FD_RUN="$(mktemp -d /tmp/aurora-e2e-frontdoor.XXXXXX)"
  for port in 8091 8092 8093; do
    if ss -ltn "sport = :$port" | grep -q "127.0.0.2:"; then echo "127.0.0.2:$port already in use (front door)" >&2; exit 1; fi
  done
  # The mock upstream's certificate (deploy/dev/gateway-e2e/ README): the doors' upstream legs
  # verify it by name, the gateway by address; the test serves it via
  # AURORA_E2E_FRONTDOOR_MOCK_{CERT,KEY} (the key never enters a container).
  # `basicConstraints=CA:FALSE` matters: `req -x509` defaults to CA:TRUE, and the gateway's
  # rustls client rejects a CA certificate presented as a server leaf (`CaUsedAsEndEntity`)
  # even though curl/OpenSSL tolerate it.
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
    -keyout "$FD_RUN/mock.key" -out "$FD_RUN/mock.pem" -days 2 \
    -subj "/CN=fd-mock.e2e.aurora.dev" \
    -addext "subjectAltName=DNS:fd-mock.e2e.aurora.dev,IP:127.0.0.2" \
    -addext "basicConstraints=critical,CA:FALSE" \
    >/dev/null 2>&1 || { echo "openssl mock certificate failed" >&2; exit 1; }
  cat "$FD_RUN/mock.pem" "$FD_RUN/mock.key" > "$FD_RUN/fd.pem"
  # The haproxy image reads these as a non-root user, so the throwaway directory must be
  # traversable and the mock material world-readable (dev-only, 2-day certificates).
  chmod 755 "$FD_RUN"
  chmod 644 "$FD_RUN/fd.pem" "$FD_RUN/mock.pem" "$FD_RUN/mock.key"
  AURORA_E2E_FRONTDOOR="127.0.0.2:8091"
  AURORA_E2E_FRONTDOOR_HAPROXY="127.0.0.2:8092"
  AURORA_E2E_FRONTDOOR_MOCK="127.0.0.2:8093"
  AURORA_E2E_FRONTDOOR_MOCK_CERT="$FD_RUN/mock.pem"
  AURORA_E2E_FRONTDOOR_MOCK_KEY="$FD_RUN/mock.key"
  export AURORA_E2E_FRONTDOOR AURORA_E2E_FRONTDOOR_HAPROXY AURORA_E2E_FRONTDOOR_MOCK \
    AURORA_E2E_FRONTDOOR_MOCK_CERT AURORA_E2E_FRONTDOOR_MOCK_KEY
  podman run -d --rm --name aurora-e2e-fd-nginx --network=host \
    -v "$FD_CONF/nginx.conf:/etc/nginx/nginx.conf:ro,Z" \
    -v "$FD_RUN:/fd:ro,Z" "$NGINX_IMAGE" >/dev/null || { echo "cannot start the nginx front door" >&2; exit 1; }
  if [ "${FRONTDOOR_HAPROXY:-}" = "1" ]; then
    podman run -d --rm --name aurora-e2e-fd-haproxy --network=host \
      -v "$FD_CONF/haproxy.cfg:/usr/local/etc/haproxy/haproxy.cfg:ro,Z" \
      -v "$FD_RUN:/fd:ro,Z" "$HAPROXY_IMAGE" >/dev/null || { echo "cannot start the haproxy front door" >&2; exit 1; }
  fi
  for name in aurora-e2e-fd-nginx $( [ "${FRONTDOOR_HAPROXY:-}" = "1" ] && echo aurora-e2e-fd-haproxy ); do
    podman ps --filter "name=$name" --filter "status=running" --format '{{.Names}}' | grep -qx "$name" || {
      echo "front door $name is not running:" >&2; podman logs "$name" 2>&1 | tail -20 >&2; exit 1; }
  done
  echo "front doors up: nginx :8091$( [ "${FRONTDOOR_HAPROXY:-}" = "1" ] && echo ', haproxy :8092' ) (mock upstream :8093)"
fi

cargo test -p aurora-e2e-gateway -- --test-threads=1 "$@"
rc=$?
exit $rc
