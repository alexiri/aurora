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
# Exempt: hooks without pre-upgrade (gateway-ca: pre-install runs before any policy exists).
# (FIX-102) migrate and gateway-role (no API identity, database egress) are checked too: the policy
# that admits them must itself be a hook, since a regular resource does not exist yet when an upgrade
# from a chart that predates it runs its hooks. Only a policy carrying a helm.sh/hook annotation counts.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

fail() { echo "hook-policies: FAIL: $*" >&2; exit 1; }

# shellcheck source=scripts/guards/lib.sh
source scripts/guards/lib.sh
require_helm hook-policies || exit 0

check_render() {
  local label="$1"; shift
  # (FIX-106) allowSharedLogin: these checks are about hooks, not the gateway's database role.
  helm template guard deploy/helm/aurora --set database.allowSharedLogin=true "$@" | analyse "$label"
}
# analyse <label>: the rendered chart on stdin.
analyse() {
  local label="$1"
  python3 -I -c '
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
        # FIX-068: a selector the guard cannot evaluate must fail it, never be treated as a match.
        if op not in ("In", "NotIn", "Exists", "DoesNotExist"):
            sys.exit("hook-policies: FAIL: %s: unsupported matchExpressions operator %r" % (label, op))
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
        # (FIX-102) only a policy that is itself a hook exists when the hooks run on an upgrade.
        if HOOK_KEY in (d.get("metadata", {}).get("annotations") or {}):
            policies.append(d["spec"]["podSelector"])
    elif kind == "CiliumNetworkPolicy":
        cnps.append(d["spec"]["endpointSelector"])

def fail(msg, job=None):
    where = ("%s: " % job) if job else ""
    sys.exit("hook-policies: FAIL: %s: %s%s" % (label, where, msg))

checked = 0     # pre-upgrade hook Jobs with a serviceAccountName that were actually inspected
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
    # (FIX-102) migrate has no API identity but needs the database: its allow policy must be a hook
    # too (an upgrade from a chart that predates the policy would otherwise default-deny the Job).
    checked += 1
    covered_np = any(selector_matches(s, labels) for s in policies)
    if not covered_np:
        fail("pre-upgrade hook Job %r (serviceAccountName=%r, pod labels %r) is matched by no "
             "rendered NetworkPolicy podSelector: on helm upgrade its kubectl calls run under the "
             "previous release\u2019s default-deny policy and the upgrade times out; render the "
             "allow policy as a helm hook (like the -hooks policy) or label the pod to match one"
             % (name, spec.get("serviceAccountName"), labels), name)
    if not cnps or not spec.get("serviceAccountName"):
        continue  # the Cilium kube-apiserver entity matters only to hooks that talk to the API
    covered_cnp = any(selector_matches(s, labels) for s in cnps)
    if not covered_cnp:
        fail("pre-upgrade hook Job %r is matched by no CiliumNetworkPolicy endpointSelector: on "
             "Cilium the NetworkPolicy ipBlock never matches the kube-apiserver entity, so the "
             "upgrade still times out; render the CNP as a hook too (like -hooks-kube-api)" % name,
             name)
# FIX-068: a guard that inspected nothing proves nothing — renaming/removing the secrets hook Job
# (or its serviceAccountName) used to leave this green.
if checked < 1:
    fail("inspected no pre-upgrade hook Job with a serviceAccountName (the secrets hook is "
         "expected): the guard would pass vacuously")
print("hook-policies: %s ok (%d hook Job(s) checked)" % (label, checked))
' "$label"
}

check_render "default values"
check_render "cilium" --set networkPolicy.cilium.enabled=true

# (FIX-102) Mutation self-test: the migrate allow policy as a regular resource (no hook annotation) is
# exactly the upgrade-from-an-older-chart hole, and must fail.
if helm template guard deploy/helm/aurora --set database.allowSharedLogin=true | python3 -I -c '
import sys, yaml
docs = [d for d in yaml.safe_load_all(sys.stdin) if d]
for d in docs:
    if d["kind"] == "NetworkPolicy" and d["metadata"]["name"].endswith("-migrate"):
        d["metadata"].pop("annotations", None)
yaml.safe_dump_all(docs, sys.stdout)
' | analyse "self-test" >/dev/null 2>&1; then
  fail "self-test: a migrate allow policy that is not a hook passed"
fi
echo "hook-policies: self-test ok"
