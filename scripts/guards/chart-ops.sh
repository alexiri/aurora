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
                     ("-relay", 4193), ("-terminator", 4194), ("-connector-host", 4192)):
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
want = {4101, 4103, 4104, 4106, 4192, 4193, 4194}
if ports != want:
    sys.exit("chart-ops: FAIL: monitoring ports %s, want %s" % (sorted(ports), sorted(want)))
print("chart-ops: monitoring peers ok")
'
"${H[@]}" --set monitoring.podMonitor.enabled=true --set relay.enabled=true --set relay.domain=previews.guard.invalid \
  | grep -c 'kind: PodMonitor' | grep -qx 6 || fail "the PodMonitor flag must render six monitors (api, gateway, hookhost, runner, relay, connector host)"

# --- (P4-SSH-2, docs/15 §SSH access) the terminator: nothing without ssh.enabled; with it the workload has no
#     service-account token, database URL or Vault, its policy admits only networkPolicy.sshFrom on 2222 and
#     reaches only the api (4101), the runner (4108) and DNS; the CA key reaches the api pods only, the host key
#     the terminator pods only; a PodMonitor and the ops port 4194 follow monitoringFrom ---
SSH_ON=(--set ssh.enabled=true --set terminal.enabled=true --set 'networkPolicy.sshFrom[0].ipBlock.cidr=203.0.113.0/24')
if "${H[@]}" | grep -qE 'component: terminator|aurora-terminator|AURORA_SSH_|AURORA_TERMINATOR|AURORA_BOOTSTRAP_TERMINATOR'; then
  fail "SSH resources rendered while ssh.enabled is false (SSH off must change nothing)"
fi
"${H[@]}" "${SSH_ON[@]}" --set monitoring.podMonitor.enabled=true \
  --set 'networkPolicy.monitoringFrom[0].namespaceSelector.matchLabels.kubernetes\.io/metadata\.name=monitoring' \
  | python3 -I -c '
import sys, yaml
docs = [d for d in yaml.safe_load_all(sys.stdin) if d]
def fail(m): sys.exit("chart-ops: FAIL: ssh: " + m)
def env(d): return d["spec"]["template"]["spec"]["containers"][0].get("env") or []
dep = {d["metadata"]["name"]: d for d in docs if d["kind"] == "Deployment"}
term = next((d for n, d in dep.items() if n.endswith("-terminator")), None)
if term is None: fail("no terminator Deployment")
if term["spec"]["replicas"] != 2: fail("terminator replicas default must be 2")
pod = term["spec"]["template"]["spec"]
if pod.get("automountServiceAccountToken") is not False or pod.get("serviceAccountName"):
    fail("the terminator pod must have no service-account token")
names = {e["name"] for e in env(term)}
if names != {"AURORA_CONFIG", "AURORA_TERMINATOR_TOKEN", "AURORA_SSH_HOST_KEY"}:
    fail("terminator env must be the config path, its token and the host key, got %s" % sorted(names))
for n, d in dep.items():
    en = {e["name"] for e in env(d)}
    if n.endswith("-api") and not {"AURORA_SSH_CA_KEY", "AURORA_BOOTSTRAP_TERMINATOR_TOKEN"} <= en:
        fail("the api must get the CA key and the terminator anchor")
    if not n.endswith("-api") and "AURORA_SSH_CA_KEY" in en: fail("the CA key reached %s" % n)
    if not n.endswith("-terminator") and "AURORA_SSH_HOST_KEY" in en: fail("the host key reached %s" % n)
svc = {d["metadata"]["name"]: d for d in docs if d["kind"] == "Service"}
pub = next((s for n, s in svc.items() if n.endswith("-terminator-ssh")), None)
if pub is None or pub["spec"]["type"] != "LoadBalancer" or pub["spec"]["externalTrafficPolicy"] != "Local":
    fail("the published SSH Service must be a LoadBalancer with externalTrafficPolicy Local")
if [(p["port"], p["targetPort"]) for p in pub["spec"]["ports"]] != [(22, 2222)]:
    fail("the SSH Service must publish 22 -> 2222")
