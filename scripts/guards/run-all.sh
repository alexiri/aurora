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

echo "== no-dev-idp-in-release (SEC-42)"
bash scripts/guards/no-dev-idp-in-release.sh

echo "== no-live-tokens (built-in plugin fixtures)"
bash scripts/guards/no-live-tokens.sh

echo "== dev-gateway-address (kind dev profile: in-cluster gateway proxy)"
bash scripts/guards/dev-gateway-address.sh

echo "== no-conflict-markers"
bash scripts/guards/no-conflict-markers.sh

echo "== no-deployment-defaults (RFE-019: neutral brief, public digest-pinned sandbox base)"
bash scripts/guards/no-deployment-defaults.sh

echo "== all guards passed"
