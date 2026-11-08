#!/usr/bin/env bash
# P2-CAT-0: recorded HTTP and fixtures of built-in packages must carry no live credentials.
# Fails on anything token-shaped anywhere under plugins/builtin (text files; `tests/` is the
# one that matters, but a token in a manifest or guide is no better). Use obviously fake values
# such as `tok-fixture-1`.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

[ -d plugins/builtin ] || exit 0

patterns=(
  'glpat-[A-Za-z0-9_-]{20,}'                         # GitLab personal/project/group access token
  'gl(oas|dt|rt|cbt|ptt|ft|imt|agent|soat)-[A-Za-z0-9_-]{20,}' # other GitLab token families
  'gh[pousr]_[A-Za-z0-9]{30,}'                       # GitHub tokens
  'github_pat_[A-Za-z0-9_]{30,}'
  '(AKIA|ASIA)[0-9A-Z]{16}'                          # AWS access key id
  'xox[abprs]-[A-Za-z0-9-]{10,}'                     # Slack
  'ind[po]_[A-Za-z0-9]{40,}'                         # Indico personal (indp_) and OAuth (indo_) tokens
  'ATATT[A-Za-z0-9_=-]{20,}'                         # Atlassian API token
  'sntry[su]_[A-Za-z0-9+/_=-]{20,}'                  # Sentry organization (sntrys_) and user (sntryu_) auth tokens
  '(ntn_|secret_|nrt_)[A-Za-z0-9]{20,}'              # Notion integration tokens, legacy secrets, refresh tokens
  'lin_(api|oauth)_[A-Za-z0-9]{20,}'                 # Linear personal API keys and OAuth tokens
  'sk-[A-Za-z0-9_-]{32,}'                      # API secret keys
  'eyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}' # JWT
  '-----BEGIN [A-Z ]*PRIVATE KEY-----'
  '(Bearer|Basic) [A-Za-z0-9+/_=.-]{40,}'            # a long literal credential in a header
  'Set-Cookie:'                                       # recorded responses must be scrubbed
)

fail=0

# Throw-away fixture keys (e.g. an RSA key that signs a GitHub App JWT in a hook fixture) are
# exempt from the PRIVATE KEY pattern ONLY, and only for files listed in fixture-keys.allow (one
# repo-relative path per line, `#` comments). Every other pattern still applies to them. A listed
# file must exist and carry the marker line, so the exemption is explicit and reviewable.
KEY_PATTERN='-----BEGIN [A-Z ]*PRIVATE KEY-----'
MARKER='# aurora-fixture-key: throw-away, generated for fixtures, never valid upstream'
allowed=()
if [ -f scripts/guards/fixture-keys.allow ]; then
  while IFS= read -r line; do
    line="${line%%#*}"; line="$(echo "$line" | xargs)"
    [ -n "$line" ] || continue
    if [ ! -f "$line" ]; then
      echo "fixture-keys.allow lists a missing file: $line" >&2; fail=1; continue
    fi
    if ! grep -qxF -e "$MARKER" "$line"; then
      echo "fixture-keys.allow: $line lacks the marker line: $MARKER" >&2; fail=1; continue
    fi
    allowed+=("$line")
  done < scripts/guards/fixture-keys.allow
fi

for p in "${patterns[@]}"; do
  if hits="$(grep -rInE --exclude='*.png' -e "$p" plugins/builtin 2>/dev/null)"; then
    if [ "$p" = "$KEY_PATTERN" ] && [ "${#allowed[@]}" -gt 0 ]; then
      kept=""
      while IFS= read -r h; do
        skip=""
        for a in "${allowed[@]}"; do [ "${h%%:*}" = "$a" ] && skip=1; done
        [ -n "$skip" ] || kept+="$h"$'\n'
      done <<< "$hits"
      [ -n "$kept" ] || continue
      hits="${kept%$'\n'}"
    fi
    echo "possible live credential under plugins/builtin (pattern: $p):" >&2
    # Show the location only, never the value.
    echo "$hits" | cut -d: -f1,2 | sed 's/^/  /' >&2
    fail=1
  fi
done
if [ "$fail" -ne 0 ]; then
  echo "Scrub recorded HTTP: replace tokens with fixture values (see plugins/builtin/README.md)." >&2
  exit 1
fi
echo "no live-looking tokens under plugins/builtin"
