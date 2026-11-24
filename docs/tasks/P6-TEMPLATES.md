# P6-TEMPLATES: Agent templates — ready-made agents from template repositories (owner, 2026-11-21 and 2026-11-22, phase 6, D-435, D-436, D-456..D-458, C-1103..C-1119, C-1141..C-1144, SEC-146)

> **Status: decided (2026-11-21, amended 2026-11-22).** The owner proposed the feature, reviewed the sketch and
> answered every question (D-435). On 2026-11-22 the owner changed how templates are distributed (D-456): any https
> git repository, templates kept as source directories, CI jobs that package them, skills referenced from elsewhere,
> helper scripts, and an older version that keeps working while a newer one waits. The coordinator's design for that
> is D-457. The owner then asked for a description that people read before deploying: a required `README.md` per
> template (D-458). The contracts are C-1103..C-1119 and C-1141..C-1144; C-1141 replaces the branch's first contract, which
> collided with master's C-1102. The invariant is SEC-146. Three migrations take the next free numbers when they are
> placed. This file is the design; where it and the contracts differ, the contracts win.
>
> Implementation briefs: **P6-TPL-1** (format, crate, CLI, the repository format and its CI, publisher guide,
> example repository), **-2** (sources, ingest, approval, catalogue API), **-6** (helper files: storage, projection,
> routes), **-3** (create from a template, provenance, updates), **-4** (dashboard), **-5** (Save as template).
> Phase 6 starts on the owner's word; these briefs wait for it like the rest of phase 6.

## Goal

Owner, 2026-11-21: "agent templates. Instance owner could add an external GitHub repo with various precooked
configuration options for a new agent, including prompts, sets of tools, skills, etc. A workspace owners could create a
new agent from one of these templates, fill in secrets for the preselected tools and be good to go. … common agents that
people could instantiate for themselves, like a schedule manager (indico+calendar), a service monitoring agent
(Jira+grafana+gitlab), etc. Once the template is deployed, they could tweak it to their liking, but it saves them the
initial gruntwork."

Owner, 2026-11-22 (D-456):
- templates come from any (https) git repo, not only github ones through the app;
- the remote repository itself does not contain zip files, it contains the unzipped sources, one subdirectory per
  agent (with further subdirs as needed). The repository may contain CI jobs to package each agent into a zip file
  that is added to the repository artifacts, which are then used for the import;
- define and standardize the remote repository format too, including the CI jobs (based on GitHub Actions as an
  example);
- templates may include fully specified skills, but they may also include references to a skill defined in another
  place (the name of a skill from a skill library, a core skill, etc.): those do not need to be fully defined in the
  agent template;
- templates may also include scripts or helper tools;
- when a new version is waiting for approval, the previous version continues to work normally.

A template sets an agent up in one guided step. Everything a person can already do by hand is proposed by the template
and confirmed by the person: name, instructions, skills, memories, schedules, webhooks, and accounts with their access.
Afterwards the agent is an ordinary agent.

Owner, 2026-11-22 (D-458): "make sure agent templates also include some sort of description or text explaining what
the agent is designed for, etc. so the humans can decide if they're interested in deploying it or not. this could
very well be just a README.md in the agent template subfolder."

## What it builds on

- **The agent export** (P5-REFINE-19, D-413, `14 §Export and import an agent`): a zip of `agent.yaml`,
  `instructions.md`, `memory/`, `skills/` and `automations/`, built in the browser. **A template is that layout plus
  `template.yaml`, with `helpers/` added** (C-1141). The same files exist in two forms. In a repository they are a
  directory `<id>/`; packed, they are a zip whose only top-level directory is `<id>/`. Save as template writes the
  packed form, which can be unzipped into a repository as it is (C-1118). The web Import still reads a template zip
  and ignores `template.yaml`, `helpers/` and skill references.
- **Purpose templates** (D-413, `web/src/lib/onboarding/templates.ts`): the built-in **Start from** chips of the New
  agent dialog. They stay as they are. Repository templates appear next to them under **More templates…** (C-1116).
