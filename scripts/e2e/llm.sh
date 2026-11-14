#!/usr/bin/env bash
# TEST-017 / decision I-30: live `claude-code` acceptance against the real LiteLLM proxy in front of
# VidaiMock (a scripted Anthropic-API stand-in; not a model). Pinned images, Aurora-owned scenarios
# in deploy/dev/llm-mock/vidaimock/.
#
#   bash scripts/e2e/llm.sh                          smoke + supervisor-level live conformance
#   AURORA_TEST_PODMAN=1 AURORA_TEST_LLM=1 bash scripts/e2e/llm.sh
#                                                    ... plus the hosted claude-code e2e (tests/e2e-hosted/tests/llm.rs)
#
# Needs: podman, node/npm, curl, jq. Containers/network are named aurora-llm-*; all removed on exit.
# Supervisor-level run: the supervisor suite has no gateway, so it talks to LiteLLM directly
# (127.0.0.1:$AURORA_LLM_PORT) and sends the dev master key as x-litellm-api-key. The hosted e2e goes
# through the gateway LLM lane (key injected by the gateway). Networking for the hosted part is as in
# scripts/e2e/hosted.sh: the suite runs in podman's rootless netns and LiteLLM is reached by its
# container IP; Postgres for it is an ephemeral container too, because the shared dev Postgres listens
# on host loopback only, which the rootless netns cannot reach.
# Env: LLM_ONLY_HOSTED=1 skips the conformance part (debugging); SKIP_IMAGE_BUILD=1 reuses localhost/aurora-llm-sandbox:0.1.0; AURORA_LLM_PORT (default 54000);
#      AURORA_CLAUDE_PREFIX (default target/aurora-llm/claude) for the local pinned CLI.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

# Pinned images (keep in sync with deploy/dev/llm-mock/{llm-mock.yaml,vidaimock.Containerfile,vidaimock/README.md}).
VIDAIMOCK_IMAGE="ghcr.io/vidaiuk/vidaimock@sha256:c306a4e87637976bfd9034d0275ae6932497b18ee71614c46a0f143f393127c6"
LITELLM_IMAGE="ghcr.io/berriai/litellm@sha256:d8b4ac356743d647a13bb93f45b941fcd5f5354122cfd205948a1b192790389d"
CLAUDE_VERSION="2.1.319"
KEY="sk-aurora-mock-dev"   # dev-only dummy master key; protects nothing

NET=aurora-llm-net; VID=aurora-llm-vidaimock; LLM=aurora-llm-litellm
PORT="${AURORA_LLM_PORT:-54000}"
LLM_IP=10.89.253.5
HOSTED=0
[ "${AURORA_TEST_PODMAN:-}" = "1" ] && [ "${AURORA_TEST_LLM:-}" = "1" ] && HOSTED=1

SBX_NET=aurora-llm-sbx; DB_NET=aurora-llm-db; PG=aurora-llm-pg; ANCHOR=aurora-llm-anchor
TOKEN="aur_llmhostedtoken"
FP=$(printf '%s' "$TOKEN" | sha256sum | cut -c1-16)
# Both tags are overridable so a supervisor change can be tested without replacing the shared images.
BASE_IMAGE="${AURORA_E2EL_BASE_IMAGE:-localhost/aurora-sandbox:0.1.0}"
IMAGE="${AURORA_E2EL_IMAGE:-localhost/aurora-llm-sandbox:0.1.0}"

cleanup() {
  podman ps -a -q --filter "label=aurora.installation=$FP" | xargs -r podman rm -f -t 1 >/dev/null 2>&1
  podman volume ls -q --filter "label=aurora.installation=$FP" | xargs -r podman volume rm -f >/dev/null 2>&1
  podman rm -f -t 1 "$VID" "$LLM" "$PG" "$ANCHOR" >/dev/null 2>&1
  podman network rm "$NET" "$SBX_NET" "$DB_NET" >/dev/null 2>&1
}
trap cleanup EXIT
cleanup

command -v jq >/dev/null || { echo "jq is required" >&2; exit 1; }

echo "== start vidaimock + litellm"
podman network create --subnet 10.89.253.0/24 --gateway 10.89.253.1 "$NET" >/dev/null || exit 1
podman run -d --name "$VID" --network "$NET" --network-alias vidaimock \
  -v "$ROOT/deploy/dev/llm-mock/vidaimock:/config:ro,Z" "$VIDAIMOCK_IMAGE" \
  --host 0.0.0.0 --port 8100 --config-dir /config --isolated >/dev/null || exit 1
podman run -d --name "$LLM" --network "$NET" --ip "$LLM_IP" -p "127.0.0.1:$PORT:4000" \
  -e LITELLM_MASTER_KEY="$KEY" -e LITELLM_TELEMETRY=False \
  -v "$ROOT/deploy/dev/llm-mock/litellm.yaml:/etc/litellm/litellm.yaml:ro,Z" "$LITELLM_IMAGE" \
  --config /etc/litellm/litellm.yaml --port 4000 >/dev/null || exit 1

echo "== scenario smoke (curl through litellm)"
bash scripts/e2e/llm-smoke.sh "http://127.0.0.1:$PORT" "$KEY" || { echo "smoke failed" >&2; podman logs --tail 30 "$LLM" >&2; exit 1; }

