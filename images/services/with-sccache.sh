#!/bin/sh
# `with-sccache <cmd...>` (images/services/Containerfile, OPS-023): run a command with sccache as the
# rustc wrapper and print the hit statistics. Copied into the build stages as
# /usr/local/bin/with-sccache. A plain file rather than a Containerfile heredoc, because older
# buildah/podman (e.g. 4.9 on CI runners) cannot parse `COPY <<EOF`.
set -u
if [ -r /run/secrets/actions_results_url ] && [ -r /run/secrets/actions_runtime_token ]; then
  ACTIONS_RESULTS_URL=$(cat /run/secrets/actions_results_url)
  ACTIONS_RUNTIME_TOKEN=$(cat /run/secrets/actions_runtime_token)
  export ACTIONS_RESULTS_URL ACTIONS_RUNTIME_TOKEN SCCACHE_GHA_ENABLED=true
  echo "with-sccache: backend = GitHub Actions cache"
else
  echo "with-sccache: backend = local directory ${SCCACHE_DIR:-/root/.cache/sccache}"
fi
if sccache --start-server >/dev/null 2>&1; then
  export RUSTC_WRAPPER=sccache
else
  # a failing GHA backend (revoked token, outage): retry on the local directory
  echo "with-sccache: backend did not start; retrying on the local directory" >&2
  unset ACTIONS_RESULTS_URL ACTIONS_RUNTIME_TOKEN SCCACHE_GHA_ENABLED
  if sccache --start-server >/dev/null 2>&1; then export RUSTC_WRAPPER=sccache
  else echo "with-sccache: sccache unavailable; building without it" >&2; fi
fi
"$@"; rc=$?
sccache --show-stats || true
sccache --stop-server >/dev/null 2>&1 || true
exit $rc
