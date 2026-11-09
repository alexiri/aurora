#!/usr/bin/env bash
# RFE-019 / D-63 (18 §Deployment-neutral defaults): the normative defaults carry no deployment-specific
# content. The sandbox Containerfile's SANDBOX_BASE_IMAGE default is a public registry image pinned by
# digest, the built-in default agent brief has no persona names or backstory, and the legacy ALMA_BASE
# name is gone from every build path.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
fail=0

# 1. Containerfile default: docker.io/library/almalinux:<tag>@sha256:<64 hex>, no private host.
line=$(grep -E '^ARG SANDBOX_BASE_IMAGE=' images/sandbox/Containerfile || true)
if ! printf '%s\n' "$line" | grep -Eq '^ARG SANDBOX_BASE_IMAGE=docker\.io/library/almalinux:[A-Za-z0-9._-]+@sha256:[0-9a-f]{64}$'; then
  echo "no-deployment-defaults: FAIL: images/sandbox/Containerfile must default SANDBOX_BASE_IMAGE to a public docker.io/library/almalinux image pinned by sha256 digest (got: ${line:-<none>})" >&2
  fail=1
fi
if grep -nE '\.cern\.ch' images/sandbox/Containerfile | grep -vE '^[0-9]+:\s*#' >/dev/null; then
  echo "no-deployment-defaults: FAIL: a *.cern.ch host in an uncommented line of images/sandbox/Containerfile" >&2
  fail=1
fi

# 2. built-in default brief: no persona names, no backstory.
brief=$(grep -E '^pub const DEFAULT_AGENT_BRIEF' crates/aurora-config/src/raw.rs || true)
if [ -z "$brief" ]; then
  echo "no-deployment-defaults: FAIL: DEFAULT_AGENT_BRIEF not found in crates/aurora-config/src/raw.rs" >&2
  fail=1
elif printf '%s\n' "$brief" | grep -Eiq 'daneel|olivaw|cern|robot|positronic|sassy|snarky|never say you'; then
  echo "no-deployment-defaults: FAIL: DEFAULT_AGENT_BRIEF carries a persona or deployment-specific text" >&2
  fail=1
fi

# 3. the old build-arg name is not used anywhere (scripts, CI, Containerfiles, operator docs).
if git grep -nI 'ALMA_BASE' -- scripts deploy images .github docs/operator crates services ':!scripts/guards/no-deployment-defaults.sh' | grep .; then
  echo "no-deployment-defaults: FAIL: ALMA_BASE was renamed SANDBOX_BASE_IMAGE" >&2
  fail=1
fi

[ "$fail" -eq 0 ] || exit 1
echo "no-deployment-defaults: ok"