if [ "${LLM_ONLY_HOSTED:-}" != "1" ]; then
echo "== supervisor-level live conformance (claude-code $CLAUDE_VERSION -> litellm -> vidaimock)"
PREFIX="${AURORA_CLAUDE_PREFIX:-$ROOT/target/aurora-llm/claude}"
CLAUDE="$PREFIX/node_modules/.bin/claude"
if ! "$CLAUDE" --version 2>/dev/null | grep -q "^$CLAUDE_VERSION "; then
  mkdir -p "$PREFIX" && npm install --prefix "$PREFIX" --no-audit --no-fund "@anthropic-ai/claude-code@$CLAUDE_VERSION" >/dev/null 2>&1
  # Some npm configs skip lifecycle scripts; the native binary is fetched by the postinstall.
  "$CLAUDE" --version >/dev/null 2>&1 || node "$PREFIX/node_modules/@anthropic-ai/claude-code/install.cjs" >/dev/null 2>&1
fi
"$CLAUDE" --version | grep -q "^$CLAUDE_VERSION " || { echo "cannot install claude-code $CLAUDE_VERSION" >&2; exit 1; }
AURORA_LIVE_CLAUDE=1 \
ANTHROPIC_BASE_URL="http://127.0.0.1:$PORT" \
AURORA_CLAUDE_BIN="$CLAUDE" \
AURORA_LIVE_MCP_BRIDGE="node $ROOT/scripts/e2e/mcp-stub.mjs" \
AURORA_LIVE_CUSTOM_HEADERS="x-litellm-api-key:$KEY" \
AURORA_LIVE_SCENARIO_MODELS="say=scn-pong,long=scn-count,tool=scn-memory-tool,fail=scn-ratelimit,steer=scn-steer" \
  timeout 900 cargo test -p aurora-supervisor --test conformance -- --nocapture --test-threads=1 || { echo "live conformance failed" >&2; exit 1; }

fi

if [ "$HOSTED" != 1 ]; then
  echo "hosted e2e skipped: set AURORA_TEST_PODMAN=1 AURORA_TEST_LLM=1"
  exit 0
fi

echo "== hosted claude-code e2e"
free_kb=$(df --output=avail -k . | tail -1)
if [ "$free_kb" -lt $((15 * 1024 * 1024)) ]; then echo "less than 15 GB free; stopping" >&2; exit 1; fi
cargo build -p aurora-api -p aurora-gateway -p aurora-runner || { echo "cargo build failed" >&2; exit 1; }
if [ "${SKIP_IMAGE_BUILD:-}" != "1" ]; then
  # (FIX-103) Always rebuild the base (a cached layer build is quick): reusing an existing image tested an
  # old supervisor. SKIP_IMAGE_BUILD=1 reuses the images as they are.
  scripts/images/build-sandbox-min.sh "${BASE_IMAGE#localhost/}" || { echo "base image build failed" >&2; exit 1; }
  podman build -f scripts/e2e/llm.Containerfile --build-arg "BASE=$BASE_IMAGE" -t "$IMAGE" . || { echo "llm sandbox image build failed" >&2; exit 1; }
fi
podman image exists "$IMAGE" || { echo "missing $IMAGE" >&2; exit 1; }
cargo test -p aurora-e2e-hosted --test llm --no-run || exit 1

podman network create --internal --disable-dns --subnet 10.89.251.0/24 --gateway 10.89.251.1 "$SBX_NET" >/dev/null || exit 1
podman network create --disable-dns --subnet 10.89.254.0/24 --gateway 10.89.254.1 "$DB_NET" >/dev/null || exit 1
podman run -d --name "$ANCHOR" --network "$SBX_NET" --entrypoint sleep "$IMAGE" 100000 >/dev/null || exit 1
podman run -d --name "$PG" --network "$DB_NET" --ip 10.89.254.5 \
  -e POSTGRES_USER=aurora -e POSTGRES_PASSWORD=aurora -e POSTGRES_DB=postgres \
  docker.io/library/postgres:16 -c max_connections=300 -c fsync=off >/dev/null || exit 1
for _ in $(seq 60); do podman exec "$PG" pg_isready -U aurora >/dev/null 2>&1 && break; sleep 1; done

SOCK=$(podman info --format '{{.Host.RemoteSocket.Path}}')
podman unshare --rootless-netns env \
  AURORA_TEST_DATABASE_URL="postgres://aurora:aurora@10.89.254.5:5432/postgres" \
  AURORA_E2EH_SOCK="$SOCK" AURORA_E2EH_NETWORK="$SBX_NET" AURORA_E2EH_TOKEN="$TOKEN" \
  AURORA_E2EH_FP="$FP" AURORA_E2EH_IMAGE="$IMAGE" \
  AURORA_TEST_PODMAN=1 AURORA_TEST_LLM=1 \
  AURORA_LLM_BASE_URL="http://$LLM_IP:4000" AURORA_LLM_KEY="$KEY" \
  cargo test -p aurora-e2e-hosted --test llm -- --test-threads=1 --nocapture "$@"
exit $?
