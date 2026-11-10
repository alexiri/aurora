# Harnesses: enabling Codex

A **harness** is the coding agent that runs inside an agent's computer. Aurora ships two real
ones: **Claude Code** (the default, `claude-code`) and the **OpenAI Codex CLI** (`codex`,
phase 4, D-122). Nothing changes for a deployment that does nothing: the default is
`hosted.enabled_harnesses = ["claude-code"]`, and an agent's harness is chosen by Aurora's API
only, never from inside the sandbox.

The design is in [docs/12 §Harness selection and adapter contract](../12-supervisor-harness.md#harness-selection-and-adapter-contract-phase-4-p4-har-d-122-sec-66)
and [§Adapter codex](../12-supervisor-harness.md#adapter-codex-phase-4-p4-har); decisions D-122 and D-153.

## What Codex needs

| Need | Where |
|------|-------|
| The harness is enabled | `hosted.enabled_harnesses` includes `codex` |
| A sandbox image that contains the Codex CLI | the default `aurora-sandbox` image does; build with `--build-arg SANDBOX_HARNESSES=claude-code` to leave it out |
| A runner that knows the image has it | podman reads the image label `org.aurora.harnesses`; on Kubernetes set `runner.image_harnesses` |
| An LLM endpoint that serves the **`openai_responses`** dialect | a LiteLLM endpoint (it serves `openai_chat`, `openai_responses` and `anthropic_messages` by default) or any endpoint that lists `openai_responses` in `dialects` |
| A granted LLM key on that endpoint | the usual key and grant flow ([llm-endpoints.md](llm-endpoints.md)) |

The pinned Codex CLI is **0.167.0**. That version no longer speaks the OpenAI chat-completions
wire API, so an endpoint that only serves `openai_chat` is shown as "no shared dialect" for
Codex agents. A model is only ever used through an endpoint whose dialect the harness speaks;
the API enforces this, never the harness.

## Enable it

```toml
[hosted]
enabled_harnesses = ["claude-code", "codex"]
# default_harness = "claude-code"   # must be one of enabled_harnesses
```

With Helm, put the same keys under `config.hosted`:

```yaml
config:
  hosted:
    enabled_harnesses: ["claude-code", "codex"]
  runner:
    # Kubernetes cannot read the image label: say what the sandbox image contains.
    image_harnesses: ["claude-code", "codex"]
```

An unknown id, an empty list, a `default_harness` that is not enabled, or `fake` without
`hosted.allow_fake_harness` stops startup naming the key.

## The sandbox image

`images/sandbox/Containerfile` installs both runtimes by default and writes the label
`org.aurora.harnesses` (for example `claude-code,codex,fake`):

```sh
podman build -f images/sandbox/Containerfile --target sandbox -t aurora-sandbox:dev .
# without Codex (a slimmer image; the Codex binary is about 300 MB):
podman build -f images/sandbox/Containerfile --target sandbox \
  --build-arg SANDBOX_HARNESSES=claude-code -t aurora-sandbox:slim .
```

The Codex release (`rust-v0.167.0`, the static musl build) is downloaded at build time and its
SHA-256 is verified before it is unpacked; a mismatch fails the build. To move to another
version change `CODEX_VERSION` and both checksums together with `PINNED_VERSION` in
`services/aurora-supervisor/src/adapters/codex/mod.rs`, and re-run the Codex conformance suite.
An image built before phase 4 has no label and is treated as `claude-code,fake`, so old
images keep working and simply do not offer Codex.

## Choosing a harness for an agent

In the dashboard, **Instructions → Harness** lists the enabled harnesses when more than one is
enabled. One that no runner offers is disabled ("no runner offers this"). The same is
`PATCH /v1/agents/{agent}` with `{ "harness": "codex" }` and `GET /v1/harnesses`.

Changing an agent's harness:

- is refused while a turn is running (`409 turn_active`);
- starts a fresh harness session in every conversation of the agent; the transcript stays, but
  the new harness does not see the old one's private context;
- deletes the old harness's session files in the agent's computer, using the old harness's own
  code, and restarts the computer so it boots with the new one;
- drops a model override whose endpoint the new harness cannot use (with a notice). With no
  compatible granted key the next turn fails `no_model_key`.

## Failures you may see

| Where | Code | Meaning and fix |
|-------|------|-----------------|
| A turn | `harness_unavailable` | At least one runner is online and none lists the agent's harness. Enable an image that contains it (`runner.image`, `runner.image_harnesses`) or choose another harness. |
| A turn / the computer | `harness_unsupported` | The supervisor refused to boot: the harness id is unknown to the image, or its runtime is missing. There is no fallback to another harness. Use an image built with that runtime. |
| A turn | `no_model_key` | No granted key's endpoint serves the harness's dialect (`openai_responses` for Codex). |
| API | `422 harness_not_enabled` | The harness is not in `hosted.enabled_harnesses`. |

## What Codex can and cannot do here

- It runs with Codex's own sandbox and approval prompts **off**, because the Aurora sandbox and
  gateway are the isolation and policy boundaries (Codex's inner sandbox cannot nest in the
  container). Everything it does still goes through the gateway: policy, approvals, credentials.
- Its model traffic goes through the per-session proxy with the turn's credential. The files and
  environment it sees hold only the placeholder `OPENAI_API_KEY=aurora-placeholder`; the gateway
  injects the real key. No `auth.json` is ever written, and **ChatGPT-account sign-in is not
  supported**.
- Follow-up messages become separate turns (Codex cannot take a message while it runs), and there
  is no "move to background" for its running tool.
- Update checks, analytics, web search and Codex's own app, browser and plugin features are
  turned off. Skills are listed under `~/.codex/skills/aurora`; instructions are in `AGENTS.md`.
- Deleting a conversation or switching harness removes Codex's session (rollout) files. Codex also
  keeps a small index row (thread id, title, timestamps) in its local SQLite state, which stays
  until Codex rebuilds the index (D-153).