ops = next((s for n, s in svc.items() if n.endswith("-terminator")), None)
if ops is None or ops["spec"].get("clusterIP") != "None" or [p["port"] for p in ops["spec"]["ports"]] != [4194]:
    fail("the ops Service must be headless on 4194 only")
pol = {d["metadata"]["name"]: d for d in docs if d["kind"] == "NetworkPolicy"}
tp = next((p for n, p in pol.items() if n.endswith("-terminator")), None)
if tp is None: fail("no terminator NetworkPolicy")
ing = tp["spec"]["ingress"]
if len(ing) != 1 or ing[0]["from"] != [{"ipBlock": {"cidr": "203.0.113.0/24"}}] or [p["port"] for p in ing[0]["ports"]] != [2222]:
    fail("terminator ingress must be sshFrom on 2222 only, got %s" % ing)
eg = {(r["to"][0]["podSelector"]["matchLabels"]["app.kubernetes.io/component"], r["ports"][0]["port"]) for r in tp["spec"]["egress"]}
if eg != {("api", 4101), ("runner", 4108)}: fail("terminator egress must be api:4101 + runner:4108, got %s" % sorted(eg))
def ports_from(p, comp):
    return {q["port"] for r in p["spec"].get("ingress") or [] for f in r.get("from") or []
            if f.get("podSelector", {}).get("matchLabels", {}).get("app.kubernetes.io/component") == comp for q in r["ports"]}
if ports_from(next(p for n, p in pol.items() if n.endswith("-api")), "terminator") != {4101}: fail("api must admit the terminator on 4101")
if ports_from(next(p for n, p in pol.items() if n.endswith("-runner")), "terminator") != {4108}: fail("runner must admit the terminator on 4108")
if sum(1 for d in docs if d["kind"] == "PodMonitor" and d["metadata"]["name"].endswith("-terminator")) != 1: fail("no terminator PodMonitor")
if not any(d["kind"] == "PodDisruptionBudget" and d["metadata"]["name"].endswith("-terminator") for d in docs): fail("no terminator PDB at two replicas")
# The migrate hook Job validates the API configuration, so it needs the CA key too (and no other Job does).
for d in docs:
    if d["kind"] == "Job":
        names = {e["name"] for c in d["spec"]["template"]["spec"]["containers"] for e in (c.get("env") or [])}
        want = d["metadata"]["name"].endswith("-migrate")
        if ("AURORA_SSH_CA_KEY" in names) != want:
            fail("the CA key must reach the migrate Job and no other Job (%s)" % d["metadata"]["name"])
print("chart-ops: ssh terminator posture ok")
'
"${H[@]}" "${SSH_ON[@]}" | analyse "ssh on"
echo "chart-ops: ssh render checks ok"

# --- tracing (FIX-146 Part C, D-232): telemetryTo egress, the web env only with an endpoint, the NOTES warning ---
telemetry_check() { # expect: none | policy, args after: helm --set flags
  local expect="$1" ratio="$2"; shift 2
  "${H[@]}" --set 'connectors[0].id=trace' --set 'connectors[0].image=guard.invalid/c:1' \
    --set 'connectors[0].tokenSecret.name=s' --set 'connectors[0].tokenSecret.key=k' "$@" \
    | python3 -I -c '
import sys, yaml
expect, ratio = sys.argv[1], sys.argv[2]
docs = [d for d in yaml.safe_load_all(sys.stdin) if d]
def fail(m): sys.exit("chart-ops: FAIL: tracing: " + m)
pol = [d for d in docs if d["kind"] == "NetworkPolicy" and d["metadata"]["name"].endswith("-telemetry")]
web = [d for d in docs if d["kind"] == "Deployment" and d["metadata"]["name"].endswith("-web")][0]
env = {e["name"]: e.get("value") for c in web["spec"]["template"]["spec"]["containers"] for e in c["env"]}
if expect == "none":
    if pol: fail("a telemetry policy rendered without telemetryTo")
else:
    if len(pol) != 1: fail("want one -telemetry NetworkPolicy, got %d" % len(pol))
    comps = [e for e in pol[0]["spec"]["podSelector"]["matchExpressions"] if e["key"].endswith("/component")][0]["values"]
    want = {"api", "gateway", "hookhost", "runner", "relay", "terminator", "connector-host", "connector-trace"}
    if set(comps) != want: fail("telemetry policy components %s, want %s" % (sorted(comps), sorted(want)))
    if "web" in comps: fail("the web workload must not get collector egress")
    if pol[0]["spec"]["policyTypes"] != ["Egress"] or not pol[0]["spec"]["egress"][0]["to"]:
        fail("the telemetry policy must be egress-only with the operator peers")
    if not any(p["metadata"]["name"].endswith("-default-deny") for p in docs if p["kind"] == "NetworkPolicy"): fail("no default-deny")
got = env.get("AURORA_TRACING_SAMPLE_RATIO")
if got != (ratio if ratio != "-" else None): fail("web AURORA_TRACING_SAMPLE_RATIO=%r, want %r" % (got, ratio))
' "$expect" "$ratio"
}
telemetry_check none - || exit 1
if telemetry_check none - --set 'networkPolicy.telemetryTo[0].ipBlock.cidr=10.0.0.9/32' >/dev/null 2>&1; then
  fail "self-test: the 'no telemetry policy' check passed although telemetryTo rendered one"
