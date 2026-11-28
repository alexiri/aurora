# Built-in app packages

One directory per package, shipped in the API image at `/usr/share/aurora/plugins` (as signed
`<id>-<version>.aurora-plugin` archives) and loaded through the `builtin` source
(`docs/09-app-plugins.md` §10). `gitlab/` is the reference: copy it.

```
plugins/builtin/<package>/
  plugin.yaml            manifest (publisher: aurora, contract: 1)
  docs/setup.md          setup guide (CommonMark, no raw HTML); the package format fixes this path
  assets/icon.png        PNG, square, >= 64 px, <= 256 KiB (assets/icon-dark.png optional)
  tests/*.yaml           fixtures (docs/09 §12)
  presets/*.json         webhook presets, only for a send-mode `secret_link` app (`webhook`)
  hooks/                 only when permissions.hooks is non-empty: the hook guest crate (below)
```

## Hooks

A package whose manifest declares `permissions.hooks` keeps the guest in `hooks/`, a standalone cargo
project (copy `plugins/fixtures/hook-example/hooks`):

```
plugins/builtin/<package>/hooks/
  Cargo.toml    crate-type = ["cdylib"]; aurora-plugin-sdk by path (../../../../crates/aurora-plugin-sdk);
                an empty [workspace] table so it is not part of the Aurora workspace
  Cargo.lock    committed; the build runs with --locked
  src/lib.rs    `impl Hooks for Guest` for the declared points only, then `export_hooks!(Guest);`
```

`aurora plugin build plugins/builtin/<package>` compiles it (`cargo build --release --locked --target
wasm32-wasip2`; install the target with `rustup target add wasm32-wasip2`, the repo's
`rust-toolchain.toml` already lists it) and packs the result as `hooks.wasm`. Do not commit a
`hooks.wasm` or a `target/` directory (a prebuilt `hooks.wasm` next to `hooks/` is an error).
`aurora plugin test` and `check-builtin.sh` build it for you; the first build compiles the SDK's
dependencies (about 25 s), later ones take a moment. Every `hooks:` egress host must be a host the
vendor documents; `approval.summarize` never has egress.

Hook fixtures (in `tests/*.yaml`, next to the ordinary cases) run the real hook host with `fetch`
answered from recorded HTTP. Key/value lists are written as maps. Each case still carries a
`request:` (the case schema requires it; any request of the package will do):

```yaml
- name: token.mint returns a fresh token
  request: { method: GET, url: "https://api.example.test/v1/items" }
  hook:
    point: token.mint          # connect.authorize | connect.exchange | token.mint | approval.summarize
    app_id: my-app             # optional when the package has one app
    http_cassette:             # recorded responses; each request consumes the first matching entry
      - request: { method: POST, url: "https://auth.example.test/mint" }
        response: { status: 200, body: "tok-fixture-2" }
    input: { credentials: { installation_id: "7" } }
    expect: { credentials: { access_token: "tok-fixture-2" } }    # a subset of the output
- name: token.mint outside its egress list is denied
  request: { method: GET, url: "https://api.example.test/v1/items" }
  hook:
    point: token.mint
    egress_override: [other.example.test]   # replaces permissions.egress for this case
    input: { credentials: { installation_id: "7" } }
    expect: { error: egress-denied, message_contains: "egress denied" }
```

`expect` is either `{ error: <code> [, message_contains: ...] }` (`invalid-input`, `upstream`,
`reauth-required`, `egress-denied`, `unsupported`, `internal`, `timeout`, ...) or a subset of the
output (`url_contains` for `connect.authorize`'s URL). A multi-call flow (`connect.exchange` that
continues) uses `steps: [ { input, expect }, ... ]` over one cassette. The hook output is validated
against the manifest exactly as in production (declared credential names, https URLs, continue
host in the egress list), so a fixture cannot pass with output the host would refuse.

## Adding a package

1. `aurora plugin new <id> --dir plugins/builtin --publisher aurora`, then fill in the manifest.
   Use the narrowest scopes as `default_scopes`; put every mutating tool in `tools.write` and give
   each one an `approvals` template. Read tools must be GET (or `git: fetch`).
2. Write fixtures (below), then run `bash scripts/plugins/check-builtin.sh <id>` until green.
3. Do not hand-edit archives and never commit a signature. CI signs in the release pipeline.

## What the checks require (`scripts/plugins/check-builtin.sh`, every package)

