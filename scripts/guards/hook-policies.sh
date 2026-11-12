#!/usr/bin/env bash
# Every pre-upgrade hook Job that talks to the Kubernetes API is admitted by a policy rendered as
# a hook (OPS-024, the 2026-11-11 production timeout). On `helm upgrade` the hooks run before any
# regular manifest, under the previous release's default-deny NetworkPolicies, so a hook needing
# API or database access whose allow policy is a regular resource — or missing — hangs the upgrade
# (incident root cause: the secrets hook pod carried no `app.kubernetes.io/component` label, so
# the default-deny policy selected it and no allow policy did; its `kubectl` calls never returned).
# This guard proves the RENDER, not enforcement: it checks that every rendered hook Job whose
# pod template (a) sets serviceAccountName (it talks to the Kubernetes API) and (b) has a
# `helm.sh/hook` annotation containing pre-upgrade is matched by at least one rendered
# NetworkPolicy podSelector — and, with networkPolicy.cilium.enabled, also by a
# CiliumNetworkPolicy endpointSelector (the API server is Cilium's `kube-apiserver` entity, which
# no NetworkPolicy ipBlock ever matches, so the NetworkPolicy alone admits nothing there).
# Exempt: hooks without pre-upgrade (gateway-ca: pre-install runs before any policy exists) and
# hook pods that do not use the API (migrate: automountServiceAccountToken false, no
# serviceAccountName; its database egress policy is a regular resource and install order covers it).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

fail() { echo "hook-policies: FAIL: $*" >&2; exit 1; }

if ! command -v helm >/dev/null 2>&1; then
  echo "hook-policies: helm not installed; skipping chart checks" >&2
  exit 0
fi

check_render() {
  local label="$1"; shift
  helm template guard deploy/helm/aurora "$@" | python3 -I -c '
import sys, yaml
label = sys.argv[1]
HOOK_KEY = "helm.sh/hook"

def selector_matches(policy_selector, pod_labels):
    # An empty podSelector (default-deny, allow-dns) matches every pod but admits nothing on its
    # own: default-deny has no egress rules at all. It must not count as the hook’s allow policy,
    # or deleting the -hooks policy would leave the guard green (default-deny alone is exactly the
    # incident’s failure condition).
    if not (policy_selector.get("matchLabels") or policy_selector.get("matchExpressions")):
        return False
    match = policy_selector.get("matchLabels") or {}
    for k, v in match.items():
        if pod_labels.get(k) != v:
            return False
    exprs = policy_selector.get("matchExpressions") or []
    for e in exprs:
        k, op = e.get("key"), e.get("operator")
        vals = e.get("values") or []
        if op == "In" and pod_labels.get(k) not in vals:
            return False
        if op == "NotIn" and pod_labels.get(k) in vals:
            return False
        if op == "Exists" and k not in pod_labels:
            return False
        if op == "DoesNotExist" and k in pod_labels:
            return False
    return True

docs = [d for d in yaml.safe_load_all(sys.stdin) if d]
jobs = []
policies = []   # NetworkPolicy podSelectors
cnps = []       # CiliumNetworkPolicy endpointSelectors
for d in docs:
    kind = d.get("kind")
    if kind == "Job":
        jobs.append(d)
    elif kind == "NetworkPolicy":
        policies.append(d["spec"]["podSelector"])
    elif kind == "CiliumNetworkPolicy":
        cnps.append(d["spec"]["endpointSelector"])

def fail(msg, job=None):
    where = ("%s: " % job) if job else ""
    sys.exit("hook-policies: FAIL: %s: %s%s" % (label, where, msg))

for job in jobs:
    ann = (job.get("metadata", {}).get("annotations") or {})
    hooks = ann.get(HOOK_KEY) or ""
    if "pre-upgrade" not in hooks.split(","):
        continue  # e.g. the gateway-ca job (pre-install runs before any policy exists)
    pod = job["spec"]["template"] or {}
    pod_meta = pod.get("metadata") or {}
    labels = (pod_meta.get("labels") or {})
    spec = pod.get("spec") or {}
    name = job["metadata"]["name"]
    if not spec.get("serviceAccountName"):
        continue  # no API identity -> not a kubectl-to-API hook (e.g. migrate)
    covered_np = any(selector_matches(s, labels) for s in policies)
    if not covered_np:
        fail("pre-upgrade hook Job %r (serviceAccountName=%r, pod labels %r) is matched by no "
             "rendered NetworkPolicy podSelector: on helm upgrade its kubectl calls run under the "
             "previous release\u2019s default-deny policy and the upgrade times out; render the "
             "allow policy as a helm hook (like the -hooks policy) or label the pod to match one"
             % (name, spec.get("serviceAccountName"), labels), name)
    if not cnps:
        continue
    covered_cnp = any(selector_matches(s, labels) for s in cnps)
    if not covered_cnp:
        fail("pre-upgrade hook Job %r is matched by no CiliumNetworkPolicy endpointSelector: on "
             "Cilium the NetworkPolicy ipBlock never matches the kube-apiserver entity, so the "
             "upgrade still times out; render the CNP as a hook too (like -hooks-kube-api)" % name,
             name)
print("hook-policies: %s ok" % label)
' "$label"
}

check_render "default values"
check_render "cilium" --set networkPolicy.cilium.enabled=true
