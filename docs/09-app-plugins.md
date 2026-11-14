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
the API logs a warning at startup). (FIX-107, D-187) **`key_id` is the canonical
standard-base64 (padded) encoding of the 32-byte public key**, or `builtin`; any other
spelling of the same key (URL-safe, unpadded) is `signature_malformed`, so revocation and
`enable` can compare key ids as strings. A migration rewrote older rows.

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

(FIX-115) **Hook-based connects report no requested scopes**: `requested_scopes` stays
empty and `missing_scopes` is always empty for them; the hook reports what was granted.
`app_connections.source_connection_id` is reserved (nothing writes it yet); deletion
handles one level of children until it is used.

### 3.3 `oauth_clients` (shared by the package's apps)
```yaml
oauth_clients:
  - id: google
    authorize_url: https://accounts.google.com/o/oauth2/v2/auth
    token_url: https://oauth2.googleapis.com/token
    # authorize_url may be templated the same way (S-14)
    # (FIX-107, SEC-17) Static token and authorize endpoints lie on a claimed host or a host
    # listed in `permissions.token_hosts` (exact hosts, never reserved or private); a new token
    # endpoint host is a widening and is shown at approval.
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

**Several methods of one app: one app per service** (FIX-123, D-195(a), D-214). Ways to
authenticate to the same service are **methods of one app**, not separate apps (a GitHub App
installation and an OAuth login are two methods of `github`; an assumed role and an access
key pair are two methods of `aws-keys`). Every method (any `kind`) accepts, all optional:

```yaml
- id: github_app
  kind: hook
  label: GitHub App                 # ≤ 60 characters, the chooser's name for the method
  description: "An app installed on the repositories you choose …"   # ≤ 400, one or two sentences:
                                    # what it is, when to use it
  recommended: true                 # the chooser preselects it and badges it "Recommended"; at most one
  token: { kind: hook, max_lifetime_secs: 3600 }   # overrides the app's `token` for connections made with this method
```

- **`token` per method.** A connection made with a method that declares `token` uses it; one made
  with a method that does not (and a connection with no recorded method, `app_connections.
  connect_method` null) uses the app's `token` (default `static`). The gateway's refresh and mint
  paths, and every other reader of the token kind, resolve it per connection:
  `AppEntry::token_for(connect_method)`.
- **`credentials` are the union** of what the methods produce; a credential a method does not
  produce is declared `optional: true`. The linter (§6) checks that a required credential read by
  host templates is produced by every `fields` and `oauth2` method (a `hook` method's output is
  not visible to the manifest). The rule applies only to an app in which a method declares its own
  `token`, so a package written before the key stays valid.
- **`tools[].connect_methods: [method ids]`** (the brief's `tools[].methods`; `methods` is the
  HTTP methods) restricts a tool to connections made with those methods: it is hidden from pickers
  and ignored by classification for connections of other methods (a tool with no list applies to
  every method; a connection with no recorded method hides nothing).
- The connect window asks "How do you want to connect?" when an app has more than one method,
  showing `label`, `description` and a **Recommended** badge; a reconnect keeps the connection's
  method unless the person picks another.

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
  Manifests and the linter are unchanged by base paths (below): `{instance}`
  stays a whole host in every manifest.
- An **app instance** (`03 app_instances`) is a host registered for one app by
  an admin, optionally with a **base path** (phase 5, D-231, FIX-145):
  instance-wide by an instance admin, or for one organization by its
  owners/admins (`05 §App instances`). Registration is the trust decision
  about where that app's credentials may be sent; members never type a host.
- At registration the host MUST be a DNS name in lower case: no IP literal,
  userinfo, port or path, and not under `aurora.internal`. It MUST NOT equal or
  fall under a static claim of any active package (another app's host) or a
  governed LLM host (`08 §6`), and MUST NOT be one of Aurora's own addresses;
  these checks are per host and ignore the base path, so a path never gets
  around them (`instance_host_conflict`). The optional `probe` runs through the gateway's
  destination guard. A private address is reachable only where the operator
  allows it (`gateway.allowed_private`, `06`).
- (phase 5, D-231) **Base path.** A registration may carry a base path for an
  app served under a path (`https://its.cern.ch/jira`: host `its.cern.ch`, base
  path `/jira`). Validation (`instance_base_path_invalid`, pointer `/base_path`):
  it starts with `/`; it has 1–8 segments; each segment is 1–64 characters from
  `[A-Za-z0-9._~-]` and is neither `.` nor `..`; the total length is ≤ 128.
  Normalization before validation: a trailing `/` is dropped, and `""` or `/`
  means no base path (`null`). Case is kept (paths are case-sensitive). The base
  path cannot be changed after registration, like the host (register again
  instead). `default_host` never has a base path: it stays a bare host.