| Check | Requirement |
|---|---|
| validate | `aurora plugin validate` passes (unsigned allowed here; the release signs). Publisher is `aurora`. |
| test | `aurora plugin test` passes, and fixtures cover: every `hosts[]` entry, every tool with at least one positive case (`observed.tool` is the tool) and one negative case named `negative <tool_id>: ...` (the request does not select the tool), every `approvals` template (a case with `expect.summary.title`). |
| scopes | OAuth `default_scopes` are all `access: read`; every scope declares `access`; read tools only use GET/HEAD (or `git: fetch`, graphql `query`, or a `params` classifier that proves the read, e.g. AWS `Describe*`). |
| refused | (FIX-118) the dangerous untooled requests of `REFUSED` in `scripts/plugins/check_builtin.py` (repository deletion, Drive file deletion, calendar clear, mail deletion) each have a passing case named `refused <what>: ...` whose request selects no tool. |
| hooks | the archive holds `hooks.wasm` exactly when `permissions.hooks` is non-empty; for every declared point a passing case where the hook succeeds and (except `approval.summarize`) one where it fails; every `permissions.egress` host is reached by a recorded request; one case per egress point shows a host outside the list denied (`egress_override` + `expect: { error: egress-denied }`); a hook case for an undeclared point fails. Hook cases never count as skipped. |
| token | when the package declares `token`, it has `reauth_codes` (oauth2_refresh) and a fixture shows the injected bearer. Every `oauth2_refresh` app also has replayed token lifecycle cases (`token:` with an `http_cassette`, `docs/09 §12`): at least one with `outcome: refreshed` and one with `outcome: reauth_required`; an instance-bound app has both on its default host and on a registered instance host, and a negative case showing the refresh request is built for the instance host only (SEC-59). Use fake `oauth_client` values. Hook-minted tokens (`token: hook`) are covered by their `token.mint` hook cases. |
| texts | (P5-UX-13, C-939, C-941, C-949) every tool has a `description` of at most 200 characters, in plain words ("Lists the objects in a bucket."); every `connect` method has a `label` and a `description`; the guide and each `connect_note` contain none of `<your Aurora URL>`, `<external_url>`, `Admin, Apps,` or "Instances" (the page is Self-hosted apps): use the placeholders below. A third-party package gets the linter warning `tool_description_missing` instead. |
| docs | `aurora plugin docs` renders (md and html) twice with identical output and contains the permissions table. Rendered output is regenerated, not committed. |
| secrets | `scripts/guards/no-live-tokens.sh` (part of `scripts/guards/run-all.sh`) fails on anything token-shaped under `plugins/builtin/**/tests`. |

## Skills for agents (P5-APPSKILL-1, D-453, `docs/24 §3` and `§10`)

What an agent is told about an app costs context only when it uses the app. Three kinds of text,
each with its own job (the old agent guide, `agent.guide`, is gone: it is ignored with the warning
`agent_guide_ignored`, and the linter warning fails a built-in):

- **`agent.summary`**: one line of at most 160 characters, what the app is for, **naming the tasks
  it serves** ("Events, timetables, room search and booking"). It is the only plugin text in the
  instruction file, so it is what makes an agent pick the app. Required when the app ships skills
  or an operations file.
- **`agent.instructions`** (the essentials): what must be known *before the first request* (which
  account rules apply, what never to retry, what the gateway adds for the agent), at most about
  600 characters. Endpoint lists and parameter tables do not belong here.
