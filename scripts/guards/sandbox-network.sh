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

# shellcheck source=scripts/guards/lib.sh
source scripts/guards/lib.sh
require_helm sandbox-network || exit 0

check() {
  local label="$1"; shift
  # (FIX-106) allowSharedLogin: not about the gateway's database role.
  helm template guard deploy/helm/aurora --set database.allowSharedLogin=true "$@" | analyse "$label"
}
# analyse <label>: the rendered chart on stdin.
analyse() {
  local label="$1"
  python3 -I -c '
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
# (FIX-102) The other half: what the CORE namespace admits from the sandboxes namespace. Every ingress
# rule whose peer is the sandboxes namespace may open gateway:4102 and runner:4105 and nothing else.
sb_ns = sb_pol[0]["metadata"]["namespace"]
admitted = []
for d in policies:
    if d in sb_pol:
        continue
    for rule in d["spec"].get("ingress") or []:
        for peer in rule.get("from") or []:
            sel = (peer.get("namespaceSelector") or {}).get("matchLabels") or {}
            if sel.get("kubernetes.io/metadata.name") == sb_ns:
                for p in rule.get("ports") or []:
                    admitted.append((d["metadata"]["name"], p["port"]))
got = sorted(p for _, p in admitted)
if got != [4102, 4105]:
    fail("the core namespace must admit exactly gateway:4102 + runner:4105 from the sandboxes namespace, got %s" % admitted)
print("sandbox-network: %s ok (egress 4102+4105 only, core ingress from sandboxes 4102+4105 only)" % label)
' "$label"
}

check "previews off" --set relay.enabled=false
# (FIX-056) single-switch form: relay.enabled mirrors config.previews.enabled.
check "previews on" --set relay.enabled=true --set relay.domain=previews.guard.invalid
# (P4-TERM) the web terminal must not widen the sandbox posture either: it dials :4108 from the
# api (control plane) and serves the PTY inside the sandbox — no new sandbox egress.
check "terminal on" --set terminal.enabled=true --set config.terminal.enabled=true
check "terminal+previews on" --set terminal.enabled=true --set config.terminal.enabled=true --set relay.enabled=true --set relay.domain=previews.guard.invalid

# (FIX-102) Mutation self-test: opening one more port (the api's 4101) from the sandboxes namespace in the
# gateway policy must fail the guard.
if helm template guard deploy/helm/aurora --set database.allowSharedLogin=true \
    | sed '0,/ports: \[{protocol: TCP, port: 4102}\]/s//ports: [{protocol: TCP, port: 4102}, {protocol: TCP, port: 4101}]/' \
    | analyse "self-test" >/dev/null 2>&1; then
  fail "self-test: a core ingress opened to the sandboxes namespace on 4101 passed"
fi
echo "sandbox-network: self-test ok"
