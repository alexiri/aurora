#!/usr/bin/env bash
# Sandbox base-image override smoke test (docs/18 §Deployment-neutral defaults, AUDIT-003 F-9).
#
#   scripts/images/sandbox-base-smoke.sh
#
# Builds the `sandbox-min` target of images/sandbox/Containerfile twice:
#   A) with the Containerfile's default base (`ARG SANDBOX_BASE_IMAGE=...@sha256:<digest>`), and
#   B) with `--build-arg SANDBOX_BASE_IMAGE=<registry>/<image>@sha256:<digest>`: the SAME image by
#      another reference (the digest without its tag), the way a deployment points the build at a mirror;
# then compares the installed package list (`rpm -qa`) and the `org.aurora.capabilities` label of both
# images. A deployment override must change where the base comes from, never what the image contains.
# SMOKE_FULL=1 also does the same for the full `sandbox` target (Chromium, pinned harness; slow, needs
# the network): that is the target which declares `org.aurora.capabilities="browser"`.
#
# Env: SMOKE_PREFIX (podman tag prefix, default aurora-sbxbase-smoke), SMOKE_FULL=1, SMOKE_KEEP=1 (keep the images).
# Needs ~15 GB free (the supervisor builder); the digest-pinned base must be pullable or already local.
set -euo pipefail
cd "$(dirname "$0")/../.."
prefix="${SMOKE_PREFIX:-aurora-sbxbase-smoke}"
cf=images/sandbox/Containerfile

free_kb=$(df --output=avail -k . | tail -1)
if [ "$free_kb" -lt $((15 * 1024 * 1024)) ]; then
  echo "refusing to run: less than 15 GB free" >&2
  exit 1
fi

default_ref=$(sed -n 's/^ARG SANDBOX_BASE_IMAGE=//p' "$cf" | head -1)
case "$default_ref" in
  *@sha256:*) ;;
  *) echo "FAIL: the Containerfile default SANDBOX_BASE_IMAGE is not digest-pinned: '$default_ref'" >&2; exit 1 ;;
esac
digest="${default_ref##*@}"
repo="${default_ref%%@*}"; repo="${repo%%:*}"
# `docker.io/library/almalinux:10@sha256:X` -> `docker.io/library/almalinux@sha256:X` (no tag: another reference).
override_ref="$repo@$digest"
[ "$override_ref" != "$default_ref" ] || { echo "FAIL: could not derive a different reference for $default_ref" >&2; exit 1; }
echo "== default base:  $default_ref"
echo "== override base: $override_ref"

tags=()
cleanup() {
  [ "${SMOKE_KEEP:-}" = 1 ] && return 0
  [ "${#tags[@]}" -gt 0 ] && podman rmi -f "${tags[@]}" >/dev/null 2>&1
  return 0
}
trap cleanup EXIT

fail=0
compare_target() { # <target>
  local target="$1" a="localhost/$prefix-$1-default:smoke" b="localhost/$prefix-$1-override:smoke"
  tags+=("$a" "$b")
  echo "== build $target (default base)"
  podman build -q -f "$cf" --target "$target" -t "$a" . >/dev/null
  echo "== build $target (SANDBOX_BASE_IMAGE=$override_ref)"
  podman build -q -f "$cf" --target "$target" --build-arg "SANDBOX_BASE_IMAGE=$override_ref" -t "$b" . >/dev/null
  local pa pb la lb
  pa=$(podman run --rm --entrypoint rpm "$a" -qa --qf '%{NAME}-%{VERSION}-%{RELEASE}.%{ARCH}\n' | sort)
  pb=$(podman run --rm --entrypoint rpm "$b" -qa --qf '%{NAME}-%{VERSION}-%{RELEASE}.%{ARCH}\n' | sort)
  la=$(podman image inspect -f '{{index .Config.Labels "org.aurora.capabilities"}}' "$a")
  lb=$(podman image inspect -f '{{index .Config.Labels "org.aurora.capabilities"}}' "$b")
  if [ -z "$pa" ]; then echo "FAIL [$target]: empty package list (rpm -qa produced nothing)"; fail=1; return; fi
  if [ "$pa" = "$pb" ]; then
    echo "ok   [$target] same package list ($(printf '%s\n' "$pa" | wc -l) packages)"
  else
    echo "FAIL [$target]: package lists differ:"; diff <(printf '%s\n' "$pa") <(printf '%s\n' "$pb") | head -20; fail=1
  fi
  if [ "$la" = "$lb" ]; then
    echo "ok   [$target] same org.aurora.capabilities label ('${la}')"
  else
    echo "FAIL [$target]: org.aurora.capabilities differs: default='${la}' override='${lb}'"; fail=1
  fi
  if [ "$target" = sandbox ] && [ "$la" != browser ]; then
    echo "FAIL [$target]: the full image no longer declares org.aurora.capabilities=browser (got '${la}')"; fail=1
  fi
}

compare_target sandbox-min
[ "${SMOKE_FULL:-}" = 1 ] && compare_target sandbox

if [ "$fail" -ne 0 ]; then echo "== sandbox-base-smoke: FAILED"; exit 1; fi
echo "== sandbox-base-smoke: passed"
