#!/usr/bin/env bash
# Upgrade test (OPS-014, docs/19 M7): previous ref -> HEAD with a running sandbox.
#
#   bash scripts/e2e/upgrade.sh        (heavy: two release-less builds + two image builds, ~15 GB free needed)
#
# Flow: build baseline (UPGRADE_BASE, default 8ba42973) binaries + sandbox image in a temporary git
# worktree, build HEAD the same way, start the baseline stack (api, gateway, runner as host processes
# like scripts/e2e/hosted.sh; sandboxes are real podman containers running the fake harness), create
# an agent state (conversation, memory, schedule, running sandbox), then upgrade in place:
# `aurora-api migrate` (HEAD) -> api -> gateway -> runner. The sandbox container is never touched.
# Verifies migrations, the old supervisor against the new runner/api, restart_to_update, data
# survival, and that a restart boots the new image.
#
# Defects found are printed as `DEFECT` lines. They do not fail the run unless UPGRADE_STRICT=1.
#
# Baseline phase: UPGRADE_BASE_PHASE=0|2 (1 = 2). Unset: detected from the base commit (its
# aurora-config knows `dev_idp` => phase 1+, else phase 0). Phase 0 signs up with a password and
# asserts the plaintext -> TLS-only story (RFE-002); phase 1+ writes an OIDC (dev IdP) + TLS config,
# signs in through the dev IdP and checks conversation, memory, secret, second agent, schedule and
# the running sandbox across the upgrade (UPGRADE_BASE=cc6ccbb345cf03ad2b881d9585e8a19329ceac9a).
#
# Env: UPGRADE_BASE (git ref), UPGRADE_BASE_PHASE, SKIP_BUILD=1 (reuse binaries/images from a previous run),
#      KEEP=1 (do not delete the scratch dir/worktree at the end), PG_RELAY_PORT (default 55442).
#
# Isolation: everything this script creates in podman is prefixed `aurora-upg-`; the scratch dir is
# target/aurora-upg. Postgres is the shared dev instance from .env (a private database that is
# dropped at the end), reached from podman's rootless netns through a unix-socket relay. The
# baseline sandbox image name is fixed in the baseline API (`aurora-sandbox:0.1.0`); a private
# registries.conf alias for a private podman service maps it to localhost/aurora-upg-sandbox so the
# shared `localhost/aurora-sandbox` tag is never touched.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
W="$ROOT/target/aurora-upg"
BASE_REF="${UPGRADE_BASE:-8ba42973}"
SBX_NET=aurora-upg-sbx
ANCHOR=aurora-upg-anchor
BASE_IMG=localhost/aurora-upg-sandbox:0.1.0
HEAD_IMG=localhost/aurora-upg-sandbox:head
BRIDGE=10.89.245.1
API_IP=127.0.0.141
OPS_IP=127.0.0.142
ORIGIN=http://localhost:4100
RELAY_PORT="${PG_RELAY_PORT:-55442}"
TOKEN="aur_upg$(od -An -N6 -tx1 /dev/urandom | tr -d ' \n')"
FP=$(printf '%s' "$TOKEN" | sha256sum | cut -c1-16)