fi
telemetry_check policy 1 --set config.telemetry.otlp_endpoint=http://c:4318 --set 'networkPolicy.telemetryTo[0].ipBlock.cidr=10.0.0.9/32' || exit 1
telemetry_check policy 0.0 --set config.telemetry.otlp_endpoint=http://c:4318 --set config.telemetry.sample_ratio=0.0 --set 'networkPolicy.telemetryTo[0].ipBlock.cidr=10.0.0.9/32' || exit 1
telemetry_check policy 0.25 --set config.telemetry.otlp_endpoint=http://c:4318 --set config.telemetry.sample_ratio=0.25 --set 'networkPolicy.telemetryTo[0].ipBlock.cidr=10.0.0.9/32' || exit 1
# an endpoint with no telemetryTo: no policy, the env still rendered, and NOTES.txt warns
telemetry_check none 1 --set config.telemetry.otlp_endpoint=http://c:4318 || exit 1
# NOTES.txt is not part of `helm template` output, and `helm install --dry-run=client` is no help:
# helm v3 still contacts the cluster ("Kubernetes cluster unreachable" on a runner, "exists and cannot
# be imported" against a developer's cluster); only helm v4 is truly client-side. So render the same
# template through `helm template` from a scratch copy of the chart in which NOTES.txt is wrapped as a
# YAML block scalar. Works unchanged on helm v3 and v4, with or without a cluster.
notes_chart="$(mktemp -d)"
trap 'rm -f "$planted"; rm -rf "$notes_chart"' EXIT
cp -r deploy/helm/aurora "$notes_chart/aurora"
rm "$notes_chart/aurora/templates/NOTES.txt"
{
  echo '{{- define "guard.notes" -}}'
  cat deploy/helm/aurora/templates/NOTES.txt
  printf '\n{{- end }}\nnotes: |\n{{ include "guard.notes" . | indent 2 }}\n'
} >"$notes_chart/aurora/templates/zz-guard-notes.yaml"
# Capture the exit status explicitly: under `set -e` a failing `notes="$(...)"` would end the guard silently.
notes_of() {
  local out
  if ! out="$(helm template guard "$notes_chart/aurora" -n aurora --set database.allowSharedLogin=true --show-only templates/zz-guard-notes.yaml "$@" 2>&1)"; then
    printf '%s\n' "$out" >&2
    fail "rendering NOTES.txt failed (helm $(helm version --short 2>/dev/null)); output above"
  fi
  printf '%s' "$out"
}
notes="$(notes_of --set config.telemetry.otlp_endpoint=http://c:4318)" || exit 1
grep -q 'networkPolicy.telemetryTo is empty' <<<"$notes" || fail "NOTES.txt must warn when an endpoint is set and telemetryTo is empty"
notes="$(notes_of --set config.telemetry.otlp_endpoint=http://c:4318 --set 'networkPolicy.telemetryTo[0].ipBlock.cidr=10.0.0.9/32')" || exit 1
! grep -q 'networkPolicy.telemetryTo is empty' <<<"$notes" || fail "NOTES.txt warns although telemetryTo is set"
echo "chart-ops: tracing egress, web env and NOTES ok"

