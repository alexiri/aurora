# LLM endpoints and governance

This page covers `[[llm.endpoints]]`, the instance-level governance ceiling, and key
origin rules. The design is in [docs/08 §6 and §11](../08-secrets-credentials.md#6-llm-providers),
and the example in [docs/15 §Governance example](../15-deployment-operations.md#governance-example-corporate-llm-accounts-only-strict-egress).

The API and the gateway both read this configuration. Each endpoint is an instance
ceiling: organizations and workspaces can narrow it but cannot widen it.

## Administration › AI models › Endpoints (dashboard first)

(Phase 5, SET-3, D-236, D-237.) An instance admin sets up providers and endpoints on **Administration › AI
models › Endpoints**; there is no Helm change and no restart. The page has three parts:

- **Enabled providers** and **Preference order** — which providers this instance offers and the order a
  hosted agent picks a key's provider in. The preference order may only name enabled providers (a list
  the page writes is refused otherwise).
- **Endpoints** — one row each. Add or edit an endpoint in the dialog: pick the provider (only providers
  this release can run are offered), fill the provider's fields, the allowed models, a key-origin rule and
  extra allowed paths. **Test connection** sends one request to the endpoint's models path from the API,
  with **no credential** at all, through the same destination guard as every other server-side request:
  a private address needs `gateway.allowed_private`. Any answer below 500 (a 401 included) means the
  endpoint is there; a 5xx, a refused destination, DNS, TLS and timeout failures are named.
- Saving shows what the change does first: how many LLM keys in which organizations would stop being
  usable (see below), and a warning for a LiteLLM endpoint without a default model.

The API and every gateway apply a save at once: key creation, the model pickers, model readiness and the
setup wizard in the API; the endpoint table, the reserved LLM hosts and the known LLM hosts list in the
gateway (a host that becomes an endpoint's host leaves the known list, so a sanctioned-only organization
uses the endpoint, not a generic secret on that host). Entries are checked by the same rules as startup:
a key that looks like a credential (`api_key`, `token`, …) is refused by name, a malformed base URL or
path list is refused with the field named, and an unknown `key_origin.method` gets
the nearest valid suggestion. With `governance.llm.require_key_origin` on, every endpoint needs a
`key_origin`.

**Helm still works, and locks what it sets.** An endpoint defined in `config.llm.endpoints` appears on the
page as **Managed by Helm** and can be read but not edited; endpoints you add on the page sit beside it
under other ids (an id a Helm endpoint already uses is refused: `entry_id_in_helm`). `enabled_providers`
and `provider_preference` lock **as whole values** when Helm sets either. To hand a Helm endpoint to the
dashboard, remove it from the values file: it stays with its last definition and becomes editable at the
next start.

**Removing or narrowing an endpoint.** Keys that organizations stored on it are kept but stop being
usable: they show as *not permitted* (the endpoint is gone) or *mismatch* (its verified origin no longer
complies), the gateway stops injecting them, and agent model overrides the new allow-list no longer
permits are cleared. Nothing is deleted; adding the endpoint back with the same id makes the keys usable
again, so re-use an id only for the same service.

Everything below documents the same settings as Helm values, for an install that keeps them in the values
file.

## Providers

Phase 0 has two providers (docs/08 §6):

| Provider | Endpoint | Notes |
|----------|----------|-------|
| `litellm` | Declared by you, `provider = "litellm"` and a `base_url` (for example a central LiteLLM proxy). | Auth is a LiteLLM virtual key. Default dialects: `openai_chat`, `openai_responses`, `anthropic_messages`. |
| `anthropic` | Fixed `https://api.anthropic.com`. | Enabled by default. An implicit endpoint `id = "anthropic"` exists when no `anthropic` endpoint is declared. Declare one only to attach a key-origin rule or an allow-list. |

(Phase 5, P4-LLM-1, D-260) Two more key-only providers are available. Add the id to `llm.enabled_providers`
to use them; the default stays as below.

| Provider | Endpoint | Notes |
|----------|----------|-------|
| `openai` | Fixed `https://api.openai.com`. | Auth is an OpenAI API key (`Authorization: Bearer`). Optional `organization` and `project` entry keys are set as `OpenAI-Organization` / `OpenAI-Project` and removed from the agent's request. Key origin `provider_account` reads the `openai-organization` / `openai-project` response headers. Dialects `openai_chat`, `openai_responses`. |
| `openai_compatible` | Declared by you: `provider = "openai_compatible"`, `base_url` (https, or plain http for a Helm entry; an optional path prefix is kept). | Any OpenAI-style server. Optional `auth_header` (default `Authorization` with `Bearer`), `models_path` (default `/v1/models`) and `dialects` (default `["openai_chat"]`; `anthropic_messages` makes the endpoint usable by Claude Code). Key origin: `endpoint` only. |

Both providers (and `anthropic`, since phase 5) forward **inference paths only**; see "Inference paths only" below.
An entry key that looks like a credential (`api_key`, `token`, `client_secret`, ...) stops startup. A malformed
`organization`, `project`, `auth_header` or `models_path` stops startup with `value_invalid`.

`llm.enabled_providers` defaults to `["litellm", "anthropic"]`. An endpoint whose
`provider` is not enabled stops startup with
`endpoint `<id>`: provider `<name>` is unknown or not enabled` (`crates/aurora-llm/src/registry.rs`).
An unknown name in `enabled_providers` also stops startup.

`llm.provider_preference` (default `["litellm", "anthropic"]`) sets the auto-selection order
for hosted agents (`services/aurora-api/src/domains/hosted/model.rs`).

## Cloud providers and how a key signs in (phase 5, P4-LLM-2..6)

Three cloud providers sit beside the key-only ones. Enable the provider (`llm.enabled_providers`, or the
checkbox on the page), add an endpoint, then organizations add their keys on **Credentials › LLM keys**.
A provider's **auth modes** decide what a key is; the key form offers the modes the endpoint admits, the
recommended one first, and shows the verified account after saving (never a secret again).

| Provider | Endpoint fields | Auth modes (recommended first) |
|----------|-----------------|--------------------------------|
| `openai` | optional `organization`, `project` | API key |
| `azure_openai` | `base_url` (`https://<resource>.openai.azure.com`, host only), `models` and `default_model` (deployment names) | Microsoft Entra application (tenant id, client id, client secret); resource key |
| `bedrock` | `region` (for example `eu-central-1`), optional `base_url` (a VPC endpoint, https, host only); no `allowed_paths` | Assume a role (role ARN, source access key, optional session length); access keys; Bedrock API key |
| `vertex` | `project_id`, `location` (`global` or a region), `models` and `default_model`; no `base_url`, no `allowed_paths` | Service account key file |
| `openai_compatible` | `base_url`, optional `auth_header` and `models_path` | API key |

**Assume a role (Bedrock).** Aurora never stores long-lived access to the role: it assumes it with short-lived
credentials. Create an IAM role in the account whose Bedrock models you want, give it a trust policy that allows
the source IAM user and **requires the organization's external id** (the form shows the id and a policy you can
copy; Aurora always sends that id, so a role that does not require it works for anyone who knows the ARN), and
attach `bedrock:InvokeModel*`. A refused assume shows "AWS refused to let this key assume the role"; check the
trust policy first. Access keys should be limited to `bedrock:InvokeModel*` as well.

