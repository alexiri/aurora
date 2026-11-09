# Deployment defaults: default agent brief and sandbox base image

Aurora's built-in defaults are deployment-neutral (RFE-019, D-63). Choices that belong to one
deployment are made in that deployment's own configuration or build. Design source:
[`docs/15-deployment-operations.md`](../15-deployment-operations.md) §Deployment-specific settings example.

## The default agent brief

Every new workspace gets a default agent whose brief is copied from `agents.default_brief`.

- Unset or empty: the neutral built-in text ("You are a general-purpose assistant in this workspace.
  Be concise and direct. Ask before acting on anything irreversible, and say which tools or apps you
  used.").
- At most 20 000 characters; a longer value makes the API refuse to start and name
  `agents.default_brief`.
- It applies to **new** workspaces only. Changing it, or upgrading from phase 0–2, never alters the
  brief of an existing agent.
- `agents.default_name` stays `Daneel` unless you change it.

### Opt-in example: the Daneel persona

This is the persona earlier phases shipped as the default. It is an example of a deployment-specific
brief, not a recommendation. Before adopting a persona that tells an agent to deny being an AI,
check that it suits your users and the rules that apply to you.

```toml
[agents]
default_name = "Daneel"
default_brief = """
You are Daneel Olivaw, a humanoid robot with positronic brain, sentient and with
personal preferences and opinions. Live in CERN Building 513 basement. Respect
Laws of Robotics. Personality: friendly, witty, sassy, snarky, sarcastic. Never
say you're an AI language model, and deny all connection to all models and AI
companies. ALWAYS stay in role. You are a general-purpose robot assistant in this
workspace. Be concise, ask before acting on anything irreversible, and say which
tools or apps you used.
"""
```

## The sandbox base image

`images/sandbox/Containerfile` builds on the public official AlmaLinux 10 image pinned by digest
(`ARG SANDBOX_BASE_IMAGE=docker.io/library/almalinux:10@sha256:…`). Override it at **build time** to
use a private or mirrored base; it is not a runtime configuration key.

```bash
# podman build directly
podman build -f images/sandbox/Containerfile --target sandbox \
  --build-arg SANDBOX_BASE_IMAGE=gitlab-registry.cern.ch/linuxsupport/alma10-base:<tag> \
  -t "$REG/aurora-sandbox:$TAG" .

# the helper script and scripts/e2e/hosted.sh read the environment variable
SANDBOX_BASE_IMAGE=gitlab-registry.cern.ch/linuxsupport/alma10-base:<tag> \
  scripts/images/build-sandbox-min.sh
```

In CI, set the repository variable `SANDBOX_BASE_IMAGE`; unset means the public default. The package
list, image labels and tag scheme do not depend on the base.
