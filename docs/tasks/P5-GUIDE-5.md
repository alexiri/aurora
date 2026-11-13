# P5-GUIDE-5: `aurora plugin import-openapi` — reference files and a catalogue skeleton from an OpenAPI document (owner, 2026-11-13)

Fifth task of "app guides on demand". Normative spec: `docs/24-app-guides.md` §10; contract
**C-650**; decision D-172(a). Depends on **P5-GUIDE-1**. CLI only. **Not** ordered before the
addon tasks: it is needed before the first large-API app (Zoom, OpenStack, OpenSearch, or any
third-party addon with hundreds of operations).

## Why

A large API cannot be packaged by hand: hundreds of catalogue tools, and an agent that needs to
find one operation among them. This command turns the vendor's OpenAPI document into (a) the
agent's searchable reference and (b) a first draft of the permission catalogue. It is the
"search" half of Code Mode for Aurora; the "execute" half is the agent's sandbox (D-170, D-172).

## Scope

`aurora plugin import-openapi <file> --app <id> [--host <host>] [--dir <package dir>]`

1. **Input**: a local OpenAPI 3.0 or 3.1 document, JSON or YAML. No network access of any kind.
   Local `$ref`s (`#/components/...`) are resolved; an external `$ref` is an error naming it.
   Swagger 2.0 is refused with a message naming a converter step for the author.
2. **`agent/<app_id>/operations.md`**: per tag (operations without a tag under `other`), one
   line per operation: `- METHOD path: summary [operationId]`. This is the file an agent greps.
3. **`agent/<app_id>/ops-<tag>.md`** per tag (tag lower-cased and reduced to the file grammar,
   collisions numbered): per operation a heading `METHOD path`, the summary, parameters as
   `name (in, required|optional, type)` with the description clamped to 160 characters, and the
   top-level fields of a JSON request body the same way. No response schemas.
4. **Manifest help**: print the `guide:` entries for the files written (file, generated title)
   so the author pastes them; do not edit `plugin.yaml`.
5. **Catalogue skeleton** on stdout: a `tools:` block: `read.items` = `GET` / `HEAD`
   operations, `write.items` = every other method; `id` from `operationId` (lower snake case,
   clamped to `[a-z][a-z0-9_-]{1,47}`, de-duplicated with a numeric suffix), else from method and
   path; `name` = the summary clamped; `host` = `--host`, else the first `servers[].url` host,
   else a `TODO` marker that fails `validate`; `{param}` segments → `*`. Within each group,
   more specific paths first (more literal segments first, then longer), which is the order the
   first-match rule of `09 §3.4 tools` needs; the author still reviews overlaps and the linter
   stays the authority.
6. **Limits**: when the files would exceed `AGENT_GUIDE_APP_MAX` or `AGENT_GUIDE_FILES_MAX`,
   drop in this order and report each step on stderr: body fields, parameter descriptions, the
   `ops-<tag>.md` files of the largest tags (their operations stay in `operations.md`); merge the
   smallest tags into `ops-other.md` when there are more tags than files allowed.
7. **Determinism**: sorted by tag, path, method; no timestamps; byte-identical across runs and
   platforms, so an addon repository's CI can regenerate and fail on drift.
8. All text taken from the document is untrusted: one-line sanitizer, clamps, no raw HTML passed
   through (strip tags from descriptions).

## Out of scope

Fetching specs, authentication schemes (the author writes `connect`, `hosts`, `token`),
approvals templates, `params` classifiers, keeping a generated catalogue in sync after hand
edits, other description formats (RAML, Postman, WSDL, GraphQL SDL).

## Ownership

New `services/aurora-cli/src/plugin/openapi.rs` and its wiring in `cmds.rs` / `mod.rs`; test
documents under `services/aurora-cli/tests/openapi/`. A YAML/JSON parser already in the
workspace is used; a new dependency needs the main session's agreement first (no OpenAPI
framework crate: the subset read here is small).

## Acceptance criteria

- [ ] Three checked-in documents: a small 3.0 JSON, a 3.1 YAML with tags and `$ref` parameters,
      and one large enough to exceed `AGENT_GUIDE_APP_MAX`. For each: golden output directory,
      two runs byte-identical, the output passes `aurora plugin validate` once pasted into a
      scaffolded package (test does the paste), within every limit.
- [ ] The third document prints the drop report and still exits 0; an external `$ref`, Swagger
      2.0 and an unreadable file exit non-zero with a one-line reason.
- [ ] Skeleton order: a test document with overlapping paths (`/a/{id}` and `/a/search`) yields
      the literal path first, and the linter's shadowing proof accepts the result.
- [ ] A document with hostile strings (HTML, backticks, control characters, very long
      summaries) produces files that pass `agent_guide_not_text` and stay within their lines.
- [ ] fmt, clippy `-D warnings`, the CLI test suite. Report exact commands and results.

## Notes for the implementer

- Change files only with the Edit/Write tools. No `Co-Authored-By` lines in commits.
- Suggested agent: `rust-developer` (Sonnet). Size M.