- **Skill libraries from any git URL** (P5-SKLIB, D-448, D-449, C-1122..C-1128): the API's own git smart-HTTP reader
  (C-1125), the address rules (C-1123), check before save (C-1124) and the sealed read-only token (C-1126). Template
  sources use all four unchanged (D-457(a)). No app connection or GitHub account is involved.
- **Addon sources** (P5-ASRC, D-250, C-590..C-609): a repository an instance admin registers, a poll job under a
  lease, versions downloaded and checked. Template sources keep that admin shape and its poll job and leases. They
  have **no signing key** (D-435(b)).
- **The git skill library's rules** (P4-GSL, D-148, D-149): repository text is untrusted and shown in full as data.
  Content is applied only by a person who saw exactly that content. Updates are proposed and never apply themselves.
  Templates follow the same rules for their review and their updates. A template's **library skill references** are
  added through the library's own add path, so they stay ordinary library skills (C-1142).
- **Agents, accounts and access** (P5-UX, D-321..D-354): accounts; the three levels `read`, `ask` and `act`; an
  organization account's limit; `custom_only`; the connect journey; and the one grant writer. A template only
  **preselects** a level. The grant is written through the one grant writer, by the person's own creation request.
- **The home projection** (`12 §Home projection`): the managed roots the supervisor writes read-only and prunes to a
  manifest, as for skills and the app folders (`.aurora/apps/`, written by Aurora only). Helper files get one more managed root,
  `.aurora/helpers/` (C-1143).

## The model

1. **A publisher keeps templates as source in a git repository** (C-1103). The repository has `aurora-templates.yaml`
   at its root (or in a folder), then one directory per template, named by the template's id. A template directory
   holds a `README.md` saying what the agent is for, `template.yaml`, the export files, its skills (with their
   scripts), its helper files and an avatar. Every
   publisher uses the same CI (C-1144; GitHub Actions is the reference, and a GitLab CI sketch is in the guide). On
   every pull request, `aurora template check .` validates the whole repository. On every push to the default branch,
   each template whose `version` has no package yet is packed with `aurora template pack` into
   `<id>-<version>.zip` and published as a release asset. A version already published is never replaced, and CI fails
   when a published template changes without a new version.
2. **An instance admin adds the repository** (Administration › What agents may do › Agent templates). The admin gives
   its https git address, an optional branch or tag and folder, and an optional read-only token, as for a skill
   library. **Check** shows what Aurora found before anything is saved.
3. **Aurora checks the repository** daily, and on **Check now**. When the branch head moves, Aurora reads
   `aurora-templates.yaml` and each template's `template.yaml` at that commit. For a template whose version is newer
   than any version Aurora has stored, it downloads the package from the address the manifest gives (`packages.url`,
   for example the GitHub release asset). It then reads the same template directory from the repository at that
   commit, and **accepts the package only if its content is exactly the repository's** (C-1108). Until CI has
   published the package, the version is **Waiting for its package**, and Aurora looks again every 15 minutes, for up
   to a day. An accepted version is stored, parsed, as **Waiting for review**. Aurora never downloads it again.
4. **The admin reads the whole version and approves or rejects that exact content** (C-1108). That means the
   instructions, skills and their files, helper files, skill references, memories, automations, apps and access, and
   inputs. Nothing is offered before an admin approves it.
5. **The previous version keeps working** (D-456(f), D-457(f)). The offered version is always the newest approved
   one. A version that is waiting for its package, waiting for review, rejected or invalid changes nothing. A failing
   check, an unreachable repository or a deleted repository also change nothing. People keep creating agents from the
   offered version, and existing agents keep running unchanged. When the newer version is approved, new agents use it
   and existing agents are offered it, part by part.
6. **Organization owners and admins** can turn any template off for their organization (C-1109). All are on by
   default.
