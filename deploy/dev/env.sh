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
export AURORA_CONNECTOR_TOKEN=aun_devtoken