- **Registrations that share a host.** Two registrations visible to each other
  (instance-wide and any organization's, or two of one organization) on the same
  host conflict (`instance_host_conflict`, `kind: instance`) when their base
  paths **overlap**: either is `null`, they are equal, or one is a segment
  prefix of the other (`/jira` overlaps `/jira/x`, not `/jira2`). Disjoint base
  paths on one host may be registered, for one app or different apps (Jira DC at
  `/jira`, Confluence DC at `/confluence`). Promoting an organization's
  registration to instance-wide (`instance_host_shared`) uses the same overlap
  rule. The per-host lock taken while checking is unchanged.
- **`{instance}` resolution** (D-231(b)). For a registration with a base path,
  every `https://{instance}/…` the core resolves (registration probe, connect
  probes, OAuth authorize/token URLs, token refresh, `token.mint`, `library`
  fetch) is `https://<host><base_path>/…`, built by one resolver in
  `aurora-registry`; no other code substitutes `{instance}` itself. Hook
  `permissions.egress` `{instance}` stays the host (egress is checked per host);
  the hook host request also carries `instance_base_path` (C-249, C-731).
- A connection of an instance-bound app records its `instance_id` (or none for
  `default_host`). The gateway injects it **only** on that instance's host and,
  when the instance has a base path, only under it (the path equals the base path
  or starts with base path + `/`), exactly as a tenant-bound connection only on
  its `tenant_host` (SEC-04, SEC-59).
- (phase 5, D-231(c)) **Matching under a base path.** A request belongs to a
  registration when its host equals the instance host and its path, normalized
  as for tool matching (`06 §Per-request pipeline` step 3), is the base path or
  starts with base path + `/`; a registration without a base path matches every
  path. Among several matches the longest base path wins. Everything the
  manifest decides uses the **app path**, the path with the base path removed
  (`hosts[].match.path_prefix`, tool `path`, `{path.N}` in approval templates,
  `params` conditions), so manifests work unchanged. The upstream request, the
  audit `path` and the approval display keep the **full** path. A request to the
  host outside every registered base path matches no instance and nothing is
  injected (fail closed, as for an unknown host). This supersedes D-92(e) and
  P2-CAT-7 G7 ("an instance is a bare host") for registered instances.
  A connection without an instance stands for `default_host`: `{instance}`
  resolves to that host in every gateway path (token refresh/exchange, hook
  egress, injection guard), it is injected only there, never on a registered
  instance's host (D-86).
- OAuth: each self-hosted instance has its own OAuth application, so the client
  is a BYOC client bound to the instance (`app_oauth_clients.instance_id`); the
  templated URLs resolve to that instance's host and base path and MUST be https.
- Registered instance hosts are claims for `§7`: a package activated later
  whose static claim overlaps a registered instance host needs an instance-admin
  decision like any other conflict, whatever the instance's base path (static
  claims stay host-level).
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
A connect method may carry its own `token` (`connect[]`, FIX-123): it overrides this one for
connections made with that method, so one app can offer an OAuth login (`oauth2_refresh`) and a
minted installation token (`hook`) side by side.

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
`description` optional; `connect_methods` optional (a list of the app's `connect[]` ids, §3.4
`connect[]`); tool ids are immutable; renaming = remove + add.

**Which tool a request is.** A request is classified as the **first** catalogue tool that matches
its host, method and path: `read` items are tried before `write` items, each in declaration order.
There are no segment exclusions, so when two patterns overlap the order decides (for example
`/export/categ/favorites*` must be declared before `/export/categ/*`, and `/rooms/api/rooms/search`
before `/rooms/api/rooms/*`). Reordering `items` therefore changes which tool id a grant, an
approval and the request log see; keep specific patterns first and cover each overlap with a
fixture. Query strings never participate in matching. (phase 4, P4-GRANTS, Q8, D-124: **except
through `params`**, below; a tool without `params` is matched exactly as before.)

**Parameter-level matching (phase 4, P4-GRANTS, Q8, D-124, SEC-68).** Some APIs put the
operation in a parameter, not the path: AWS Query APIs (`POST https://ec2.<region>.amazonaws.com/`
with `Action=DescribeInstances`, STS with `Action=GetCallerIdentity`), AWS JSON APIs (header
`X-Amz-Target`). A tool may add an optional **`params`** list, a classifier like `graphql` and `git`:

```yaml
- { id: ec2_describe, name: Describe EC2 resources, host: "ec2.*.amazonaws.com", path: /, methods: [GET, POST],
    params: [ { source: form, name: Action, match: { prefix: Describe } } ] }
- { id: sts_get_caller_identity, name: Who am I, host: "sts.*.amazonaws.com", path: /, methods: [GET, POST],
    params: [ { source: form, name: Action, match: { equals: GetCallerIdentity } } ] }
- { id: dynamodb_query, name: Query a table, host: "dynamodb.*.amazonaws.com", path: /, methods: [POST],
    params: [ { source: header, name: x-amz-target, match: { prefix: "DynamoDB_20120810.Query" } } ] }
```

- **Conditions.** `params` is a list of 1–4 conditions, all of which must hold (AND). `source`:
  `query` (the URL query), `form` (the body, only when `Content-Type` is exactly
  `application/x-www-form-urlencoded`, optional `charset=utf-8`) or `header` (only a header from a
  fixed list, phase 4: `x-amz-target`; never `authorization`, `cookie`, `proxy-*`, `x-api-key` or any
  header a mechanism injects). `name`: `[A-Za-z0-9_.-]`, ≤64 chars (case-sensitive for `query` and `form`,
  case-insensitive for `header`). `match`: exactly one of `equals: "<v>"`, `in: [<v>, …]` (≤32), or
  `prefix: "<v>"` (≥4 chars); values `[A-Za-z0-9_.:/-]`, ≤128 chars. No regular expressions, no
  numeric or JSON-body matching (GraphQL stays the only structured-body classifier).
- **Fail closed.** The value is extracted once per request: percent-decoded one time, `+` as
  space in `form`, no case folding. The parameter must occur **exactly once** in its source, and a
  `form` parameter must not also appear in the query. If it is missing, repeated, present in both
  places, in a body that is not urlencoded, compressed (`Content-Encoding`), not valid UTF-8 or
  **truncated beyond the buffer** (`CONDITION_BODY_BUFFER`), the condition is **unknown, the tool
  does not match**, and classification continues down the catalogue (typically reaching the generic
  write tool of the same endpoint, or `tool = None`). Doubt therefore never produces a `read`
  classification. (FIX-085) A condition also treats as doubt (no match, and the request is untooled):
  **a raw `;`** anywhere in the query string or form body (some servers split pairs on it, so one
  `Action` for the gateway may be two for the upstream; `%3B` is exact and stays an ordinary
  character); **two parameters whose names differ only in case** (`ACTION` beside `Action`: a
  server that folds case sees one parameter given twice), and a lone case variant of the name
  (`ACTION=...` is not the parameter and is not read as absent either); and, for a
  `json_pointer` rule condition (`07`), **a key repeated on the way to the pointer** in the JSON
  body (the parsers disagree on which copy wins), which makes the condition unknown for allow and
  restrictive rules alike. A `form` condition requires the tool to list body methods (the linter
  refuses a tool with no `methods`, or only `GET`/`HEAD`). The linter requires, for every tool with `params`, fixtures for the positive case
  and for each of: other value, missing, repeated, query/body conflict, wrong content type,
  truncated body.
- **Query conditions and a body.** Servers merge the query and an urlencoded body, so a `query`
  condition holds only if the request has no body, or has a complete, uncompressed, urlencoded
  UTF-8 body that does not carry the parameter at all; a body that cannot be checked (other
  type, compressed, longer than the buffer) means the tool does not match.
- **Cost.** The gateway buffers the body prefix only for a request whose host, path and
  method already match a `params` tool with a `form` or `query` condition (the existing *Body prefix* step 4 of the
  `06` request pipeline); other requests are unaffected.
- **Order and superset rules.** The first-match rule above is unchanged: a `params` tool is declared
  before the generic tool that would also match (`ec2_describe` before `ec2_call`). A group wildcard
  carries no `params` and remains a superset check on host/path/methods only; a tool with `params`
  is a subset of what its host, path and methods allow.
- **Approvals, effective views, simulation.** Summaries read parameters with the existing
  `form` decoder; the effective view and the simulator build a tool's synthetic request from its
  first example **including its `params`** (`equals` → the value, `in` → the first, `prefix` → the
  prefix followed by `Example`).
