# Model roles, model details and LiteLLM fallback

(Phase 5, P5-MODELS-1..3, D-302..D-304; contracts C-875, C-876, C-878..C-880.) This page covers how to
serve more than one model on one LiteLLM endpoint, name what each role means there, let an agent use a
cheaper model for subagents, put a LiteLLM model group with fallbacks behind Aurora, and keep cost in
check with allow-lists. The design is in [docs/08 §Model roles](../08-secrets-credentials.md#model-roles-phase-5-p5-models-1-d-302-d-303).
The endpoint keys themselves are listed in [llm-endpoints.md](llm-endpoints.md#endpoint-keys).

## The idea in four lines

- An agent has one **default model**. It is the model the agent's harness starts each session with.
- The harness may also use other models **on the same endpoint**: for subagents and for background
  requests. Aurora names them with three harness-neutral **roles**: `large`, `medium` and `small`.
- `claude-code` maps them to its own aliases: `large` = `opus`, `medium` = `sonnet`, `small` = `haiku`.
  Codex declares no roles, so roles do nothing there (the dashboard says "Not used by Codex").
- Each role resolves in this order: what the **agent** set, then what the **endpoint** sets, then
  nothing. On an `anthropic` endpoint "nothing" is right, because the built-in aliases are real Claude
  models. On any other provider it is a **warning** (`role_unset`): the harness will ask for Claude model
  names the endpoint may not serve.

So on a LiteLLM endpoint that serves Claude **and** other models, set the roles. Otherwise a subagent that
asks for "haiku" sends the id `claude-haiku-…` to a server that may not have it.

## 1. A LiteLLM config that serves Claude and GLM

LiteLLM's `model_name` is the id Aurora and the harness send. Pick names you will also use in Aurora's
allow-list and roles. This is the part of LiteLLM's own `config.yaml` that matters here; the model ids and
the GLM `api_base` are examples, so use the ones your providers document:

```yaml
model_list:
  - model_name: claude-opus
    litellm_params:
      model: anthropic/<your Claude large model id>
      api_key: os.environ/ANTHROPIC_API_KEY
  - model_name: claude-sonnet
    litellm_params:
      model: anthropic/<your Claude medium model id>
      api_key: os.environ/ANTHROPIC_API_KEY
  - model_name: glm-4.6
    litellm_params:
      model: openai/glm-4.6
      api_base: https://api.z.ai/api/paas/v4
      api_key: os.environ/ZAI_API_KEY
  - model_name: glm-4.5-air
    litellm_params:
      model: openai/glm-4.5-air
      api_base: https://api.z.ai/api/paas/v4
      api_key: os.environ/ZAI_API_KEY
```

Give Aurora a LiteLLM virtual key for these four models and nothing else (see
[Inference paths only](llm-endpoints.md#inference-paths-only)). Keep `claude-code`'s Anthropic-format
requests working: LiteLLM must serve `POST /v1/messages` for every model, GLM included. Check that with
**Test connection** on the endpoint and with one turn.

## 2. The Aurora endpoint: roles and model details

An endpoint that serves GLM models the harness does not know needs two more keys beside `default_model`
and `allowed_models`:

- `model_roles`: which of the endpoint's models each role stands for.
- `model_info`: what the harness cannot learn from the id: the context window and the capabilities
  (`effort`, `thinking`, `adaptive_thinking`, `interleaved_thinking`).

```toml
[[llm.endpoints]]
id = "central"
provider = "litellm"
base_url = "https://llm.example.org"
default_model = "claude-sonnet"
models = ["claude-opus", "claude-sonnet", "glm-4.6", "glm-4.5-air"]   # only when LiteLLM's list is not available
allowed_models = ["claude-opus", "claude-sonnet", "glm-4.6", "glm-4.5-air"]
key_origin = { method = "endpoint" }
model_roles = { large = "claude-opus", medium = "claude-sonnet", small = "glm-4.5-air" }
model_info = { "glm-4.6" = { context_window = 200000, capabilities = ["thinking"] }, "glm-4.5-air" = { context_window = 128000 } }
```

The same entry in Helm values (put `config.llm.endpoints` as a list; the shape is the TOML's):

```yaml
config:
  llm:
    endpoints:
      - id: central
        provider: litellm
        base_url: https://llm.example.org
        default_model: claude-sonnet
        allowed_models: [claude-opus, claude-sonnet, glm-4.6, glm-4.5-air]
        key_origin: { method: endpoint }
        model_roles: { large: claude-opus, medium: claude-sonnet, small: glm-4.5-air }
        model_info:
          glm-4.6: { context_window: 200000, capabilities: [thinking] }
          glm-4.5-air: { context_window: 128000 }
```

On **Administration › AI models › Endpoints** the same two things are the dialog's **Model roles** (three
fields: pick one of the endpoint's models or type an id) and **Model details** (one row per model id, with
the context window and the capability checkboxes). An endpoint that is not `anthropic` and has no roles
shows "Agents on this endpoint will ask for Claude model names for subagents and background work. Set the
roles."

Rules the server enforces (a violation refuses startup, or the settings write, with `value_invalid` naming
the key):

- Every id matches `[A-Za-z0-9._:/@-]{1,200}`.
- A role must be on `allowed_models` when that is set, and on `models` when the list is static.
- `context_window` is 1 000 to 10 000 000; `capabilities` come from the four names above; at most 64
  `model_info` entries.
- Only the **default** model's `context_window` reaches `claude-code` (the CLI has one context override,
  not one per alias). Give the default a correct one if it is not a Claude model. Capabilities do apply
  per role.

Roles never name another endpoint, because one harness process has one base URL. To use two endpoints,
give them one LiteLLM in front.

## 3. Gather-then-write with `subagent_model = small`

A common cost pattern: the agent's **default model** is the strong one, and the reading, searching and
summarising is done by **subagents** on the cheap one.

1. On the endpoint, `small` is the cheap model (above: `glm-4.5-air`).
2. On the agent's **Instructions** tab, set **Subagents use** to **Small**. (API: `subagent_model =
   "small"` on `PATCH /v1/agents/{agent}`.) Subagents whose definition names no model then run on `small`.
   A subagent definition that names its own `model:` still wins; Aurora never forces one.
3. Tell the agent in its brief how to work: "Send reading, searching and summarising to subagents. Write
   the final answer yourself."

**Access › Model readiness** shows what the next turn will use: the default, each role and where it came
from (the endpoint or this agent), and warnings. A role that is not allowed for the organization counts as
unset for its agents and says so there. Roles an agent set that stopped being usable (a different
endpoint, or a model that is no longer allowed) are dropped, never re-pointed, and the next turn notes it.

## 4. A LiteLLM model group with fallbacks

Aurora has no fallback chain across providers (D-302(c)). When you want "Anthropic first, then another
provider", let LiteLLM do it with a **model group** and router fallbacks, and use the group's name as the
default model or as a role:

```yaml
model_list:
  - model_name: agent-medium          # the group Aurora asks for
    litellm_params:
      model: anthropic/<your Claude medium model id>
      api_key: os.environ/ANTHROPIC_API_KEY
  - model_name: agent-medium-backup   # used only when the first fails
    litellm_params:
      model: bedrock/<your Claude medium model id on Bedrock>
router_settings:
  num_retries: 1
  fallbacks:
    - agent-medium: [agent-medium-backup]
```

```toml
[[llm.endpoints]]
id = "central"
provider = "litellm"
base_url = "https://llm.example.org"
default_model = "agent-medium"
allowed_models = ["agent-medium"]
model_roles = { medium = "agent-medium" }
```

What this does **not** give you, and what to expect:

- **The upstream keys live in LiteLLM.** Aurora holds one LiteLLM key. Provider keys, their budgets and
  rotation are LiteLLM's. Nothing in Aurora sees or injects them.
- **Governance and key-origin checks apply to the LiteLLM key only.** A `key_origin` rule says which
  account the LiteLLM key belongs to. It cannot say which provider account answered a request.
- **Usage shows the group name.** Aurora records the model the request asked for (`agent-medium`), in
  turn usage, in the request log's **Model** column and in usage views. It does not learn which backend
  answered. If you need that, read it in LiteLLM's own logs.
- **Fallback is per request, inside LiteLLM.** An agent's turn can use the backup for one request and the
  primary for the next. Make sure both back ends serve the features the harness uses (tool use, streaming).
- Any model **outside** the allow-list is refused by the gateway even though LiteLLM might serve it.

## 5. Allow-lists are the cost control

The effective allow-list is the only control over which models an agent may use (D-302(a), (e)): agents
have none of their own. It is the instance's `allowed_models` for the endpoint, narrowed (never widened)
by the organization on **AI models › Rules**. The gateway refuses any other model on an LLM request, a
role or the default included.

- Put only the models you are willing to pay for in `allowed_models`. A model the harness picks for a
  subagent or a background request is still checked against it.
- Make the allow-list contain **every role you set** (the server insists), and the default.
- Read the effect on **Activity**: the request log shows the **Model** of each LLM request, so a
  surprising model shows up there. Usage records tokens per model, subagents included. A turn whose
  harness reported a model the turn could not have used is marked "model reported by the harness could
  not be verified", and those tokens are counted under the default model (D-304).
