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

# 4. (FIX-104, D-180) `gateway.insecure_loopback_injection` is test-only: the Helm chart never sets
# it (no value, template line or env name beyond the one `fail` that refuses it in `config`), and
# the gateway refuses to start with it inside a cluster.
chart_hits=$(git grep -nI 'insecure_loopback_injection\|INSECURE_LOOPBACK_INJECTION' -- deploy ':!deploy/helm/aurora/templates/configmap.yaml' || true)
if [ -n "$chart_hits" ]; then
  echo "no-deployment-defaults: FAIL: gateway.insecure_loopback_injection is test-only; deploy/ must not set it:" >&2
  echo "$chart_hits" >&2
  fail=1
fi
# shellcheck disable=SC2016  # $cfg is the Helm template's variable
if ! grep -q 'hasKey (default (dict) \$cfg.gateway) "insecure_loopback_injection"' deploy/helm/aurora/templates/configmap.yaml; then
  echo "no-deployment-defaults: FAIL: configmap.yaml must refuse config.gateway.insecure_loopback_injection" >&2
  fail=1
fi
if ! grep -q 'KUBERNETES_SERVICE_HOST' services/aurora-gateway/src/main.rs; then
  echo "no-deployment-defaults: FAIL: the gateway must refuse insecure_loopback_injection in a cluster (main.rs)" >&2
  fail=1
fi

# 5. (P4-LLM-1, C-812, D-260(e)) `llm.dev_upstream_overrides` is test-only as well: the chart never renders it
# (only the one `fail` in configmap.yaml names it), and the config layer refuses it without
# gateway.insecure_loopback_injection and inside a cluster.
override_hits=$(git grep -nI 'dev_upstream_overrides' -- deploy ':!deploy/helm/aurora/templates/configmap.yaml' || true)
if [ -n "$override_hits" ]; then
  echo "no-deployment-defaults: FAIL: llm.dev_upstream_overrides is test-only; deploy/ must not set it:" >&2
  echo "$override_hits" >&2
  fail=1
fi
# shellcheck disable=SC2016  # $cfg is the Helm template's variable
if ! grep -q 'hasKey (default (dict) \$cfg.llm) "dev_upstream_overrides"' deploy/helm/aurora/templates/configmap.yaml; then
  echo "no-deployment-defaults: FAIL: configmap.yaml must refuse config.llm.dev_upstream_overrides" >&2
  fail=1
fi
if ! grep -q 'KUBERNETES_SERVICE_HOST' crates/aurora-config/src/validate.rs; then
  echo "no-deployment-defaults: FAIL: the config layer must refuse llm.dev_upstream_overrides in a cluster (validate.rs)" >&2
  fail=1
fi

# 6. (P4-011B-3, C-837, D-267(b)) `push.dev_endpoint_overrides` is a test seam as well: the chart never renders it
# (only the one `fail` in configmap.yaml names it) and the config layer refuses it unless every target and
# `server.external_url` are loopback, and inside a cluster.
push_hits=$(git grep -nI 'dev_endpoint_overrides' -- deploy ':!deploy/helm/aurora/templates/configmap.yaml' || true)
if [ -n "$push_hits" ]; then
  echo "no-deployment-defaults: FAIL: push.dev_endpoint_overrides is test-only; deploy/ must not set it:" >&2
  echo "$push_hits" >&2
  fail=1
fi
# shellcheck disable=SC2016  # $ps is the Helm template's variable
if ! grep -q 'hasKey \$ps "dev_endpoint_overrides"' deploy/helm/aurora/templates/configmap.yaml; then
  echo "no-deployment-defaults: FAIL: configmap.yaml must refuse config.push.dev_endpoint_overrides" >&2
  fail=1
fi
if ! grep -q 'dev_endpoint_overrides' crates/aurora-config/src/validate.rs; then
  echo "no-deployment-defaults: FAIL: the config layer must refuse push.dev_endpoint_overrides outside loopback and in a cluster (validate.rs)" >&2
  fail=1
fi

[ "$fail" -eq 0 ] || exit 1
echo "no-deployment-defaults: ok"