- **Compatibility.** Additive in contract 1: a package without `params` is untouched. A core that
  does not know `params` rejects the package (unknown key, and `min_core`); a **gateway** that does
  not know the classifier refuses the snapshot that names it, exactly like an unknown mechanism
  (`06 §Registry snapshot`), and keeps the previous snapshot, so a rolling upgrade can never run a
  `params` tool as a path-only tool. Existing packages need no change. Adopting `params` in a
  package that already has a broader tool (splitting `ec2_call` into `ec2_describe` + `ec2_call`)
  changes which tool id the carved-out requests get: new tools arrive **blocked** in existing custom
  grants (fail closed), so the package's release notes say so and its fixtures cover both.

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
`mime_header` (name; every header of that name, joined), `mime_text` (first text/plain part,
clamped). (FIX-118, D-188) `json` accepts `*` as a path segment (every element of an array,
scalar leaves joined with `, `: `/message/toRecipients/*/emailAddress/address`), `json_count`
(pointer: the number of elements of that array, `0` when absent), `json_text` (pointer: every
`text`/`content` string below it, joined with spaces — Atlassian document format, Notion rich
text) and `mime_attachments` (the number of attachments of a message). **A summary of a request
that sends content to people outside the workspace (mail, calendar invitations, public issues and
pages) lists every recipient and shows a clamped preview of the content; packages use a
`summarize` hook where a template cannot.** Link
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

#### `grants` (phase 4, P4-GRANTS, D-123, SEC-67)
```yaml
grants:
  custom_only: true      # default false: `full` grants are not offered and are refused for this app
  block_untooled: true   # default false: a request that matches no catalogue tool is blocked under every grant
```
Optional, per app; **absent = both `false` = today's behaviour**, so no existing package
must change. For apps whose credential is far broader than their catalogue (Indico's
`full:everything` token), the author can say "only what the catalogue names":

- `custom_only` — `full` mode is not available for this app's connections: the API
  refuses it (422 `full_grant_forbidden`), the dashboard does not offer it, and the
  evaluator never lets a target without `tools` allow a connection of this app
  (`07 §Grants and their compilation`). Custom grants (allow / ask / block per tool;
  new tools arrive blocked; unmatched requests blocked) are the only way to use the
  app. Requires at least one tool in the catalogue (linter error `custom_only_without_tools`).
  **`custom_only: true` implies `block_untooled`** (FIX-085): the tools "are the only way to
  use the app", so a request that matches none is blocked before any rule, a hand-written
  `network` allow for the host included. A manifest that sets `custom_only` with `block_untooled`
  false or absent is refused (`custom_only_needs_block_untooled`); the evaluator applies the
  implication itself, so a snapshot built before the rule is just as strict.
