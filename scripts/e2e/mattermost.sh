#!/usr/bin/env bash
# MM-002 acceptance: the real aurora-connector-mattermost against the real core (api + gateway +
# runner + podman sandboxes with the fake harness) and a real Mattermost 11.7 server
# (scripts/mattermost/setup.sh). Gated by AURORA_TEST_PODMAN=1.
#
# Same networking as scripts/e2e/channels.sh: the whole suite runs inside podman's rootless netns
# (the sandbox bridge IP 10.89.251.1 lives there) and the shared dev Postgres from `.env` is
# bridged in through a unix socket. The Mattermost container is started from inside that netns
# with `--network host`, i.e. it shares the netns: the connector, the core and the test reach it on
# 127.0.0.1:$AURORA_MM_PORT, and Mattermost reaches the connector's callbacks on 127.0.0.1:4310.
# The netns and the 10.89.251.0/24 bridge are shared by every e2e script: this one waits while
# another run holds them (up to AURORA_E2E_WAIT_MIN, default 30).
# Env: SKIP_IMAGE_BUILD=1 reuses localhost/aurora-sandbox:0.1.0. Extra arguments go to `cargo test`.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
if [ "${AURORA_TEST_PODMAN:-}" != "1" ]; then echo "skipped: set AURORA_TEST_PODMAN=1" >&2; exit 0; fi
command -v socat >/dev/null || { echo "socat is required" >&2; exit 1; }

if [ -z "${AURORA_TEST_DATABASE_URL:-}" ] && [ -f .env ]; then
  AURORA_TEST_DATABASE_URL=$(grep -E '^DATABASE_URL=' .env | head -1 | cut -d= -f2-)
fi
[ -n "${AURORA_TEST_DATABASE_URL:-}" ] || { echo "no AURORA_TEST_DATABASE_URL / .env DATABASE_URL" >&2; exit 1; }
HOSTPORT=$(printf '%s' "$AURORA_TEST_DATABASE_URL" | sed -E 's#^[a-z]+://[^@]*@([^/?]+).*#\1#')
PGHOST_=${HOSTPORT%%:*}
PGPORT_=${HOSTPORT##*:}

SBX_NET=aurora-mm2-sbx
ANCHOR=aurora-mm2-anchor
MM_NAME=aurora-mm2-e2e
TOKEN="aur_e2emattermosttoken"
FP=$(printf '%s' "$TOKEN" | sha256sum | cut -c1-16)
IMAGE="localhost/aurora-sandbox:0.1.0"
SOCKDIR=""
HOST_SOCAT=""

cleanup() {
  podman ps -a -q --filter "label=aurora.installation=$FP" | xargs -r podman rm -f -t 1 >/dev/null 2>&1
  podman volume ls -q --filter "label=aurora.installation=$FP" | xargs -r podman volume rm -f >/dev/null 2>&1
  podman rm -f -t 1 "$ANCHOR" "$MM_NAME" >/dev/null 2>&1
  podman network rm "$SBX_NET" >/dev/null 2>&1
  [ -n "$HOST_SOCAT" ] && kill "$HOST_SOCAT" >/dev/null 2>&1
  [ -n "$SOCKDIR" ] && rm -rf "$SOCKDIR"
}
trap cleanup EXIT

# Wait for the shared netns / bridge subnet to be free (other e2e scripts use 10.89.251.0/24).
busy() { podman network ls --format '{{.Name}}' | grep -v "^$SBX_NET$" | xargs -r -I{} podman network inspect {} --format '{{range .Subnets}}{{.Subnet}} {{end}}' 2>/dev/null | grep -q '10.89.251.0/24'; }
deadline=$(( $(date +%s) + ${AURORA_E2E_WAIT_MIN:-30} * 60 ))
while busy; do
  [ "$(date +%s)" -lt "$deadline" ] || { echo "another e2e run holds 10.89.251.0/24; giving up" >&2; exit 1; }
  echo "waiting for another e2e run to release 10.89.251.0/24..." >&2
  sleep 20
done
cleanup
SOCKDIR=$(mktemp -d "${TMPDIR:-/tmp}/aurora-mm2-XXXXXX")

free_kb=$(df --output=avail -k . | tail -1)
if [ "$free_kb" -lt $((15 * 1024 * 1024)) ]; then echo "less than 15 GB free; stopping" >&2; exit 1; fi

cargo build -p aurora-api -p aurora-gateway -p aurora-runner -p aurora-connector-mattermost || { echo "cargo build failed" >&2; exit 1; }
if [ "${SKIP_IMAGE_BUILD:-}" != "1" ]; then
  scripts/images/build-sandbox-min.sh aurora-sandbox:0.1.0 || { echo "image build failed" >&2; exit 1; }
fi
podman image exists "$IMAGE" || { echo "missing $IMAGE" >&2; exit 1; }
cargo test -p aurora-e2e-hosted --test mattermost --no-run || exit 1

podman network create --internal --disable-dns --subnet 10.89.251.0/24 --gateway 10.89.251.1 "$SBX_NET" >/dev/null || exit 1
podman run -d --name "$ANCHOR" --network "$SBX_NET" --entrypoint sleep "$IMAGE" 100000 >/dev/null || exit 1

socat "UNIX-LISTEN:$SOCKDIR/pg.sock,fork,mode=600" "TCP:$PGHOST_:$PGPORT_" &
HOST_SOCAT=$!

SOCK=$(podman info --format '{{.Host.RemoteSocket.Path}}')
podman unshare --rootless-netns env \
  AURORA_TEST_DATABASE_URL="$AURORA_TEST_DATABASE_URL" PGBRIDGE_SOCK="$SOCKDIR/pg.sock" PGBRIDGE_PORT="$PGPORT_" \
  AURORA_E2EH_SOCK="$SOCK" AURORA_E2EH_NETWORK="$SBX_NET" AURORA_E2EH_TOKEN="$TOKEN" \
  AURORA_E2EH_FP="$FP" AURORA_E2EH_IMAGE="$IMAGE" AURORA_MM_CONTAINER="$MM_NAME" \
  bash -c '
    socat "TCP-LISTEN:$PGBRIDGE_PORT,bind=127.0.0.1,fork,reuseaddr" "UNIX-CONNECT:$PGBRIDGE_SOCK" &
    BR=$!
    bash scripts/mattermost/setup.sh up || { kill $BR 2>/dev/null; exit 1; }
    set -a; . target/mattermost/env; set +a
    cargo test -p aurora-e2e-hosted --test mattermost -- --test-threads=1 --nocapture "$@"
    rc=$?
    kill $BR 2>/dev/null
    exit $rc
  ' bash "$@"
rc=$?
exit $rc