# --- half-enabled switches fail the render ---
must_fail() {
  local what="$1"; shift
  if "${H[@]}" "$@" >/dev/null 2>&1; then fail "$what rendered but must refuse"; fi
}
must_fail "config.scim.enabled=true without scim.enabled" --set config.scim.enabled=true
must_fail "config.terminal.enabled=true without terminal.enabled" --set config.terminal.enabled=true
must_fail "terminal without a runner" --set terminal.enabled=true --set runner.enabled=false
must_fail "previews without a runner" --set relay.enabled=true --set relay.domain=previews.guard.invalid --set runner.enabled=false
must_fail "ssh without networkPolicy.sshFrom" --set ssh.enabled=true --set terminal.enabled=true
must_fail "config.ssh.enabled=true without ssh.enabled" --set config.ssh.enabled=true
must_fail "ssh.enabled with config.ssh.enabled=false" "${SSH_ON[@]}" --set config.ssh.enabled=false
must_fail "ssh without a runner" "${SSH_ON[@]}" --set runner.enabled=false
must_fail "ssh without the terminal's wiring" --set ssh.enabled=true --set 'networkPolicy.sshFrom[0].ipBlock.cidr=203.0.113.0/24'
PUSH_ON=(--set push.enabled=true --set 'push.networkPolicy.egressTo[0].ipBlock.cidr=0.0.0.0/0')
must_fail "push.enabled without push.networkPolicy.egressTo" --set push.enabled=true
must_fail "config.push.enabled=true without push.enabled" --set config.push.enabled=true
must_fail "push.enabled with config.push.enabled=false" "${PUSH_ON[@]}" --set config.push.enabled=false
must_fail "config.push.dev_endpoint_overrides" --set config.push.dev_endpoint_overrides.x=y
"${H[@]}" "${PUSH_ON[@]}" >/dev/null || fail "push.enabled with egressTo must render"
"${H[@]}" "${PUSH_ON[@]}" --set push.vapid.existingSecret=vk >/dev/null || fail "push.enabled with an operator key must render"
"${H[@]}" --set runner.enabled=false >/dev/null || fail "runner.enabled=false alone must render"
echo "chart-ops: half-enabled switches refuse to render"

# --- (SET-5, D-237(d)) the terminal and SCIM chart values render `available`, never the movable switch ---
toml_of() { "${H[@]}" "$@" | python3 -I -c '
import sys, tomllib, yaml
out = {}
for d in yaml.safe_load_all(sys.stdin):
    if d and d.get("kind") == "ConfigMap":
        for n, t in (d.get("data") or {}).items():
            if n.endswith(".toml"):
                out.update(tomllib.loads(t))
print(repr({k: out.get(k) for k in ("terminal", "scim")}))
'; }
dflt="$(toml_of)" || fail "default render failed"
[ "$dflt" = "{'terminal': None, 'scim': None}" ] || fail "default render must set neither [terminal] nor [scim]: $dflt"
on="$(toml_of --set terminal.enabled=true --set scim.enabled=true)" || fail "terminal+scim render failed"
[ "$on" = "{'terminal': {'available': True}, 'scim': {'available': True}}" ] || fail "terminal.enabled / scim.enabled must render available only: $on"
rate="$(toml_of --set scim.ratePerMinute=900)" || fail "scim.ratePerMinute render failed"
[ "$rate" = "{'terminal': None, 'scim': {'rate_per_minute': 900}}" ] || fail "scim.ratePerMinute must render rate_per_minute only when set: $rate"
locked="$(toml_of --set terminal.enabled=true --set config.terminal.enabled=false)" || fail "terminal on with config.terminal.enabled=false must render (kept off, locked)"
[ "$locked" = "{'terminal': {'available': True, 'enabled': False}, 'scim': None}" ] || fail "an operator-set config.terminal.enabled must be kept next to available: $locked"
echo "chart-ops: terminal and SCIM render availability"
