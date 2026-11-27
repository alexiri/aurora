#!/usr/bin/env bash
# Runs every architecture guard and the schema drift check. Fails on the first failure.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

echo "== schema guard (docs/03 vs migrations)"
python3 scripts/extract-schema.py --check

echo "== no-integration-names (SEC-33)"
bash scripts/guards/no-integration-names.sh

echo "== dep-graph"
bash scripts/guards/dep-graph.sh

echo "== edge-routes (Helm Ingress/HTTPRoute)"
bash scripts/guards/edge-routes.sh

echo "== hook-policies (OPS-024: pre-upgrade hooks admitted by hook-rendered policies)"
bash scripts/guards/hook-policies.sh

echo "== chart-ops (FIX-102: no v1 Endpoints, httpGet probes, ops-port peers, no silent 0.0.0.0/0, hook RBAC, PDB, half-enabled switches)"
bash scripts/guards/chart-ops.sh

echo "== helm-gateway-role (FIX-106: the gateway's least-privilege role is automatic, required, or explicitly waived)"
source scripts/guards/lib.sh
if require_helm helm-gateway-role; then bash deploy/helm/check-gateway-role.sh; fi

echo "== limits-readers (FIX-116: every limits.rs constant has a reader and a docs/02 row)"
bash scripts/guards/limits-readers.sh

echo "== blob-references (FIX-223: every *blob_key column is a reference the orphan-blob sweep honours)"
bash scripts/guards/blob-references.sh

echo "== live-settings-readers (FIX-168: no boot-config read of a live-settings key; use state.ops())"
bash scripts/guards/live-settings-readers.sh

echo "== docs-routes (FIX-119: docs/05 catalogue vs web/openapi.json, phase tags, docs/17 invariant table contiguous)"
python3 -I scripts/guards/docs-routes.py --self-test
python3 -I scripts/guards/docs-routes.py

echo "== release-environment (FIX-119: environment: release in exactly one push-only job)"
bash scripts/guards/release-environment.sh

echo "== e2e-skip (FIX-101: a SKIPPED: line fails an e2e script in CI; gated suites use require_or_skip)"
bash scripts/guards/e2e-skip.sh

echo "== image-pins (FIX-103: every base image pinned by digest)"
bash scripts/guards/image-pins.sh

echo "== no-dev-idp-in-release (SEC-42)"
bash scripts/guards/no-dev-idp-in-release.sh

echo "== ghostty-wasm (FIX-098: the vendored terminal WASM matches its recorded sha256 and the pinned package)"
bash scripts/guards/ghostty-wasm.sh

echo "== no-live-tokens (built-in plugin fixtures)"
bash scripts/guards/no-live-tokens.sh

echo "== dev-gateway-address (kind dev profile: in-cluster gateway proxy)"
bash scripts/guards/dev-gateway-address.sh

echo "== no-conflict-markers"
bash scripts/guards/no-conflict-markers.sh

echo "== sandbox-network (SEC-09: sandboxes reach gateway:4102 + runner:4105 only, previews never widen it)"
bash scripts/guards/sandbox-network.sh

echo "== no-deployment-defaults (RFE-019: neutral brief, public digest-pinned sandbox base)"
bash scripts/guards/no-deployment-defaults.sh

echo "== helm-values-docs (FIX-069: docs/23 documents every values.yaml key and names none that do not exist)"
bash scripts/guards/helm-values-docs.sh

echo "== settings-chart (SET-1, D-237(c): the default render contains no movable instance-setting key)"
bash scripts/guards/settings-chart.sh

echo "== agent-content-advisory (SEC-91, SEC-150: the gateway, policy, hook host and runner never read the agent folder or package skills)"
bash scripts/guards/agent-content-advisory.sh --self-test
bash scripts/guards/agent-content-advisory.sh

echo "== dev scripts syntax (VEL-002, FIX-255; shellcheck when installed, else bash -n)"
dev_scripts=(scripts/dev/quick-gate.sh scripts/dev/heavy.sh scripts/dev/test-pg.sh scripts/dev/test.sh scripts/dev/test_test-runner.sh)
if command -v shellcheck >/dev/null 2>&1; then
  shellcheck "${dev_scripts[@]}"
else
  for s in "${dev_scripts[@]}"; do bash -n "$s"; done
fi

echo "== test-runner (FIX-255: one shared test Postgres, one crate at a time, refusals; stubbed podman and cargo)"
bash scripts/dev/test_test-runner.sh

echo "== all guards passed"
