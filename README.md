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

- `--llm-mock` adds a mock LLM endpoint (LiteLLM in front of VidaiMock; it is not a real model) and makes
  hosted agents run the **real claude-code harness** (see "Try it" below). Without it, hosted agents use a
  fake harness. To use a real model, see [LLM endpoints](docs/operator/llm-endpoints.md).
- `--connector-fake` adds a fake chat connector, to try the chat-channel features without a real chat
  platform.

#### Try it: say hello to your agent

With `just kind-up --llm-mock` running, this is the whole first-run path (it is exactly what the Playwright
journey `web/e2e/first-run.spec.ts` does, run nightly and by `bash scripts/e2e/first-run.sh`):

1. Open **http://localhost:4000**, click **Development sign-in** and pick a user (or any e-mail). Your first
   sign-in creates your organization, a workspace and the default agent, **Daneel**.
2. **Credentials → LLM keys**: name it, choose the endpoint **LiteLLM (VidaiMock, dev only)** and paste the
   dev key `sk-aurora-mock-dev`. Click **Add LLM key**.
3. **Agents → Daneel → Access**: tick **Grant** next to your key.
4. **Chat**: send `hello`. The first message starts a sandbox and the Claude Code CLI (under a minute). The
   answer is **pong**: the mock's scripted reply, which went chat → API → runner → sandbox → gateway →
   LiteLLM → VidaiMock and back.
5. **Activity** shows the model call as an allowed request to `litellm.aurora-dev-mock.svc.cluster.local`,
   and the checklist on **Overview** shows all three steps done.

To point the same stack at a real LiteLLM instead of the mock, skip `--llm-mock` and add your endpoint to
the chart values (`config.llm.endpoints` with `provider: litellm` and your `base_url`; see
[LLM endpoints](docs/operator/llm-endpoints.md)). A key's host must be reachable from the gateway: private
addresses stay blocked unless listed in `gateway.allowed_private`, and the gateway's NetworkPolicy needs an
egress rule (`networkPolicy.gatewayExtraEgress`), as `deploy/kind/values-llm-mock.yaml` does for the mock. The
hosted claude-code harness needs a sandbox image that contains the CLI: build
`scripts/e2e/llm.Containerfile` on your sandbox image and set `config.runner.image`.

A second cluster beside another one: `AURORA_DEV_CLUSTER=mine AURORA_DEV_PORT=4010 just kind-up --llm-mock`
(open http://localhost:4010).

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