7. **Anyone who can create agents in a workspace** picks a template (New agent › More templates…). They read what it
   is for (its README) and what it needs in the template's details, and choose **Use this template**. They fill its
   inputs (for example a Jira project key) and choose an account for each app: an existing one, or one connected in
   the journey. The access is preselected. They choose which schedules start on and which optional library skills to
   add, review everything, and create. One request makes one transaction: the agent, its skills, its library skills,
   its helper files, memories, schedules, paused webhooks and access are all created, or nothing is (C-1111).
8. **The agent is then theirs.** Everything that was copied into it is editable as usual. Library skills behave as
   library skills: they are pinned, read-only, and updated through the library's own proposals. Helper files are
   read-only in the agent's computer and can be removed. When an admin approves a newer version, the agent's Settings
   page offers it part by part: instructions, each skill, the helper files and the library skill references, each with
   a full diff. Parts the person changed are left alone unless they choose otherwise (C-1112, C-1117). Until they
   apply it, the agent keeps running exactly as it is.
9. **Save as template** turns an agent into a template zip (C-1118). Library skills are written as references, not
   copies. `aurora template check`, `pack` and `list` help publishers (C-1105). `docs/25-agent-templates.md` is the
   publisher's guide, and `templates/examples/` is a complete example repository with its CI.

### What the template is for: its README (D-458, C-1141, C-1116)
Every template directory has a **required `README.md`**. It is written for the people deciding whether to deploy the
agent: what it does and doesn't do, who it suits, what it needs, what it runs on its own, and how to adapt it. The
one-line `summary` in `template.yaml` stays for the gallery card.

The README is part of the approved content and of `content_sha256`, so the admin reviews it with everything else (as
source, with a preview). It has no placeholders, because it is read before any input is filled.

People see it in these places:
- in the template's **details view**, which opens when they choose a card in the gallery, before **Use this
  template**;
- in a fold above the four steps;
- in the update dialog, as the new version's README.

It is rendered with the shared sanitized `Markdown` component: no raw HTML, images not loaded, only `https:`/`mailto:`
links, relative links as plain text.

**Save as template** asks for it ("What it's for", prefilled with an outline that must be replaced). `CHANGELOG.md`
stays optional, outside the content, and is used as release notes by CI.

### Skills a template can bring (D-456(d), C-1142)
- **Its own skills** under `skills/<name>/`: fully specified, with their files and scripts. They are copied into the
  agent as ordinary agent-tier skills.
- **A skill from a skill library**, by reference: the library's git address (with an optional branch and folder) and
  the skill's name. When the agent is created, Aurora looks for a library with that address that the organization can
  use: the organization's own library or an instance library. It adds the skill from that library at its current
  commit, through the library's add path. The skill is then pinned and read-only, and its updates go through the
  library's own review. If no such library has been added, the template says "Needs the skill library <repo>". A
  required reference makes the template not ready; an optional one is skipped. **Adding a library stays an admin's
  decision**: a template never adds one.
- **A skill the organization already has**, by name (`organization: <name>`): an organization-tier or workspace-tier
  skill that every agent of the workspace already gets. Nothing is written. The template only says it expects the
  skill, and a required one that is missing makes the template not ready.
