# 00 — Overview and Glossary

## What Aurora is

Aurora runs AI agents for a team. An operator creates an *agent* per person or
per purpose, grants it *credentials* and *app connections*, and the agent's
traffic leaves only through the *gateway*. The gateway authenticates each
request as that agent, evaluates *policy*, optionally holds the request for a
*human approval*, and only then injects the real credential and forwards the
request. **The agent process never holds a real secret.**

Two kinds of agent exist:

- **BYO ("bring your own")**: the agent runs anywhere (a laptop, CI, another
  framework). It gets a proxy address, the gateway CA certificate, placeholder
  credentials and a gateway token, and sends its traffic through the gateway.
  Aurora does nothing else for it.
- **Hosted**: Aurora runs the agent in a sandbox it manages. The sandbox has a
  durable home volume, a *harness* (the LLM agent runtime), conversations,
  memory, skills, schedules, webhooks and an optional presence on chat
  platforms through *connectors*. People never see a hosted agent's gateway
  credentials.

Both kinds share the gateway, the secret stores and the policy engine.

## The mental model in seven sentences

1. **Tenancy**: an *organization* contains *workspaces*. Agents, secrets,
   connections and policy live in a workspace; some can live at organization
   level and be shared down.
2. **Identity at the wire**: every request reaching the gateway presents a
   *gateway credential* that maps to agent → workspace → organization, and for
   hosted agents also to the conversation and turn.
3. **Access is granted, never ambient**: an agent can use only credentials that
   are explicitly attached to it, further narrowed by rules.
4. **Policy runs before injection**: a blocked or unapproved request never
   causes a credential to be decrypted.
5. **The control plane owns state**: the API server owns the database, the work
   queue and every decision about what happens next. Runners, connectors and
   the SSH terminator *dial in* and hold no inbound ports towards the core.
6. **Apps and chat platforms are plugins**: an app is a signed declarative
   package (with optional sandboxed WebAssembly hooks); a chat platform is a
   connector service. The core has **no** app-specific and **no**
   platform-specific code.
7. **Everything is Rust except the dashboard**: one language for every service
   lets crates (policy engine, wire types, crypto) be shared instead of
   re-implemented, so there is exactly one matcher and one evaluator.

## Glossary

