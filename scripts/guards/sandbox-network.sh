#!/usr/bin/env bash
# Sandbox network isolation (SEC-09, docs/15 §Networking, docs/17): sandboxes reach the gateway
# proxy (:4102) and the runner sandbox channel (:4105) and NOTHING else — the chart's sandboxes
# namespace has a default-deny policy and a single egress policy. Port previews (P4-PRV-2) must
# not widen this: the relay (:4112), the runner stream listener (:4108) and the api (:4101) are
# control-plane only. Renders the chart with previews off and on and checks both.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

fail() { echo "sandbox-network: FAIL: $*" >&2; exit 1; }

if ! command -v helm >/dev/null 2>&1; then
  echo "sandbox-network: helm not installed; skipping" >&2
  exit 0
fi

check() {
  local label="$1"; shift
  helm template guard deploy/helm/aurora "$@" | python3 -I -c '
import sys, yaml
label = sys.argv[1]
docs = [d for d in yaml.safe_load_all(sys.stdin) if d]
def fail(msg):
    sys.exit("sandbox-network: FAIL: %s: %s" % (label, msg))
policies = [d for d in docs if d["kind"] == "NetworkPolicy"]
# The sandboxes namespace policies are the two named ones (they carry their namespace
# explicitly); every other NetworkPolicy lives in the release namespace.
sb_pol = [d for d in policies if d["metadata"].get("namespace", "").endswith("sandboxes")]
names = sorted(d["metadata"]["name"] for d in sb_pol)
if names != ["allow-gateway-runner", "default-deny"]:
    fail("the sandboxes namespace must have exactly default-deny + allow-gateway-runner, got %s" % names)
deny = next(d for d in sb_pol if d["metadata"]["name"] == "default-deny")
allow = next(d for d in sb_pol if d["metadata"]["name"] == "allow-gateway-runner")
if deny["spec"]["podSelector"] != {} or sorted(deny["spec"]["policyTypes"]) != ["Egress", "Ingress"]:
    fail("the sandboxes default-deny must select every pod and deny ingress AND egress")
if sorted(allow["spec"]["policyTypes"]) != ["Egress"]:
    fail("allow-gateway-runner must be egress-only")
ports = sorted(p["port"] for r in allow["spec"]["egress"] for p in r.get("ports", []))
if ports != [4102, 4105]:
    fail("sandboxes egress must be exactly gateway:4102 + runner:4105, got %s" % ports)
print("sandbox-network: %s ok (egress 4102+4105 only)" % label)
' "$label"
}

check "previews off" --set relay.enabled=false
check "previews on" --set relay.enabled=true --set relay.domain=previews.guard.invalid --set config.previews.enabled=true