- `block_untooled` — whatever grants exist, a request on which the app's connection
  would be the winning credential but **no catalogue tool matches** (`tool = None`) is
  blocked (403 `blocked_by_policy`, `details.reason = "app_untooled_blocked"`), and
  no hand-written rule can allow it. With `custom_only` it is implied; the flag matters
  for packages that keep `full` available but want it to cover
  *catalogued* tools only (a `full` grant then means "every tool the catalogue names,
  including future ones", never "anything the token can do").
- **The one credentialed request outside the tools** is the skill-library fetch (D-148,
  `§3.4 library`): the gateway's own request for a package's declared `library` templates,
  bounded by those approved templates (exact host, path and method); policy and grants do not
  apply to it, and neither flag changes it. Nothing else a connection's credential signs is
  exempt from `custom_only`.

Both are properties of the manifest the instance admin approves (shown in the
permissions review, `§10`), not of a grant, so a user cannot switch them off.
They sit in contract 1 as an additive key: a core that does not know `grants` rejects the
package (unknown key; packages that use it declare `min_core` of the release that
introduces it), which fails closed. The linter also rejects unknown keys inside `grants` and
non-boolean values. Both flags are copied into the snapshot's `apps[].grants` (`§9`).

#### `library` (phase 4, P4-GSL, D-148, D-149, SEC-84)
```yaml
library:                                  # optional, per app: "this app is a git host that can serve a skill library"
  repo_pattern: "^[A-Za-z0-9_.-]{1,100}/[A-Za-z0-9_.-]{1,100}$"   # what an admin may type as the repository
  request_host: api.github.com            # exactly one host of the app's `hosts[]` (an `{instance}` host for instance-bound apps)
  resolve_ref:                            # ref (branch, tag or sha; empty = the default) → commit sha
    path: "/repos/{repo}/commits/{ref}"
    default_path: "/repos/{repo}"         # used when no ref is set: the default branch is read from the response
    sha: "$.sha"
    default_branch: "$.default_branch"
  list_tree:                              # the files of one commit
    path: "/repos/{repo}/git/trees/{sha}?recursive=1"
    entries: "$.tree[*]"
    fields: { path: "$.path", type: "$.type", mode: "$.mode", blob: "$.sha", size: "$.size" }
    file_type: blob
    truncated: "$.truncated"
    paginate: none                        # none | link_header | page_param (+ max_pages)
  get_blob:                               # one file by blob id
    path: "/repos/{repo}/git/blobs/{blob}"
    content: "$.content"
    encoding: "$.encoding"                # base64 | raw (a `raw` endpoint returns the bytes themselves)
  required_access: read                   # the only accepted value (anything else is refused by the linter); the connection should be read-only
```
Optional, per app; **absent = the app cannot serve a library** (no existing package changes). The
block is **data, not code**: three GET request templates, a ref grammar and JSON-path extractors
(the `json` decoder of `§4`, no hooks, no new mechanism), so the core stays integration-neutral
(SEC-33; the built-in GitHub and GitLab packages are the first users, **GitLab** with the
repository tree endpoint paginated by `link_header` and the blob endpoint `encoding: raw`, a
project path URL-encoded by the template function `{repo|urlencode}`). Rules (the linter enforces
them; ids in the errors are `library_*`):

- `request_host` is **exactly** one of the app's exact `hosts[]` entries (no wildcard, no
  `rewrite_host`; for an instance-bound app the `{instance}` host, `§Instance-bound hosts`), over
  https, and the host rule that covers the request path must have an `inject` stage
  (`library_no_credential`). Every `path` starts with `/` and may contain only the placeholders `{repo}`,
  `{repo|urlencode}`, `{ref}`, `{ref|urlencode}` (D-154), `{sha}`, `{blob}` and a literal query
  (`page` is never templated: the core adds it for paginated trees); each step must use its own
  placeholders (`resolve_ref`: repo and ref, `list_tree`: repo and sha, `get_blob`: repo and blob);
  `{repo}` is substituted only after it matched `repo_pattern` (and the hard grammar: no empty, `.` or
  `..` segment); an optional per-step `method` may only be `GET` (`library_method`); `{ref}` only after matching the ref grammar `[A-Za-z0-9._/-]{1,200}`
  with no `..`, no leading `-`, no `//`; `{sha}` and `{blob}` only after matching `[0-9a-f]{40,64}`.
  Nothing else (no free-form path or query) can reach the request line.
- Only `GET`. No request body, no header templates, no redirects followed. The response must be JSON
  (or raw bytes for `encoding: raw`), at most the size limits of `02 §Limits` (`LIBRARY_FILE_MAX`,
  `LIBRARY_TREE_MAX`); every JSON path is validated at load (anchored, no recursive descent, no
  filters, ≤ 3 segments of wildcards).
- **Credentials are never named or read by the block**: the request is made by the gateway with the
  host entry's ordinary inject stage (`§3.4 hosts`), so the credential, its refresh and the host
  guard are exactly those of an agent's request to the same host (`06 §Library fetch`).
- It is part of the manifest the instance admin approves: the permissions review shows
  "Can serve a skill library: reads repository files from `<request_host>`" and a package update
  that **adds or changes** `library` needs the same re-approval as a widened host claim (`§10`). It
  sits in contract 1 as an additive key; a core that does not know it rejects the package, which
  fails closed, and the linter rejects unknown keys inside `library` and non-conforming values. The
  block is copied into the snapshot's `apps[].library` (`§9`).

#### `database` (phase 2)
```yaml
database: { engine: postgres }
```
Marks a database app reached through the gateway database lane (`21`); its
credentials must include `host`, `port`, `dbname`, `user`, `password`,
optional `sslmode`, `ca_pem` (and, in connection metadata, the audited opt-out `tls_verification = off` of server certificate verification under `require`, `06 §Database lane`). Its tools match the synthetic host `*.db.aurora.internal` (path
`/read`, `/write`, `/ddl`, `/admin`, method `POST`); the linter accepts that host without a
`hosts[]` entry (P2-DBL-2, C-233). Fixtures send a SQL body to `https://<uuid>.db.aurora.internal/`
and expect a `class` and `tool` (`plugins/builtin/postgresql/tests`).

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
  external host, internal service hosts, `*.aurora.internal`); (phase 5 (D-138, moved from phase 4), P4-LLM) this
  includes any claim whose pattern **matches** an exact configured LLM endpoint host
  (Bedrock `bedrock-runtime.<region>.amazonaws.com`, Vertex `<location>-aiplatform.googleapis.com`,
  an Azure resource host), so a package can never obtain a second, ungoverned way to
  inject on those hosts; the built-in AWS packages (`s3.*`, `ec2.*`) are unaffected;
