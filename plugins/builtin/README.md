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
| scopes | OAuth `default_scopes` are all `access: read`; every scope declares `access`; read tools only use GET/HEAD (or `git: fetch`, graphql `query`). |
| hooks | the archive holds `hooks.wasm` exactly when `permissions.hooks` is non-empty; for every declared point a passing case where the hook succeeds and (except `approval.summarize`) one where it fails; every `permissions.egress` host is reached by a recorded request; one case per egress point shows a host outside the list denied (`egress_override` + `expect: { error: egress-denied }`); a hook case for an undeclared point fails. Hook cases never count as skipped. |
| token | when the package declares `token`, it has `reauth_codes` (oauth2_refresh) and a fixture shows the injected bearer. The offline runner cannot replay token endpoint responses; the refresh mechanism itself is tested in the gateway. |
| docs | `aurora plugin docs` renders (md and html) twice with identical output and contains the permissions table. Rendered output is regenerated, not committed. |
| secrets | `scripts/guards/no-live-tokens.sh` (part of `scripts/guards/run-all.sh`) fails on anything token-shaped under `plugins/builtin/**/tests`. |

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
