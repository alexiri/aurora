#!/usr/bin/env bash
# images-builder watchdog (ci.yml, CI only). images-builder runs in parallel with the check jobs, so when a
# check fails it would still compile for ~18 min for images that nothing will publish. This runs in the
# background of that job: once a minute it lists this run's jobs, and when a check job that the publishing
# jobs need has failed, it writes their names to <marker-file> and stops the `docker buildx build` client
# (BuildKit cancels the solve), so the build step fails at once. The other check jobs keep running.
# GitHub cannot cancel a single job, and cancelling the whole run would hide the other checks' results.
#   scripts/ci/stop-builder-on-failed-check.sh <marker-file>
# Env: GH_TOKEN (actions: read), GITHUB_REPOSITORY, GITHUB_RUN_ID, GITHUB_RUN_ATTEMPT;
#      WATCH_INTERVAL (seconds, default 60: ~18 calls per run, well inside the GITHUB_TOKEN rate limit).
set -uo pipefail
marker="${1:?usage: $0 <marker-file>}"
interval="${WATCH_INTERVAL:-60}"
# Job names of the check jobs in the publishing jobs' needs: (matrix legs add " (n)"). Not `vault`:
# nothing waits for it, so its failure must not stop the builder.
checks='^(rust-lint|rust-deny|rust-build|rust-test|e2e-gateway|gated-suites|e2e-hosted|web|guards)( \(.*\))?$'
jobs_url="repos/${GITHUB_REPOSITORY}/actions/runs/${GITHUB_RUN_ID}/attempts/${GITHUB_RUN_ATTEMPT:-1}/jobs?per_page=100"

while :; do
  if [ ! -s "$marker" ]; then
    # A failed API call (rate limit, network) just means "nothing failed yet"; try again next round.
    failed="$(gh api "$jobs_url" --jq '.jobs[] | select(.conclusion == "failure") | .name' 2>/dev/null \
      | grep -E "$checks" || true)"
    if [ -n "$failed" ]; then
      printf '%s\n' "$failed" > "$marker"
      echo "$(date -u +%H:%M:%S) check job(s) failed: $(paste -sd, "$marker"); stopping the build"
      interval=5   # the build step may not have started yet: keep stopping it until the job ends
    fi
  fi
  if [ -s "$marker" ]; then
    # Anchored: only the client build-push-action spawns (argv[0] is docker or its absolute path), never
    # a shell whose command text merely contains the words.
    pkill -TERM -f '^([^ ]*/)?docker buildx build' && echo "$(date -u +%H:%M:%S) sent SIGTERM to docker buildx build"
  fi
  sleep "$interval"
done