- unknown mechanism or decoder;
- (phase 4, P4-GRANTS) `grants` with unknown keys or non-boolean values, `custom_only` on an
  app with no tools (`custom_only_without_tools`), `custom_only` without `block_untooled`
  (`custom_only_needs_block_untooled`, FIX-085); a tool `params` list that is empty, longer
  than 4, has an unknown `source` or header name, a `match` without exactly one of `equals` /
  `in` / `prefix`, an over-long or non-conforming value, a `prefix` shorter than 4 characters, a
  `form` condition on a tool that lists no methods or only `GET`/`HEAD` (it must list the
  methods that carry a body), or a `params` tool declared
  **after** a broader tool that would shadow it (the linter proves the order by testing the
  tool's own first example against the earlier tools; (FIX-085) a `params` tool also shadows a
  later `params` tool when every condition of the earlier one is implied by a condition of the
  later one on the same source and name: `equals` within `equals` / `in` / a prefix of it, `in`
  within a superset, `prefix` within a shorter-or-equal prefix); a group wildcard carrying
  `params`; a `params` tool missing any of the required fixtures (`§3.4 tools`), where the
  `positive` fixture counts only if the tool's conditions really hold for the request it
  describes (a claim is not a match);
- (phase 4, P4-GSL) `library` violating the rules of `§3.4 library` (`library_host_not_exact`,
  `library_no_credential`, `library_path_placeholder`, `library_repo_pattern`, `library_json_path`, `library_method`,
  unknown keys or non-conforming values), or `library` on an app without a credential that can
  authenticate requests to `request_host`;
- (FIX-123, D-214) connect methods: a duplicate method id (`duplicate_connect_method`), a `label`
  over 60 or a `description` over 400 characters, blank or with control characters
  (`method_text_invalid`), more than one `recommended` (`recommended_multiple`), a
  `tools[].connect_methods` entry that names no method (`tool_method_unknown`), a method `token` whose
  `client` is not declared (`unknown_oauth_client`; a method `token` of `kind: hook` counts for
  `permissions.hooks` like the app's), and, for an app whose methods use `token`, a required credential
  that host templates read and a `fields` or `oauth2` method does not produce
  (`method_credential_unproduced`);
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

**A credential is injected only into a request that leaves over TLS to the port the claim
names (443 when it names none)** (FIX-104, D-180, SEC-04). A request to a claimed host over
plain HTTP, or on another port, is handled as an uncredentialed request to that destination
and is subject to policy as such. A registered instance host (below) is claimed the same
way: TLS to the claimed port. **A wildcard in a host claim must leave a registrable domain
to its right** (public-suffix check, as for zones, SEC-17): `*.herokuapp.com` and
`*.github.io` are refused with `zone_too_broad`, `s3.*.amazonaws.com` is accepted.

(FIX-123, D-195(a)) The built-in packages never conflict with each other: ways to authenticate to one
service are connect methods of one app (`§3.4 connect[]`), so a fresh instance starts with one GitHub
and one AWS app and no host-claim decision to make. A package that a present package supersedes
(`§10`) is not offered to the admin as a conflict.

(phase 2, D-74) A registered app instance host (`§3.4 Instance-bound hosts`) is
a claim of its app. Registration refuses a host that conflicts with an existing
static claim; a static claim activated later that overlaps a registered
instance host is a conflict decided as above. (phase 5, D-231) Both checks are
per host: an instance's base path never narrows the claim it is compared with,
so a static claim on the host refuses a registration with any base path.
Registrations among themselves follow the overlap rule of `§3.4`.

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
  `{ "package_id", "package_digest", "app_id", "input": <hook input as JSON>,
  "instance_host"?, "instance_base_path"? }`
  → `{ "ok": <output> }` or `{ "error": { "code", "message" } }`. Auth:
  `Authorization: Bearer <internal secret>`. The egress allow-list is taken
  from the snapshot, never from the caller. `instance_host` (C-249) is what
  `{instance}` in `permissions.egress` resolves to (absent = nothing);
  `instance_base_path` (phase 5, C-731, D-231) is the connection's registered
  base path (absent = none). Egress stays checked per host: the base path
  neither narrows nor widens it.
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
- Metrics per package/hook: calls, errors, duration, epoch kills (the runtime uses epoch
  interruption and the memory limit; it does not meter fuel).
- (FIX-107, D-187) A cold module pull and compile is single-flight per digest and holds no
  lock another digest or the snapshot refresh needs; the `connect.authorize` URL host must be
  one the package declared (egress for `connect.authorize` / `connect.exchange`, a claimed
  host of the app, or an OAuth client's authorize host); a hook call fails (`internal`) when
  the OS random source cannot be read; `plugin.yaml` is at most `PACKAGE_MANIFEST_MAX`
  (256 KiB, `manifest_too_large`) and the decompressed package at most 4 × `PACKAGE_MAX`.
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
              "token": {}, "tools": { "read": {}, "write": {} }, "approvals": {}, "agent": {}, "database": null,
              "grants": { "custom_only": false, "block_untooled": false } } ],
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
  one is. (FIX-107, D-187) A version is **also a first install** — and needs
  instance-admin approval, with the reason shown on the approval screen — when its
  `package.publisher` differs from the installed package's (`publisher_changed`),
  when it is lower than the highest approved version of that id
  (`version_downgrade`), or when the package id belongs to the built-in source and the
  version does not (`builtin_id_reserved`). Widening includes a new static token-endpoint
  host or `permissions.token_hosts` entry (SEC-17).
