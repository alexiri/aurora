#!/usr/bin/env bash
# Shared helpers for the architecture guards (source this; do not run it).

# FIX-068: a Helm guard that cannot render must FAIL, not "skip and exit 0" — run-all.sh would
# otherwise end with "all guards passed" having checked nothing. Every dev box and the CI runner
# has helm (`just kind-live` needs it too). The one opt-out, for a box that genuinely has none,
# is AURORA_GUARDS_SKIP_HELM=1; it is ignored when CI=true.
require_helm() {
  local guard="$1"
  command -v helm >/dev/null 2>&1 && return 0
  if [ "${AURORA_GUARDS_SKIP_HELM:-}" = "1" ] && [ "${CI:-}" != "true" ]; then
    echo "$guard: SKIPPED chart checks (helm not installed, AURORA_GUARDS_SKIP_HELM=1)" >&2
    return 1
  fi
  echo "$guard: FAIL: helm is not installed, so the chart checks cannot run (set AURORA_GUARDS_SKIP_HELM=1 to skip locally; never allowed with CI=true)" >&2
  exit 1
}
