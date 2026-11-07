#!/usr/bin/env bash
# Mattermost test server for the connector conformance suite (MM-001).
#
#   scripts/mattermost/setup.sh up      # start (or reuse) the container and seed it
#   scripts/mattermost/setup.sh down    # remove the container
#   scripts/mattermost/setup.sh env     # print the seeded environment (sourced by conformance.sh)
#
# Image: mattermost/mattermost-preview 11.7.15 (MM-003: production runs Mattermost 11.7), pinned
# by digest (the multi-arch index).
#   digest   sha256:6483f05baf4f6234d897dd7e9ca333183002645ea0217bcb202c91bd4c0b64cf
#   size     ~1.5 GB uncompressed (amd64 image 1,541,227,780 bytes), all-in-one: it bundles
#            PostgreSQL 14 and Mattermost Team Edition.
#   license  Mattermost Team Edition: the compiled binary is MIT-licensed; PostgreSQL is under the
#            PostgreSQL license. The preview image is meant for evaluation and CI, never production,
#            and is never shipped with Aurora. (Check `podman image inspect` labels on a bump.)
# Bumping the pin: pull the new tag, copy the index digest from `podman manifest inspect`.
#
# The container runs on the host network (Mattermost on 127.0.0.1:$AURORA_MM_PORT, its bundled
# PostgreSQL on 127.0.0.1:5432) so the server can call the connector's loopback callbacks.
# Resources are prefixed `aurora-mm2-` (override the container name with AURORA_MM_CONTAINER). State lives in target/mattermost/env (gitignored).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
IMAGE="docker.io/mattermost/mattermost-preview@sha256:6483f05baf4f6234d897dd7e9ca333183002645ea0217bcb202c91bd4c0b64cf"
NAME="${AURORA_MM_CONTAINER:-aurora-mm2-server}"
PORT="${AURORA_MM_PORT:-18065}"
URL="http://127.0.0.1:${PORT}"
STATE="$ROOT/target/mattermost"
ENVFILE="$STATE/env"
TEAM="aurora-conf"
ADMIN_USER="aurora-admin"
ADMIN_PASS="Aur0ra-Conf-$(printf '%s' "$ROOT" | sha256sum | cut -c1-12)!"
BOT_NAME="aurora-shared-bot"

log() { echo "mattermost: $*" >&2; }
api() { # api METHOD PATH [json] — uses $TOKEN
  local m="$1" p="$2" d="${3:-}"
  if [ -n "$d" ]; then
    curl -fsS -X "$m" -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' -d "$d" "$URL/api/v4$p"
  else
    curl -fsS -X "$m" -H "Authorization: Bearer $TOKEN" "$URL/api/v4$p"
  fi
}

down() {
  podman rm -f "$NAME" >/dev/null 2>&1 || true
  rm -f "$ENVFILE"
  log "removed $NAME"
}

start() {
  if podman container exists "$NAME" 2>/dev/null; then
    [ "$(podman inspect -f '{{.State.Running}}' "$NAME")" = true ] || podman start "$NAME" >/dev/null
    return
  fi
  df -h /home | tail -1 >&2
  log "starting $NAME on $URL"
  # Settings the suite relies on, as MM_* environment overrides (no config file edits):
  #  - loopback callbacks must be allowed (interactive actions, slash commands);
  #  - bot accounts and personal access tokens enabled; open signup so the first user can be created;
  #  - verification not required; email addresses visible to bots.
  podman run -d --name "$NAME" --network host \
    -e MM_SERVICESETTINGS_LISTENADDRESS=":${PORT}" \
    -e MM_SERVICESETTINGS_SITEURL="$URL" \
    -e MM_SERVICESETTINGS_ALLOWEDUNTRUSTEDINTERNALCONNECTIONS="127.0.0.1 localhost" \
    -e MM_SERVICESETTINGS_ENABLEBOTACCOUNTCREATION=true \
    -e MM_SERVICESETTINGS_ENABLEUSERACCESSTOKENS=true \
    -e MM_SERVICESETTINGS_ENABLECOMMANDS=true \
    -e MM_SERVICESETTINGS_ENABLEONLYADMININTEGRATIONS=false \
    -e MM_SERVICESETTINGS_ENABLEAPICHANNELDELETION=true \
    -e MM_SERVICESETTINGS_ENABLEAPIPOSTDELETION=true \
    -e MM_TEAMSETTINGS_ENABLEOPENSERVER=true \
    -e MM_TEAMSETTINGS_MAXUSERSPERTEAM=100000 \
    -e MM_TEAMSETTINGS_ENABLEUSERCREATION=true \
    -e MM_EMAILSETTINGS_REQUIREEMAILVERIFICATION=false \
    -e MM_EMAILSETTINGS_ENABLESIGNUPWITHEMAIL=true \
    -e MM_PRIVACYSETTINGS_SHOWEMAILADDRESS=true \
    -e MM_RATELIMITSETTINGS_ENABLE=false \
    "$IMAGE" >/dev/null
}

