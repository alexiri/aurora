# 25. Agent templates

An **agent template** is a ready-made agent that a person can create in one
step: instructions, skills, memories, automations, suggested apps and access
levels, all reviewed by an instance admin before anyone sees it. Templates are
kept in **any https git repository**, as source directories — one directory per
template — and each version is packaged by CI and published as an ordinary
release artifact.

This document is normative for publishers. It defines the repository format
(`aurora-templates.yaml`, C-1103), the template directory format (C-1141), the
reference CI (C-1144) and the `aurora template` CLI (C-1105). Aurora's
behaviour is specified in `03` (data), `05 §Instance administration` (the
admin surface) and the contracts C-1103..C-1112.

Owner references: `docs/tasks/P6-TEMPLATES.md` (design decisions D-435..D-458),
`02 §Limits` (`TEMPLATE_*`), `17` SEC-146 (a template never grants access),
`14 §Export and import an agent` (the export format a template extends),
`10 §Skills` (skill and file rules), `22-addon-repos.md` (the addon-repository
guide, whose shape this chapter follows).

## Principles

1. **An admin reviews every version.** A template version becomes available
   only when an instance admin approves it, by its canonical content hash.
   Approval is of exactly those bytes; a package whose content differs from the
   repository is refused (`source_mismatch`).
2. **A template never gives access.** The access levels in `template.yaml` are
   suggestions. Every grant is a person's choice at creation time, written
   through the same audited grant writer as any other connection (SEC-146).
   Levels are capped by the account's own limit.
3. **Nothing runs on the platform.** A template is data: markdown, YAML and
   text files. The only executable content is helper files and skill files,
   which are copied to the agent's sandbox and read by an admin before
   approval — which is why **every executable file must be plain UTF-8 text**.
4. **The package must be the source.** Aurora downloads the package CI
   published and compares it with the template directory at the tracked commit.
   CI cannot ship content the repository does not have.
5. **A published version is never replaced.** `1.2.0` of a template is
   immutable. Changing the template means raising the version; CI refuses to
   overwrite a published package, and Aurora never re-downloads a version it
   has accepted.
6. **A waiting version changes nothing.** Until the new version is approved,
   the previously approved version keeps working — for the gallery, for new
   agents and for agents made from the template. A failed check or a broken
   package never touches it.

## The repository

Any git repository readable over https: public, or private with a read-only
token that an instance admin stores when adding the source. No GitHub or
GitLab app, no connection, no account. A source is the address, an optional
branch or tag (`ref`; default the default branch) and a **folder** (`path`,
default the root).

### Layout

```
aurora-templates.yaml             # the repository manifest (required)
schedule-manager/                 # one directory per template, named by its id
  README.md                       # required: for the people deciding whether to deploy it
  template.yaml                   # the template's metadata (below)
  agent.yaml                      # suggested agent name and messaging posture
  instructions.md                 # the agent's brief
  memory/…                        # optional starting memories
  skills/what-is-on/SKILL.md      # the template's own skills
  helpers/…                       # optional helper files, copied byte for byte
  automations/…                   # optional schedules and webhooks
  CHANGELOG.md                    # optional; not part of the content; used as release notes
service-monitor/
  …
README.md                         # the repository's own README; ignored by Aurora
.github/workflows/aurora-templates.yml    # the reference CI (below)
```

Rules:

- **A template is every directory directly in the folder that contains
  `template.yaml`.** Everything else — the repository README, `docs/`, CI
  files — is ignored. At most 100 templates per source
  (`TEMPLATES_PER_SOURCE_MAX`).
- **No zip is ever kept in the repository.** Packages are CI artifacts,
  published at the URL the manifest names.
- Directory names are template ids: lowercase words joined by dashes
  (`[a-z0-9]+(-[a-z0-9]+)*`, at most 64 characters). A directory whose name is
  not the `id` in its `template.yaml` is refused (`template_dir_name`).

### `aurora-templates.yaml`

One strict YAML file at the top of the folder, at most 64 KiB
(`TEMPLATE_MANIFEST_MAX`). Unknown keys are refused, as are YAML anchors,
aliases, tags and multiple documents:

```yaml
# Integer. Aurora refuses values it does not understand: a newer api means
# "upgrade Aurora first".
api: 1

# The repository, shown to the reviewing admin.
repo:
  name: CERN agent templates     # 1..80 characters, required
  description: >-                # ≤ 280 characters, required
    Ready-made agents for experiments, meetings and services at CERN.
  homepage: https://aurora.cern/templates   # https URL, optional
  contact: mailto:aurora-admin@cern.example # https or mailto URL, optional

# Where packages are published. An https URL template with exactly the
# placeholders {id} and {version} (each at least once, nothing else in braces).
packages:
  url: https://github.com/cern/agent-templates/releases/download/{id}-v{version}/{id}-{version}.zip
```

`packages.url` renders to at most 500 characters, without user info, query or
fragment. Common shapes:

| Host | `packages.url` |
|------|----------------|
| GitHub releases | `https://github.com/<owner>/<repo>/releases/download/{id}-v{version}/{id}-{version}.zip` |
| GitLab generic packages | `https://gitlab.example.org/api/v4/projects/<numeric id>/packages/generic/{id}/{version}/{id}-{version}.zip` |
| Any static https host | `https://packages.example.org/aurora/{id}/{version}/{id}-{version}.zip` |

### Private repositories and the read-only token

An instance admin may store a **read-only token** with a source. Aurora uses
it for two things, and keeps it on the repository's origin only: reading the
repository (git), and downloading packages. What we found when checking the
hosts:

- **GitLab — including gitlab.cern.ch and self-hosted GitLab.** The generic
  package registry accepts the token as HTTP Basic authentication (any
  username, the token as the password) or as a `PRIVATE-TOKEN` header, for
  both public and private projects. A private GitLab repository therefore
  works end to end with `packages.url` pointing at the project's generic
  package registry. Use a project access token or personal access token with
  at least `read_api` (downloads) and `read_repository` (reading the repo).
- **GitHub.** Reading a private repository works with a fine-grained or
  classic token (`Contents: read-only`). **Release assets do not**: the
  `github.com/<owner>/<repo>/releases/download/…` URL is a website URL and
  does not accept token authentication; the API asset endpoint
  (`api.github.com/.../releases/assets/<numeric id>`) does accept it, but its
  URL cannot be written as a `{id}`/`{version}` template. So a private GitHub
  repository cannot serve template packages from release downloads — keep the
  repository public, or publish packages to GitLab or a static https host
  while keeping the sources on GitHub.
- **Static hosts.** Any https host that serves the same bytes works, as long
  as Aurora can fetch the URL with the token as Basic auth (or without any
  token for a public host). The token never follows a redirect to another
  origin.

## A template directory

A template is the agent export's layout (D-413) plus `template.yaml` and
`helpers/`. The same files can be read from the repository (a directory tree
with git modes) or from a packed zip — `aurora template pack` writes one, and
so does the dashboard's *Save as template*. Both forms must produce identical
content; Aurora checks that the package matches the source.

An annotated `service-monitor/`:

```
service-monitor/
  README.md            # required, ≤ 20 000 characters — see "The README first"
  template.yaml        # required — the template's metadata, below
  agent.yaml           # required — format: 1; name; may_message_first
  instructions.md      # required (may be empty), ≤ 20 000 characters
  memory/
    on-call-notes.md   # optional starting memory; front matter key + optional locked
  skills/
    triage-alert/
      SKILL.md         # front matter name + use-it-when; then the body
      severity.py      # the skill's own files
  helpers/
    alert-summary.sh   # copied byte for byte; projected read-only at .aurora/helpers/
  automations/
    morning-check.yaml # a schedule (or a webhook)
  avatar.png           # optional: at most one avatar (png, jpg or webp)
  CHANGELOG.md         # optional: release notes for CI; not part of the content
```

Size caps: a packed template is at most 5 MiB; unpacked at most 8 MiB and 200
files; per-file and per-kind caps are in `02 §Limits`. Names are relative and
`/`-separated, `[A-Za-z0-9._/-]`, at most 6 segments below the root; `..`,
hidden segments, symlinks and submodules are refused. Files outside the
expected names are refused (`template_file_unexpected`), except
`CHANGELOG.md`, which is allowed but not part of the content.

### The README first

`README.md` is **required** (`template_readme_missing` when it is missing or
empty). It is what the admin reads when reviewing, and what people on the
workspace read when deciding whether to create the agent — write it for them,
not for Aurora. It is at most 20 000 characters of Markdown, is shown before
any input is filled (so **no placeholders**), and gets the text hygiene
cleanup (control characters, bidirectional controls and zero-width characters
removed). Relative links and images are not resolved: the README stands
alone.