- **Skills**, in `skills/<app_id>/<name>/SKILL.md` plus files, the format of a skill-library skill
  (`docs/10 §Skills`):
  - **a skill per task**: the task a person asks for ("Find a free room and book it"), as numbered
    steps with the exact requests (or `aurora-agent` calls), the pitfalls, and how to confirm the
    result. The `description` is the task, in one line (at most 1,024 characters; keep it short,
    it is listed to the agent with every other skill's).
  - **a reference skill** for what several tasks share (how the API is addressed, paginated and
    rate-limited, what its errors mean): one topic per skill, the first screen says what is in it,
    and the `description` starts with what it is for ("How the GitLab REST API is addressed,
    paginated and rate-limited; read before the first request").
  - names: `<app_id>` or `<app_id>-<topic>` (lowercase letters, digits and single hyphens, at most
    64 characters, never `aurora-…`); in a package with `jira` and `jira-dc`, a `jira-dc-…` skill is
    under `skills/jira-dc/`. An app id with an underscore cannot ship skills.
  - **scripts** go in `scripts/` next to the `SKILL.md`, committed **executable** (git mode
    `100755`: the executable bit is signed with the content). Every file is UTF-8 text (no NUL, no
    byte-order mark), at most 64 KiB; at most 16 skills and 256 KiB per app, 50 files per skill.
    A script runs only when the agent runs it, with the agent's access, and every request it makes
    passes the gateway like any other: it holds **no secret** and **no hard-coded host**: the host
    comes from the account shown in `app.md` (`$H` in the examples).
  - refer to the generated file as `app.md`, never `guide.md`.

Fill the tool `description`s too: `tools.md` prints each one after the tool's name. Run
`aurora plugin preview <dir> --app <id>` to see exactly what an agent gets, and
`aurora plugin validate` for the rules and warnings.

Fixture notes: `plugins/fixtures/fixture-app-skills` is a valid package with a summary,
essentials, an operations file and two skills (one with an executable script and a nested
`reference/` file); `fixture-legacy-guide` is a package in the old format (valid, one
`agent_guide_ignored` warning); `fixture-invalid-app-skill-*` are the rejected shapes
(`expected-invalid.txt`). The ones that are too big to check in as text (`body-too-large`,
`too-large`, `file-too-large`, `too-many-files`, `not-text`, `not-text-bom`, `too-many-skills`,
`app-too-large`) have their files written by `scripts/fixtures/mkfixture.py`.
`services/aurora-cli/tests/golden/preview/` is the checked-in output of `aurora plugin preview`
on `fixture-app-skills`.

## Guide placeholders and approval titles (P5-UX-13, D-334, C-940)

`GET /v1/apps/{app}/guide?workspace_id=|org_id=` fills these `{{aurora.…}}` placeholders for the person
reading (the raw text stays what `aurora plugin docs` prints). Write the placeholder bare: the value is
Markdown-escaped, and the callback URL and the external id arrive as code. Any other
`{{aurora.…}}` token is the package error `guide_unknown_placeholder`.

| Placeholder | Becomes |
|---|---|
| `{{aurora.callback_url}}` | the redirect URI to register at the vendor (`<external_url>/v1/oauth/callback`) |
| `{{aurora.external_id}}` | the organization's external id, for a member of the organization; else "(shown to members of your organization)" |
| `{{aurora.app_setup}}` | a link to the app's setup page at the reader's level; else who can open it |
| `{{aurora.page:oauth_clients}}` | a link to the OAuth clients tab at the highest level the reader can open (in a workspace, the app's setup page); else who can |
| `{{aurora.page:self_hosted}}` | the same, for Self-hosted apps |
| `{{aurora.page:packages}}` | the same, for Packages (Administration) |

Say where a person registers an OAuth client with `{{aurora.page:oauth_clients}}`, never as "Admin,
Apps, …", and add that a workspace owner can register a client for their own workspace on
`{{aurora.app_setup}}`. A connect method that needs a value in the other service declares
`show_context: [org_external_id]` (it must be in `permissions.context`); the dashboard shows it with
Copy above the method's fields.

An approval `title` completes the sentence "<Agent> wants to …": "create an issue", "upload a file to a
bucket", not "Linear mutation". Every write tool has one.

Link checking (`about.api_docs`, `about.api_spec`, `homepage`, `help_url`) needs the network, so it
is separate: `bash scripts/plugins/check-links.sh` runs in the non-blocking `plugin-links` workflow
(weekly and on demand), never in the blocking CI jobs.

## Fixtures and recorded HTTP

The runner is offline: a case is a request plus the connection data that Aurora would hold, and
the expectation is the matched app/tool, the injected upstream headers and URL, and the approval
summary. There are no live calls.

- Use obviously fake credentials such as `tok-fixture-1` (never a real token, never a copy of one
  with a few characters changed). Real-looking prefixes (`glpat-` followed by 20 or more
  characters, `ghp_`, `AKIA...`, `xox...`, JWTs, PEM blocks) fail the secrets guard.
- To base request bodies on reality, record against a throwaway account, then **scrub before
  saving**: replace tokens with fixture values, replace emails/usernames/ids with `example` ones,
  and delete `Set-Cookie` and `Authorization` headers. Keep only the request line, content type
  and the body fields the summary reads.
- Name negative cases `negative <tool_id>: <what is different>`.

## Self-managed and tenant hosts

A package may only claim hosts it declares. A product that can be self-hosted declares `instances` (label, optional `default_host` for the SaaS
host, optional `probe`) and uses the whole-host placeholder `{instance}` in `hosts`, tool hosts,
probe and OAuth URLs (D-74, `docs/09 §3.4`); GitLab is the reference. Fixtures set
`connection.instance_host` to register an instance for a case, and `expect: { refused: host_mismatch,
expected_host }` to prove the guard. Approval links use `{request.host}`.

## Signing and release

`plugins/fixtures/hook-example` is a test-only package in built-in shape with `token.mint` and
`approval.summarize` hooks. `check-builtin.sh` checks it with the real packages (it proves the hook
path end to end) and the image build compiles it; it is never shipped.

Release archives are built and signed by CI (`docs/ops/builtin-plugin-signing.md`). Dev and e2e
load the repository packages unsigned from the `plugins.dir` directory path, not the builtin one:
`bash scripts/plugins/build-builtin.sh --out <dir>` writes unsigned archives you can point
`plugins.dir` at with `plugins.allow_unsigned = true`. The builtin directory accepts only
project-signed archives and refuses everything else.
