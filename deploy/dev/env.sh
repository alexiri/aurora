# Source me: `. deploy/dev/env.sh` (dev-only throwaway secrets).
K=$(printf 'a%.0s' $(seq 1 32) | base64)
export AURORA_CONFIG="$(dirname "${BASH_SOURCE[0]}")/aurora.dev.toml"
export AURORA_DATABASE_URL="${AURORA_DATABASE_URL:-postgres://aurora:aurora@127.0.0.1:55432/aurora}"
export AURORA_ENCRYPTION_KEYS="k1:$K"
export AURORA_ENCRYPTION_ACTIVE_KEY=k1
export AURORA_SIGNING_KEY="$K"
export AURORA_INTERNAL_SECRET="$K"
export AURORA_RUNNER_SESSION_KEY="$K"
export AURORA_RUNNER_TOKEN=aur_devtoken
export AURORA_BOOTSTRAP_RUNNER_TOKEN="$AURORA_RUNNER_TOKEN"   # the API registers the dev runner at start (OPS-018)
export AURORA_CONNECTOR_TOKEN=aun_devtoken
# P1-005 (RFE-004): all sign-in is OIDC. `just dev` runs aurora-dev-idp on :4107 (the API registers it as the
# provider `dev`, deploy/dev/aurora.dev.toml). It lists these users and, for scripts and Playwright, signs in any
# `login_hint` e-mail. Development only: the API refuses the dev IdP unless server.external_url is loopback.
export AURORA_DEV_IDP_LISTEN=127.0.0.1:4107
export AURORA_DEV_IDP_USERS="$(dirname "${BASH_SOURCE[0]}")/dev-idp-users.json"
export AURORA_DEV_IDP_ALLOW_ANY_HINT=1