A recommended outline:

- **What it does** — and what it deliberately does not do.
- **Who it's for** — which team or kind of person.
- **What it needs** — apps and their access levels, accounts, skill libraries.
- **What it does on its own** — the schedules and webhooks it will create.
- **Adapting it** — what to change afterwards: memories, instructions, skills,
  schedules. (Updates from the template are offers; the person applies them.)

### `template.yaml`

Strict YAML: unknown keys are refused. Field by field:

```yaml
format: 1                     # integer; any other value: template_format_unsupported
id: service-monitor           # [a-z0-9]+(-[a-z0-9]+)*, ≤ 64; equals the directory name
version: 1.0.0                # semver, no build metadata
name: Service monitor         # 1..80 characters
summary: Watches dashboards and opens tickets.   # 1..280 characters
min_core: 6.0.0               # optional: the oldest Aurora release this version needs
effort: medium                # optional: low | medium | high | max (the agent's effort)
inputs:                       # at most 10
  - id: jira_project          # [a-z][a-z0-9_]{0,31}, unique
    label: Jira project key   # 1..80 characters
    help: The key in issue numbers, like OPS in OPS-123.   # optional, ≤ 200
    kind: text                # text (default) | choice
    required: true            # default true
    pattern: "^[A-Z][A-Z0-9]+$"   # text only, optional, ≤ 200 characters; Rust regex,
                                  # compiled with a 64 KiB size limit, whole-value match
    max_length: 20            # text only, 1..200, default 100
    options: [Low, High]      # choice only: 1..20 distinct options, each 1..80
    default: OPS              # optional; must satisfy the rules above
apps:                         # at most 10; each names the app and a suggested level
  - app: grafana              # exactly one of app / any_of
    required: true            # default true
    level: read               # read (default) | ask | act
    why: Reads the dashboards in the folder you choose.   # 1..200 characters
  - app: atlassian
    required: true
    level: ask
    tools:                    # optional, only with `app`: a custom split
      allow: [search_issues, get_issue]
      ask: [create_issue, add_issue_comment]
    why: Reads issues and files new ones after your approval.
  - any_of: [microsoft-365, caldav, google-workspace]   # 2..5 distinct app ids
    required: false
    level: ask
    why: Books meetings on your calendar.
skills:                       # skill references, at most 20 — see below
  - library: https://gitlab.cern.ch/agents/skills.git
    skill: incident-writeup
    ref: main                 # optional
    path: incident-writeup    # optional
    required: false
    why: Writes the incident summary the way the service desk wants it.
```

Notes:

- An app id is syntax-checked only (`[a-z0-9]+(-[a-z0-9]+)*`). Whether the
  installation actually has the app is decided when the template is offered
  and used: an app nobody has connected is shown as "Needs <App>, which isn't
  available here". An app id may appear in at most one entry. `tools` needs at
  least one tool in `allow ∪ ask`, and no tool may be in both.
- `any_of` means "one of these apps": the person picks the one they use at
  creation. The first installed app is preselected.
- `min_core` above the running Aurora marks the version "needs a newer
  Aurora"; it is not offered until the deployment upgrades.

### Placeholders and inputs

`{input.<id>}`, `{agent.name}` and `{person.name}` are filled when a person
creates the agent — in `instructions.md`, memory texts, a skill's
`use-it-when` and body, schedule prompts and webhook instructions, and
**nowhere else** (not in names, keys, cron expressions, other skill files,
helpers or `template.yaml`). An optional input left empty becomes the empty
string.

Substitution is **one pass**: an inserted value is never scanned again, so a
value that itself contains `{input.x}` stays exactly as typed. Any other text
in braces is kept literally. `{agent.name}` is the name the person chose;
`{person.name}` is their display name.

### Skills

A template can carry three kinds of skills, and a skill name may be used only
once across all of them:

- **Its own skills**, `skills/<name>/SKILL.md` plus that skill's other files
  (at most 25 skills): front matter `name` (equal to the directory name, the
  skill-name rules of `10 §Skills`, never `aurora-*`) and `use-it-when` (one
  line — this becomes the skill's description, so make it a clear sentence);
  then the body. The library's file rules apply: 64 KiB per file, at most 50
  files per skill, depth ≤ 4.
- **Library references** (`library:` in `template.yaml`): a git address (the
  same rules as the skill library's — a page URL is accepted and normalized),
  a skill name, optional `ref` and `path`. The skill is added through the
  library's ordinary add path: pinned to a commit, read-only, updated by its
  library, removable. **A template never creates or edits a library.**
- **Organization references** (`organization: <name>`): a skill expected to
  exist at organization or workspace tier. Nothing is ever written — the
  agent simply inherits what the workspace already has.

There is **no `core` kind** in format 1: Aurora ships no built-in skills
beside app skills, and app skills need no reference at all — they reach every
agent that has access to the app, so the `apps:` entry is enough.

### Helper files

`helpers/<path>` (optional; at most 50 files, 256 KiB each, 2 MiB together,
depth ≤ 4) are copied **byte for byte** — never placeholder-filled — and
projected read-only into the agent's sandbox at `.aurora/helpers/<path>`.
Agent tools cannot write them; people can remove them; updates offer changed
helpers as one part.

**When to put a script where:** a file only one skill uses, and that belongs
with its skill's documentation, goes in the skill's directory
(`skills/<name>/…`); a script several skills use, or one that is part of the
agent's machinery rather than one skill's, goes in `helpers/`. Helpers are
visible at one stable place (`.aurora/helpers/`) for every skill and for the
person.

**The executable-text rule:** a file with the executable bit — in `helpers/`
or in a skill — must be valid UTF-8 without a NUL byte, so the admin can read
everything that will run. Every **helper file** must be text, executable or
not (owner decision 2026-11-24): the home sync carries UTF-8 text only, so a
non-text helper would be stored but silently never installed. A skill's
non-executable data files may be binary.

### What is refused, and why

- **One-time schedules** (`cadence: { preset: once, … }`): a template creates
  recurring schedules only. A one-off would run at a fixed wall-clock time
  chosen by someone who is not the person creating the agent.
- **Named models** (`model:` other than `default` in `agent.yaml`): model
  names differ between installations; the template would break elsewhere.
- **Policy rules, secrets, connections, tokens**: access is a person's
  decision at creation, never a template's.
- **Binary executables**: see the executable-text rule.
- **Anything outside the file set** (`template_file_unexpected`): the admin
  reviews everything an agent would be told or given; unknown files would be
  outside that review.

## Publishing a version

The release flow is deliberately boring: **raise `version` in
`<id>/template.yaml`, open a pull request (CI checks), merge (CI publishes).**
Aurora watches the repository's head, not the packages, so a version only
exists once its source is on the tracked branch.

### GitHub Actions: `.github/workflows/aurora-templates.yml`

Two jobs:

- **`check`** — runs on every pull request and every push, with
  `permissions: contents: read` and no secret: checkout; install the `aurora`
  CLI at a pinned version; `aurora template check .`.
- **`publish`** — runs only on push to the default branch, `needs: check`,
  with `permissions: contents: write` and
  `concurrency: { group: aurora-templates-publish, cancel-in-progress: false }`
  (two merges racing must not race in the publish job). For each
  `{ id, version, dir }` of `aurora template list . --json`:
  1. Pack: `aurora template pack <dir> -o dist/<id>-<version>.zip`.
  2. If the release `<id>-v<version>` already exists, download that release's
     `<id>-<version>.zip` and compare with `cmp`. When they differ the job
     **fails** with: `<id> changed without a new version: raise version in <dir>/template.yaml`.
  3. Otherwise create the release:
     `gh release create <id>-v<version> dist/<id>-<version>.zip --target "$GITHUB_SHA" --title "<name> <version>"`
     with `--notes-file <dir>/CHANGELOG.md` when that file exists.
  4. It never uses `--clobber` and never deletes a release.

The workflow as a whole lives in `templates/examples/.github/workflows/aurora-templates.yml`,
and its shell logic in `templates/examples/.github/aurora-publish.sh`.

### GitLab CI

The same two jobs in `.gitlab-ci.yml`; the publish job uploads to the
**generic package registry** with the job token
(`PUT …/projects/<id>/packages/generic/<id>/<version>/<id>-<version>.zip`)
after the same compare against an existing package. The project setting
"Allow duplicate generic packages" **off** is the no-overwrite rule.
`packages.url` is the matching download URL (see the table above).

### What makes `publish` fail, and how to fix it

| Failure | Meaning | Fix |
|---------|---------|-----|
| `check` fails | The template has problems | `aurora template check .` locally prints every problem with a path |
| "`<id>` changed without a new version" | The packed bytes differ from a published release of the same version | Raise `version` in `<id>/template.yaml`; never edit a published version |
| `gh release create` fails with "already exists" | The release exists but the packed file was not equal (should not happen after 2) — or two templates share an id | Check the ids; a release is never deleted or replaced by CI |
| `pack` refuses | The directory is not a valid template | Same as `check` |

### Any other CI

Any CI works if it publishes, at the rendered `packages.url`, exactly the
bytes that `aurora template pack` writes, and never replaces them. Aurora
checks every package against the repository, so a CI that packs something
else only makes that version `invalid` (`source_mismatch`) — it can never
ship content that differs from the repository.

## What Aurora does

An instance admin adds the repository as a **template source** (address,
optional ref, folder, optional read-only token, check interval). Then, per
check (default daily, minimum every 6 hours):

1. Aurora reads the head of the tracked ref. If nothing changed since the last
   check and no version is waiting for its package, it stops.
2. It reads `aurora-templates.yaml` and every `<dir>/template.yaml`: new
   templates are listed, broken ones are flagged for the admin, and a head
   `version` newer than every stored version becomes a **candidate**.
3. For each candidate it downloads the package from the rendered
   `packages.url` (https, at most 3 redirects, streamed under the size cap,
   30 s timeout). A missing or failing package is retried for up to 24 hours
   (first retry after about 15 minutes) and then the version is marked
   invalid for good.
4. A downloaded package is unpacked, validated, and compared with the source
   tree at that commit: both must have the same `content_sha256`. On a match
   the version goes to **pending approval** with its full parsed content.
5. An admin reviews — everything the agent would be told or given, in full —
   and approves or rejects. Only an approved version is offered.

The previous approved version keeps working throughout: the gallery, creation
and existing agents are untouched by a waiting, rejected or invalid version.

## What an admin sees, and how updates reach agents

- **Review**: the README, instructions, inputs, apps with their levels and
  tools, skills (own bodies and references), helpers with the executable
  bits, memories (locked ones marked), automations and the avatar. Everything
  as data, never summarized.
- **Updates are parts, and nothing is automatic.** When a newer version is
  approved, agents made from the template are shown a diff as *parts*:
  instructions, each skill, helpers, automations. The person chooses which
  parts to apply. **Memories are never offered again** (they are the agent's
  own after creation), and apps a new version adds are listed with a link to
  give access.
- **Library skills update themselves**: a referenced library skill is pinned
  to a commit but refreshed and proposed by its library, like any library
  skill. Its content is not part of the template's approval — it is trusted
  the way every library skill is.
- A source removed, or a template deleted upstream, never changes an existing
  agent: agents are copies, and their provenance keeps the template's id,
  name and version as text.

## Save as template

The easy way to start a template: on an agent you like, use the dashboard's
**Save as template**. It exports the agent in exactly this format — a zip
rooted at `<id>/` with a `template.yaml` added. Unzip it into your repository
as `<id>/`, adjust `template.yaml` (id, version, inputs, apps), write the
README, and push. The CI's `check` then tells you what is missing.

The dialog asks what the file is for before it saves: the name, the id (it
follows the name until edited; it is also the zip's root directory), the
version (a fresh template starts at 1.0.0), a one-line summary and the
README's **What it's for** text from an outline, plus one **Why it's needed**
line per app. Library skills are written as references to their library,
never copies; memories are unticked by default. **Save** refuses one problem
per sentence and only downloads `<id>-<version>.zip` — nothing is published.
To publish: unzip the file into your repository folder as `<id>/`, raise
nothing (the version is already 1.0.0), open a pull request and merge; the
CI's `check` then tells you what is missing.

## The CLI

`aurora template` needs no server and no login:

```
aurora template check <path>       # a template directory, a .zip, or a repository folder
aurora template pack <dir> [-o <file>]   # validate + write the deterministic zip
aurora template list <folder> [--json]   # what CI would publish
```

`check` accepts a template directory, a packed zip, or a **repository folder**
(a directory containing `aurora-templates.yaml` — for a repository it checks
the manifest, every template directory and that `packages.url` renders for
each). Problems print as `<path>: <code>: <message>`, one per line, and the
exit code is 1 when there are any. `pack` writes `<id>-<version>.zip` by
default and prints both the archive and the content hash — the content hash is
what an admin approves, and CI compares a fresh pack with the published
package byte for byte.
