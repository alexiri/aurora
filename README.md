<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/design/ui-refresh/brand/aurora-lockup-dark.svg">
    <img src="docs/design/ui-refresh/brand/aurora-lockup.svg" alt="Aurora — build · run · collaborate" width="320">
  </picture>
</p>

Aurora is a self-hosted platform for running AI agents as a team. Every agent gets its own identity, only
the credentials and apps it was explicitly granted, and optionally a sandboxed computer. Agents never hold
real secrets: all of their traffic goes through a gateway that authenticates each request, evaluates
policy, can hold it for human approval, and only then injects the real credential. People work with agents
from a web dashboard or from chat platforms such as Mattermost.

The backend is Rust and the dashboard is Next.js. The specification lives in
[`docs/MASTER_SPEC.md`](docs/MASTER_SPEC.md).

## Run a local copy

Two ways to run Aurora on your machine. Both are for development and trying things out, not for
production ([production install](docs/operator/install-helm.md)).

### Full stack on kind (recommended for playing around)

The production Helm chart on a local Kubernetes cluster, built from your checkout. It includes hosted agents
running in sandbox pods.

You need `podman` (rootless), `kind` 0.30 or newer, `kubectl`, `helm` 3, `just`, and about 15 GB of free
disk. Rust and Node are not needed on the host: everything is built in containers.

```sh
just kind-up --llm-mock --connector-fake   # first run takes about 20–25 min; later runs reuse the build cache
```

Open **http://localhost:4000** and click **Development sign-in**: all sign-in is OIDC, and the dev stack ships
a development identity provider (`aurora-dev-idp`) that lists a few users (`deploy/dev/dev-idp-users.json`
and `devIdp.users` in the chart) and signs you in without a password. Sign-up is open on the dev stack, so
your first sign-in creates your account with its own organization and a default agent. The development
sign-in only runs on a `localhost` URL; never enable it anywhere else.

To make someone an instance administrator (plugins, connectors, trusted keys), pre-provision them and then
sign in with that e-mail, which binds the identity:

```sh
deploy/dev/kind-dev.sh bootstrap-admin you@example.test
# the same thing by hand: the dev images call the binary /usr/local/bin/aurora-service, not aurora-api
kubectl --context kind-aurora-dev -n aurora exec deploy/aurora-api -- \
  /usr/local/bin/aurora-service bootstrap-admin --email you@example.test
```

- Agents run a **fake harness**, and `--llm-mock` adds a mock LLM endpoint (it is not a real model). To use
  a real model, see [LLM endpoints](docs/operator/llm-endpoints.md).
- `--connector-fake` adds a fake chat connector, to try the chat-channel features without a real chat
  platform.

After changing code, rebuild and restart only what changed (about 40 s per service):

```sh
just kind-dev            # or name services: just kind-dev api gateway web
just kind-live           # or let it watch your edits and run kind-dev for you
just kind-status         # pods of the dev cluster
just kind-down           # delete the cluster (the cargo cache volume is kept)
```

### Lightweight loop on the host

The API, gateway, hook host, runner and dashboard as plain host processes, with live reload: a Rust service is
rebuilt and restarted when you save, and the dashboard uses Next.js Fast Refresh. It needs the least setup.
The runner uses a simulated sandbox backend by default; [live reload](docs/ops/live-reload.md) explains how
to switch to real podman sandboxes.

You need Rust (stable, via `rustup`; `rust-toolchain.toml` selects it), Node 20.9 or newer with `pnpm`,
`python3`, `podman` (for the dev Postgres) and `just`.

```sh
pnpm --dir web install   # once
just dev                 # Postgres on 127.0.0.1:55432, migrations, then all services; Ctrl-C stops them
```

Open **http://localhost:4100** and click **Development sign-in**. `just dev` also starts the development
identity provider on `http://localhost:4107` (all sign-in is OIDC); it lists the users of
`deploy/dev/dev-idp-users.json`, and the first sign-in creates your account. For an instance administrator,
run `. deploy/dev/env.sh && cargo run -p aurora-api -- bootstrap-admin --email you@example.test` and sign in
with that e-mail.

### More detail

- [Dev stack on kind](docs/operator/install-kind-dev.md): profiles, fast iteration, tests against the stack,
  troubleshooting.
- [Development environments](docs/ops/dev-environments.md): why kind replaced compose, with measurements.
- `just --list` shows every recipe, including `just test` for the workspace tests.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
