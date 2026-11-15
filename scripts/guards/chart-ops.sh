#!/usr/bin/env bash
# Operational posture of the Helm chart (FIX-102, docs/15 §Networking, §Observability, §Upgrades):
#   - no template reads the deprecated core v1 Endpoints (the API server addresses come from the
#     discovery.k8s.io/v1 EndpointSlice; `helm upgrade` on Kubernetes >= 1.33 warns about v1 Endpoints);
#   - readiness is `httpGet /readyz` and liveness `httpGet /healthz` wherever the workload has an ops router;
#   - every ops port (4101 api, 4103 gateway, 4104 hookhost, 4106 runner, 4192 connector host, 4193 relay)
#     has an allowed peer when networkPolicy.monitoringFrom is set, and none without it;
#   - the runner declares its ops port, and its Kubernetes API egress never silently opens 0.0.0.0/0;
#   - half-enabled switches (scim, terminal/previews without a runner) refuse to render;
#   - hook Roles name the Secrets they get/patch (resourceNames), one ServiceAccount per Job;
#   - a PodDisruptionBudget only from two replicas.
# Each check is also run against a planted violation (mutation self-test).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

fail() { echo "chart-ops: FAIL: $*" >&2; exit 1; }

# shellcheck source=scripts/guards/lib.sh
source scripts/guards/lib.sh
require_helm chart-ops || exit 0

# --- no v1 Endpoints (grep guard, with a planted-pattern self-test) ---
endpoints_scan() { grep -rnE 'lookup +"v1" +"Endpoints"|"v1" "Endpoints"|kind: Endpoints$' "$@" 2>/dev/null || true; }
found="$(endpoints_scan deploy/helm)"
[ -z "$found" ] || fail "a template reads the deprecated v1 Endpoints (use discovery.k8s.io/v1 EndpointSlice): $found"
planted="$(mktemp)"
trap 'rm -f "$planted"' EXIT
echo '{{- $eps := lookup "v1" "Endpoints" "default" "kubernetes" }}' >"$planted"
[ -n "$(endpoints_scan "$planted")" ] || fail "self-test: the v1 Endpoints scan missed a planted lookup"

H=(helm template guard deploy/helm/aurora --set database.allowSharedLogin=true)

# --- render checks on one rendered chart (stdin) ---
analyse() {
  python3 -I -c '
import sys, yaml
label = sys.argv[1]
docs = [d for d in yaml.safe_load_all(sys.stdin) if d]
def fail(msg):
    sys.exit("chart-ops: FAIL: %s: %s" % (label, msg))
deps = {d["metadata"]["name"]: d for d in docs if d["kind"] == "Deployment"}
# probes
for suffix, port in (("-api", 4101), ("-hookhost", 4104), ("-gateway", 4103), ("-runner", 4106),
                     ("-relay", 4193), ("-connector-host", 4192)):
    for name, d in deps.items():
        if not name.endswith(suffix) or "connector-host" in name and suffix != "-connector-host":
            continue
        c = d["spec"]["template"]["spec"]["containers"][0]
        r, l = c.get("readinessProbe", {}), c.get("livenessProbe", {})
        if r.get("httpGet", {}).get("path") != "/readyz" or r["httpGet"].get("port") != port:
            fail("%s readiness must be httpGet /readyz on %d, got %s" % (name, port, r))
        if l.get("httpGet", {}).get("path") != "/healthz" or l["httpGet"].get("port") != port:
            fail("%s liveness must be httpGet /healthz on %d, got %s" % (name, port, l))
runner = [d for n, d in deps.items() if n.endswith("-runner")]
if runner:
    ports = [p["containerPort"] for p in runner[0]["spec"]["template"]["spec"]["containers"][0]["ports"]]
    if 4106 not in ports:
        fail("the runner must declare its ops port 4106, got %s" % ports)
# runner egress to the Kubernetes API is never 0.0.0.0/0 unless kubeApiOpen (the caller passes expect_open)
expect_open = len(sys.argv) > 2 and sys.argv[2] == "open"
for d in docs:
    if d["kind"] == "NetworkPolicy" and d["metadata"]["name"].endswith("aurora-runner"):
        cidrs = [t["ipBlock"]["cidr"] for r in d["spec"].get("egress") or [] for t in r.get("to") or [] if "ipBlock" in t]
        if ("0.0.0.0/0" in cidrs) != expect_open:
            fail("runner egress opens 0.0.0.0/0 = %s, expected %s" % ("0.0.0.0/0" in cidrs, expect_open))
        for r in d["spec"].get("egress") or []:
            if "to" in r and not r["to"]:
                fail("an egress rule with an empty `to` allows every destination")
# hook Roles name their Secrets
for d in docs:
    if d["kind"] == "Role" and d["metadata"]["name"].endswith(("-hooks", "-hooks-ca")):
        for rule in d["rules"]:
            if set(rule["verbs"]) & {"get", "patch"} and not rule.get("resourceNames"):
                fail("hook Role %s: %s without resourceNames" % (d["metadata"]["name"], rule["verbs"]))
# The migrate hook policy admits DNS itself: on a first install no regular allow-dns policy exists yet, so a
# hook policy that selects the pod without a DNS rule leaves the Job unable to resolve the database host.
for d in docs:
    if d["kind"] == "NetworkPolicy" and d["metadata"]["name"].endswith("-migrate") \
            and "helm.sh/hook" in (d["metadata"].get("annotations") or {}):
        ports = {p["port"] for r in d["spec"].get("egress") or [] for p in r.get("ports") or []}
        if 53 not in ports:
            fail("hook policy %s has no DNS (53) egress: the migrate Job cannot resolve the database on a first install" % d["metadata"]["name"])
# one ServiceAccount per hook Job
sas = {}
for d in docs:
    if d["kind"] == "Job" and "helm.sh/hook" in (d["metadata"].get("annotations") or {}):
        sa = d["spec"]["template"]["spec"].get("serviceAccountName")
        if sa:
            sas.setdefault(sa, []).append(d["metadata"]["name"])
for sa, jobs in sas.items():
    if len(jobs) > 1:
        fail("ServiceAccount %s is shared by hook Jobs %s" % (sa, jobs))
# PDB only from two replicas
for d in docs:
    if d["kind"] == "PodDisruptionBudget":
        dep = deps.get(d["metadata"]["name"])
        if dep is None or dep["spec"]["replicas"] < 2:
            fail("PodDisruptionBudget %s at fewer than two replicas" % d["metadata"]["name"])
print("chart-ops: %s ok" % label)
' "$@"
}