- **Trust revocation**: revoking a key disables every package it signed; their
  connections stop injecting and show why; nothing is deleted.
- **App ids and tool ids are immutable**; credential fields may only be added
  (optional) within a major version. A **major** version may change `credentials`
  incompatibly; what that does to live connections is defined next.
- **Major-version upgrade and live connections** (phase 3, RFE-022, D-65).
  Phase 0–2 left this undefined (connections would simply fail at injection time
  with `credential_unavailable`). A *major upgrade* is an activation of version
  *N.x* superseding a previously active *M.y* of the same package with `N > M`
  (semver major). (FIX-107, SEC-57) **The check below runs for every version that
  supersedes an active one, not only for majors**: a minor that renames or repurposes a
  required credential field is held (`breaks_connections`) and flips the failing
  connections on approval exactly as a major does. Upgrades are **not refused** while connections exist (unlike app
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
- (phase 4, P4-GRANTS, D-123) **Activation that introduces `custom_only`** for an app that
  has live `full` grants: in the activation transaction every such grant is **converted
  to a custom grant** (`mode = custom`, `allow_tools` = every tool of the new version's
  catalogue, `ask_tools` empty, `ask_timeout_secs` null) and its derived rules are recompiled,
  so what each agent can do with catalogued tools today is unchanged and what it loses is exactly
  what a full grant added: requests matching no tool, and tools added later. One audit row
  `grant.converted_by_app` per grant (security-critical set, atomic). The permissions review
  shows **"N agent grants will be converted from full to custom access"** (counts by app and
  workspace, nothing else), and a built-in or uploaded upgrade with N > 0 is held
  `pending_approval` (reason `converts_grants`) like `breaks_connections`, never auto-activated.
  Activation that introduces `block_untooled` converts nothing (the evaluator applies it at
  once). A later version that drops either flag converts nothing back: custom grants stay custom.
- (FIX-123, D-195(a), D-215) **Superseded apps.** A built-in app that became a **connect method of
  another app** is listed in the API's table of superseded apps (`github-app` → `github`/`github_app`,
  `github-app-ghes` → `github-ghes`/`github_app`, `aws-role` → `aws-keys`/`assume_role`). In the
  **same transaction as the activation** of a version that contains the target app with that
  method (approval of `github` 2.0.0 or `aws-keys` 2.0.0, §10 widening applies as usual: the new
  hooks and egress need one instance-admin approval), the API: (1) moves every `app_connections` row
  of the old app (any status, any scope) to the target app, package and version with
  `connect_method` set; connection ids, `credentials_ref`, scopes, metadata and the connection's
  grants and rules are untouched, and tool ids carry over unchanged because the merged apps keep
  the old tool ids; (2) moves `app_instances` (a registration of the old GHES app merges into an
  existing one of the same org and host, whose id the connections then reference),
  `skill_libraries.app_id` and `org_app_availability` (an org that had the old app available gets
  the target available); (3) deletes the `host_claim_decisions` between the old and the new
  package (no longer meaningful); (4) disables the old package (`status_reason: superseded_by_<package>`)
  when it is active, so its apps leave the snapshot; (5) writes one `plugin.supersede` audit row with
  counts per app and a dry-run-equivalent count in the upgrade log. Idempotent: a run with nothing
  to move changes nothing. History (`request_log`, `approvals`) keeps the old app id. A later
  re-enable of the retired package is refused (`superseded`).
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
  - `build` — compile the guest crate in `hooks/` (`cargo build --release --locked --target
    wasm32-wasip2`; a cdylib on that target already is a component, no `cargo component`) into
    `hooks.wasm`, or take a prebuilt `hooks.wasm`, and assemble (D-77);
  - `validate` — run the linter;
  - `compile` — print the snapshot fragment this package contributes;
  - `test` — run `tests/*.yaml` through `aurora-gateway check-snapshot` and
    hooks through an embedded hook-host runtime against recorded HTTP (`hook:` cases with an
    `http_cassette`; format in `plugins/builtin/README.md`);
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
- name: token is not sent outside its prefixes
  connection: { credentials: { api_token: "tok" } }
  request: { method: GET, url: "https://api.todoist.com/public/x" }
  expect:
    upstream: { headers_absent: [authorization, x-api-key] }   # fails if any listed header is on the upstream request
