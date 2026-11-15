#!/usr/bin/env bash
# FIX-119 (docs/ops/builtin-plugin-signing.md): the `release` environment holds the project signing key.
# Across every workflow, `environment: release` must appear in exactly ONE job, and that job must be
# push-only (`if: github.event_name == 'push'`). A second job (or a pull-request-reachable one) that names the
# environment could read the key. (Whether the environment itself restricts branches and requires a reviewer
# is a repository setting this guard cannot see: the document says to verify it.)
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

fail() { echo "release-environment: FAIL: $*" >&2; exit 1; }

# check <workflow files...>: prints "ok" or a problem. Reads the YAML with python3 (read-only).
check() {
  python3 -I -c '
import sys, yaml
users = []   # (file, job, if)
for path in sys.argv[1:]:
    wf = yaml.safe_load(open(path)) or {}
    for name, job in (wf.get("jobs") or {}).items():
        env = job.get("environment")
        env_name = env.get("name") if isinstance(env, dict) else env
        if env_name == "release":
            users.append((path, name, str(job.get("if", ""))))
if len(users) != 1:
    sys.exit("expected `environment: release` in exactly one job, found %d: %s" % (len(users), users))
path, name, cond = users[0]
compact = cond.replace(" ", "")
if "github.event_name==" not in compact or "push" not in compact or "pull_request" in compact:
    sys.exit("job %s (%s) uses environment release but is not push-only: if=%r" % (name, path, cond))
print("ok %s %s" % (path, name))
' "$@"
}

# shellcheck disable=SC2046
out="$(check $(ls .github/workflows/*.yml))" || fail "$out"
echo "release-environment: $out"

# Self-test: a planted workflow with two release jobs, and one that is not push-only, must fail.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cat >"$tmp/two.yml" <<'YAML'
jobs:
  a: {if: "github.event_name == 'push'", environment: release, runs-on: x}
  b: {if: "github.event_name == 'push'", environment: release, runs-on: x}
YAML
cat >"$tmp/open.yml" <<'YAML'
jobs:
  a: {environment: release, runs-on: x}
YAML
check "$tmp/two.yml" >/dev/null 2>&1 && fail "self-test: two jobs with environment release passed"
check "$tmp/open.yml" >/dev/null 2>&1 && fail "self-test: a job without the push-only condition passed"
echo "release-environment: self-test ok"