"${H[@]}" | analyse "defaults"
"${H[@]}" --set relay.enabled=true --set relay.domain=previews.guard.invalid --set terminal.enabled=true \
  --set api.replicas=2 | analyse "previews+terminal, 2 api replicas"
"${H[@]}" --set networkPolicy.kubeApiOpen=true | analyse "kubeApiOpen" open
"${H[@]}" --set 'networkPolicy.kubeApiCidrs={10.1.2.3/32}' | analyse "kubeApiCidrs"
# PDB check actually bites: a planted PDB for a one-replica workload must fail.
if "${H[@]}" | python3 -I -c '
import sys, yaml
docs = [d for d in yaml.safe_load_all(sys.stdin) if d]
one = next(d for d in docs if d["kind"] == "Deployment" and d["metadata"]["name"].endswith("-runner"))
docs.append({"apiVersion": "policy/v1", "kind": "PodDisruptionBudget", "metadata": {"name": one["metadata"]["name"]}, "spec": {"minAvailable": 1}})
yaml.safe_dump_all(docs, sys.stdout)
' | analyse "self-test: planted PDB" >/dev/null 2>&1; then
  fail "self-test: a PodDisruptionBudget on a single replica passed"
fi
# readiness check bites: a TCP probe on the api must fail.
if "${H[@]}" | python3 -I -c '
import sys, yaml
docs = [d for d in yaml.safe_load_all(sys.stdin) if d]
api = next(d for d in docs if d["kind"] == "Deployment" and d["metadata"]["name"].endswith("-api"))
api["spec"]["template"]["spec"]["containers"][0]["readinessProbe"] = {"tcpSocket": {"port": 4101}}
yaml.safe_dump_all(docs, sys.stdout)
' | analyse "self-test: planted tcp probe" >/dev/null 2>&1; then
  fail "self-test: a TCP readiness probe on the api passed"
fi

# --- monitoring peers: nothing by default, every ops port with monitoringFrom ---
if "${H[@]}" | grep -q -- '-monitoring$'; then fail "a monitoring policy rendered without networkPolicy.monitoringFrom"; fi
"${H[@]}" --set 'networkPolicy.monitoringFrom[0].namespaceSelector.matchLabels.kubernetes\.io/metadata\.name=monitoring' \
  --set relay.enabled=true --set relay.domain=previews.guard.invalid \
  | python3 -I -c '
import sys, yaml
docs = [d for d in yaml.safe_load_all(sys.stdin) if d]
pol = [d for d in docs if d["kind"] == "NetworkPolicy" and d["metadata"]["name"].endswith("-monitoring")]
if len(pol) != 1:
    sys.exit("chart-ops: FAIL: expected one -monitoring NetworkPolicy, got %d" % len(pol))
ports = {p["port"] for r in pol[0]["spec"]["ingress"] for p in r["ports"]}
want = {4101, 4103, 4104, 4106, 4192, 4193}
if ports != want:
    sys.exit("chart-ops: FAIL: monitoring ports %s, want %s" % (sorted(ports), sorted(want)))
print("chart-ops: monitoring peers ok")
'
"${H[@]}" --set monitoring.podMonitor.enabled=true --set relay.enabled=true --set relay.domain=previews.guard.invalid \
  | grep -c 'kind: PodMonitor' | grep -qx 6 || fail "the PodMonitor flag must render six monitors (api, gateway, hookhost, runner, relay, connector host)"

# --- half-enabled switches fail the render ---
must_fail() {
  local what="$1"; shift
  if "${H[@]}" "$@" >/dev/null 2>&1; then fail "$what rendered but must refuse"; fi
}
must_fail "config.scim.enabled=true without scim.enabled" --set config.scim.enabled=true
must_fail "terminal without a runner" --set terminal.enabled=true --set runner.enabled=false
must_fail "previews without a runner" --set relay.enabled=true --set relay.domain=previews.guard.invalid --set runner.enabled=false
"${H[@]}" --set runner.enabled=false >/dev/null || fail "runner.enabled=false alone must render"
echo "chart-ops: half-enabled switches refuse to render"
