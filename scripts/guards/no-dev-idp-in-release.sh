#!/usr/bin/env bash
# SEC-42 (phase 1, RFE-004): the development IdP and the test sign-in helpers can never reach a production
# artefact. All sign-in is OIDC; the only "bypass" is the development IdP (`aurora-dev-idp`, crate
# aurora-testkit), which therefore must be:
#   1. never a normal (non-dev) dependency of any other workspace crate, so no service binary links it;
#   2. absent from the release image build (images/services/Containerfile);
#   3. off in the Helm chart's default values (no dev-idp resource, no `dev` provider in the rendered config).
# (The API additionally refuses `auth.dev_idp` unless server.external_url is a loopback origin: config tests.)
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

fail() { echo "no-dev-idp-in-release: FAIL: $*" >&2; exit 1; }

# 1. aurora-testkit is only ever a dev-dependency. `--no-deps` lists the workspace members with their
# declared dependencies (and kinds) without resolving the registry, so this works on a cold CI runner.
meta_json="$(cargo metadata --format-version 1 --no-deps --locked)" \
  || fail "cargo metadata failed (see the error above); cannot check the aurora-testkit dependency kind"
printf '%s' "$meta_json" | python3 -I -c '
import json, sys
meta = json.load(sys.stdin)
members = set(meta["workspace_members"])
bad = []
for p in meta["packages"]:
    if p["id"] not in members or p["name"] == "aurora-testkit":
        continue
    for d in p["dependencies"]:
        if d["name"] == "aurora-testkit" and d.get("kind") in (None, "build"):
            bad.append(p["name"])
if bad:
    sys.exit("aurora-testkit is a normal dependency of: " + ", ".join(sorted(bad)))
' || fail "aurora-testkit (dev IdP, test sign-in helpers) must be a [dev-dependencies] entry only"

# 2. The release image set does not build it.
if grep -n -i -E 'aurora-testkit|dev-idp' images/services/Containerfile >/dev/null; then
  fail "images/services/Containerfile must not build or ship the development IdP"
fi

# 3. Chart defaults: off.
grep -q -E '^devIdp:' deploy/helm/aurora/values.yaml || fail "devIdp block missing from values.yaml"
python3 -I - <<'PY' || fail "devIdp.enabled must default to false"
import re, sys
t = open("deploy/helm/aurora/values.yaml").read()
m = re.search(r"^devIdp:\n((?:[ \t]+.*\n|\n)*)", t, re.M)
if not m or not re.search(r"^\s+enabled:\s*false\b", m.group(1), re.M):
    sys.exit(1)
PY
if command -v helm >/dev/null 2>&1; then
  out="$(helm template guard deploy/helm/aurora)"
  if grep -q -E 'dev-idp|dev_idp|aurora-dev-idp' <<<"$out"; then
    fail "the chart's default render contains the development IdP"
  fi
fi
echo "no-dev-idp-in-release: ok"
