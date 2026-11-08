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
  hooks.wasm             only when permissions.hooks is non-empty (built outside this tree)
```

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

A package may only claim hosts it declares. Packages ship the vendor's own hosted service with
plain host strings (GitLab: `gitlab.com`, written the same way in every host, tool host and OAuth
URL). Self-managed instances of products arrive with P2-INST (admin-registered app instances,
D-74), which will switch those strings to the `{instance}` placeholder; keep hosts in one
consistent plain form so that change stays mechanical.

## Signing and release

Release archives are built and signed by CI (`docs/ops/builtin-plugin-signing.md`). Dev and e2e
load the repository packages unsigned from the `plugins.dir` directory path, not the builtin one:
`bash scripts/plugins/build-builtin.sh --out <dir>` writes unsigned archives you can point
`plugins.dir` at with `plugins.allow_unsigned = true`. The builtin directory accepts only
project-signed archives and refuses everything else.
