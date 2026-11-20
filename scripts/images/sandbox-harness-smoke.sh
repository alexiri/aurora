#!/usr/bin/env bash
# Web-terminal harness smoke (FIX-190, FIX-191; docs/12 §Terminal): starts the terminal wrappers of a
# built FULL sandbox image (`--target sandbox`) the way a person does in the web terminal: in a PTY,
# as the agent user, with a supervisor-shaped /run/aurora/harness.env, a fresh /workspace, and the
# sandbox's confinement as the podman runner applies it (backends/podman.rs `host_config`: read-only
# root, the /tmp and /run/aurora tmpfs, every capability dropped, no-new-privileges, the sandbox seccomp
# profile `deploy/helm/aurora/files/sandbox-seccomp.json`, the default pids, memory and CPU limits of
# `runner_plane/boot.rs`) and the supervisor's `TERM`/`COLORTERM` (pty.rs). No model is reachable
# (`--network none`): the check is the harness's FIRST SCREEN, not a turn.
#
#   scripts/images/sandbox-harness-smoke.sh <image> [harness...]     (default: pi opencode)
#
# Per harness it fails when the program dies of a signal (FIX-190: Pi's Node segfaulted at start)
# or when its first screen does not show the expected prompt (a setup wizard, a question, a blank
# screen). The screen log of a failure is printed (SMOKE_SHOW=1: of every harness). Needs podman
# (rootless is fine); builds nothing.
set -euo pipefail
cd "$(dirname "$0")/../.."
image="${1:?usage: sandbox-harness-smoke.sh <image> [harness...]}"
shift
harnesses=("$@")
[ "${#harnesses[@]}" -gt 0 ] || harnesses=(pi opencode)
seccomp="$PWD/deploy/helm/aurora/files/sandbox-seccomp.json"
[ -r "$seccomp" ] || { echo "FAIL: $seccomp is missing" >&2; exit 1; }
podman image exists "$image" || { echo "FAIL: no image $image" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
model=smoke-model
host=http://llm.invalid
# What the supervisor writes (harness_env.rs `render`), for one model on the gateway's Anthropic
# Messages endpoint. Nothing here is secret: the gateway adds the real key.
models_json='{"providers":{"aurora":{"baseUrl":"'"$host"'","api":"anthropic-messages","apiKey":"$ANTHROPIC_API_KEY","models":[{"id":"'"$model"'","name":"'"$model"'"}]}}}'
pi_settings='{"retry":{"enabled":true,"maxRetries":2,"baseDelayMs":2000},"defaultProjectTrust":"never","enableInstallTelemetry":false,"quietStartup":true,"collapseChangelog":true}'
# `opencode::config::terminal_config` for the same model (Anthropic Messages, no roles, no effort).
oc_variants='{"low":{"thinking":{"type":"enabled","budgetTokens":4000}},"medium":{"thinking":{"type":"enabled","budgetTokens":10000}},"high":{"thinking":{"type":"enabled","budgetTokens":16000}},"max":{"thinking":{"type":"enabled","budgetTokens":32000}}}'
oc_config='{"$schema":"https://opencode.ai/config.json","autoupdate":false,"share":"disabled","permission":{"*":"allow"},"model":"aurora/'"$model"'","small_model":"aurora/'"$model"'","agent":{"title":{"disable":true}},"instructions":["/workspace/AGENTS.md"],"provider":{"aurora":{"npm":"@ai-sdk/anthropic","name":"Aurora","options":{"baseURL":"'"$host"'/v1","apiKey":"{env:ANTHROPIC_API_KEY}"},"models":{"'"$model"'":{"name":"'"$model"'","variants":'"$oc_variants"'}}}}}'
oc_version=$(sed -n 's/^ARG OPENCODE_VERSION=//p' images/sandbox/Containerfile | head -1)
cat >"$work/harness.env" <<EOF
# Written by sandbox-harness-smoke.sh in the shape of aurora-supervisor's harness.env.
HARNESSES_ENABLED='claude-code codex pi goose opencode'
HTTPS_PROXY='http://127.0.0.1:3128'
HTTP_PROXY='http://127.0.0.1:3128'
https_proxy='http://127.0.0.1:3128'
http_proxy='http://127.0.0.1:3128'
NO_PROXY='localhost,127.0.0.1'
SSL_CERT_FILE='/etc/pki/tls/certs/ca-bundle.crt'
NODE_EXTRA_CA_CERTS='/etc/pki/tls/certs/ca-bundle.crt'
DISABLE_AUTOUPDATER='1'
DISABLE_TELEMETRY='1'
OPENAI_API_KEY='aurora-placeholder'
ANTHROPIC_API_KEY='aurora-placeholder'
AURORA_PI_MODEL='$model'
AURORA_PI_MODELS_JSON='$models_json'
AURORA_PI_KEY_VAR='ANTHROPIC_API_KEY'
AURORA_PI_SETTINGS_JSON='$pi_settings'
AURORA_OPENCODE_MODEL='aurora/$model'
AURORA_OPENCODE_CONFIG='$oc_config'
AURORA_OPENCODE_KEY_VAR='ANTHROPIC_API_KEY'
AURORA_OPENCODE_PLUGIN_VERSION='$oc_version'
EOF

# Inside the container: start `$1` in a PTY (`script`), type the quit keys after a few seconds, and
# report the exit status; the screen log stays in /tmp/screen for the caller.
cat >"$work/run.sh" <<'EOF'
cd /workspace
h=$1
( sleep 12; printf '\004'; sleep 1; printf '\003'; sleep 1; printf '\003'; sleep 2 ) |
    timeout 30 script -qefc "stty rows 40 cols 120; exec /opt/aurora/bin/$h" /tmp/screen >/dev/null 2>&1
echo "exit=$?"
cat /tmp/screen
EOF

fail=0
for h in "${harnesses[@]}"; do
  # What the harness's own prompt shows on its first screen (a setup wizard or a question has none).
  case $h in
    # Pi's footer and OpenCode's prompt box both name the model.
    pi | opencode) want="$model" ;;
    *) echo "FAIL: no first-screen check for $h" >&2; exit 2 ;;
  esac
  out=$(podman run --rm --network none --user 1000:1000 \
          --read-only \
          --tmpfs /tmp:rw,nosuid,nodev,size=512m \
          --tmpfs /run/aurora:rw,nosuid,nodev,noexec,size=16m,mode=1777,U \
          --tmpfs /workspace:rw,nosuid,nodev,size=512m,U \
          -v "$work/harness.env:/run/aurora/harness.env:ro,Z" \
          -v "$work/run.sh:/smoke-run.sh:ro,Z" \
          --cap-drop ALL --security-opt no-new-privileges --security-opt "seccomp=$seccomp" \
          --pids-limit 512 --memory 2g --memory-swap 2g --cpus 1 \
          -e TERM=xterm-256color -e COLORTERM=truecolor \
          --entrypoint /bin/sh "$image" /smoke-run.sh "$h" 2>&1) || true
  code=$(printf '%s\n' "$out" | sed -n 's/^exit=\([0-9]*\)$/\1/p' | head -1)
  # Printable text only: drop escape sequences, then look for the prompt's marker.
  text=$(printf '%s\n' "$out" | sed -e 's/\x1b\][^\x07\x1b]*\(\x07\|\x1b\\\)//g' -e 's/\x1b\[[0-9;?<>=]*[a-zA-Z~]//g' -e 's/\x1b[()][0-9A-Za-z]//g' -e 's/\x1b[=>78]//g')
  if [ -z "$code" ]; then
    echo "FAIL [$h]: the container did not run the check"; fail=1
  elif [ "$code" -ge 128 ] && [ "$code" -ne 124 ]; then
    echo "FAIL [$h]: ended by signal $((code - 128)) (exit $code)"; fail=1
  elif ! printf '%s\n' "$text" | grep -q "$want"; then
    echo "FAIL [$h]: its first screen does not show its prompt (exit $code)"; fail=1
  else
    echo "ok   [$h] first screen shows its prompt (exit $code)"
    [ "${SMOKE_SHOW:-}" = 1 ] || continue
  fi
  printf '%s\n' "$text" | tail -40
done
if [ "$fail" -ne 0 ]; then echo "== sandbox-harness-smoke: FAILED"; exit 1; fi
echo "== sandbox-harness-smoke: passed"
