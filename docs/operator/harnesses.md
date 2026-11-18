# Harnesses

A **harness** is the coding agent that runs inside an agent's computer. Aurora ships five real
ones: **Claude Code** (the default, `claude-code`), the **OpenAI Codex CLI** (`codex`, phase 4,
D-122), and, since phase 5 (P5-HARN, D-361..D-363), **Pi** (`pi`), **goose** (`goose`) and
**OpenCode** (`opencode`). All five get the same treatment: selection per agent, instructions and
skills, platform tools, the default model, session resume and purge, switching, provider-error
messages, the web terminal command and the dashboard. Claude Code remains the default.
Nothing changes for a deployment that does nothing: the default is
`hosted.enabled_harnesses = ["claude-code"]`, and an agent's harness is chosen by Aurora's API
only, never from inside the sandbox. **The added harnesses are not enabled by default.**

The design is in [docs/12 §Harness selection and adapter contract](../12-supervisor-harness.md#harness-selection-and-adapter-contract-phase-4-p4-har-d-122-sec-66),
[§Adapter codex](../12-supervisor-harness.md#adapter-codex-phase-4-p4-har) and
[§Adapters pi, goose and opencode](../12-supervisor-harness.md#adapters-pi-goose-and-opencode-phase-5-p5-harn-d-361-d-362-sec-133);
decisions D-122, D-153, D-361, D-362 and D-363.

(Phase 5, SET-5.) The list is an instance setting: **Administration › Agents › Defaults › Hosted agents**. It can only name
harnesses a registered runner reports (otherwise the save is refused with `harness_not_available`), and the runners' harnesses
are shown on the same page. `config.hosted.enabled_harnesses` in Helm locks it as a whole.

## The harnesses at a glance

| Harness (`id`) | Pinned version | Dialects it speaks | Reachable providers | Model roles | Steer a running turn | Added to the image |
|----------------|----------------|--------------------|---------------------|-------------|----------------------|--------------------|
| Claude Code (`claude-code`) | npm lockfile pin | `anthropic_messages`, `anthropic_bedrock`, `anthropic_vertex` | Anthropic, Bedrock and Vertex directly; anything else through LiteLLM | large, medium, small (aliases) | no | the base of every image (2.16 GB slim) |
| Codex (`codex`) | 0.167.0 | `openai_responses` | OpenAI, Azure OpenAI, LiteLLM, OpenAI-compatible endpoints that list `openai_responses` | none | no | about 300 MB |
| Pi (`pi`) | 1.5.2 | `anthropic_messages`, `openai_responses`, `openai_chat` | every endpoint that serves one of the three: Anthropic, OpenAI, Azure OpenAI, LiteLLM, OpenAI-compatible | none (Pi has no subagents) | yes | 154 MB |
| goose (`goose`) | 1.59.2 | `anthropic_messages`, `openai_chat` (not `openai_responses`) | Anthropic; LiteLLM and OpenAI-compatible endpoints that serve chat completions | large, medium, small (the subagent's model) | no | 199 MB |
| OpenCode (`opencode`) | 1.19.6 (the v1.19 line, D-363) | `anthropic_messages`, `openai_responses`, `openai_chat` | as Pi | large, medium, small (the small model and the subagent's model) | no | 206 MB |

None of the added harnesses declares Bedrock or Vertex: their SDKs sign with cloud credentials
that never enter a sandbox, so those providers reach Pi, goose and OpenCode only through a LiteLLM
endpoint. A model is only ever used through an endpoint whose dialect the harness speaks; the API
enforces this, never the harness, and an endpoint that none of the enabled harnesses speaks is
flagged on **Administration › LLM** ("No enabled harness speaks this API"). Sizes are measured
with podman: the default image is 3.07 GB with all five, 2.49 GB with `claude-code,codex`, and
2.16 GB with `SANDBOX_HARNESSES=claude-code`.

## What a harness needs

| Need | Where |
|------|-------|
| The harness is enabled | `hosted.enabled_harnesses` includes its id |
| A sandbox image that contains it | the default `aurora-sandbox` image contains all five; build with `--build-arg SANDBOX_HARNESSES=claude-code` (or any comma-separated subset) to leave some out |
| A runner that knows the image has it | podman reads the image label `org.aurora.harnesses`; on Kubernetes set `runner.image_harnesses` (it has no chart default, `[]`, and the podman backend reads the label) |
| An LLM endpoint that serves one of its dialects | the table above; a LiteLLM endpoint serves `openai_chat`, `openai_responses` and `anthropic_messages` by default |
| A granted LLM key on that endpoint | the usual key and grant flow ([llm-endpoints.md](llm-endpoints.md)) |

A LiteLLM endpoint's key may also read `GET /model/info` (goose asks for the model's context
window at every start); that one path is on by default for LiteLLM endpoints only
([llm-endpoints.md](llm-endpoints.md#inference-paths-only), D-367).

The pinned Codex CLI no longer speaks the OpenAI chat-completions wire API, so an endpoint that
only serves `openai_chat` is shown as "no shared dialect" for Codex agents but works with Pi,
goose and OpenCode.

## Enable them

```toml
[hosted]
enabled_harnesses = ["claude-code", "codex", "pi", "goose", "opencode"]
# default_harness = "claude-code"   # must be one of enabled_harnesses
```

With Helm, put the same keys under `config.hosted`:

```yaml
config:
  hosted:
    enabled_harnesses: ["claude-code", "codex", "pi", "goose", "opencode"]
  runner:
    # Kubernetes cannot read the image label: say what the sandbox image contains.
    image_harnesses: ["claude-code", "codex", "pi", "goose", "opencode"]
```

An unknown id, an empty list, a `default_harness` that is not enabled, or `fake` without
`hosted.allow_fake_harness` stops startup naming the key.

## The sandbox image

`images/sandbox/Containerfile` installs all five runtimes by default and writes the label
`org.aurora.harnesses` (for example `claude-code,codex,pi,goose,opencode,fake`):

```sh
podman build -f images/sandbox/Containerfile --target sandbox -t aurora-sandbox:dev .
# only Claude Code (a slimmer image, 2.16 GB):
podman build -f images/sandbox/Containerfile --target sandbox \
  --build-arg SANDBOX_HARNESSES=claude-code -t aurora-sandbox:slim .
```

Every runtime is pinned and checked at build time: Codex, goose and OpenCode are release archives
downloaded with a SHA-256 per architecture (a mismatch fails the build); Claude Code and Pi are npm
trees installed from a lockfile (`images/sandbox/harness/<id>/package-lock.json`, `npm ci
--ignore-scripts`; Pi needs Node.js 22.19 or newer, which the image's `nodejs` is). A `--version`
check equal to the adapter's `PINNED_VERSION` ends each install. To move to another version change
the version, the checksums (or the lockfile) and `PINNED_VERSION` together, and re-run that
harness's conformance suite. An image built before phase 4 has no label and is treated as
`claude-code,fake`, so old images keep working and simply do not offer the other harnesses.

## Choosing a harness for an agent

In the dashboard, **Instructions → Harness** lists the enabled harnesses by their display names
when more than one is enabled. One that no runner offers is disabled ("no runner offers this").
Model roles that a harness does not map say "Not used by <harness>." The same is
`PATCH /v1/agents/{agent}` with `{ "harness": "opencode" }` and `GET /v1/harnesses`.

Changing an agent's harness (between any two of the five):

- is refused while a turn is running (`409 turn_active`);
- starts a fresh harness session in every conversation of the agent; the transcript stays, but
  the new harness does not see the old one's private context;
- deletes the old harness's session files in the agent's computer, using the old harness's own
  code, and restarts the computer so it boots with the new one;
- drops a model override whose endpoint the new harness cannot use (with a notice). With no
  compatible granted key the next turn fails `no_model_key`.

## In the web terminal

`claude`, `codex`, `pi`, `goose` and `opencode` are commands in an agent's terminal. Each uses the
agent's own model through the same machine proxy and gateway as a turn, keeps its state in
`/workspace/.aurora/terminal/<harness>` (never the agent harness's), and prints "not enabled on
this instance" (exit 127) when its harness is not enabled or not in the image. When the agent has
no usable model for it, it prints why and exits 1.

## Failures you may see

| Where | Code | Meaning and fix |
|-------|------|-----------------|
| A turn | `harness_unavailable` | At least one runner is online and none lists the agent's harness. Enable an image that contains it (`runner.image`, `runner.image_harnesses`) or choose another harness. |
| A turn / the computer | `harness_unsupported` | The supervisor refused to boot: the harness id is unknown to the image, or its runtime is missing. There is no fallback to another harness. Use an image built with that runtime. |
| A turn | `no_model_key` | No granted key's endpoint serves the harness's dialect. |
| A turn | `provider_rate_limited` after about 70 s | OpenCode retries a rate limit or server error about six times before it reports it (it has no setting for that); the message says so. |
| API | `422 harness_not_enabled` | The harness is not in `hosted.enabled_harnesses`. |

## What every added harness can and cannot do here

- Approvals and the harness's own sandbox are **off**, because the Aurora sandbox and gateway are
  the isolation and policy boundaries. Everything it does still goes through the gateway: policy,
  approvals, credentials.
- Its model traffic goes through the per-session proxy with the turn's credential. The files and
  environment it sees hold only the placeholder (`aurora-placeholder`) referred to through an
  environment variable; the gateway injects the real key. Credential stores (`auth.json`,
  `secrets.yaml`, a keyring) are removed before every start, and provider sign-in is not supported.
- **No vendor network**: telemetry, update checks, model catalogues (pi.dev, models.dev,
  models.opencode.ai), share links, session naming and OpenCode's background package install are
  switched off, and a turn calls only the endpoint host on its inference path.
- Platform tools are exposed directly (never behind the harness's own code mode), and only a
  descendant of the harness process Aurora started can call them.
- Pi can take a follow-up while a turn runs; Claude Code, Codex, goose and OpenCode turn
  follow-ups into separate turns. None offers "move to background" for a running tool.
- Skills are read from `.agents/skills` (Pi, goose, OpenCode) or the harness's own directory
  (Claude Code, Codex); instructions are in `AGENTS.md`.
- Deleting a conversation or switching harness removes that harness's session files (Codex also
  keeps a small index row in its local SQLite state until it rebuilds the index, D-153).
