# 09 — The App Plugin System

Phase: **0** (system, SDK, CLI, fixture packages) · **2** (built-in app
catalogue). Crates: `aurora-registry`, `aurora-summary`, `aurora-hook-wit`,
`aurora-plugin-sdk`; services: API (ingest/compile), gateway (mechanisms),
`aurora-hookhost`.

Implements Proposal 01. **Apps are data; the core provides mechanisms.**

## 1. Principles (normative)

1. **Configure, don't extend.** A manifest selects and parameterizes core
   mechanisms. A new mechanism is a core change (bar: needed by ≥2 apps or must
   run on the request path).
2. **Code only on slow paths, only in the sandbox.** Hooks run in the hook host
   during connect flows, on token expiry (mint) and when a request is held
   (summarize). Never per request, never in the gateway process.
3. **Substitution, not a language.** Templates substitute declared variables,
   escaped for their context. No conditionals or loops.
4. **Structure enforces safety.** A package injects only into hosts it declares;
   tenant-derived and rewritten hosts sit under a declared zone; token endpoints
   are https and fixed or zone-constrained. Checked at load.
5. **Least privilege, declared up front, approved by an instance admin.**
   Hooks, egress per hook, context variables and host claims are listed in the
   manifest, shown at install, enforced by the core; widening needs re-approval.
6. **All plugin output is untrusted data** (labels, metadata, summaries, guides,
   icons, agent snippets): sanitized, clamped, rendered as data.
7. **Fail per plugin, not per platform.** An invalid package is disabled with a
   visible reason; everything else keeps working.
8. **Built-in apps use the same format and the same sandbox.** The core
   contains zero app-specific code.

## 2. Package format

A package is a **gzip-compressed tar** (`<package_id>-<version>.aurora-plugin`,
≤ `PACKAGE_MAX`) of:

```
plugin.yaml            # manifest (required)
assets/icon.png        # required, PNG, ≤256 KiB, square, ≥64 px
assets/icon-dark.png   # optional
docs/setup.md          # required, CommonMark without raw HTML, ≤64 KiB
hooks.wasm             # optional, WebAssembly component (wasm32-wasip2 target, no WASI imports used)
tests/*.yaml           # fixtures (required for built-ins; recommended otherwise)
SIGNATURE              # required
```

No other paths, no symlinks, no absolute or `..` paths, no duplicate entries.

**Canonical digest**: list every file except `SIGNATURE`, sorted by path
(bytewise); for each emit `<path>\0<sha256 hex of content>\n`; the package
digest is `sha256` of that text. **SIGNATURE** contains
`aurora-sig-v1\n<key_id>\n<base64 ed25519 signature over the digest bytes>\n`.
`key_id` is `builtin` or the base64 public key. The signature verifies against
the compiled-in project key (publisher `aurora`) or a non-revoked
`trusted_keys` row whose `publisher` equals the manifest's
`package.publisher`.

Unsigned packages are refused unless `plugins.allow_unsigned = true` (dev only;
the dashboard shows a standing warning banner to instance admins).

## 3. Manifest contract v1 (`plugin.yaml`)

YAML, strict: unknown keys are errors at contract 1. Ids:
`[a-z][a-z0-9_-]{1,47}`. All human strings are sanitized on load.

### 3.1 `package`
```yaml
package:
  id: todoist                   # immutable package id
  version: 1.2.0                # semver
  contract: 1                   # manifest contract version
  min_core: 0.1.0               # minimum Aurora version
  publisher: aurora             # must own the signing key
  name: Todoist
  description: Tasks and projects
  homepage: https://example.org  # optional, https
```

### 3.2 `permissions` (reviewed and approved by an instance admin)
```yaml
permissions:
  hooks: [connect.exchange, token.mint, approval.summarize]   # must equal hooks.wasm capabilities used
  egress:                                                     # per hook; approval.summarize never allowed
    connect.exchange: [github.com, api.github.com]
    token.mint: [api.github.com]
  context: [org_external_id]                                  # core context variables read by templates/hooks
```
Host claims are derived from `apps[].hosts` and shown alongside.

### 3.3 `oauth_clients` (shared by the package's apps)
```yaml
oauth_clients:
  - id: google
    authorize_url: https://accounts.google.com/o/oauth2/v2/auth
    token_url: https://oauth2.googleapis.com/token
    # authorize_url may be templated the same way (S-14)
    # or tenant-derived: token_url: { template: "https://{connection.tenant_host}/oauth/token", zone: "*.zendesk.com" }
    pkce: true                    # S256
    client_auth: body             # body | basic
    body_format: form             # form | json
    fields:                       # what an admin enters when configuring the client (BYOC / instance)
      - { name: client_id, label: Client ID }
      - { name: client_secret, label: Client secret, secret: true }
    setup_note: "Create an OAuth client of type Web application."
```
The redirect URI to register with every provider is always
`<external_url>/v1/oauth/callback`.

