#!/usr/bin/env bash
# SEC-33: core crates and services must not name chat-platform or app integrations.
# Scans crates/ and services/ (skipping any tests/ directories). connectors/,
# plugins/, docs/ and tests/ are out of scope by design.
# Exceptions live in scripts/guards/integration-names.allow as `path_regex:line_regex`.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ALLOW="$ROOT/scripts/guards/integration-names.allow"

TERMS='mattermost|slack|discord|telegram|msteams|github|gitlab|bitbucket|jira|confluence|gmail|notion|linear|sentry|salesforce|hubspot|zendesk'
# Whole-word-ish match: the term must not be surrounded by ASCII letters, so
# "EgressEntry" (contains "sEntry") and "githubusercontent" do not match.
PATTERN="(^|[^A-Za-z])(${TERMS})([^A-Za-z]|\$)"

cd "$ROOT"
hits="$(grep -rniE --binary-files=without-match \
    --exclude-dir=target --exclude-dir=tests --exclude-dir=node_modules \
    "$PATTERN" crates services || true)"

# Drop allow-listed hits. Each hit is "path:line:content".
violations=""
while IFS= read -r hit; do
    [ -z "$hit" ] && continue
    path="${hit%%:*}"
    rest="${hit#*:}"
    content="${rest#*:}"
    allowed=0
    if [ -f "$ALLOW" ]; then
        while IFS= read -r entry; do
            case "$entry" in ''|'#'*) continue ;; esac
            path_re="${entry%%:*}"
            line_re="${entry#*:}"
            if [[ "$path" =~ $path_re ]] && [[ "$content" =~ $line_re ]]; then
                allowed=1
                break
            fi
        done < "$ALLOW"
    fi
    if [ "$allowed" -eq 0 ]; then
        violations+="$hit"$'\n'
    fi
done <<< "$hits"

if [ -n "$violations" ]; then
    echo "no-integration-names: FAIL — integration names found in crates/ or services/:" >&2
    printf '%s' "$violations" >&2
    echo "Add a justified entry to scripts/guards/integration-names.allow if this is a legitimate exception." >&2
    exit 1
fi
echo "no-integration-names: OK"