# ------------------------------------------------------------------------------------------------
# Inner phase: runs inside podman's rootless netns (where the sandbox bridge lives).
# ------------------------------------------------------------------------------------------------
if [ "${1:-}" = "--inner" ]; then
  set -uo pipefail
  PASS=0; FAIL=0; DEFECTS=()
  ok()   { echo "  ok    $*"; PASS=$((PASS + 1)); }
  bad()  { echo "  FAIL  $*"; FAIL=$((FAIL + 1)); }
  check() { local msg="$1"; shift; if "$@"; then ok "$msg"; else bad "$msg"; fi; }
  defect() { echo "  DEFECT $*"; DEFECTS+=("$*"); }
  sql() { PGPASSWORD="$DB_PASS" psql -h 127.0.0.1 -p "$RELAY_PORT" -U "$DB_USER" -d "$DB_NAME" -Atq -c "$1"; }
  jar="$W/cookies"
  api() { # method path [json]
    local m="$1" p="$2" b="${3:-}"
    if [ -n "$b" ]; then
      curl -sS -m 40 -X "$m" -b "$jar" -c "$jar" -H "Origin: $ORIGIN" -H 'content-type: application/json' \
        -d "$b" -w '\n%{http_code}' "http://$API_IP:4101$p"
    else
      curl -sS -m 40 -X "$m" -b "$jar" -c "$jar" -H "Origin: $ORIGIN" -w '\n%{http_code}' "http://$API_IP:4101$p"
    fi
  }
  body() { sed '$d'; }
  code() { tail -1; }

  secrets_env() {
    local k; k=$(printf 'a%.0s' $(seq 32) | base64 -w0)
    export AURORA_DATABASE_URL="postgres://$DB_USER:$DB_PASS@127.0.0.1:$RELAY_PORT/$DB_NAME"
    export AURORA_ENCRYPTION_KEYS="k1:$k" AURORA_ENCRYPTION_ACTIVE_KEY=k1 AURORA_SIGNING_KEY="$k"
    export AURORA_INTERNAL_SECRET="$k" AURORA_RUNNER_SESSION_KEY="$k" AURORA_RUNNER_TOKEN="$TOKEN"
    export AURORA_BOOTSTRAP_RUNNER_TOKEN="$TOKEN" AURORA_CONNECTOR_TOKEN=aun_upgtoken
  }
  secrets_env

  # cfg <name> <bind> <image-line> <proxy-scheme>: the phase-0 baseline gateway serves plaintext
  # (http), HEAD is TLS-only (RFE-002: https; an old in-sandbox supervisor must be restarted).
  # A phase-1+ baseline is already TLS-only (https) and signs in through the dev IdP (sign_in below).
  write_cfg() {
    local f="$W/$1.toml"
    cat >"$f" <<EOF
[server]
external_url = "$ORIGIN"
bind = "$2"

[runner]
ops_bind = "$OPS_IP"
backend = "podman"
api_url = "http://$API_IP:4101"
podman_socket = "$W/podman.sock"
sandbox_network = "$SBX_NET"
${3}
max_sandboxes = 4
reconcile_interval_secs = 20
capacity_wait_secs = 5

[runner.sandbox_hosts]
aurora-gateway = "$BRIDGE"
aurora-runner = "$BRIDGE"

[hosted]
enabled = true
allow_fake_harness = true
idle_stop_secs = 1800

[auth]
signup_mode = "open"
org_creation = "anyone"

[telemetry]
log_level = "info"

[gateway]
internal_bind = "$OPS_IP"
allowed_private = ["127.0.0.1/32"]
ca_dir = "$W/ca"
advertised_proxy_url = "$4://aurora-gateway:4102"

[api]
hookhost_url = "http://127.0.0.1:9"
EOF
    # Phase 1 (RFE-004): HEAD needs an OIDC provider to start (the baseline predates the key and
    # rejects unknown ones). Nothing signs in on HEAD here: the baseline session is the test.
    # A phase-1+ baseline already requires it (auth.oidc_providers or auth.dev_idp.enabled).
    if [ "$1" = head ] || [ "$PHASE" != 0 ]; then
      printf '\n[auth.dev_idp]\nenabled = true\nissuer = "http://localhost:4107"\n' >>"$f"
    fi
  }

  PIDS=()
  start_svc() { # <label> <bin-dir> <exe> <cfg> <bind>
    local log="$W/$1-$3.log"
    AURORA_CONFIG="$4" "$2/$3" serve >"$log" 2>&1 &
    PIDS+=("$!"); echo "$! $3 $1" >>"$W/pids"
  }
  stop_svcs() {
    for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null; done
    for p in "${PIDS[@]:-}"; do [ -n "$p" ] && timeout 20 tail --pid="$p" -f /dev/null 2>/dev/null; done
    PIDS=()
  }
  wait_ok() {
    for _ in $(seq 150); do curl -fsS -m 2 "$1" >/dev/null 2>&1 && return 0; sleep 0.4; done
    echo "not ready: $1" >&2; return 1
  }
  wait_turn() { # turn-id secs -> prints final status
    local s
    for _ in $(seq $(($2 * 2))); do
      s=$(sql "SELECT status FROM turns WHERE id='$1'")
      case "$s" in done|failed|aborted) echo "$s"; return 0;; esac
      sleep 0.5
    done
    echo "timeout:$s"; return 1
  }
  say() { # conv text -> turn id
    api POST "/v1/conversations/$1/messages" "$(jq -nc --arg t "$2" '{text:$t}')" | body | jq -r '.turn.id'
  }
  sandbox_json() { api GET "/v1/agents/$AGENT/sandbox" | body; }
  wait_sandbox() { # state secs
    local j
    for _ in $(seq $(($2 * 2))); do
      j=$(sandbox_json); [ "$(echo "$j" | jq -r .state)" = "$1" ] && { echo "$j"; return 0; }
      sleep 0.5
    done
    echo "$j"; return 1
  }
  container() { podman ps -a -q --filter "label=aurora.installation=$FP" | head -1; }
  svc_logs() { for f in "$W"/*.log; do echo "--- $f"; tail -15 "$f"; done; }

  trap 'stop_svcs; [ -n "${RELAY_PID:-}" ] && kill "$RELAY_PID" 2>/dev/null' EXIT

  # Database relay: host unix socket -> netns loopback.
  socat TCP-LISTEN:"$RELAY_PORT",bind=127.0.0.1,fork,reuseaddr UNIX-CONNECT:"$W/pg.sock" &
  RELAY_PID=$!
  for _ in $(seq 25); do (echo >/dev/tcp/127.0.0.1/"$RELAY_PORT") 2>/dev/null && break; sleep 0.2; done

  # Mock upstream on the netns loopback (gateway.allowed_private opens 127.0.0.1/32): records every
  # request path, so "the request never reached the upstream" is observable (RFE-002 upgrade check).
  UP_PORT=18099; HITS="$W/up.hits"; : >"$HITS"
  python3 -I -c '
import http.server, sys
hits = sys.argv[1]
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        open(hits, "a").write(self.path + "\n")
        self.send_response(200); self.send_header("Content-Length", "2"); self.end_headers(); self.wfile.write(b"ok")
    def log_message(self, *a): pass
http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[2])), H).serve_forever()
' "$HITS" "$UP_PORT" & UP_PID=$!
  trap 'stop_svcs; [ -n "${RELAY_PID:-}" ] && kill "$RELAY_PID" 2>/dev/null; kill "$UP_PID" 2>/dev/null' EXIT
  hit() { grep -qx "$1" "$HITS"; }
  wait_hit() { for _ in $(seq 20); do hit "$1" && return 0; sleep 0.5; done; return 1; }

  echo "== phase 1: baseline stack @ $BASE_REF"
  BASE_BIN="$W/base/target/debug"; HEAD_BIN="$ROOT/target/debug"
  rm -rf "$W/ca" "$W/pids" "$W/cookies"; mkdir -p "$W/ca"
  "$BASE_BIN/aurora-gateway" generate-ca --out "$W/ca" >/dev/null || { echo "generate-ca failed"; exit 1; }
  BASE_SCHEME=http; [ "$PHASE" != 0 ] && BASE_SCHEME=https
  if [ "$PHASE" != 0 ]; then
    # Development IdP on the netns loopback (issuer http://localhost:4107, as in the dev config);
    # any login_hint signs in. It stays up across the upgrade (not part of PIDS).
    AURORA_DEV_IDP_LISTEN=127.0.0.1:4107 AURORA_DEV_IDP_USERS="$ROOT/deploy/dev/dev-idp-users.json" AURORA_DEV_IDP_ALLOW_ANY_HINT=1 \
      "$HEAD_BIN/aurora-dev-idp" >"$W/idp.log" 2>&1 &
    IDP_PID=$!
    trap 'stop_svcs; [ -n "${RELAY_PID:-}" ] && kill "$RELAY_PID" 2>/dev/null; kill "$UP_PID" "$IDP_PID" 2>/dev/null' EXIT
    wait_ok "http://127.0.0.1:4107/.well-known/openid-configuration" || { cat "$W/idp.log"; exit 1; }
  fi
  write_cfg base "$BRIDGE" "" "$BASE_SCHEME"   # baseline: image is fixed by the API (aurora-sandbox:0.1.0)
  for n in api gateway runner; do cp "$W/base.toml" "$W/base-$n.toml"; done
  sed -i "s/^bind = .*/bind = \"$API_IP\"/" "$W/base-api.toml"
  sed -i "s/^bind = .*/bind = \"$BRIDGE\"/" "$W/base-gateway.toml" "$W/base-runner.toml"
  AURORA_CONFIG="$W/base-api.toml" "$BASE_BIN/aurora-api" migrate >"$W/base-migrate.log" 2>&1 && ok "baseline migrate" || { bad "baseline migrate"; cat "$W/base-migrate.log"; exit 1; }
  BASE_MIGS=$(sql "SELECT count(*) FROM _sqlx_migrations WHERE success")
  start_svc base "$BASE_BIN" aurora-api "$W/base-api.toml"
  wait_ok "http://$API_IP:4101/readyz" || { svc_logs; exit 1; }
  start_svc base "$BASE_BIN" aurora-gateway "$W/base-gateway.toml"
  start_svc base "$BASE_BIN" aurora-runner "$W/base-runner.toml"
  wait_ok "http://$OPS_IP:4103/readyz" && wait_ok "http://$OPS_IP:4106/readyz" || { svc_logs; exit 1; }

  if [ "$PHASE" = 0 ]; then
    r=$(api POST /v1/auth/signup '{"email":"upg@e2e.test","password":"correct horse battery staple 42","display_name":"Upg"}')
    [ "$(echo "$r" | code)" = 201 ] && ok "signup" || { bad "signup: $r"; exit 1; }
  else
    # Scripted OIDC sign-in (docs/operator/sign-in.md): start -> IdP authorize (login_hint) -> callback.
    # The callback URL is built from external_url (no web server here): point it at the API.
    # The flow cookie is `__Host-aurora_oidc` (Secure): curl's jar would not send it over http, so pass the pair by hand.
    hdrs=$(curl -sS -m 20 -D - -o /dev/null "http://$API_IP:4101/v1/auth/oidc/dev/start?login_hint=upg%40e2e.test&return_to=%2F" | tr -d '\r')
    flow=$(echo "$hdrs" | sed -n 's/^[Ss]et-[Cc]ookie: \(__Host-aurora_oidc=[^;]*\).*/\1/p' | head -1)
    loc=$(echo "$hdrs" | sed -n 's/^[Ll]ocation: //p' | head -1)
    loc2=$(curl -sS -m 20 -o /dev/null -w '%{redirect_url}' "$loc")
    loc2="${loc2/$ORIGIN/http://$API_IP:4101}"
    cb=$(curl -sS -m 20 -c "$jar" -b "$jar" -H "Cookie: $flow" -o /dev/null -w '%{http_code} %{redirect_url}' "$loc2")
    case "$cb" in 3*" "*error=*) bad "oidc sign-in refused: $cb"; exit 1;; 3*) ok "sign-in through the dev IdP ($cb)";; *) bad "oidc sign-in: $cb (start=$loc idp=$loc2)"; exit 1;; esac
  fi
  me=$(api GET /v1/me | body)
  ORG=$(echo "$me" | jq -r '.memberships[0].org.id')
  WS=$(echo "$me" | jq -r '.memberships[0].workspaces[0].id')
  AGENT=$(api GET "/v1/workspaces/$WS/agents" | body | jq -r '.items[] | select(.handle=="daneel") | .id')
  [ -n "$AGENT" ] && [ "$AGENT" != null ] || { bad "daneel not found"; exit 1; }
  [ "$(api PATCH "/v1/agents/$AGENT" '{"harness":"fake"}' | code)" = 200 ] && ok "agent on fake harness" || bad "set fake harness"
  CONV=$(api POST "/v1/agents/$AGENT/conversations/direct" '{}' | body | jq -r .id)
  T1=$(say "$CONV" "[[say: before-upgrade]]")
  [ "$(wait_turn "$T1" 120)" = done ] && ok "baseline turn done (sandbox booted)" || { bad "baseline turn"; svc_logs; exit 1; }
  # Positive control: the phase-0 supervisor reaches the upstream through the plaintext baseline gateway.
  TH=$(say "$CONV" "[[http: GET http://127.0.0.1:$UP_PORT/before-upgrade]]")
  wait_turn "$TH" 120 >/dev/null
  wait_hit /before-upgrade && ok "baseline: sandbox request reaches the upstream through the gateway" || bad "baseline http through the gateway did not reach the upstream (policy/guard?)"
  r=$(api POST "/v1/agents/$AGENT/memories" '{"key":"upgrade-fact","title":"Upgrade","content":"survives the upgrade"}')
  case "$(echo "$r" | code)" in 200|201) ok "memory created (baseline API)";; *) bad "memory: $r";; esac
  # The baseline API has no schedules routes: seed the (existing) table directly.
  if [ "$PHASE" = 0 ]; then
    SCHED=$(sql "INSERT INTO schedules (id, agent_id, name, prompt, cron, timezone, enabled, next_fire_at) VALUES (gen_random_uuid(), '$AGENT', 'nightly', 'say hi', '0 3 * * *', 'UTC', true, now() + interval '1 day') RETURNING id")
    [ -n "$SCHED" ] && ok "schedule seeded via SQL (baseline has no schedules API)" || bad "schedule seed"
  else
    # Phase 1+: the baseline has the schedules, secrets and agents APIs; create the data through them.
    r=$(api POST "/v1/agents/$AGENT/schedules" '{"name":"nightly","prompt":"say hi","cron":"0 3 * * *","timezone":"UTC"}')
    SCHED=$(echo "$r" | body | jq -r '.id // empty')
    case "$(echo "$r" | code)" in 200|201) [ -n "$SCHED" ] && ok "schedule created (baseline API)" || bad "schedule: $r";; *) bad "schedule: $r";; esac
    r=$(api POST "/v1/orgs/$ORG/secrets" '{"name":"upg-secret","kind":"generic","host_pattern":"upg.example.test","injection":{"kind":"bearer"},"value":"upg-secret-value-1"}')
    SECRET=$(echo "$r" | body | jq -r '.id // empty')
    case "$(echo "$r" | code)" in 200|201) [ -n "$SECRET" ] && ok "secret created (baseline API)" || bad "secret: $r";; *) bad "secret: $r";; esac
    SECRET_VER=$(api GET "/v1/secrets/$SECRET" | body | jq -r .value_version)
    r=$(api POST "/v1/workspaces/$WS/agents" '{"name":"Upg Second","kind":"byo","handle":"upgsecond"}')
    AGENT2=$(echo "$r" | body | jq -r '(.agent // .).id // empty')
    case "$(echo "$r" | code)" in 200|201) [ -n "$AGENT2" ] && ok "second agent created (baseline API)" || bad "agent2: $r";; *) bad "agent2: $r";; esac
  fi
  BOOT1=$(sandbox_json | jq -r .boot_id)
  CID=$(container)
  CIMG_BASE=$(podman inspect -f '{{.Image}}' "$CID")
  EV_BEFORE=$(api GET "/v1/conversations/$CONV/events?limit=500" | body | jq '.items | length')
  echo "  baseline: boot=$BOOT1 container=${CID:0:12} image=${CIMG_BASE:0:12} events=$EV_BEFORE migrations=$BASE_MIGS"
  [ "$CIMG_BASE" = "$(podman image inspect -f '{{.Id}}' "$BASE_IMG")" ] && ok "sandbox runs the baseline image" || bad "sandbox not on baseline image"

  echo "== phase 2: upgrade in place to HEAD ($(git -C "$ROOT" rev-parse --short HEAD))"
  stop_svcs   # baseline api/gateway/runner stop; the sandbox container stays
  sleep 1
  [ "$(podman inspect -f '{{.State.Running}}' "$CID" 2>/dev/null)" = true ] && ok "sandbox container still running with services down" || bad "sandbox died when services stopped"
  write_cfg head "$BRIDGE" "image = \"$HEAD_IMG\"" https
  for n in api gateway runner; do cp "$W/head.toml" "$W/head-$n.toml"; done
  sed -i "s/^bind = .*/bind = \"$API_IP\"/" "$W/head-api.toml"
  AURORA_CONFIG="$W/head-api.toml" "$HEAD_BIN/aurora-api" migrate >"$W/head-migrate.log" 2>&1 && ok "HEAD migrate over the baseline schema" || { bad "HEAD migrate"; cat "$W/head-migrate.log"; }
  HEAD_MIGS=$(sql "SELECT count(*) FROM _sqlx_migrations WHERE success")
  FILES=$(ls "$ROOT"/crates/aurora-db/migrations/*.sql | wc -l)
  check "all $FILES HEAD migrations recorded as success (baseline had $BASE_MIGS)" [ "$HEAD_MIGS" = "$FILES" ]
  start_svc head "$HEAD_BIN" aurora-api "$W/head-api.toml"
  wait_ok "http://$API_IP:4101/readyz" && ok "HEAD api ready" || { bad "HEAD api"; svc_logs; }
  start_svc head "$HEAD_BIN" aurora-gateway "$W/head-gateway.toml"
  start_svc head "$HEAD_BIN" aurora-runner "$W/head-runner.toml"
  wait_ok "http://$OPS_IP:4103/readyz" && wait_ok "http://$OPS_IP:4106/readyz" && ok "HEAD gateway and runner ready" || { bad "HEAD gateway/runner"; svc_logs; }
  # The session cookie of the baseline survives (sessions are rows).
  [ "$(api GET /v1/me | code)" = 200 ] && ok "baseline session still valid on HEAD api" || bad "session invalid after upgrade"

  echo "== phase 3: verify"
  SB=$(wait_sandbox running 90) && ok "sandbox reported running by HEAD api" || bad "sandbox state after upgrade: $(echo "$SB" | jq -c '{state,failure_code}')"
  check "same container (id ${CID:0:12})" [ "$(container)" = "$CID" ]
  check "container still running" [ "$(podman inspect -f '{{.State.Running}}' "$CID")" = true ]
  check "same boot (no restart happened)" [ "$(echo "$SB" | jq -r .boot_id)" = "$BOOT1" ]
  # Old supervisor, new runner/api: a new turn on the running sandbox.
  T2=$(say "$CONV" "[[say: after-upgrade]]")
  R=$(wait_turn "$T2" 120)
  if [ "$R" = done ]; then ok "turn on the OLD supervisor via the NEW runner/api completes"; else bad "turn after upgrade: $R"; svc_logs; fi
  check "same boot after the turn" [ "$(sandbox_json | jq -r .boot_id)" = "$BOOT1" ]
  # No data loss.
  EV_AFTER=$(api GET "/v1/conversations/$CONV/events?limit=500" | body | jq '.items | length')
  check "conversation history preserved ($EV_BEFORE events before, $EV_AFTER after)" [ "$EV_AFTER" -gt "$EV_BEFORE" ]
  api GET "/v1/conversations/$CONV/events?limit=500" | body | jq -c '.items' | grep -q 'before-upgrade' && ok "pre-upgrade transcript text readable" || bad "pre-upgrade transcript missing"
  [ "$(api GET "/v1/agents/$AGENT/memories/upgrade-fact" | body | jq -r .content)" = "survives the upgrade" ] && ok "memory readable via HEAD api" || bad "memory lost"
  sr=$(api GET "/v1/schedules/$SCHED")
  [ "$(echo "$sr" | code)" = 200 ] && [ "$(echo "$sr" | body | jq -r .cron)" = "0 3 * * *" ] && ok "schedule readable via HEAD api (cron intact)" || bad "schedule: $sr"
  if [ "$PHASE" = 0 ]; then
    # (phase 1, RFE-002) The running phase-0 supervisor forwards plaintext to a TLS-only 4102: it must
    # fail closed (nothing reaches the upstream, no plaintext listener exists).
    TF=$(say "$CONV" "[[http: GET http://127.0.0.1:$UP_PORT/after-upgrade]]")
    RF=$(wait_turn "$TF" 120)
    echo "  old-supervisor http turn after the upgrade: $RF"
    sleep 1
    hit /after-upgrade && bad "FAIL-OPEN: the old sandbox's request reached the upstream after the gateway became TLS-only" || ok "old sandbox fails closed: its request never reached the upstream"
    check "gateway log shows no plaintext proxy request served after the upgrade" [ "$(grep -c 'after-upgrade' "$W/head-aurora-gateway.log" 2>/dev/null || true)" = 0 ]
  else
    # Phase 1+: the old supervisor already speaks TLS to the gateway, so its request must still reach
    # the upstream through the NEW gateway (same CA, same TLS listener contract).
    TF=$(say "$CONV" "[[http: GET http://127.0.0.1:$UP_PORT/after-upgrade]]")
    wait_turn "$TF" 120 >/dev/null
    wait_hit /after-upgrade && ok "old supervisor's request reaches the upstream through the NEW gateway over TLS" || bad "old supervisor's request did not reach the upstream after the upgrade"
    # Data-bearing rows written through the baseline API read back through HEAD.
    check "secret still listed by HEAD api (value_version $SECRET_VER unchanged)" [ "$(api GET "/v1/secrets/$SECRET" | body | jq -r '[.name,.status,.value_version]|join(",")')" = "upg-secret,active,$SECRET_VER" ]
    check "second agent readable via HEAD api" [ "$(api GET "/v1/agents/$AGENT2" | body | jq -r '(.agent // .).handle')" = upgsecond ]
  fi
  # restart_to_update (docs/15 §Upgrades). Phase 0: the boot inputs changed (gateway.tls), so it is flagged.
  # Phase 1+: whether HEAD's boot inputs differ is not specified; the flag must be a boolean and clear after the restart.
  SB=$(sandbox_json)
  echo "  sandbox view: $(echo "$SB" | jq -c '{state,image,restart_to_update}')"
  check "sandbox view carries restart_to_update (boolean)" [ "$(echo "$SB" | jq -r '.restart_to_update|type')" = boolean ]
  [ "$PHASE" = 0 ] && check "restart_to_update is true for the sandbox booted before TLS-only (boot inputs changed)" [ "$(echo "$SB" | jq -r .restart_to_update)" = true ]
  # Restart: the next boot is on the new image.
  rs=$(api POST "/v1/agents/$AGENT/restart" '{}')
  [ "$(echo "$rs" | code)" = 202 ] && ok "restart accepted" || bad "restart: $rs"
  T3=$(say "$CONV" "[[say: on-new-image]]")
  R=$(wait_turn "$T3" 180)
  [ "$R" = done ] && ok "turn after restart completes" || { bad "turn after restart: $R"; svc_logs; }
  TN=$(say "$CONV" "[[http: GET http://127.0.0.1:$UP_PORT/on-new-boot]]")
  wait_turn "$TN" 120 >/dev/null
  wait_hit /on-new-boot && ok "after restart the sandbox reaches the upstream over TLS" || bad "after restart the request did not reach the upstream (TLS path broken)"
  BOOT2=$(sandbox_json | jq -r .boot_id)
  check "restart minted a new boot" [ "$BOOT2" != "$BOOT1" ]
  CID2=$(container)
  CIMG2=$(podman inspect -f '{{.Image}}' "$CID2")
  check "restarted sandbox runs the HEAD image" [ "$CIMG2" = "$(podman image inspect -f '{{.Id}}' "$HEAD_IMG")" ]
  check "old container is gone (one container left)" [ "$(podman ps -a -q --filter "label=aurora.installation=$FP" | wc -l)" = 1 ]
  sandbox_json | jq -e '.restart_to_update == false' >/dev/null && ok "restart_to_update clear after restart" || bad "restart_to_update still set after restart"
  [ "$(api GET "/v1/agents/$AGENT/memories/upgrade-fact" | body | jq -r .content)" = "survives the upgrade" ] && ok "memory still there after restart" || bad "memory lost after restart"
  # Protocol N/N-1.
  BASE_PROTO=$(git -C "$ROOT" show "$BASE_REF:crates/aurora-protocol/src/version.rs" | grep -A6 'pub const fn current' | grep -c '=> 1,')
  HEAD_PROTO=$(grep -A6 'pub const fn current' "$ROOT/crates/aurora-protocol/src/version.rs" | grep -c '=> 1,')
  echo "  protocol planes at version 1: baseline=$BASE_PROTO head=$HEAD_PROTO (of 4)"
  echo "  NOTE  every plane is still at protocol 1 on both refs, so N-1 (= 0) does not exist yet: the old supervisor/1 working against the new runner/api (above) is the real N==N check; N/N-1 negotiation (accept N-1, 426 outside the range) is covered by crates/aurora-protocol unit tests only, not end to end."
  echo
  echo "== summary: $PASS passed, $FAIL failed, ${#DEFECTS[@]} defect(s) reported"
  for d in "${DEFECTS[@]:-}"; do [ -n "$d" ] && echo "DEFECT: $d"; done
  if [ "$FAIL" -gt 0 ]; then svc_logs; exit 1; fi
  [ "${#DEFECTS[@]}" -gt 0 ] && [ "${UPGRADE_STRICT:-}" = 1 ] && exit 3
  exit 0
fi

# ------------------------------------------------------------------------------------------------
# Outer phase: build, podman plumbing, cleanup.
# ------------------------------------------------------------------------------------------------
[ -f .env ] || { echo ".env missing (cp from the main checkout)" >&2; exit 1; }
DBURL=$(grep -E '^DATABASE_URL=' .env | cut -d= -f2-)
re='^postgres(ql)?://([^:]+):([^@]+)@([^:/]+):([0-9]+)/(.+)$'
[[ "$DBURL" =~ $re ]] || { echo "cannot parse DATABASE_URL" >&2; exit 1; }
export DB_USER="${BASH_REMATCH[2]}" DB_PASS="${BASH_REMATCH[3]}" DB_HOST="${BASH_REMATCH[4]}" DB_PORT="${BASH_REMATCH[5]}"
export DB_NAME="aurora_upg_$(od -An -N4 -tx1 /dev/urandom | tr -d ' \n')"
# Baseline phase. Auto-detect: a base whose aurora-config knows `dev_idp` (phase 1, RFE-004: OIDC-only
# sign-in, TLS-only proxy listener) is a phase-1+ baseline ("2"); an older one is phase 0.
if [ -z "${UPGRADE_BASE_PHASE:-}" ]; then
  if git -C "$ROOT" grep -q dev_idp "$BASE_REF" -- crates/aurora-config/src 2>/dev/null; then PHASE=2; else PHASE=0; fi
else
  PHASE="$UPGRADE_BASE_PHASE"
fi
case "$PHASE" in 0) ;; 1|2) PHASE=2;; *) echo "UPGRADE_BASE_PHASE must be 0, 1 or 2" >&2; exit 1;; esac
echo "== baseline phase: $PHASE (base $BASE_REF)"
export PHASE W BASE_REF SBX_NET BASE_IMG HEAD_IMG BRIDGE API_IP OPS_IP ORIGIN RELAY_PORT TOKEN FP ROOT

admin_sql() { PGPASSWORD="$DB_PASS" psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d postgres -Atq -c "$1"; }

SVC_PID=""; RELAY_HOST_PID=""
cleanup() {
  [ -n "$RELAY_HOST_PID" ] && kill "$RELAY_HOST_PID" 2>/dev/null
  podman ps -a -q --filter "label=aurora.installation=$FP" | xargs -r podman rm -f -t 1 >/dev/null 2>&1
  podman volume ls -q --filter "label=aurora.installation=$FP" | xargs -r podman volume rm -f >/dev/null 2>&1
  podman rm -f -t 1 "$ANCHOR" >/dev/null 2>&1
  [ -n "$SVC_PID" ] && kill "$SVC_PID" 2>/dev/null
  for _ in 1 2 3; do podman network rm -f "$SBX_NET" >/dev/null 2>&1 && break; sleep 1; done
  admin_sql "DROP DATABASE IF EXISTS $DB_NAME WITH (FORCE)" >/dev/null 2>&1
  if [ "${KEEP:-}" != "1" ]; then
    git -C "$ROOT" worktree remove --force "$W/base" >/dev/null 2>&1
    git -C "$ROOT" worktree prune
    rm -rf "$W/base" "$W/ca" "$W/cookies" "$W/pg.sock" "$W/podman.sock" "$W/pids" "$W"/*.toml "$W"/*.log "$W/registries.conf"
    if [ "${SKIP_BUILD:-}" != "1" ]; then
      podman rmi -f "$BASE_IMG" "$HEAD_IMG" >/dev/null 2>&1
    fi
  fi
}
trap cleanup EXIT
mkdir -p "$W"

free_kb=$(df --output=avail -k "$ROOT" | tail -1)
if [ "$free_kb" -lt $((15 * 1024 * 1024)) ]; then echo "less than 15 GB free; stopping" >&2; exit 1; fi

echo "== build"
if [ "${SKIP_BUILD:-}" != "1" ]; then
  git -C "$ROOT" worktree remove --force "$W/base" >/dev/null 2>&1; rm -rf "$W/base"
  git -C "$ROOT" worktree add --detach "$W/base" "$BASE_REF" >/dev/null || { echo "worktree add failed" >&2; exit 1; }
  (cd "$W/base" && timeout 3000 cargo build -p aurora-api -p aurora-gateway -p aurora-runner) || { echo "baseline cargo build failed" >&2; exit 1; }
  (cd "$ROOT" && timeout 3000 cargo build -p aurora-api -p aurora-gateway -p aurora-runner) || { echo "HEAD cargo build failed" >&2; exit 1; }
  if [ "$PHASE" != 0 ]; then
    (cd "$ROOT" && timeout 3000 cargo build -p aurora-testkit --bin aurora-dev-idp) || { echo "dev IdP build failed" >&2; exit 1; }
  fi
  # The image builder keeps one cargo target in a shared cache mount: bump source mtimes so cargo
  # never reuses the other tree's artifacts (COPY preserves mtimes).
  touch_sources() { (cd "$1" && git ls-files -z '*.rs' '*.toml' Cargo.lock | xargs -0 touch); }
  touch_sources "$W/base"
  (cd "$W/base" && timeout 3000 scripts/images/build-sandbox-min.sh "$BASE_IMG") || { echo "baseline image build failed" >&2; exit 1; }
  touch_sources "$ROOT"
  (cd "$ROOT" && timeout 3000 scripts/images/build-sandbox-min.sh "$HEAD_IMG") || { echo "HEAD image build failed" >&2; exit 1; }
fi
for b in aurora-api aurora-gateway aurora-runner; do
  [ -x "$W/base/target/debug/$b" ] && [ -x "$ROOT/target/debug/$b" ] || { echo "missing binary $b (run without SKIP_BUILD)" >&2; exit 1; }
done
podman image exists "$BASE_IMG" && podman image exists "$HEAD_IMG" || { echo "missing sandbox images" >&2; exit 1; }
[ "$(podman image inspect -f '{{.Id}}' "$BASE_IMG")" != "$(podman image inspect -f '{{.Id}}' "$HEAD_IMG")" ] || echo "note: baseline and HEAD sandbox images are identical" >&2

echo "== podman plumbing (aurora-upg-*)"
cat >"$W/registries.conf" <<EOF
unqualified-search-registries = ["docker.io"]
[aliases]
"aurora-sandbox" = "localhost/aurora-upg-sandbox"
EOF
rm -f "$W/podman.sock"
CONTAINERS_REGISTRIES_CONF="$W/registries.conf" podman system service -t 0 "unix://$W/podman.sock" >"$W/podman-service.log" 2>&1 &
SVC_PID=$!
for _ in $(seq 50); do [ -S "$W/podman.sock" ] && break; sleep 0.2; done
[ -S "$W/podman.sock" ] || { echo "podman service did not start" >&2; exit 1; }
podman network rm -f "$SBX_NET" >/dev/null 2>&1   # leftover of an aborted run (aurora-upg- only)
podman network create --internal --disable-dns --subnet 10.89.245.0/24 --gateway "$BRIDGE" "$SBX_NET" >/dev/null || { echo "network create failed (subnet in use?)" >&2; exit 1; }
podman run -d --name "$ANCHOR" --network "$SBX_NET" --entrypoint sleep "$HEAD_IMG" 100000 >/dev/null || exit 1

admin_sql "CREATE DATABASE $DB_NAME" || { echo "cannot create database on the shared Postgres" >&2; exit 1; }
rm -f "$W/pg.sock"
socat UNIX-LISTEN:"$W/pg.sock",fork,mode=600 TCP:"$DB_HOST":"$DB_PORT" &
RELAY_HOST_PID=$!
for _ in $(seq 25); do [ -S "$W/pg.sock" ] && break; sleep 0.2; done

echo "== upgrade test (database $DB_NAME)"
podman unshare --rootless-netns env "W=$W" bash "$0" --inner
rc=$?
echo "== upgrade.sh exit $rc"
exit $rc
