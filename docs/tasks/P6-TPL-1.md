# P6-TPL-1: Agent templates — the format, the repository format and its CI, the `aurora-template` crate, `aurora template check/pack/list`, the publisher guide and the example repository (owner, 2026-11-21 and 2026-11-22, phase 6, D-435, D-436, D-456, D-457, C-1103..C-1105, C-1113, C-1141, C-1142, C-1144, SEC-146)

## Why
The owner wants instance admins to add template repositories, from which people create ready-made agents
(`docs/tasks/P6-TEMPLATES.md`). Templates are kept in **any https git repository as source directories**, one per
template. CI packages each template and publishes the package as a release artifact. Aurora downloads the package and
checks it against the source (D-456, D-457).

This brief builds the pure part every other brief uses: reading and validating a template from a zip or from a tree of
files, the repository manifest, skill references, filling inputs, hashing and packing. It also standardizes the
repository and its CI, gives publishers their guide and a CLI, and writes an example repository used as a fixture.

Read first:
- `P6-TEMPLATES.md` (all of it);
- `API_CONTRACTS` **C-1141, C-1103, C-1104, C-1105, C-1142, C-1144** (all of them) and C-1113's limits;
- `17` SEC-146;
- `14 §Export and import an agent` and `web/src/lib/export/agent-export.ts`: the export format a template extends.
  Your reader must accept any zip the web export writes once it is rooted at `<id>/` and has a `template.yaml`;
- `22-addon-repos.md`: the tone and shape of a publisher guide, and its §Publishing a release workflow, the precedent
  for C-1144;
- `10 §Skills` and `§Git skill library`: skill and file rules, text cleaning;
- C-1123: the address parser in `aurora-core`, which skill references use.

## Scope

### 1. New crate `crates/aurora-template` (pure: no IO, no async, no database)
This is the single implementation of C-1141, C-1103 (parsing), C-1104 and C-1142 (parsing). The API (P6-TPL-2/3), the
CLI and P6-TPL-5's test must not re-implement any of it. It may depend on `aurora-core`, for the limits and the C-1123
address parser.
- **Model:**
  - `TemplateContent`: meta from `template.yaml`; agent fields; instructions; memories (each with `locked`: the
    optional front-matter boolean, default false, any other value `template_field` at `memory/<key>.md#/locked`;
    part of `content_sha256`; C-1151, D-463(d)); skills with files and executable bits; skill references; helpers;
    automations; avatar bytes. Add a locked memory to the example repository and the `25` format section.
  - `TemplateMeta`, `InputDecl`, `AppEntry` (`App { app, required, level, tools?, why }` or
    `AnyOf { any_of, required, level, why }`), `SkillRef` (`Library { url, ref, path, skill, required, why }` with the
    address normalized by C-1123, or `Organization { name, required, why }`).
  - `TemplateManifest`, `TemplateProblem { code, path, message }`.
  - `serde` with `deny_unknown_fields` wherever the contract says strict.
- **`read_archive(&[u8])`:** a small zip reader (end of central directory, central directory entries, local headers;
  stored and deflate through `flate2`, already in the workspace).
  - Enforce every archive rule of C-1141 that can refuse early, before inflating anything: count, names, the one root
    directory, symlink mode, declared sizes.
  - Enforce the unpacked caps and the 100× ratio while inflating. Never trust declared sizes alone.
  - Then hand the files to the same function `read_tree` uses.
- **`read_tree(root_name, files)`:** the same checks on `(path, bytes, git mode)` entries. Mode `100755` is
  executable, `100644` is not, and `120000`, `160000` or anything else is refused. Both entry points end in **one**
  validation function, so they cannot disagree. A test builds the same files both ways and asserts equal
  `TemplateContent` and equal `content_sha256`.