**Harnesses.** An endpoint's API format decides which agent harnesses can use it: Claude Code speaks the Anthropic,
Bedrock and Vertex formats, Codex the OpenAI Responses format. The endpoint list shows the enabled harnesses an
endpoint serves; one that serves none says so. To use a model across formats (a Claude agent on an OpenAI-only
endpoint, say), put a **LiteLLM** endpoint in front: it translates, and the agent talks to LiteLLM.

## Where an endpoint may point (D-262)

An LLM endpoint is a destination the gateway dials on every agent request, so who defines it matters:

- **An endpoint defined in Helm** (`config.llm.endpoints`) is the operator's own choice. Its host passes the
  gateway's destination guard even when it resolves to a private address (an in-cluster LiteLLM, say).
  Plain `http` is allowed for `litellm` and `openai_compatible` entries, as before.
- **An endpoint added in the dashboard has no such pass.** To reach a private, loopback or otherwise special
  address its host (or address) must be covered by `gateway.allowed_private` (Administration › Platform › Limits),
  the audited setting that governs every private destination. A dashboard endpoint on a private name is
  marked **Private address not allowed** on the list, and the save confirmation warns
  (`endpoint_resolves_private`); the gateway refuses it at dial time until an operator opens the address. Plain
  `http` is accepted only for a loopback host or one that `gateway.allowed_private` covers
  (`http_needs_private_host`).
