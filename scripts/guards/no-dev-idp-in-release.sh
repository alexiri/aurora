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
# (FIX-101) every Containerfile under images/ (services, web, sandbox), not one file.
containerfiles="$(find images -type f \( -name 'Containerfile*' -o -name '*.Containerfile' \) | sort)"
[ -n "$containerfiles" ] || fail "no Containerfiles found under images/"
dev_idp_scan() { grep -n -i -E 'aurora-testkit|dev-idp' "$@" 2>/dev/null; }
# shellcheck disable=SC2086
if hits="$(dev_idp_scan $containerfiles)"; then
  fail "a Containerfile under images/ builds or ships the development IdP: $hits"
fi
# Self-test: a planted reference must be found by the same scan.
planted="$(mktemp)"
trap 'rm -f "$planted"' EXIT
printf 'RUN cargo build -p aurora-testkit --bin aurora-dev-idp\n' >"$planted"
dev_idp_scan "$planted" >/dev/null || fail "self-test: the scan missed a planted dev-idp build line"

# 3. Chart defaults: off.
grep -q -E '^devIdp:' deploy/helm/aurora/values.yaml || fail "devIdp block missing from values.yaml"
python3 -I - <<'PY' || fail "devIdp.enabled must default to false"
import re, sys
t = open("deploy/helm/aurora/values.yaml").read()
m = re.search(r"^devIdp:\n((?:[ \t]+.*\n|\n)*)", t, re.M)
if not m or not re.search(r"^\s+enabled:\s*false\b", m.group(1), re.M):
    sys.exit(1)
PY
# shellcheck source=scripts/guards/lib.sh
source scripts/guards/lib.sh
if require_helm no-dev-idp-in-release; then
  # (FIX-106) allowSharedLogin: an external database needs the role pair or this explicit value to render.
  out="$(helm template guard deploy/helm/aurora --set database.allowSharedLogin=true)" || fail "the default chart render failed"
  [ -n "$out" ] || fail "the default chart render is empty"
  if grep -q -E 'dev-idp|dev_idp|aurora-dev-idp' <<<"$out"; then
    fail "the chart's default render contains the development IdP"
  fi
fi
echo "no-dev-idp-in-release: ok"