### 3.4 `apps[]`
One package may contain several apps (e.g. a vendor suite sharing one client).

#### `about`
```yaml
about:
  id: gmail                     # immutable app id, globally unique across packages
  name: Gmail
  description: Read and send email
  category: communication       # open vocabulary for UI grouping
  api_docs: https://developers.google.com/gmail/api
  api_spec: https://…/openapi.json      # optional
  web_hosts: [mail.google.com]          # hosts allowed in approval links
  connect_note: { text: "An admin must enable API access first.", url: https://… }   # optional
  label_hint: "Account email"
```

#### `credentials` and `metadata`
```yaml
credentials:                    # stored encrypted; the vocabulary templates and hooks may use
  - { name: access_token, secret: true }
  - { name: refresh_token, secret: true, optional: true }
metadata:                       # stored in clear, non-secret
  - { name: email }
  - { name: org }
```
`connection.tenant_host` is a built-in variable when any host is tenant-bound.

#### `connect[]` (one or more methods)

**`oauth2`**
```yaml
- id: oauth
  kind: oauth2
  client: google
  scopes:
    - { scope: "https://www.googleapis.com/auth/gmail.readonly", name: Read mail, access: read }
    - { scope: "https://www.googleapis.com/auth/gmail.send", name: Send mail, access: write }
  default_scopes: [ "https://www.googleapis.com/auth/gmail.readonly", "https://www.googleapis.com/auth/gmail.send" ]
  authorize_params: { access_type: offline, prompt: consent }
  access_levels:                # optional choice shown in the connect window
    - { id: user, label: "My account", authorize_params: {} }
  callback: query               # query | fragment (fragment providers use the bridge page)
  token_response:               # JSON pointers into the token response → credential fields
    access_token: /access_token
    refresh_token: /refresh_token
    expires_in: /expires_in     # special: converted to credentials_expires_at
    scope: /scope               # special: becomes connection.scopes
  fields:                       # optional (S-14); only store: metadata | tenant_host
    - { name: subdomain, label: Host, type: text, store: tenant_host, required: true, placeholder: "acme.zendesk.com" }
  probe: { … }                  # optional, names the account
```
`fields` on `oauth2` exist so a **first** connect with a templated `authorize_url`/`token_url`
can take `connection.tenant_host` from the form. The host is validated (`tenant` zone, DNS
name only: no IP literal, userinfo, port or path) **before** the authorize redirect or any
outbound request and is carried in the encrypted state. `store: credentials` (and `json_file`)
is rejected (`oauth_field_store`): credentials come from the token response only.

**`fields`** (API keys, credential imports)
```yaml
- id: api_key
  kind: fields
  fields:
    - name: api_token
      label: API token
      type: secret              # text | secret | select | file | json_file
      store: credentials        # credentials | metadata | tenant_host
      required: true
      placeholder: "…"
      help_url: https://todoist.com/prefs/integrations
      # select: options: [{ value, label, warning? }]
      # conditional: show_when: { field: kind, equals: service_account }
      # json_file: json_map: { client_email: /client_email, private_key: /private_key }
  probe: { … }
```

**`probe`** (inside `oauth2` or `fields`, executed by the core, not by plugin code)
```yaml
probe:
  method: GET
  url: "https://api.todoist.com/rest/v2/projects"   # host must be one of the app's hosts
  expect_status: [200]
  label: /email                 # JSON pointer → connection label (sanitized)
  metadata: { org: /org/name }  # JSON pointer → declared metadata fields
```
The probe request is authenticated with the app's own host injection for that
host, uses the gateway's destination guard rules, a 10 s timeout and a 1 MiB
response cap. Failure → 422 `connect_probe_failed` with the upstream status.

**`hook`**
```yaml
- id: github_app
  kind: hook
  authorize: true               # call connect.authorize to obtain the redirect URL
  fields: []                    # optional form collected first and passed to the hook
```

Server-owned values are never taken from client input: templates and hooks
read them from **context variables** listed in `permissions.context`:

| Context variable | Value |
|------------------|-------|
| `org_external_id` | `organizations.external_id` — stable per-org opaque id, e.g. an AWS external id |
| `org_id`, `workspace_id` | ids of the connection's scope |
| `instance_url` | external URL |
| `agent_name`, `agent_handle` | only in request-path transforms (e.g. commit trailers) |

#### `hosts[]` (request-path behaviour, executed by gateway mechanisms)
```yaml
hosts:
  - match: { host: api.todoist.com, path_prefix: /rest }   # host pattern (one * max), optional path prefix
    inject:
      - bearer: "{credentials.api_token}"
  - match: { host: "*.zendesk.com" }
    tenant: { field: connection.tenant_host, zone: "*.zendesk.com" }  # request host MUST equal the stored host
    inject:
      - basic: { user: "{metadata.email}/token", password: "{credentials.api_token}" }
  - match: { host: "s3.*.amazonaws.com" }
    inject: []
    finalize: { aws_sigv4: { region: "{metadata.region}", service: s3,
                             access_key: "{credentials.access_key_id}", secret_key: "{credentials.secret_access_key}",
                             session_token: "{credentials.session_token}" } }
```

