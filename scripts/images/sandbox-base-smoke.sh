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
# The package list is read without the image's own rpm binary: the `base` stage deliberately deletes
# /usr/bin/rpm* from the full image (RUN-003: no package management for the agent; FIX-031 addendum),
# so the freshly built `os` stage — which still has the binary — is built once per compared target
# (layer-cached) and its /usr/bin/rpm is extracted and bind-mounted read-only into the image under
# test for the `rpm -qa` calls (librpm* stays in the image). If the bare binary fails to start, the
# librpm*/librpmio*/libpopt* libraries are bind-mounted from the extract container too; if that also
# fails, the smoke stops rather than switching the comparison mechanism.
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
os_tag="$prefix-os"
os_tag_added=0
extract_dir="$PWD/rpm-extract"
extract_ctr="$prefix-extract"
cleanup() {
  [ "${SMOKE_KEEP:-}" = 1 ] && return 0
  podman rm -f "$extract_ctr" >/dev/null 2>&1 || true
  rm -rf "$extract_dir"
  [ "${#tags[@]}" -gt 0 ] && podman rmi -f "${tags[@]}" >/dev/null 2>&1 || true
  return 0
}
trap cleanup EXIT

# Extract /usr/bin/rpm — and, only for the fallback, the librpm*/librpmio*/libpopt* libraries it
# links — from the freshly built os-stage image: the compared images have the libraries but, by the
# deliberate `base`-stage deletion (RUN-003; FIX-031 addendum), no agent-facing rpm binaries.
extract_rpm() {
  podman build -q -f "$cf" --target os -t "$os_tag" . >/dev/null
  if [ "$os_tag_added" -eq 0 ]; then tags+=("$os_tag"); os_tag_added=1; fi
  podman rm -f "$extract_ctr" >/dev/null 2>&1 || true
  rm -rf "$extract_dir"; mkdir -p "$extract_dir/lib"
  podman create --name "$extract_ctr" "$os_tag" true >/dev/null
  podman cp "$extract_ctr":/usr/bin/rpm "$extract_dir/rpm" || true
  local libs l f t i any
  libs=$(podman run --rm --entrypoint /bin/sh "$os_tag" \
           -c 'ls -1 /usr/lib64/librpm.so.* /usr/lib64/librpmio.so.* /usr/lib64/libpopt.so.* 2>/dev/null' || true)
  for l in $libs; do podman cp "$extract_ctr":"$l" "$extract_dir/lib/" 2>/dev/null || true; done
  # Resolve the soname symlinks one level at a time so every mount source is present in lib/
  # (podman cp preserves symlinks; their targets may be versioned files not yet copied).
  for i in 1 2 3; do
    any=0
    for f in "$extract_dir"/lib/*; do
      [ -L "$f" ] || continue
      t=$(readlink "$f"); t="${t#/usr/lib64/}"
      if [ -e "$extract_dir/lib/$t" ]; then
        ln -sfn "$t" "$f"
      elif podman cp "$extract_ctr":/usr/lib64/"$t" "$extract_dir/lib/" 2>/dev/null; then
        any=1
      else
        rm -f "$f"
      fi
    done
    if [ "$any" -eq 0 ]; then break; fi
  done
  podman rm -f "$extract_ctr" >/dev/null
  if [ ! -s "$extract_dir/rpm" ]; then
    echo "FAIL: could not extract /usr/bin/rpm from the os stage ($os_tag); is rpm installed in the os stage?" >&2
    exit 1
  fi
}

rpm_qa() { # <image> — package list through the bind-mounted os-stage rpm (libraries only if needed)
  local img="$1" out mounts=() f base
  if out=$(podman run --rm -v "$extract_dir/rpm:/usr/bin/rpm:ro" --entrypoint /usr/bin/rpm "$img" \
             -qa --qf '%{NAME}-%{VERSION}-%{RELEASE}.%{ARCH}\n' 2>/dev/null) && [ -n "$out" ]; then
    printf '%s\n' "$out"
    return 0
  fi
  for f in "$extract_dir"/lib/*; do
    [ -e "$f" ] || [ -L "$f" ] || continue
    base="${f##*/}"
    mounts+=(-v "$f:/usr/lib64/$base:ro")
  done
  echo "note: the bare extracted rpm failed in $img; retrying with librpm*/librpmio*/libpopt* mounted" >&2
  if [ "${#mounts[@]}" -gt 0 ]; then
    if out=$(podman run --rm -v "$extract_dir/rpm:/usr/bin/rpm:ro" "${mounts[@]}" --entrypoint /usr/bin/rpm "$img" \
               -qa --qf '%{NAME}-%{VERSION}-%{RELEASE}.%{ARCH}\n' 2>&1) && [ -n "$out" ]; then
      printf '%s\n' "$out"
      return 0
    fi
  else
    echo "note: no librpm*/libpopt* libraries were extracted; cannot attempt the library fallback" >&2
  fi
  echo "FAIL: the extracted os-stage rpm cannot run inside $img even with librpm*/libpopt* bind-mounted; stopping (the FIX-031 addendum forbids switching the comparison mechanism)" >&2
  exit 1
}

fail=0
compare_target() { # <target>
  extract_rpm
  local target="$1" a="localhost/$prefix-$1-default:smoke" b="localhost/$prefix-$1-override:smoke"
  tags+=("$a" "$b")
  echo "== build $target (default base)"
  podman build -q -f "$cf" --target "$target" -t "$a" . >/dev/null
  echo "== build $target (SANDBOX_BASE_IMAGE=$override_ref)"
  podman build -q -f "$cf" --target "$target" --build-arg "SANDBOX_BASE_IMAGE=$override_ref" -t "$b" . >/dev/null
  local pa pb la lb
  pa=$(rpm_qa "$a" | sort)
  pb=$(rpm_qa "$b" | sort)
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
  # (FIX-103) A declared `browser` capability must be true: the runner schedules browser boots from the
  # label, so an image that carries it without Chromium would accept boots it cannot serve.
  for img in "$a" "$b"; do
    case ",$(podman image inspect -f '{{index .Config.Labels "org.aurora.capabilities"}}' "$img")," in
      *,browser,*)
        if podman run --rm --entrypoint /bin/sh "$img" -c 'for c in /usr/bin/chromium-browser /usr/bin/chromium; do [ -x "$c" ] && exec "$c" --version; done; exit 127' >/dev/null 2>&1; then
          echo "ok   [$target] $img declares browser and chromium --version runs"
        else
          echo "FAIL [$target]: $img declares org.aurora.capabilities=browser but chromium --version does not run"; fail=1
        fi ;;
    esac
  done
}

compare_target sandbox-min
[ "${SMOKE_FULL:-}" = 1 ] && compare_target sandbox

if [ "$fail" -ne 0 ]; then echo "== sandbox-base-smoke: FAILED"; exit 1; fi
echo "== sandbox-base-smoke: passed"
