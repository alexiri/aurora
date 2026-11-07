#!/usr/bin/env bash
# M6 acceptance: real aurora-api + aurora-gateway + aurora-runner (podman) + sandbox image (fake
# harness) + the real aurora-connector-fake binary registered through the connector plane
# (TEST-016). Gated by AURORA_TEST_PODMAN=1.
#
# Same networking as scripts/e2e/hosted.sh (the suite runs inside podman's rootless netns, bound on
# the sandbox bridge IP 10.89.251.1), with one difference: no Postgres container is started. The
# database is the shared dev Postgres from `.env` (DATABASE_URL, on the host's loopback), which is
# not reachable from the rootless netns, so a pair of socat processes bridges it through a unix
# socket: host `socat UNIX-LISTEN -> TCP:<host:port>` and, inside the netns,
# `socat TCP-LISTEN:<port> -> UNIX-CONNECT`. Each test creates and drops its own database.
# Env: SKIP_IMAGE_BUILD=1 reuses the existing localhost/aurora-sandbox:0.1.0.
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

SBX_NET=aurora-e2ec-sbx
ANCHOR=aurora-e2ec-anchor
TOKEN="aur_e2echannelstoken"
FP=$(printf '%s' "$TOKEN" | sha256sum | cut -c1-16)
IMAGE="localhost/aurora-sandbox:0.1.0"
SOCKDIR=$(mktemp -d "${TMPDIR:-/tmp}/aurora-e2ec-XXXXXX")
HOST_SOCAT=""

cleanup() {
  podman ps -a -q --filter "label=aurora.installation=$FP" | xargs -r podman rm -f -t 1 >/dev/null 2>&1
  podman volume ls -q --filter "label=aurora.installation=$FP" | xargs -r podman volume rm -f >/dev/null 2>&1
  podman rm -f -t 1 "$ANCHOR" >/dev/null 2>&1
  podman network rm "$SBX_NET" >/dev/null 2>&1
  [ -n "$HOST_SOCAT" ] && kill "$HOST_SOCAT" >/dev/null 2>&1
  rm -rf "$SOCKDIR"
}
trap cleanup EXIT
cleanup
SOCKDIR=$(mktemp -d "${TMPDIR:-/tmp}/aurora-e2ec-XXXXXX")

free_kb=$(df --output=avail -k . | tail -1)
if [ "$free_kb" -lt $((15 * 1024 * 1024)) ]; then echo "less than 15 GB free; stopping" >&2; exit 1; fi

cargo build -p aurora-api -p aurora-gateway -p aurora-runner -p aurora-connector-fake || { echo "cargo build failed" >&2; exit 1; }
if [ "${SKIP_IMAGE_BUILD:-}" != "1" ]; then
  scripts/images/build-sandbox-min.sh aurora-sandbox:0.1.0 || { echo "image build failed" >&2; exit 1; }
fi
podman image exists "$IMAGE" || { echo "missing $IMAGE" >&2; exit 1; }
cargo test -p aurora-e2e-hosted --test channels --no-run || exit 1

podman network create --internal --disable-dns --subnet 10.89.251.0/24 --gateway 10.89.251.1 "$SBX_NET" >/dev/null || exit 1
podman run -d --name "$ANCHOR" --network "$SBX_NET" --entrypoint sleep "$IMAGE" 100000 >/dev/null || exit 1

socat "UNIX-LISTEN:$SOCKDIR/pg.sock,fork,mode=600" "TCP:$PGHOST_:$PGPORT_" &
HOST_SOCAT=$!

SOCK=$(podman info --format '{{.Host.RemoteSocket.Path}}')
podman unshare --rootless-netns env \
  AURORA_TEST_DATABASE_URL="$AURORA_TEST_DATABASE_URL" PGBRIDGE_SOCK="$SOCKDIR/pg.sock" PGBRIDGE_PORT="$PGPORT_" \
  AURORA_E2EH_SOCK="$SOCK" AURORA_E2EH_NETWORK="$SBX_NET" AURORA_E2EH_TOKEN="$TOKEN" \
  AURORA_E2EH_FP="$FP" AURORA_E2EH_IMAGE="$IMAGE" \
  bash -c '
    socat "TCP-LISTEN:$PGBRIDGE_PORT,bind=127.0.0.1,fork,reuseaddr" "UNIX-CONNECT:$PGBRIDGE_SOCK" &
    BR=$!
    cargo test -p aurora-e2e-hosted --test channels -- --test-threads=1 --nocapture "$@"
    rc=$?
    kill $BR 2>/dev/null
    exit $rc
  ' bash "$@"
rc=$?
exit $rc