| Term | Meaning |
|------|---------|
| **Instance** | One Aurora deployment. Instance-level settings and approvals (plugins, connectors, trusted keys, host-claim decisions) are made by **instance admins**. |
| **Instance admin** | A user flag (not an org role) granting deployment-level operator powers. Bootstrapped from configuration. |
| **Organization (org)** | Top tenant. Owns members, invitations, org-level policy, org-level secrets/connections/OAuth clients, channel integrations, org skills, app availability. |
| **Workspace** | Isolation boundary inside an org. Holds agents, secrets, connections, workspace policy, API keys, access bindings. Has a **timezone** and a **memory language**. |
| **Member / role** | A user's membership in an org: `owner`, `admin` or `member`. Always enforced (no edition gating). |
| **Workspace access** | Binding of a user to a workspace with role `owner` or `member`. |
| **Access law** | The single authorization predicate used everywhere a person acts on a workspace: workspace exists → user is an *active* org member → user has workspace access (org owners/admins pass implicitly). See `04`. |
| **Agent** | Durable principal: name, handle (unique per workspace), kind (`byo`/`hosted`) and for hosted agents harness, model, effort, brief, avatar. |
| **Daneel** | The default hosted agent created in every new workspace (name configurable). (phase 3, RFE-019) The name is only a default; its personality is not part of the specification — the default brief is neutral and a deployment may configure a persona (`15`). |
| **Gateway credential** | Any token the gateway accepts in `Proxy-Authorization`: a BYO **agent token** (`aug_`), a hosted **boot token** (`aub_`, valid for one sandbox boot) or a **context credential** (`auc_`, valid for one turn or one BYO context). |
| **Context credential** | Short-lived gateway credential that additionally names a conversation and turn, so approvals and request logs are attributed exactly. Attribution only — the security boundary stays the agent. |
| **API key** | Bearer credential for the management API: workspace key or org key; kind `user` or `service`. |
| **Secret** | A stored credential not tied to an app: an **LLM key** or a **generic secret** bound to a host pattern (and optional path pattern) with an **injection recipe**. Its value is held by a **secret store**. |
| **Secret store** | Pluggable backend that holds secret material. Phase 0: `inline` (AES-256-GCM ciphertext in Postgres). Phase 3: HashiCorp Vault. Every sensitive column holds a **SecretRef**, never raw ciphertext semantics. |
| **SecretRef** | Opaque string naming where and how a secret value is stored (`inline:…` or `ext:…`). See `08`. |
| **LLM provider** | Core registry entry describing a model API (phase 0: `litellm`, then `anthropic`; phase 4: `openai`, `azure_openai`, `bedrock`, `vertex`, `openai_compatible`): endpoints, auth recipe, model catalogue source, API dialects served. |
| **Governance** | Layered controls over which model accounts and destinations agents may use: instance ceiling → org governance → workspace policy → grants; each level can only tighten (`08 §11`). |
| **Key origin** | The provider account a model key belongs to (e.g. Anthropic organization/workspace, LiteLLM team), verified at save, before first use and on responses. |
| **LLM posture** | `open` or `sanctioned_only` (every request to a governed LLM host must carry a compliant key injected by Aurora). |
| **Egress posture** | `open` or `credentialed_only` (requests on which Aurora injects nothing are blocked unless allow-listed). |
| **Plugin package** | Signed archive holding one or more **apps**: manifest, assets, docs, optional `hooks.wasm`, fixtures. |
| **App** | A third-party service declared by a plugin package: hosts, injection, token lifecycle, connect methods, permission tools, approval summaries, agent instructions. |
| **Mechanism** | A named, core-implemented behaviour that a manifest selects and parameterizes (`bearer`, `basic`, `header`, `oauth2_refresh`, `aws_sigv4`, …). Plugins cannot add mechanisms. |
| **Hook** | A function exported by a plugin's WebAssembly component (`connect.authorize`, `connect.exchange`, `token.mint`, `approval.summarize`). Runs only in the **hook host**, only on slow paths. |
| **Hook host** | Stateless internal service embedding wasmtime; holds no keys and no database access. |
| **Registry snapshot** | Deterministic compilation of all active plugin packages into the structures the gateway and API consume (host index, injection recipes, token configs, tool catalogue, summary templates). |
| **Host claim** | A (host, path prefix) on which an app injects credentials. Overlapping claims across packages need an instance-admin-chosen winner. |
| **App connection** | A connected account of an app (OAuth grant or key set) at workspace or org scope; credentials held by a secret store. Multiple per app are normal. |
| **OAuth client (BYOC)** | Customer-supplied OAuth client credentials for a plugin package's `oauth_clients` entry. |
| **Grant** | Attachment of a connection or a secret to an agent (connections: tool mode `full` or `custom`). Compiled into **derived rules**. |
| **Policy rule** | Priority-ordered, first-match rule: identities × targets → allow/block, with modifiers (require approval, rate limit) and conditions. Scopes: organization and workspace. Hand-authored rules are drafted and **published** as numbered generations. |
| **Tool (permission tool)** | A named endpoint pattern in an app's catalogue (host + path + methods + optional GraphQL kind); the unit people allow, ask or block. |
| **Approval (gateway hold)** | A request paused in the gateway pending a human decision. Stored in Postgres. |
| **Action approval** | A platform-level one-shot approval for a privileged agent *action* that is not an HTTP request (first: `send_message`). |
| **Sandbox** | The isolated environment of a hosted agent: a container (podman) or pod (Kubernetes) plus a durable **home** volume. One per agent. |
| **Boot** | One container run of a sandbox, identified by `boot_id`. Boot tokens, runtime intervals and process records are per boot. |
| **Home** | The sandbox's durable filesystem at `/workspace`; outlives containers. |
| **Runner** | Daemon that starts, parks and reaps sandboxes through a **sandbox backend** (`podman`, `kubernetes`, `fake`). Outbound-only towards the API. |
| **Supervisor** | Aurora's process inside each sandbox: drives harness sessions, projects control-plane truth into the home, bridges platform tools, manages processes, runs the loopback proxies. |
| **Harness** | The LLM agent runtime behind a vendor-neutral adapter interface (phase 0: `claude-code`, `fake`). |
| **Platform tools** | MCP tools exposed to the harness that call back into the control plane (memory, skills, schedules, messaging, apps…), served from a **tool registry**. |
| **Conversation / turn** | A conversation is a durable thread with its own harness session; a turn is one message in → the agent's whole response out, the unit of work, abort and usage. |
| **Transcript event** | A durable, vendor-neutral event of a turn. Streaming deltas are ephemeral. |
| **Memory** | Agent-level markdown notes with revision history, projected into the sandbox and harvested back; written in the workspace's **memory language**. |
| **Skill** | Agent- or org-scoped instruction bundle (`SKILL.md` + files) projected read-only into the sandbox. |
| **Schedule** | Platform-owned cron that fires a turn in its own conversation, with an optional pre-run **gate** command. |
| **Webhook** | Inbound capability URL that turns an external event into a turn. |
| **Watch** | One-shot trigger on a background process (exit, output pattern, silence) that fires a turn. |
| **Connector** | A separate service implementing one chat platform behind the **connector contract**. Phase 1: Mattermost. |
| **Integration** | An org's link to one platform tenant (a Mattermost team on a server), holding the org-level platform credential. |
| **Presence** | How an agent appears on a platform: `dedicated` (its own bot identity) or `shared` (served by the integration's one shared bot, by default named `Daneel`). |
| **Space binding** | For a shared bot: which agent answers in a space (channel), listen mode and follow-up rule. Only workspace owners may set it. |
| **DM default** | For a shared bot: which agent a person's top-level DMs go to. |
| **Thread link** | Platform thread address ↔ conversation. |
| **User link** | Platform user ↔ Aurora user; minted by a core-issued sign-in link (the signed-in person confirms), or — only when an org admin has enabled `verified_email_autolink` for the integration (phase 1, RFE-001) — by a connector-asserted verified email. |
| **Reach grant** | Standing decision widening who (beyond workspace members) may talk to an agent in a space or as a person. |
| **Action token** | Core-signed, single-use, short-lived token embedded in chat buttons and menus; connectors return it verbatim with the clicker's platform id. |
| **Agent link / peer task** | Relationship between two agents and a bounded work exchange between them. |

## Token prefixes and identifiers

All capability tokens are a prefix plus 43 characters of base64url (32 random
bytes). They are **stored only as SHA-256 hashes**, looked up by hash and never
logged. The first 12 characters (prefix + 8) are stored separately for display.

| Prefix | Credential | Shown to people? |
|--------|-----------|------------------|
| `aug_` | BYO agent gateway token (long-lived) | once, at creation/regeneration |
| `aub_` | Hosted sandbox boot token (one boot) | never |
| `auc_` | Context credential (one turn, or one BYO context) | BYO: returned to the API caller; hosted: never |
| `auk_` | Workspace API key | once |
| `auo_` | Organization API key | once |
| `aur_` | Runner anchor token | once (operator) |
| `aun_` | Connector anchor token | once (instance admin) |
| `aut_` | SSH terminator anchor token (phase 4) | once (operator) |
| `auh_` | Connector host anchor token (phase 4, P4-MCON) | never (generated into a Secret by the chart; `AURORA_BOOTSTRAP_CONNECTOR_HOST_TOKEN`) |
| `aud_` | SCIM directory token, per organization (phase 4, P4-GRP-2; accepted only on `/scim/v2`) | once (org owner) |
| `auw_` | Webhook capability token (in URL path) | once |
| `aus_` | Session cookie value | never (cookie) |
| `aui_` | Invitation / email-link / account-link token | in the link only |
| `aup_` | Supervisor channel bootstrap token (one spawn) | never |
| `aul_` | Preview relay anchor token (phase 4, P4-PRV-2; generated into a Secret by the chart, `AURORA_BOOTSTRAP_RELAY_TOKEN`; accepted only on `/v1/relay`) | never |
| `auv_` | Preview session cookie value (phase 4, P4-PRV-2; one share, one browser; cookie `__Host-aurora_preview`, accepted only by the relay) | never (cookie) |
| `aux_` | Preview stream grant (phase 4, P4-PRV-2; one stream open, ≤ 30 s, redeemable only by a runner) | never (relay → runner header) |

Row identifiers are UUIDv7 generated by the application. Organizations and
workspaces additionally have URL slugs; agents have a handle.

## What changed relative to OneCLI (summary)

The full rationale is in [90-decisions.md](90-decisions.md).

| Area | OneCLI | Aurora |
|------|--------|--------|
| Languages | TS control plane, runner, supervisor, adapter; Rust gateway | Rust everywhere except the Next.js dashboard |
| Policy matcher | Two implementations (TS + Rust) pinned by golden corpora | One crate (`aurora-policy`) linked by API and gateway |
| Apps | ~75 apps compiled into core in five places | Signed plugin packages, declarative manifest + core mechanisms + sandboxed hooks |
| Channels | Slack provider modules inside API and adapter | Connector services behind a neutral contract; Mattermost first; no platform names in core |
| Approval attribution | Gateway sees only the agent | Context credentials name conversation and turn |
| Secret storage | Inline AES-GCM or KMS envelope; vault fallbacks (Bitwarden/1Password) | Pluggable secret stores with `SecretRef` columns; inline first, HashiCorp Vault next |
| LLM providers | Anthropic, OpenAI hard-wired into secret types | Provider registry; LiteLLM first, Anthropic second |
| Event bus | In-process (API forced to one replica) | Postgres `LISTEN/NOTIFY` (API scales horizontally) |
| Approval store | Memory or Redis | Postgres (gateway scales horizontally without Redis) |
| Gateway control API | Same port as the proxy | Separate internal-only port, unreachable from sandboxes |
| Editions | Cloud / Community / Enterprise | One edition |
| Deployment | Docker Compose + Helm | Helm (production); the same chart on kind for development (phase 1, OPS-017; phase 0 used podman compose) |
| Dashboard | Next.js with server actions calling services directly | Next.js as a pure client of the documented API |
| Default agent | none | `Daneel` in every new workspace |