```
`expect.upstream.headers_absent` (D-86) lists header names (case-insensitive) that the upstream
request must not carry, whether injected by the package or sent by the client. In a case without a
`connection.instance_host`, the connection of an instance-bound app is its default-host connection
(`{instance}` is the app's `default_host`). (phase 5, D-231, C-731) `connection.instance_base_path`
(optional, next to `instance_host`, normalized and validated as at registration, `§3.4
Instance-bound hosts`) gives the registered instance a base path: requests in the case use the full
URL (`https://its.cern.ch/jira/rest/api/2/myself`), the expected `app`/`tool` and `{path.N}` in
summaries are matched on the app path (`/rest/api/2/myself`), `expect.upstream.url` is the full URL,
and a request on the host outside the base path is injected with nothing. `{instance}` URLs of the
case (refresh `request_url`, cassette entries) resolve to host + base path. (phase 4, P4-GRANTS) `request.body_truncated: true` says
the body is longer than the gateway's condition buffer (the classifier then sees a truncated
prefix; a fixture cannot carry 256 KiB), and `expect.no_tool: true` says the app matches but no
catalogue tool does; the `params` fixtures of `§3.4 tools` use both.

**Token lifecycle cases** (D-82; `token.kind: oauth2_refresh`). A case replays a stale upstream
answer, the refresh exchange against a recorded token endpoint and what the gateway does next. It
drives the gateway's own refresh functions (`build_exchange`, `interpret_exchange`), so the request
the gateway builds, the stale-code match, `reauth_codes` and rotation behave as in production; only
the network and the database are replaced by the recording.
```yaml
- name: refresh after a stale 401
  connection:
    instance_host: gitlab.example.com        # the registered instance (omit for the default-host connection)
    # instance_base_path: /gitlab            # (phase 5, D-231) optional base path of that instance
    credentials: { access_token: "tok-old", refresh_token: "refresh-1" }
    oauth_client: { client_id: "client-fixture", client_secret: "secret-fixture" }   # fake fields of the registered OAuth client
  request: { method: GET, url: "https://gitlab.example.com/api/v4/user" }
  token:
    upstream: { status: 401, body: '{"error":"invalid_token"}' }   # must match token.stale_codes
    http_cassette:                            # each refresh request consumes the first matching entry; an unused entry fails the case
      - request: { method: POST, url: "https://gitlab.example.com/oauth/token",
                   form: { grant_type: refresh_token, refresh_token: "refresh-1" } }   # `form`: subset match on the request parameters
        response: { status: 200, body: '{"access_token":"tok-new","refresh_token":"refresh-2","expires_in":7200}' }
    expect:
      outcome: refreshed                      # | reauth_required | unavailable
      request_url: "https://gitlab.example.com/oauth/token"   # optional: where the refresh was sent
      credentials: { access_token: "tok-new", refresh_token: "refresh-2" }   # subset (rotation)
      retried: { url: "https://gitlab.example.com/api/v4/user", headers: { authorization: "Bearer tok-new" } }
  expect: { app: gitlab, tool: get_current_user }
```
`outcome` is `refreshed` (the retried request carries the new credentials), `reauth_required` (a 4xx
`invalid_grant` or a declared `reauth_codes` error; nothing is retried) or `unavailable` (any other
failure, a request that no cassette entry answers, or a refresh the gateway refuses to build, for
example without a valid instance host: nothing is retried). `retried` is only allowed after
`refreshed`. Refreshed credentials are never echoed into the report.

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