- **Validation:**
  - the file rules, `template.yaml` and `agent.yaml`, front matter, automations, skill references (C-1142's format
    rules only; resolution is P6-TPL-2's), helpers;
  - **the executable-text rule** (an executable file must be UTF-8 without NUL);
  - placeholders and text cleaning;
  - return **all** problems (at most 20), each with a C-1141 code and a path.
  YAML: use `serde_yaml` (already in the workspace), and refuse anchors, aliases, tags and multiple documents
  explicitly. Check how `aurora-registry` guards its manifests and reuse that if it exists.
- **`parse_meta(&[u8])`:** `template.yaml` alone, with the same strict rules (the ingest lists a repository with it).
- **`parse_manifest(&[u8])` and `render_package_url`:** C-1103. `packages.url` has exactly `{id}` and `{version}`,
  and renders to an https URL without user info, query or fragment, at most 500 characters.
- **`check_inputs` and `render`:** C-1104. One pass over the text: find `{input.<id>}`, `{agent.name}` and
  `{person.name}`, substitute them, and never rescan inserted text. Compile `pattern` with the `regex` crate with
  `size_limit(64 * 1024)`, and use it as a whole-value match.
- **The README** (D-458): `README.md` is required (`template_readme_missing` when it is absent or empty after
  cleaning). It is at most `TEMPLATE_README_MAX`, cleaned like other text, never placeholder-filled, and kept in
  `TemplateContent.readme`.
- **`content_sha256`** (canonical serialization, C-1104; the template's `README.md` included, `CHANGELOG.md`
  excluded) and the **part
  hashes** `instructions_hash`, `skill_hash`, `helpers_hash` and `skill_ref_key`. Pin all of them with golden tests:
  changing the canonical form later would make every stored hash wrong.
- **`pack(root_name, files)`:** a deterministic zip (sorted names, the fixed DOS timestamp 1980-01-01, stored entries,
  the executable bit in the external attributes). It must be readable by `read_archive` and by the web's
  `lib/export/zip.ts` reader.
- **Constants:** add the `TEMPLATE_*` constants of C-1113 to `crates/aurora-core/src/limits.rs` and use them from
  there. Add their rows to `02 §Limits` (the `limits-readers` guard needs them). Add entries in
  `scripts/guards/limits-gaps.allow` for constants that only P6-TPL-2 or P6-TPL-6 read.

### 2. CLI (`services/aurora-cli`)
`aurora template check <path>`, `aurora template pack <dir> [-o <file>]` and
`aurora template list <repository folder> [--json]`, exactly as C-1105.
- `check` tells the three inputs apart. A directory with `aurora-templates.yaml` is a repository folder. A directory
  with `template.yaml` is a template. A file is a zip.
- A directory is read through `read_tree`, with the modes taken from the file system's executable bit.
- No server and no login are needed.
- Add the subcommands to the CLI's help and to `docs/25`.

### 3. Publisher guide `docs/25-agent-templates.md`
It is normative for publishers, in the style of `22-addon-repos.md`. It covers:
- **Principles.** An admin reviews every version. A template never gives access. Nothing runs on the platform. The
  package must be the source. A published version is never replaced.
- **The repository** (C-1103): the folder layout with an annotated tree; `aurora-templates.yaml` field by field;
  `packages.url` for GitHub releases, GitLab generic packages and a static host; private repositories and the
  read-only token (which hosts accept it for package downloads: check GitHub and GitLab, gitlab.cern.ch included, and
  write down what you found).
- **A template directory** (C-1141), with an annotated example. **The README first** (D-458): it is required, and
  people read it to decide whether to deploy the agent. Give a recommended outline (What it does, Who it's for, What
  it needs, What it does on its own, Adapting it), and say how it is rendered (no HTML, no images, only https and
  mailto links). Then `template.yaml` field by field; placeholders and
  inputs; apps and `any_of`; **skills**, meaning its own skills, library references and organization references
  (C-1142), and why there is no `core` kind; **helper files**, meaning when to put a script in a skill and when in
  `helpers/`, where they appear in the agent's computer, and the executable-text rule; what is refused and why
  (one-time schedules, named models, policy, secrets, binary executables).
- **Publishing a version** (C-1144). The release flow is: raise `version`, open a pull request (`check` runs), merge
  (`publish` packs and creates the release). The full GitHub Actions workflow, the GitLab CI sketch, what makes
  `publish` fail, and how to fix it.
- **What Aurora does** (C-1108): it reads the head, waits for the package (15 min retries, 24 h), compares it with the
  source, and puts the version up for the admin's review. The previous version keeps working meanwhile.
- **What an admin sees, and how updates reach agents**: parts, diffs, nothing automatic, and library skills updated
  by their library.
- **Save as template** as the easy way to start (P6-TPL-5): unzip the file into the repository and push.

Link it from `00-overview.md`'s reading list and from `MASTER_SPEC.md`'s reading order, the way `22` is linked.

### 4. The example repository `templates/examples/`
It is a complete repository folder, exactly as a publisher would keep it:
- `aurora-templates.yaml`, with a GitHub-release `packages.url` for a placeholder `example/agent-templates`
  repository;
- `README.md`;
- `.github/workflows/aurora-templates.yml`, the C-1144 workflow as is (it lives under `templates/examples/`, so it
  never runs in this repository);
- `.gitlab-ci.yml`, the sketch;
- `schedule-manager/` and `service-monitor/`, as source directories with the contents of `P6-TEMPLATES.md`
  §Examples: a real `README.md` each, following the guide's outline and written for someone deciding whether to
  deploy it; real, useful instructions in plain second person; two or three skills each, with clear `use-it-when`
  lines; one small script in a skill; one helper in `service-monitor/helpers/`; one optional library reference
  (`required: false`); one schedule each; inputs.
- **No zip files are checked in.**
- **Use real app ids and tool ids** from `plugins/builtin/*/plugin.yaml`. Atlassian's tool ids, for example, look like
  `list_accessible_sites`.

A test runs `check` on the folder (no problems) and packs each template twice (byte-identical). Another test runs
the workflow's `publish` shell logic, extracted into a script the workflow calls, `templates/examples/.github/aurora-publish.sh`.
It runs against a fixture with a fake `gh` on `PATH`: one template already released and unchanged (nothing
happens), one released and changed (the job fails with the C-1144 sentence), and one new (`gh release create` is
called with the right tag, asset and notes). Keep the script POSIX `sh` plus `jq`, so publishers can read it.

## Not in this brief
Reading repositories, downloads, storage, routes, approval and the poll job (P6-TPL-2); helper projection
(P6-TPL-6); creating agents and updates (P6-TPL-3); any web code (P6-TPL-4/5).

## Acceptance criteria
- [ ] Crate unit tests:
  - a template without `README.md`, or with an empty one, is `template_readme_missing`; a README change changes
    `content_sha256`, and a `CHANGELOG.md` change does not;
  - every C-1141 code has at least one failing input, built in the test with `pack`, as a tree, or by hand for the
    malformed zip cases;
  - a zip bomb (high ratio, and declared sizes that lie) is refused without allocating the unpacked size;
  - absolute, `..`, hidden, backslash, symlink, submodule and too-deep names are refused, in both forms;
  - a second top-level directory, or a root that is not the id, is refused;
  - an executable binary is refused, and a non-executable binary skill file is accepted (a non-text
    helper file is refused at every layer: every helper file must be UTF-8 text);
  - a zip written by the web export (`exportZip`, store-only), rooted at `<id>/` with a `template.yaml`, is read;
  - a deflated zip (as `zip -r` writes) is read;
  - `read_tree` and `read_archive` agree.
- [ ] Placeholders: an undeclared input is refused; `{agent.name}`, `{person.name}` and an optional empty input work.
  **Hostile values** are inserted literally once and never expanded: `{input.other}`, `{person.name}`, braces,
  newlines (refused by `check_inputs`), bidi and zero-width characters, very long values. Text outside the listed
  files, helpers included, keeps its braces.
- [ ] Inputs: each rule (`missing`, `unknown`, `too_long`, `pattern`, `not_an_option`, `control_characters`). A
  pattern over the size limit is a `template_field` problem, not a panic.
- [ ] Skill references: both kinds; a page URL normalized with its ref and folder; an `http:` or ssh address refused;
  a name clash with `skills/`; `core:` refused; more than 20.
- [ ] Golden tests for `content_sha256`, `instructions_hash`, `skill_hash`, `helpers_hash` and `skill_ref_key`.
  `pack` is byte-identical across runs.
- [ ] Manifest: a valid example; an unknown key; `api: 2`; `packages.url` without `{version}`, with another
  placeholder, with `http:`, or with a query; rendering for both example hosts.
- [ ] CLI: `check` on the example folder prints one `ok …` per template; on a broken template it prints every problem
  and exits 1; `list --json` matches; `pack` writes the same bytes twice.
- [ ] The publish-script test (three cases above).
- [ ] `cargo fmt --check`, clippy (workspace lints) for the touched crates,
  `cargo test -p aurora-template -p aurora-cli`, the script test, and `bash scripts/guards/run-all.sh`, all through
  `heavy.sh`.

## Ownership and sequencing
- **Owns:** `crates/aurora-template/**` (new; the workspace `Cargo.toml` members line is the only edit there);
  `crates/aurora-core/src/limits.rs` (the `TEMPLATE_*` constants only); `services/aurora-cli` (the `template`
  subcommand); `docs/25-agent-templates.md`; `templates/examples/**`; the `02 §Limits` rows; the reading-list links.
- This is the first P6-TEMPLATES brief. It starts after P6-MEMLOCK Part A is merged (D-463(e): locked memories land
  just before templates). P6-TPL-2, P6-TPL-5 and P6-TPL-6 start after it merges.
- `Cargo.lock` changes only by the new member. No new external crate: ask before adding one.

## Numbers
D-435, D-436, D-456, D-457, D-458, D-463(d), C-1103..C-1105, C-1113 (constants), C-1141, C-1142 (format), C-1144,
C-1151 (format), SEC-146. No migration.

## Suggested agent/model and reason
`rust-developer` (Sonnet). It is a self-contained parser and validator with adversarial unit tests; zip handling and
keeping the two read paths identical are the risky parts. It also includes a small CLI, a shell script with a test,
and a document.