wait_ready() {
  log "waiting for $URL (first start initialises the database; up to ~3 min)"
  for _ in $(seq 1 120); do
    if curl -fsS "$URL/api/v4/system/ping" 2>/dev/null | grep -q '"OK"'; then return 0; fi
    sleep 3
  done
  podman logs --tail 40 "$NAME" >&2 || true
  log "server did not become ready"
  return 1
}

seed() {
  # The first user created becomes the system admin.
  curl -fsS -X POST -H 'Content-Type: application/json' \
    -d "{\"email\":\"admin@aurora-conf.test\",\"username\":\"$ADMIN_USER\",\"password\":\"$ADMIN_PASS\"}" \
    "$URL/api/v4/users" >/dev/null 2>&1 || true
  local session
  session="$(curl -fsS -i -X POST -H 'Content-Type: application/json' \
    -d "{\"login_id\":\"$ADMIN_USER\",\"password\":\"$ADMIN_PASS\"}" "$URL/api/v4/users/login" \
    | tr -d '\r' | awk -F': ' 'tolower($1)=="token"{print $2}')"
  [ -n "$session" ] || { log "admin login failed"; return 1; }
  TOKEN="$session"
  local admin_id
  admin_id="$(api GET /users/me | jq -r .id)"

  # Personal access token of the admin (the connector's optional admin_token).
  local admin_pat
  admin_pat="$(api POST "/users/$admin_id/tokens" '{"description":"aurora-conf"}' | jq -r .token)"
  TOKEN="$admin_pat"

  # Team.
  local team_id
  team_id="$(api GET "/teams/name/$TEAM" 2>/dev/null | jq -r .id || true)"
  if [ -z "$team_id" ] || [ "$team_id" = null ]; then
    team_id="$(api POST /teams "{\"name\":\"$TEAM\",\"display_name\":\"Aurora Conformance\",\"type\":\"O\"}" | jq -r .id)"
  fi

  # Seeded people (verified email) and the shared bot with a token.
  seed_user() {
    local name="$1" verify="$2" id
    id="$(api GET "/users/username/$name" 2>/dev/null | jq -r .id || true)"
    if [ -z "$id" ] || [ "$id" = null ]; then
      id="$(api POST /users "{\"email\":\"$name@aurora-conf.test\",\"username\":\"$name\",\"password\":\"Aur0ra-Seed-Pass-1!\"}" | jq -r .id)"
    fi
    [ "$verify" = yes ] && api POST "/users/$id/email/verify/member" >/dev/null 2>&1 || true
    api POST "/teams/$team_id/members" "{\"team_id\":\"$team_id\",\"user_id\":\"$id\"}" >/dev/null 2>&1 || true
    echo "$id"
  }
  local alice_id bob_id
  alice_id="$(seed_user alice yes)"
  bob_id="$(seed_user bob yes)"

  local bot_id bot_token
  bot_id="$(api GET "/users/username/$BOT_NAME" 2>/dev/null | jq -r .id || true)"
  if [ -z "$bot_id" ] || [ "$bot_id" = null ]; then
    bot_id="$(api POST /bots "{\"username\":\"$BOT_NAME\",\"display_name\":\"Daneel\",\"description\":\"Aurora shared bot\"}" | jq -r .user_id)"
  fi
  api POST "/teams/$team_id/members" "{\"team_id\":\"$team_id\",\"user_id\":\"$bot_id\"}" >/dev/null 2>&1 || true
  bot_token="$(api POST "/users/$bot_id/tokens" '{"description":"aurora-shared"}' | jq -r .token)"

  mkdir -p "$STATE"
  umask 077
  cat >"$ENVFILE" <<EOF
AURORA_MM_URL=$URL
AURORA_MM_TEAM=$TEAM
AURORA_MM_TEAM_ID=$team_id
AURORA_MM_ADMIN_USER=$ADMIN_USER
AURORA_MM_ADMIN_PASS=$ADMIN_PASS
AURORA_MM_ADMIN_TOKEN=$admin_pat
AURORA_MM_BOT_ID=$bot_id
AURORA_MM_BOT_TOKEN=$bot_token
AURORA_MM_ALICE_ID=$alice_id
AURORA_MM_BOB_ID=$bob_id
AURORA_MM_CONTAINER=$NAME
EOF
  log "seeded: team $TEAM ($team_id), users alice/bob, bot $BOT_NAME; env in $ENVFILE"
}

case "${1:-up}" in
  up) start; wait_ready; seed ;;
  down) down ;;
  env) cat "$ENVFILE" ;;
  *) echo "usage: $0 up|down|env" >&2; exit 2 ;;
esac
