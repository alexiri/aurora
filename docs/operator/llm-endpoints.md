# LLM endpoints and governance

This page covers `[[llm.endpoints]]`, the instance-level governance ceiling, and key
origin rules. The design is in [docs/08 §6 and §11](../08-secrets-credentials.md#6-llm-providers),
and the example in [docs/15 §Governance example](../15-deployment-operations.md#governance-example-corporate-llm-accounts-only-strict-egress).

The API and the gateway both read this configuration. Each endpoint is an instance
ceiling: organizations and workspaces can narrow it but cannot widen it.

## Providers

Phase 0 has two providers (docs/08 §6):

| Provider | Endpoint | Notes |
|----------|----------|-------|
| `litellm` | Declared by you, `provider = "litellm"` and a `base_url` (for example a central LiteLLM proxy). | Auth is a LiteLLM virtual key. Default dialects: `openai_chat`, `openai_responses`, `anthropic_messages`. |
| `anthropic` | Fixed `https://api.anthropic.com`. | Enabled by default. An implicit endpoint `id = "anthropic"` exists when no `anthropic` endpoint is declared. Declare one only to attach a key-origin rule or an allow-list. |

`llm.enabled_providers` defaults to `["litellm", "anthropic"]`. An endpoint whose
`provider` is not enabled stops startup with
`endpoint `<id>`: provider `<name>` is unknown or not enabled` (`crates/aurora-llm/src/registry.rs`).
An unknown name in `enabled_providers` also stops startup.

`llm.provider_preference` (default `["litellm", "anthropic"]`) sets the auto-selection order
for hosted agents (`services/aurora-api/src/domains/hosted/model.rs`).

## Endpoint keys

Each entry of `[[llm.endpoints]]` accepts:

| Key | Meaning |
|-----|---------|
| `id` | Unique endpoint id. Duplicate ids stop startup. |
| `provider` | `litellm` or `anthropic` (enabled). |
| `display_name` | Shown in the dashboard. |
| `base_url` | Required for `litellm`. Validated as an origin. |
| `dialects` | Empty means the provider default. Known values: `anthropic_messages`, `openai_chat`, `openai_responses`. |
| `default_model` | The model a hosted agent uses when it has none of its own. **Recommended for every `litellm` endpoint** (a model id the endpoint serves, for example a Claude model for the `claude-code` harness); see [No default model](#no-default-model). |
| `models` | Optional static fallback list when the model list endpoint is not available. |
| `allowed_models` | Instance-wide model allow-list. Empty means any model. |
| `key_origin` | Key-origin rule (below). |
| `allowed_paths` | `litellm` endpoints: extra paths the key may be used on, see [Inference paths only](#inference-paths-only). Up to 32 absolute paths; a trailing `/*` admits a subtree. |

Unknown keys are rejected (`deny_unknown_fields`), so a misspelled key stops startup and names the key.

### Inference paths only

Aurora injects a `litellm` endpoint's key only on the **inference routes**: `POST /v1/messages`,
`/v1/messages/count_tokens`, `/v1/chat/completions`, `/v1/completions`, `/v1/responses`,
`/v1/embeddings` (also under a route prefix) and `GET /v1/models[/{id}]`. A request to any other
path of the endpoint, for example LiteLLM's management API (`/key/generate`, `/user/new`,
`/team/*`) or the files, images and fine-tuning routes, is refused with **403
`llm_path_not_permitted`** before policy runs and before the key is opened, so an agent cannot use
the sanctioned key to mint other keys or reach storage. If your clients need another route, list it:

```toml
[[llm.endpoints]]
id = "central"
provider = "litellm"
base_url = "https://litellm.internal"
allowed_paths = ["/health/liveliness", "/model/info"]
```

Even so, **give Aurora a LiteLLM virtual key without management rights** (no `models` wildcard
beyond what agents need, no admin or master key): the path set is a second line of defence, not a
replacement for a least-privilege key.

### No default model

A hosted agent's model comes from, in order: the model chosen on the agent (Instructions), the
endpoint's `default_model`, the first entry of an `allowed_models` list, then the first entry of
the static `models` list. Aurora never picks a model from the endpoint's live model list: a
LiteLLM listing mixes chat, embedding and image models and its order carries no meaning.

An endpoint with only `base_url` therefore has no model. A key on it is **usable**, but a hosted
agent that has no model of its own fails each turn with **`no_default_model`** ("An LLM key is
usable but no model is chosen for this agent and its endpoint has no default model"), before any
sandbox starts. Aurora logs a warning at start for each `litellm` endpoint in this state; it still
starts, so existing configurations keep working.

Fix it by setting `default_model` on the endpoint, or by choosing the model on each agent's
Instructions page. `no_model_key` is a different failure: no granted key is usable at all, and its
message counts the reasons (for example `origin_unverified`), never the key names.
Owners see each key's reason on `GET /v1/agents/{agent}/model-readiness`.

## Key origin

`key_origin` tells Aurora which account a key must belong to before it may carry prompts.
Its keys are `method`, `allowed_organization_ids`, `allowed_workspace_ids` and `allowed_team_ids`.

| `method` | Meaning | Verified by |
|----------|---------|-------------|
| `endpoint` | A key that works on your endpoint is your key. | Validation of the key against the endpoint. |
| `litellm_team` | LiteLLM's key-info endpoint returns the team and user ids for the key. | The gateway's key-origin check. |
| `provider_account` | The Anthropic `anthropic-organization-id` and `anthropic-workspace-id` headers name the key's organization and workspace. | The `GET /v1/models?limit=1` call and the response headers. |

Any other `method` value stops startup (`must be endpoint, litellm_team or provider_account`),
and the error suggests the nearest valid value. The spelling uses underscores:
`litellm-team` is refused with "did you mean `litellm_team`?".

With `litellm_team`, Aurora calls `GET <base_url>/key/info` with the key itself as the bearer
token and reads `info.team_id` and `info.user_id`. A key that reports no team verifies with
`team_id = null` when no `allowed_team_ids` applies (neither the instance rule nor the
organization's narrowing); when team ids are required it is refused. A refused or failed
verification shows why on the key (`key_reports_no_team`, `key_info_http_<status>` for the HTTP
status of `/key/info`, `endpoint_unreachable`, `bad_response`).

A key complies when its stored origin is `verified`, its `value_version` is current, and its
organization, workspace and team ids satisfy the current rule. A key on an endpoint with no
`key_origin` rule is treated as compliant. (docs/08 §11.3)

Aurora checks at three points (docs/08 §11.3):

1. **At save.** A non-compliant key is never stored.
2. **Before first use of a value.** The gateway checks a cached result with a TTL of
   `governance.llm.origin_recheck_secs` (default 3600). A non-compliant value returns 403
   `llm_key_wrong_organization`, and an unreachable provider returns 503
   `llm_origin_check_unavailable`, which fails closed.
3. **On every response.** A mismatch in the response headers replaces the response with 403.
   This is a backstop, because the request has already been sent.

## Governance ceiling

These keys live under `[governance.llm]` and `[governance.egress]`:

| Key | Default | Allowed values and meaning |
|-----|---------|----------------------------|
| `governance.llm.allowed_key_scopes` | `["organization", "workspace"]` | Scopes at which users and organizations may hold LLM keys. |
| `governance.llm.minimum_posture` | `open` | `open` or `sanctioned_only`. Floor for every organization. `sanctioned_only` sends prompts only with compliant keys. |
| `governance.llm.require_key_origin` | `false` | When `true`, every endpoint must declare `key_origin` or startup fails. |
| `governance.llm.known_llm_hosts` | built-in list | Hosts that count as model APIs even without an endpoint. Replaces the built-in list. |
| `governance.llm.known_llm_hosts_extra` | `[]` | Appended to the list. Hosts of configured endpoints are removed automatically. |
| `governance.llm.origin_recheck_secs` | `3600` | TTL of the key-origin cache. |
| `governance.egress.minimum_posture` | `open` | `open` or `credentialed_only`. Floor for every organization. |
| `governance.egress.baseline_allowlist` | `[]` | `[{ host, path? }]` merged into every organization's egress allow-list. |

Effective settings are computed, not stored: `instance ∩ organization` for lists, and
`max(instance minimum, organization)` for postures. A posture below the instance minimum is never used.

Organization owners and admins set their own posture and lists with
`GET` and `PUT /v1/orgs/{org}/governance` and read the result with
`GET /v1/orgs/{org}/governance/effective`. Any signed-in user can read
`GET /v1/governance/instance`, which shows the instance ceiling without secrets
(`services/aurora-api/src/domains/governance/routes.rs`).

## Example: corporate accounts only, strict egress

This matches [docs/15](../15-deployment-operations.md#governance-example-corporate-llm-accounts-only-strict-egress).
Replace the ids and hosts with your own.

```toml
[[llm.endpoints]]
id = "central"
provider = "litellm"
base_url = "https://llm.example.org"
key_origin = { method = "endpoint" }

[[llm.endpoints]]
id = "anthropic"
provider = "anthropic"
key_origin = { method = "provider_account", allowed_organization_ids = ["<corporate org uuid>"] }

[governance.llm]
allowed_key_scopes = ["organization", "workspace"]   # users may hold their own keys…
minimum_posture = "sanctioned_only"                  # …but only verified corporate ones carry prompts
require_key_origin = true

[governance.egress]
minimum_posture = "credentialed_only"
baseline_allowlist = [ { host = "pypi.org" }, { host = "files.pythonhosted.org" },
                       { host = "registry.npmjs.org" }, { host = "github.com" },
                       { host = "codeload.github.com" }, { host = "*.githubusercontent.com" } ]
```

In Helm, put the same TOML structure under `config:` in `values.yaml`
(see [install-helm.md](install-helm.md#configuration)), then `helm upgrade` (the pods restart, because the API and
gateway read the configuration at startup).

## Checks before going live

- Start the service. A rejected endpoint, key-origin method or posture stops startup and names the key.
- Confirm `GET /v1/governance/instance` shows your ceiling.
- Try saving a key from an account outside `allowed_organization_ids`. It should be refused at save (docs/08 §11.3).
- `allowed_models` and `known_llm_hosts` are described in docs/08 §11.5 and §11.7. This page does not
  show a test for them; write one against your own agent before you rely on them.

## Running with the mock (dev and e2e only)

The `llm-mock` dev profile runs the real LiteLLM proxy in front of
[VidaiMock](https://github.com/vidaiUK/VidaiMock), a scripted stand-in for the Anthropic API
(decision I-30). **It is not a model.** Every answer is a fixed scenario file under
`deploy/dev/llm-mock/vidaimock/scenarios/`, chosen by the model name (`scn-pong`, `scn-memory-tool`, ...).
Use it to exercise the gateway LLM lane, context credentials and the real `claude-code` CLI without
a real model account, never for anything that needs an actual answer.

```sh
just kind-up --llm-mock          # deploy/dev/kind-dev.sh up --llm-mock; see install-kind-dev.md
```

1. `deploy/kind/values-llm-mock.yaml` declares the endpoint `litellm-mock`
   (`http://litellm.aurora-dev-mock.svc.cluster.local:4000`, dialect `anthropic_messages`, default model `scn-pong`),
   opens exactly the LiteLLM Service's fixed ClusterIP `10.96.100.40/32` in `gateway.allowed_private`, and adds
   one egress rule for the gateway's NetworkPolicy (`networkPolicy.gatewayExtraEgress`). Default governance permits it.
2. Create an LLM secret for it with the dev master key `sk-aurora-mock-dev` (`LITELLM_MASTER_KEY` in
   `deploy/dev/llm-mock/llm-mock.yaml`). The key is a dummy and protects nothing. LiteLLM runs without a database, so
   only the master key is a valid key.
3. Grant the key to a hosted agent with harness `claude-code` and chat with it. The agent answers
   `pong`; a model override of `scn-memory-tool` makes it call `memory_save` and then say `done`. This needs a
   sandbox image that contains the Claude Code CLI (`scripts/e2e/llm.Containerfile`); the default dev sandbox image
   uses the fake harness.

Automated runs: `bash scripts/e2e/llm.sh` runs the scenario smoke and the supervisor-level live
conformance suite (`AURORA_LIVE_CLAUDE=1`) against LiteLLM -> VidaiMock; with
`AURORA_TEST_PODMAN=1 AURORA_TEST_LLM=1` it also runs the hosted `claude-code` e2e
(`tests/e2e-hosted/tests/llm.rs`) through the gateway. The supervisor suite has no gateway, so it talks
to LiteLLM directly. A run against a real model remains an optional manual release check
(`AURORA_LIVE_CLAUDE=1 ANTHROPIC_BASE_URL=... bash -c 'cargo test -p aurora-supervisor --test conformance'`).
Scenario selection, pinned digests, the license and LiteLLM's observed error mapping are in
`deploy/dev/llm-mock/vidaimock/README.md`.
