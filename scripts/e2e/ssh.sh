#!/usr/bin/env bash
# P4-SSH-2 (D-239, SEC-92..SEC-96, C-799): SSH access end to end on a kind cluster — the real `ssh`, `sftp`, `scp` and
# `aurora ssh` clients of this machine against the chart's terminator, the API's session plane, the runner's `auz_`
# legs and the supervisor's exec engine, in a sandbox Pod.
#
#   scripts/e2e/ssh.sh [--keep] [--fresh] [--skip-build] [--release-images]     (kind.sh's flags)
#
# What it runs: `scripts/e2e/kind.sh --no-upgrade-legs -- k9_ssh_journey`, i.e. the scenario `k9_ssh_journey` of
# tests/e2e-hosted/tests/k8s.rs against the chart installed with the SSH profile (AURORA_E2E_SSH=1:
# deploy/kind/values-ssh.yaml — ssh.enabled, terminal.enabled, the terminator as a NodePort, networkPolicy.sshFrom open;
# the terminator's Service is reached through a `kubectl port-forward` on AURORA_E2E_SSH_PORT, default 12222):
# a key registered and a certificate minted through the API; `ssh host echo aurora-ssh` with the downloaded certificate
# and `aurora ssh <agent> -- echo aurora-ssh`; stdout/stderr separation and `exit 7` -> 7; a shell with and without a
# tty; `sftp` put/get and `scp` (SFTP mode and `-O`) of a 5 MiB file with matching sha256; `-R`, `-L`, `-A`, `-X`
# refused while the shell works; a wrong pinned host key refused by the client, a plain key refused with the same
# `Permission denied (publickey)`; a person's disconnect and a freeze each close an open session within 35 s (and a
# frozen agent refuses new ones); deleting the key ends its certificate's trust; a parked sandbox wakes on connect;
# a canary (typed in a shell and passed as a command) searched for in every API, terminator, runner and sandbox log,
# in `audit_log` and in `request_log`, and the `agent.ssh.open` / `agent.ssh.close` rows exist.
#
# Skip policy: a machine without OpenSSH's `ssh`, `sftp`, `scp`, `ssh-keygen` and `ssh-agent` prints a `SKIPPED:` line
# and exits 0; CI=true turns that into a failure through scripts/e2e/lib-skip.sh, like every e2e script.
#
# One e2e suite at a time: it takes /tmp/aurora-e2e-gateway.lock (the lock of gateway.sh) and waits for it.
# Never run it against the `aurora-dev` cluster: use AURORA_E2E_CLUSTER (default aurora-e2e) and the kind.sh ports
# (AURORA_KIND_PORT, AURORA_E2E_IDP_PORT, AURORA_E2E_SSH_PORT) of this checkout.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
# shellcheck source=scripts/e2e/lib-skip.sh
. scripts/e2e/lib-skip.sh

if [ "${AURORA_E2E_CLUSTER:-aurora-e2e}" = "aurora-dev" ]; then
  echo "ssh.sh: refusing to use the aurora-dev cluster" >&2
  exit 2
fi

for t in ssh sftp scp ssh-keygen ssh-agent; do
  if ! command -v "$t" >/dev/null 2>&1; then
    echo "SKIPPED: ssh e2e needs the OpenSSH client tools (ssh, sftp, scp, ssh-keygen, ssh-agent); \`$t\` is missing"
    exit 0
  fi
done

LOCK=/tmp/aurora-e2e-gateway.lock
exec 9>"$LOCK"
if ! flock -w 3600 9; then
  echo "ssh.sh: gave up waiting for the e2e lock $LOCK" >&2
  exit 1
fi

# The CLI leg: `aurora ssh` is the debug build of this checkout.
nice -n 10 cargo build -p aurora-cli 9>&- || { echo "ssh.sh: cargo build -p aurora-cli failed" >&2; exit 1; }
export AURORA_E2E_CLI="$ROOT/target/debug/aurora"
export AURORA_E2E_SSH=1
LOG="$ROOT/target/ssh-e2e.log"
mkdir -p "$ROOT/target"
# fd 9 (the lock) stays with this shell only: nothing kind.sh starts (port-forwards, containers) may inherit it.
run_logged "$LOG" bash scripts/e2e/kind.sh --no-upgrade-legs "$@" -- k9_ssh_journey 9>&-
rc=$?
if [ "$rc" -eq 0 ]; then
  if grep -qE 'SKIPPED:' "$LOG"; then
    echo "ssh e2e: SKIPPED ($(grep -E 'SKIPPED:' "$LOG" | head -1 | sed 's/^.*SKIPPED:/SKIPPED:/'))"
  elif grep -q "test result: ok. 1 passed" "$LOG"; then
    echo "ssh e2e: PASS (k9_ssh_journey)"
  else
    echo "ssh e2e: FAILED: the scenario did not run (no \`test result: ok. 1 passed\` line in $LOG)" >&2
    exit 1
  fi
fi
exit "$rc"