#### Instance-bound hosts: self-hosted apps (phase 2, D-74)

A `tenant` zone covers SaaS subdomains (`acme.zendesk.com`). A **self-hosted**
deployment (a company's own GitLab, Jira Data Center, Sentry, Mattermost) lives
on an arbitrary domain that no zone can declare. For these, an app declares
`instances`, and its hosts, tools, probe, OAuth URLs and hook egress use the
placeholder host `{instance}`:

```yaml
instances:
  label: GitLab instance        # shown in the registration form and the connect picker
  default_host: gitlab.com      # optional: the SaaS host, a normal static claim
  probe: { method: GET, path: /api/v4/version, expect_status: [200, 401] }   # optional reachability check
hosts:
  - match: { host: "{instance}", path_prefix: /api/v4 }
    inject: [ { bearer: "{credentials.access_token}" } ]
tools:
  - { id: list_projects, host: "{instance}", path: /api/v4/projects, methods: [GET] }
oauth_clients:
  - id: gitlab
    authorize_url: "https://{instance}/oauth/authorize"
    token_url: "https://{instance}/oauth/token"
```

- `{instance}` stands for a whole host only (never part of a host or a path),
  and only in an app that declares `instances`. With `default_host`, every
  `{instance}` entry also applies to that host as an ordinary static claim.
- An **app instance** (`03 app_instances`) is a host registered for one app by
  an admin: instance-wide by an instance admin, or for one organization by its
  owners/admins (`05 §App instances`). Registration is the trust decision
  about where that app's credentials may be sent; members never type a host.
- At registration the host MUST be a DNS name in lower case: no IP literal,
  userinfo, port or path, and not under `aurora.internal`. It MUST NOT equal or
  fall under a static claim of any active package (another app's host), a
  governed LLM host (`08 §6`), or another registered instance of the same scope
  (`instance_host_conflict`). The optional `probe` runs through the gateway's
  destination guard. A private address is reachable only where the operator
  allows it (`gateway.allowed_private`, `06`).
- A connection of an instance-bound app records its `instance_id` (or none for
  `default_host`). The gateway injects it **only** on that instance's host,
  exactly as a tenant-bound connection only on its `tenant_host` (SEC-04, SEC-59).
- OAuth: each self-hosted instance has its own OAuth application, so the client
  is a BYOC client bound to the instance (`app_oauth_clients.instance_id`); the
  templated URLs resolve to that instance's host and MUST be https.
- Registered instance hosts are claims for `§7`: a package activated later
  whose static claim overlaps a registered instance host needs an instance-admin
  decision like any other conflict.
- Deleting an instance disables its connections (`status_reason:
  instance_deleted`) and its OAuth clients; nothing is injected on its host again.

#### `token`
```yaml
token:
  kind: oauth2_refresh          # static | oauth2_refresh | hook
  client: google                # oauth2_refresh: which client
  refresh_field: refresh_token
  rotate_refresh_token: true    # store a new refresh token when returned
  max_lifetime_secs: 3600       # distrust longer expiries
  extra_params: {}
  stale_codes:                  # upstream 401s meaning "token refused early" → refresh+retry once
    - { status: 401, body_contains: "INVALID_SESSION_ID" }
  reauth_codes: [invalid_grant] # token endpoint errors meaning "user must reconnect"
```
`hook` means `token.mint` produces fresh credential fields plus an expiry.

#### `tools` (permission catalogue)
```yaml
tools:
  read:
    wildcard: { id: all_read, name: All read operations }      # optional; validator proves superset
    items:
      - { id: list_tasks, name: List tasks, host: api.todoist.com, path: /rest/v2/tasks, methods: [GET] }
  write:
    items:
      - { id: close_task, name: Complete task, host: api.todoist.com, path: "/rest/v2/tasks/*/close", methods: [POST] }
      - { id: create_issue, name: Create issue, host: api.github.com, path: /graphql, methods: [POST], graphql: mutation }
      - { id: push, name: Push code, host: github.com, path: "/*/*/**", methods: [GET, POST], git: push }
```
`description` optional; tool ids are immutable; renaming = remove + add.

#### `approvals` — approval summaries (templates per tool)
```yaml
approvals:
  close_task:
    title: Complete task
    fields:
      - { label: Task, source: path, segment: 4 }
  send_message:
    title: Send email
    fields:
      - { label: To, source: body, steps: [ { json: /raw }, { base64url: {} }, { mime_header: To } ] }
      - { label: Subject, source: body, steps: [ { json: /raw }, { base64url: {} }, { mime_header: Subject } ] }
    link: "https://mail.google.com/mail/u/0/#inbox"
    summarize: hook             # optional: try approval.summarize first, then this template, then generic
```
Sources: `path` (`segment` index, 1-based over `/`-split path), `query`
(`name`), `header` (`name`, only safe headers: not `authorization`, `cookie`,
`proxy-*`, `x-api-key`, or any header injected by a mechanism), `body`
(`steps`). Steps are core **decoders**: `json` (pointer), `form` (field name),
`graphql` (`operation_name` | `field_names`), `base64url`, `base64`,
`mime_header` (name), `mime_text` (first text/plain part, clamped). Link
templates may use `{request.host}`, `{path.N}`, `{query.name}`, `{field.Label}`
and must resolve to https on a host in `hosts` or `about.web_hosts`.

#### `agent` (what the agent is told / gets in its environment)
```yaml
agent:
  instructions: |               # ≤ AGENT_SNIPPET_MAX; plugin-attributed block in the agent's instructions
    Use the Todoist REST API v2. Task ids are strings.
  env: [ { name: TODOIST_API_BASE, value: "https://api.todoist.com/rest/v2" } ]  # non-secret only
  stubs:
    - { path: ".config/gcloud/application_default_credentials.json", mode: "0600",
        content: '{"type":"authorized_user","client_id":"aurora-managed","client_secret":"aurora-managed","refresh_token":"aurora-managed"}' }
  default_blocklist: [ "registry.npmjs.org" ]   # blocked when a connection of this app exists in scope
```
`env` names match `[A-Z][A-Z0-9_]*` and may not be one of the reserved names
(`PATH`, `HOME`, `USER`, `SHELL`, `TZ`, `LANG`, anything starting with `AURORA_`,
proxy and CA variables, LLM client variables). Stub paths are relative to the
home and confined to it.

#### `database` (phase 2)
```yaml
database: { engine: postgres }
```
Marks a database app reached through the gateway database lane (`21`); its
credentials must include `host`, `port`, `dbname`, `user`, `password`,
optional `sslmode`, `ca_pem`.

## 4. Template substitution

Template strings may contain `{namespace.name}` tokens only:

| Namespace | Source | Allowed in |
|-----------|--------|------------|
| `credentials.*` | declared credential fields | inject, finalize, probe auth (via hosts), token |
| `metadata.*` | declared metadata fields | inject, finalize, rewrite_host, links |
| `connection.tenant_host` | stored tenant host | tenant, rewrite_host, token_url template |
| `context.*` | permitted context variables | inject, transform, hooks |
| `fields.*` | form fields | `fields` connect only |
| `request.host`, `path.N`, `query.x`, `field.Label` | the held request / summary | approval links only |

Escaping is by destination: header values reject CR/LF/NUL (substitution that
would introduce them fails the request with `credential_unavailable`); query
values are percent-encoded; path values percent-encoded per segment; hostnames
must remain valid DNS names under the declared zone; JSON contexts are
JSON-escaped. An undefined variable is a validation error at load; an empty
optional value at runtime substitutes the empty string, except in `basic`
user/password and `bearer` where it aborts injection (`credential_unavailable`).

## 5. Mechanisms (core, gateway-implemented)

Adding a mechanism is a core change, versioned with the contract.

| Mechanism | Stage | Parameters | Effect |
|-----------|-------|------------|--------|
| `bearer` | inject | template | `Authorization: Bearer <v>` |
| `basic` | inject | `user`, `password` templates | `Authorization: Basic …` |
| `header` | inject | `name`, `value` template | set header (overwrites agent value) |
| `query` | inject | `name`, `value` template | set/replace query parameter |
| `remove_header` | inject | `name` | remove an agent-supplied header |
| `path_template` | inject | `prefix`, `value` template | replace the remainder of the first segment starting with prefix |
| `path_regex` | inject | `pattern`, `replacement` template | anchored regex rewrite of the path |
| `rewrite_host` | route | template constrained to `zone` | upstream host (and TLS SNI) replaced |
| `tenant` | guard | `field`, `zone` | request host must equal stored host, else 421 `host_mismatch` |
| `aws_sigv4` | finalize | `region`, `service`, `access_key`, `secret_key`, `session_token?` templates | SigV4-sign the final request (payload hash `UNSIGNED-PAYLOAD` for streamed bodies unless the service requires it, in which case the body is buffered up to 8 MiB) |
| `git_commit_trailer` | transform | `trailer` template | on GitHub-style "create commit" JSON bodies (`message` field), append the trailer if absent; body buffered ≤1 MiB, else skipped |
| `oauth_token_endpoint` | intercept | `path`, `method` | answer SDK refresh calls locally with the current access token (JSON `{access_token, token_type: "Bearer", expires_in}`) |
| `registry_token_challenge` | passthrough | — | do not convert upstream 401 `WWW-Authenticate: Bearer realm=…` challenges into guidance; let anonymous registry token flows proceed |

Decoders for summaries: `json`, `form`, `graphql`, `base64url`, `base64`,
`mime_header`, `mime_text`.

## 6. Validation (the linter)

`aurora-registry::validate` runs in the API at ingest, in `aurora plugin
validate`, and in CI. A package is **rejected** (status `invalid`, reason shown)
if any of:

- signature missing/invalid/untrusted (unless dev override), publisher mismatch;
- archive rules violated; sizes exceeded; icon not PNG; guide contains raw HTML;
- unknown keys, unknown contract, `min_core` above the running version;
- duplicate ids (apps across *all* active packages; tools within an app);
- host pattern with more than one `*`; a tool host not covered by the app's
  `hosts`; a group wildcard that is not a true superset of its group;
- tenant/rewrite host not constrained to a declared zone; zone broader than
  `*.<registrable domain>` (public-suffix list check);
- token endpoint not https, or templated without a zone or `{instance}`;
- (phase 2, D-74) `{instance}` used as part of a host, in a path, or in an app
  without `instances`; `instances.default_host` that is not a plain DNS name;
- probe URL or approval link host not declared;
- template references to undeclared credentials/metadata or unpermitted
  context variables;
- `hooks.wasm` exports/imports not matching the WIT world; `permissions.hooks`
  not matching hook references (`connect.kind: hook`, `token.kind: hook`,
  `summarize: hook`); `egress` for `approval.summarize`; egress hosts that are
  reserved or private;
- a claim on a **reserved host** (LLM provider hosts, the instance's own
  external host, internal service hosts, `*.aurora.internal`);
- unknown mechanism or decoder;
- `env` or stub paths violating the rules above.

A host **conflict** with another active package is not a rejection: the package
waits in `pending_approval` until every conflicting claim has a winner.

## 7. Host claims and conflicts

A **claim** is `(host pattern, path prefix)` from a `hosts[]` entry. Claims
that merely overlap **known LLM hosts** (`08 §6`, e.g. a cloud plugin claiming
`*.amazonaws.com`) are accepted, but the gateway never injects an app
connection on a governed LLM host. Two claims
conflict when some (host, path) could match both. For each conflicting claim the
instance admin picks a winner (`host_claim_decisions`). The losing app's host
entry is compiled as `claim_status: lost`: it never injects, its tools on that
claim are hidden from pickers, rules naming them are flagged inert, and its
connections show "not injecting on <host> (lost to <app>)". Decisions survive
upgrades while `claim_fingerprint` is unchanged; a changed claim requires a new
decision. Reversing a decision is allowed and audited.

(phase 2, D-74) A registered app instance host (`§3.4 Instance-bound hosts`) is
a claim of its app. Registration refuses a host that conflicts with an existing
static claim; a static claim activated later that overlaps a registered
instance host is a conflict decided as above.

## 8. Hooks

### 8.1 WIT world (`crates/aurora-hook-wit/wit/plugin.wit`)

```wit
package aurora:plugin@1.0.0;

interface types {
  record kv { key: string, value: string }
  record http-request { method: string, url: string, headers: list<kv>, body: option<list<u8>> }
  record http-response { status: u16, headers: list<kv>, body: list<u8> }
  variant fetch-error { denied(string), timeout, too-large, network(string) }
  enum log-level { debug, info, warn, error }
  variant hook-error { unsupported, invalid-input(string), upstream(string), reauth-required(string), internal(string) }

  record authorize-input  { app-id: string, method-id: string, client: list<kv>, redirect-uri: string,
                            state: string, scopes: list<string>, access-level: option<string>,
                            fields: list<kv>, context: list<kv> }
  record authorize-output { url: string, continuation: option<string> }

  variant exchange-source { oauth-callback(list<kv>), form(list<kv>) }
  record exchange-input   { app-id: string, method-id: string, source: exchange-source, client: list<kv>,
                            client-secret: list<kv>, redirect-uri: string, pkce-verifier: option<string>,
                            continuation: option<string>, context: list<kv> }
  record connected        { credentials: list<kv>, metadata: list<kv>, scopes: list<string>, label: string,
                            expires-at: option<u64>, tenant-host: option<string> }
  record continue-with    { url: string, continuation: string }
  variant exchange-output { connected(connected), continue(continue-with) }

  record mint-input       { app-id: string, credentials: list<kv>, metadata: list<kv>,
                            tenant-host: option<string>, client: list<kv>, client-secret: list<kv>, context: list<kv> }
  record mint-output      { credentials: list<kv>, expires-at: u64 }

  record summarize-input  { app-id: string, tool-id: option<string>, method: string, host: string, path: string,
                            query: list<kv>, headers: list<kv>, body-prefix: list<u8>, truncated: bool }
  record summary          { title: string, fields: list<kv>, text: option<string>, link: option<string>,
                            group: option<string> }
}

interface host {
  use types.{http-request, http-response, fetch-error, log-level};
  fetch: func(req: http-request) -> result<http-response, fetch-error>;
  log: func(level: log-level, message: string);
  random-bytes: func(len: u32) -> list<u8>;
  now-unix-seconds: func() -> u64;
}

interface hooks {
  use types.{authorize-input, authorize-output, exchange-input, exchange-output, mint-input, mint-output,
             summarize-input, summary, hook-error};
  connect-authorize: func(input: authorize-input) -> result<authorize-output, hook-error>;
  connect-exchange:  func(input: exchange-input)  -> result<exchange-output, hook-error>;
  token-mint:        func(input: mint-input)      -> result<mint-output, hook-error>;
  approval-summarize: func(input: summarize-input) -> result<summary, hook-error>;
}

world plugin {
  import host;
  export hooks;
}
```

The SDK provides default implementations returning `unsupported`; authors
implement only what `permissions.hooks` declares. A component that imports
anything other than `aurora:plugin/host` (e.g. WASI filesystem or sockets) is
rejected by the validator.

### 8.2 Hook points

| Hook | Caller | When | Sees | Egress |
|------|--------|------|------|--------|
| `connect.authorize` | API | Connect clicked (method `hook` with `authorize: true`) | client non-secret fields, permitted context | none |
| `connect.exchange` | API | OAuth callback or form submit | callback params / form fields, client incl. secret, PKCE verifier, continuation, context | declared |
| `token.mint` | gateway → hook host | credential expired (under the refresh lock) | one connection's credentials + metadata, client, context | declared |
| `approval.summarize` | gateway → hook host | a request is held | tool, method, host, path, safe headers, body prefix | **none** |

Outputs are validated against the manifest: credential/metadata keys must be
declared; `tenant-host` must fall in the declared zone; `continue.url` must be
https on a declared host or an `oauth_clients` authorize host; summaries are
clamped to `SUMMARY_LIMITS`.

### 8.3 The hook host (`aurora-hookhost`)

- Stateless internal service (port 4104), no database access and no encryption
  keys. Reads the active registry snapshot and hook modules through the API's
  internal endpoint `GET /internal/v1/registry/active` and
  `GET /internal/v1/registry/blobs/{digest}` (internal secret), refreshing on a
  60 s poll of the active hash (it does not hold a DB connection).
- Endpoint: `POST /internal/v1/hooks/{hook}` with
  `{ "package_id", "package_digest", "app_id", "input": <hook input as JSON> }`
  → `{ "ok": <output> }` or `{ "error": { "code", "message" } }`. Auth:
  `Authorization: Bearer <internal secret>`. The egress allow-list is taken
  from the snapshot, never from the caller.
- Runtime: wasmtime with the component model; modules compiled once per
  digest; a **fresh `Store` and instance per call**; memory limit
  `HOOK_MEMORY`; epoch interruption enforcing `HOOK_WALL` (15 s for connect and
  mint hooks; 1 s for `approval.summarize`); output ≤ `HOOK_OUTPUT`.
- `fetch`: https only; host must be in the package's egress list for this hook;
  resolved through the same destination guard as the gateway (no private
  addresses); ≤ `HOOK_FETCH_MAX` calls per invocation, 10 s and 1 MiB each;
  redirects not followed; request headers `Host`/`Connection` controlled by the
  host.
- `log`: redacted (credential-looking values masked), ≤100 lines per call,
  ≤1 KiB each, emitted with package/app ids.
- Metrics per package/hook: calls, errors, duration, fuel/epoch kills.
- Runs as its own process (second isolation layer). Replicas: any number.

## 9. Registry snapshot

The API compiles all `active` packages into one deterministic JSON document
(canonical: sorted keys and arrays by id), stores it in `registry_snapshots` by
content hash and activates it.

```json
{
  "format": 1,
  "contract": 1,
  "packages": [ { "package_id", "version", "digest", "publisher", "hooks_digest": "sha256|null",
                  "egress": { "connect.exchange": ["…"] }, "context": ["…"] } ],
  "oauth_clients": [ { "package_id", "client_key", "authorize_url", "token_url", "pkce", "client_auth", "body_format", "fields": [] } ],
  "apps": [ { "app_id", "package_id", "about": {}, "credentials": [], "metadata": [], "connect": [],
              "token": {}, "tools": { "read": {}, "write": {} }, "approvals": {}, "agent": {}, "database": null } ],
  "hosts": [ { "host": "api.todoist.com", "path_prefix": "/rest", "app_id": "todoist", "claim_status": "active",
               "tenant": null, "rewrite_host": null, "inject": [ { "bearer": "{credentials.api_token}" } ],
               "finalize": null, "transform": null, "intercept": null, "passthrough": [] } ],
  "reserved_hosts": [ "api.anthropic.com", "llm.example.org", "aurora.example.org" ]
}
```

**Activation rule**: consumers (gateway, hook host, API replicas) select the
snapshot with the highest `contract` they support (the core supports contract N
and N−1), then the most recent `activated_at`. During rolling upgrades older
replicas keep serving the newest snapshot they understand.

The gateway, the API (catalogue, forms, effective views, simulation) and the
hook host all consume the same snapshot; there is no second copy of any
catalogue or matcher.

## 10. Package lifecycle

| Source | Behaviour |
|--------|-----------|
| `builtin` | Packages under `/usr/share/aurora/plugins` in the API image (phase 2 catalogue; phase 0 ships none). Signed with the project key; activated automatically on first sight and on upgrade **unless permissions widened** relative to the previously approved version. |
| `directory` | `plugins.dir` scanned at startup and every 60 s. |
| `upload` | `POST /v1/admin/plugins` (multipart). |

States: `pending_approval` → `active` (→ `superseded` by a newer active version)
| `rejected` | `disabled` | `invalid`.

- **First install** and any **widening** (new hook, new egress host, new
  context variable, new host claim, new reserved-adjacent zone) require
  instance-admin approval. Upgrades with an identical or narrower permission
  set activate automatically; the previous version stays active until the new
  one is.
- **Trust revocation**: revoking a key disables every package it signed; their
  connections stop injecting and show why; nothing is deleted.
- **App ids and tool ids are immutable**; credential fields may only be added
  (optional) within a major version. A **major** version may change `credentials`
  incompatibly; what that does to live connections is defined next.
- **Major-version upgrade and live connections** (phase 3, RFE-022, D-65).
  Phase 0–2 left this undefined (connections would simply fail at injection time
  with `credential_unavailable`). A *major upgrade* is an activation of version
  *N.x* superseding a previously active *M.y* of the same package with `N > M`
  (semver major). Upgrades are **not refused** while connections exist (unlike app
  removal). In the **same transaction as the activation**:
  1. For every app of the package present in the new version, every live
     `app_connections` row of that app (any org, any scope; statuses `active` and
     `reauth_required`) is checked: the **required fields** of the new manifest
     (`credentials` entries without `optional: true`) must all be among the
     connection's **stored credential keys** (`app_connections.credential_keys`,
     names only, recorded whenever credentials are sealed; never values — `03 §Planned
     changes`). For a legacy row with unknown keys, the keys are derived from the
     required fields of the manifest of its recorded `app_version` (if that package
     version is still stored), otherwise the row is treated as **not satisfying**
     (fail closed).
  2. A connection that fails the check becomes `status = reauth_required`,
     `reauth_required_at = now()` and `status_reason = "plugin_major_upgrade:
     <package_id> <old_version> -> <new_version>; missing: <field names>"`
     (≤ 300 chars, field *names* only). Its gateway caches are invalidated
     (`connection` scope). A connection that satisfies the check is left `active`
     and keeps its `app_version` until its next reconnect (diagnostics only).
  3. Disabled and `orphaned` connections are not touched.
  4. The count per app and in total goes into the audit row (`plugin.approve`
     metadata `connections_reauth: n`, or the system row `plugin.activate` for an
     automatic activation).
  Effects: the gateway answers 403 `connection_reauth_required` with the
  reconnect URL (`06`), exactly as for a refused refresh; the connections page and
  the connection card show the reason ("`<app>` was upgraded to a version with
  different credential fields — reconnect"); a **reconnect** (§11 *Connect flow*
  step 6) clears the state and records the new `app_version`. Nothing is deleted,
  grants and rules naming the connection keep working after the reconnect.
  Credential *templates* therefore never run against a vocabulary they were not
  connected with: a renamed field cannot surface as a confusing
  `credential_unavailable` at injection time.
- **Approval screen warning.** Every pending version that is a major upgrade shows
  **"N connections will need reconnecting"** (count of connections that fail the
  check right now, per app, no tenant names). `GET /v1/admin/plugins/{id}` returns
  `impact: { major_upgrade: bool, from_version, connections_total, connections_needing_reauth, by_app: [{ app_id, connections, needing_reauth }] }`;
  the dashboard shows it as a banner and as a mandatory section of the permissions
  review (`14`). The count is advisory (connections can change before approval);
  the activation transaction recomputes. **A major upgrade that would put at least
  one connection into `reauth_required` is never activated automatically** — not
  even a `builtin` package whose permissions did not widen: it stays
  `pending_approval` (reason `breaks_connections`) until an instance admin approves,
  so the warning is always seen. A major upgrade with `needing_reauth = 0`
  follows the ordinary rules.
- **Removing an app** (upgrade without it, or uninstall) with live connections
  is refused unless `force`; forced removal marks connections `orphaned`.
- **New tools arrive blocked** in custom grants; removed tools make referencing
  rules inert (flagged).
- Each connection records `app_version` for diagnostics.

## 11. Connect flow (API)

1. `POST /v1/apps/{app}/connect` with the chosen method, scope and inputs.
   Availability (`07 §App availability`) and permissions are checked.
2. `oauth2`: pick the OAuth client (workspace → org → instance; or the one
   requested); build the authorize URL from the snapshot (scopes, params, PKCE
   challenge); `hook` with authorize: call `connect.authorize`. The **state** is
   AEAD-encrypted (purpose `oauth-state`, TTL `OAUTH_STATE_TTL`) and carries
   `{nonce, app_id, method_id, scope, workspace_id, org_id, user_id, client_id,
   access_level, pkce_verifier, reconnect_connection_id, continuation, scopes (requested), tenant_host, metadata}`; it is
   also set as a `__Host-aurora_oauth` cookie bound to the browser.
3. `GET /v1/oauth/callback`: state must decrypt, be unexpired, match the cookie
   and the session user. Fragment-mode providers land on a bridge page that
   re-submits the fragment as a query. Exchange the code (core `oauth2`) or call
   `connect.exchange`; a `continue` answer re-enters step 3 after a redirect.
4. `fields`: validate the form against the schema (server-side), apply
   `json_map`, call `connect.exchange` if the method is `hook`.
5. Run the probe (if any) → label and metadata.
6. `seal` credentials; insert (or, for reconnect, CAS-update and clear
   `reauth_required`) the `app_connections` row; create blocklist rules;
   invalidate gateways; audit `connection.create`.
7. Compare requested vs granted scopes; report missing scopes in the response
   and on the connection page.

## 12. Developer experience

- `aurora-plugin-sdk` (Rust, `wasm32-wasip2`): generated bindings, a
  `#[aurora_plugin::hooks]` macro with default `unsupported` impls, `fetch` /
  `log` helpers, and test utilities (recorded HTTP cassettes, sample inputs).
- `aurora plugin` (CLI subcommands):
  - `new <id>` — scaffold a package;
  - `build` — compile `hooks.wasm` (cargo component build) and assemble;
  - `validate` — run the linter;
  - `compile` — print the snapshot fragment this package contributes;
  - `test` — run `tests/*.yaml` through `aurora-gateway check-snapshot` and
    hooks through an embedded hook-host runtime against recorded HTTP;
  - `sign --key <file>` — write `SIGNATURE`; `keygen` — create an ed25519 pair;
  - `docs` — render setup guide + permissions table to HTML/Markdown.

Fixture file format (`tests/*.yaml`):
```yaml
- name: list tasks injects bearer
  connection: { credentials: { api_token: "tok" }, metadata: {} }
  request: { method: GET, url: "https://api.todoist.com/rest/v2/tasks" }
  expect:
    app: todoist
    tool: list_tasks
    upstream: { headers: { authorization: "Bearer tok" }, url: "https://api.todoist.com/rest/v2/tasks" }
- name: close task summary
  request: { method: POST, url: "https://api.todoist.com/rest/v2/tasks/123/close" }
  expect: { tool: close_task, summary: { title: "Complete task", fields: { Task: "123" } } }
```

## 13. Phase 0 fixture packages (`plugins/fixtures/`)

Signed with a test key trusted only in test configurations. Each targets mock
upstreams from `aurora-testkit`. Together they exercise every mechanism and
hook point.

| Package | Exercises |
|---------|-----------|
| `fixture-bearer` | `fields` connect + probe, `bearer`, `static` token, read/write tools with wildcard, path/query summary templates, default blocklist, agent snippet + env + stub |
| `fixture-oauth` | shared `oauth_clients` with two apps, `oauth2` connect (PKCE, access levels, fragment callback variant), `oauth2_refresh` with rotation, stale codes, reauth codes, `oauth_token_endpoint` intercept |
| `fixture-tenant` | tenant-bound suffix host, `rewrite_host`, zone-constrained token URL, `basic` and `header` templates, `host_mismatch` |
| `fixture-hooks` | `hooks.wasm`: `connect.authorize` + `connect.exchange` (with `continue`), `token.mint`, `approval.summarize`; egress enforcement and the no-egress rule |
| `fixture-sigv4` | `aws_sigv4` finalizer (checked against AWS test vectors), `org_external_id` context variable |
| `fixture-graphql-git` | GraphQL query/mutation tools (fail-closed), git push/fetch tools, `git_commit_trailer`, `registry_token_challenge` |
| `fixture-conflict` | a claim overlapping `fixture-bearer` for host-claim decisions |
| `fixture-invalid-*` | one package per linter rule, each expected to be rejected with a specific reason |
