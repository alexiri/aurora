#!/bin/sh
# Summarize a Grafana alert for the triage report (docs/25 §Helper files).
#
# Reads the alert's JSON on stdin and prints the four summary lines of the
# report format:
#
#   What:     <alert title or panel name>
#   When:     <state, since when>
#   Evidence: <the deciding value or annotation, shortened>
#   Known?:   unknown — check the Jira project before calling it new
#
# Only shell + jq. Every value it prints comes straight from the alert;
# nothing is guessed here.
set -eu

project=${JIRA_PROJECT:-the project}

first_of() {
    # first_of <json> <key>... : the value of the first key the object has.
    for key in "$@"; do
        value=$(printf '%s' "$1" | jq -r --arg k "$key" \
            'if has($k) then .[$k] | tostring elif type == "object" then .labels[$k]? // empty else empty end' \
            2>/dev/null || true)
        if [ -n "$value" ] && [ "$value" != "null" ]; then
            printf '%s' "$value"
            return 0
        fi
    done
    printf 'unknown'
}

json=$(cat)

what=$(first_of "$json" title panelTitle name)
[ "$what" = unknown ] && what="unnamed alert"
state=$(first_of "$json" state status)
[ "$state" = unknown ] && state=fired
since=$(first_of "$json" startsAt activeAt firedAt)
evidence=$(first_of "$json" valueString message annotation)
evidence=$(printf '%s' "$evidence" | tr '\n\r' '  ' | cut -c1-200)

printf 'What:     %s\n' "$what"
printf 'When:     %s since %s\n' "$state" "$since"
printf 'Evidence: %s\n' "$evidence"
printf 'Known?:   unknown — check %s before calling it new\n' "$project"