- **App skills** (P5-APPSKILL, master's D-453). An app package now ships skills, at the lowest tier (`app`). Every
  agent that has access to the app gets them automatically. So a template that needs an app's skills only lists the
  app under `apps:`; nothing else is referenced or written, and the plan and the details view say "Includes the
  <App> app's skills". Aurora itself ships no other built-in skills (`aurora-*` names are reserved, and none exist).
  If some are added later, a `core:` reference kind comes with them, in a new template `format` (D-457(c)).

### Helper files (D-456(e), C-1141, C-1143)
Scripts and small tools come in two places:
- **In a skill**, as that skill's files with the executable bit, the way skills already carry scripts. This is the
  place for anything one skill uses.
- **In `helpers/`**, for files the instructions use directly. They are stored with the agent. They are projected
  read-only into its computer at `.aurora/helpers/` (`0555` for executables, `0444` for other files), and the agent's
  instructions are told where they are. People see them in the agent's Template section and can remove them. They
  are offered again with each update, as one part.

**Helper files are text** (owner decision 2026-11-24): every file under `helpers/` must be UTF-8 text without
NUL bytes — executable or not — because the home sync carries UTF-8 text only, so a non-text helper would be
stored but silently never installed. The template validation, the API's helper writer and the Save as template
dialog all refuse one (with the file's path). An executable skill file must also be text, so the admin can read
everything that will run; a non-executable skill data file may be binary, and the review shows its size and
sha256. Nothing in a template runs on the platform: helpers and skill scripts run only in the
agent's own computer, when the agent runs them.

### Examples (P6-TPL-1, the example repository `templates/examples/`)
- **Schedule manager**: Indico (read) and one of Microsoft 365, CalDAV or Google Workspace (ask before changes); inputs
  for the Indico categories to follow; a weekday-morning agenda schedule; a skill for "what is on this week" with a
  small formatting script.
- **Service monitor**: Grafana (read), Atlassian Jira (custom: reads allowed, creating and commenting ask), GitLab
  (read, optional); inputs for the Jira project key and the Grafana folder; a morning check schedule; skills for
  triaging an alert and opening an incident ticket; one helper script that turns an alert's JSON into a short summary;
  one optional library skill reference (shown as "Needs the skill library …" on installations without it). Grafana
  arrived with P5-GRAFANA; where it is not installed, the template shows "Needs Grafana, which isn't available here".

## Owner answers

**D-435 (2026-11-21).**
- (a) Template repositories are added by **instance admins only** in this version; organization-level repositories wait
  for the org tree (P6-ORGTREE).
- (b) **No signing.** The admin's review and approval of each version is the trust gate.
- (c) **Evals in templates** come later (not in this version).
- (d) **No policy rules in templates** in this version.
- (e) An app entry can name **alternatives** (`any_of`), needed for calendars.
- (f) **Save as template** is in this version.

**D-456 (2026-11-22).** The six points quoted under §Goal.

**D-458 (2026-11-22).** Every template has a description for the people deciding whether to deploy it: a required
`README.md` in its directory (§What the template is for).

## Design decisions

**D-436 (coordinator, 2026-11-21), as amended by D-457.**
- The format is the agent export plus `template.yaml` (and `helpers/`). The model is always `default` and the harness
  always the installation's default, because model names and enabled harnesses differ between installations.
- Aurora stores the parsed content when it accepts a version. What the admin approved (by `content_sha256`) is exactly
  what is used.
- A template sets no access on its own. Its levels are suggestions, capped by the account's limit and `custom_only`.
  Tools the installed app does not have are dropped and reported.
- Creation is one transaction through the existing writers: agent creation with its starter grants, skills, the
  library add path, memories, schedules, webhooks and the one grant writer.
- Webhooks are always created paused, with a new secret shown once. Schedules run with the creating person's authority.
- Memories and automations are a starting point and are not offered again. Apps a new version adds are listed with a
  link to give access.
- Deleting a source or template never changes an agent. Provenance is text (id, name, version), and a source added
  again finds its agents by template id.

**D-457 (coordinator, 2026-11-22), for D-456.**
- (a) **Sources are git repositories read by the skill library's reader.** A source is an https git address (C-1123),
  an optional branch or tag, a folder, and an optional sealed read-only token (purpose `TemplateSourceToken`). It is
  read with the C-1125 reader, with caps sized for templates. This replaces the raw-manifest URL and the C-592
  normalization of the first design. GitHub, GitLab, Gitea, Forgejo, Bitbucket and a plain `git http-backend` all
  work.
- (b) **The repository holds source; packages are CI artifacts, checked against the source.** There are no zips and
  no sha256 list in the repository. The manifest names where packages are published (`packages.url`, with `{id}` and
  `{version}`). A version is published by bumping `version` in `template.yaml` on the tracked branch; CI then
  publishes the package. Aurora only takes a package whose canonical content (`content_sha256`) equals that of the
  template directory at the head commit. So the package is what CI validated, the repository is what people reviewed
  in their pull requests, and neither can differ from the other unnoticed (`source_mismatch`). Without signing, this
  is the integrity check between the two places.
- (c) **Skill references** come in two kinds. `library` is added through the library add path, so it is pinned,
  read-only and updated by the library. `organization` is only expected, and nothing is written. A template never
  adds a library. There is no `core` kind until Aurora has built-in skills.
- (d) **Helper files** are stored with the agent and projected read-only at `.aurora/helpers/`. Agent tools cannot
  write them. People can remove them, and updates offer them as one part. Rejected alternatives: writable seed files
  in the agent's home, which would need a running computer at creation and could not be updated; and putting helpers
  in a fake skill, since skill directories depend on the harness.
- (e) **Executable files must be text**, so the admin can read everything that will run.
- (f) **A waiting version changes nothing.** The offered version is the newest approved one. Every other state of a
  newer version, and every failure of the repository or its packages, leaves the offered version, the gallery,
  creation and existing agents unchanged. On the agent side, an offer is only an offer until the person applies it.
- (g) Updates cover the instructions, the agent's own skills, the helper files and which library skills are
  referenced. A library skill's **content** is never part of a template update; the library's refresh proposes it.

## Not in this version

Organization-level repositories (after P6-ORGTREE); evals; policy rules; generic secrets with an injection host (a
template would decide where your secret is sent); channels, presences and resource sizes; BYO agents; publishing a
saved template into the installation from the dashboard (Save as template downloads a file); automatic updates; a
`core` skill reference; packages that differ from the repository (for example tools built by CI).

## Briefs and order

| Brief | Scope | Size | Lane | Depends on |
|---|---|---|---|---|
| P6-TPL-1 | `aurora-template` crate (format, repository manifest, skill references, validation, render, hashes, pack, tree reading), `aurora template check/pack/list`, `docs/25-agent-templates.md` with the repository format and CI, the example repository with its GitHub Actions workflow | M–L | Rust | — |
| P6-TPL-2 | Sources (git address, branch, folder, token), poll job, ingest (head read, package download, source comparison, waiting for packages), review and approval, organization switches, workspace catalogue with skill references; migration A; SEC-146 review | L | Rust | P6-TPL-1 |
| P6-TPL-6 | Helper files: `agent_helper_files`, the `helpers` home-sync part and the `.aurora/helpers/` managed root in the supervisor, the instructions line, list/read/remove routes; migration B | M | Rust (API + supervisor) | P6-TPL-1 (can run beside P6-TPL-2) |
| P6-TPL-3 | Create from a template (one transaction, library skills through the add path, helpers), provenance, update offers, apply, detach; migration C; SEC-146 review | L | Rust | P6-TPL-2, P6-TPL-6 |
| P6-TPL-4 | Admin tab with the git-address Add dialog and the review page, organization tab, gallery and four-step page, the agent's Template section with helpers | L | Next.js | P6-TPL-2 (parts 1–2), P6-TPL-3 and P6-TPL-6 (parts 3–4) |
| P6-TPL-5 | Save as template (web): the zip rooted at `<id>/`, library skills as references, helpers kept; a web-written golden archive read by the crate | S–M | Next.js (+1 Rust test) | P6-TPL-1 |

P6-TPL-2 and P6-TPL-3 each get a security review against SEC-146 before they merge, and P6-TPL-6 gets a short one on
its projection (managed root, modes, pruning). P6-TPL-5 can run beside P6-TPL-2. P6-TPL-6 can run beside P6-TPL-2 in
a second Rust lane. Their files are disjoint: the supervisor, the home-sync builder and a new agents module, against
the template-source modules.

Phase-6 placement: after P6-WARMPOOL Parts 1 and 2, which come first in phase 6 (D-379). Independent of P6-LIMITS and
P6-ORGTREE.