- **Cloud metadata and link-local addresses** (`169.254.0.0/16`, `fe80::/10`, `fd00:ec2::254`) are never opened
  implicitly for any source, whatever the name resolves to. Only an exact-IP `gateway.allowed_private` entry
  opens a metadata address.
- **Host ownership.** A dashboard endpoint whose host an active app claims, or that an app-instance registration
  holds, is refused (`endpoint_host_claimed`, naming the app). Remove the claim or the registration first. The
  reverse, a package claiming an endpoint's host, is already refused.
- **Handing a Helm endpoint to the dashboard.** Remove the entry from the values file (or keep a copy on the
  page under another id): the dashboard copy loses the implicit pass at that start. If it points at a private
  address, add the address to `gateway.allowed_private` in the same change, or the endpoint stops working
  (the page marks it; the save warns).
- The gateway rebuilds the endpoint table and this allowance together on every `llm` or `limits` change, with
  no restart; removing an endpoint revokes its pass at the next request.

**Test connection** probes each provider's own path with no credential, through the same guard: OpenAI
`/v1/models`, Azure `<resource>/openai/v1/models`, Bedrock the regional runtime host (or the VPC endpoint) root,
Vertex `/v1/projects/<project>/locations/<location>`, compatible servers `<base>` plus `models_path`
(default `/v1/models`). Any answer below 500, a 401 or 403 included, means the endpoint is there.

## Endpoint keys

Each entry of `[[llm.endpoints]]` accepts:

| Key | Meaning |
|-----|---------|
| `id` | Unique endpoint id. Duplicate ids stop startup. |
| `provider` | `litellm`, `anthropic`, `openai` or `openai_compatible` (enabled). |
| `display_name` | Shown in the dashboard. |
| `base_url` | Required for `litellm` and `openai_compatible`. Validated as an origin (`openai_compatible` may keep a path prefix). |
| `organization`, `project` | `openai` only: set as `OpenAI-Organization` / `OpenAI-Project` on every request. |
| `auth_header`, `models_path` | `openai_compatible` only: the key's header (default `Authorization` with `Bearer`) and the model listing path (default `/v1/models`). |
| `dialects` | Empty means the provider default. Known values: `anthropic_messages`, `openai_chat`, `openai_responses`. |
| `default_model` | The model a hosted agent uses when it has none of its own. **Recommended for every `litellm` endpoint** (a model id the endpoint serves, for example a Claude model for the `claude-code` harness); see [No default model](#no-default-model). |
| `models` | Optional static fallback list when the model list endpoint is not available. |
| `allowed_models` | Instance-wide model allow-list. Empty means any model. |
| `key_origin` | Key-origin rule (below). |
| `model_roles` | Optional, every provider (phase 5, C-875): `{ large, medium, small }`, each a model id the endpoint serves. See [llm-model-roles.md](llm-model-roles.md). |
| `model_info` | Optional, every provider (phase 5, C-875): `{ "<model id>" = { context_window, capabilities } }` for models the harness does not recognise. See [llm-model-roles.md](llm-model-roles.md). |
| `allowed_paths` | `litellm`, `anthropic`, `openai` and `openai_compatible` endpoints: extra paths the key may be used on, see [Inference paths only](#inference-paths-only). Up to 32 absolute paths; a trailing `/*` admits a subtree. |

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
