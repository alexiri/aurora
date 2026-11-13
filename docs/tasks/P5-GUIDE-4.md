# P5-GUIDE-4: Agent guides — the built-in packages adopt summaries, essentials and guide files (owner, 2026-11-13)

Fourth task of "app guides on demand": the "adapt our addons" part. Normative spec:
`docs/24-app-guides.md` §3 and §10, `plugins/builtin/README.md` (authoring rules, written by
P5-GUIDE-1). Depends on **P5-GUIDE-1** (format, linter, `aurora plugin guide`); the end-to-end
check at the end needs P5-GUIDE-2. A package task: no Rust, no dashboard.

## What it delivers

Each of the 23 apps in `plugins/builtin/` (13 packages) gets a `summary`; its
`agent.instructions` shrinks to the essentials; what was cramped into the notes, and what never
fitted, moves into guide files; catalogue tools get a `description` where the name is not
enough. Request-path behaviour does not change at all: hosts, injection, tools ids, order,
classifiers, approvals and fixtures stay as they are.

State on 2026-11-13 (characters of `agent.instructions`, catalogue tools):

| App | Notes | Tools | | App | Notes | Tools |
|---|---|---|---|---|---|---|
| indico | 1 369 | 40 | | github-app-ghes | 714 | 23 |
| aws-role | 1 208 | 8 | | confluence-dc | 693 | 10 |
| aws-keys | 1 015 | 8 | | linear | 692 | 2 |
| notion | 970 | 17 | | jira | 655 | 13 |
| postgresql | 923 | 4 | | sentry | 645 | 11 |
| confluence | 897 | 13 | | sentry-de / sentry-us | 629 / 624 | 10 |
| drive | 807 | 11 | | github | 627 | 24 |
| github-app | 765 | 23 | | jira-dc | 598 | 12 |
| microsoft-365 | 596 | 35 | | github-ghes | 581 | 24 |
| gitlab | 573 | 37 | | sentry-self-hosted | 526 | 11 |
| gmail | 514 | 11 | | calendar | 465 | 8 |

## Rules for every app

1. **`summary`** (≤ 160 characters, one line): what the app is for, in the words of a task
   ("Events, timetables, materials and room booking in Indico."). No instructions in it.
2. **Essentials** (`agent.instructions`, aim for ≤ 600 characters): only what must be known
   **before the first request**: what Aurora adds (and so what never to send or ask for), how to
   address the right host or account when that is not obvious, and actions that must never be
   retried or need confirmation (Indico room booking). Endpoint lists leave.
3. **Guide files** (`agent/<app_id>/`): one topic per file, the first lines say what the file
   contains; plain Markdown; real request examples with `curl` through the proxy as an agent
   would run them (no credential, no `-k`); pagination, filtering, error shapes, rate limits and
   the traps already known from the notes. Nothing that the vendor's documentation says better
   and that an agent does not need to get a request right. Keep each app well under the limits;
   most apps need one file of 2 to 6 KiB.
4. **Tool descriptions**: add `description` to a tool when its name does not tell what the
   request body or the main parameters are. Never change an `id`, the order or a pattern.
5. Text is written for an agent, in English, as fact. It never claims to be the platform or the
   operator, and never tells the agent to disregard other instructions (the linter cannot check
   this; the reviewer does).
6. Version: minor bump per package; `min_core` = the release that ships P5-GUIDE-1.

## Depth per app

- **Full guide** (reference moved out of the notes plus what was missing): `indico` (the export
  API families, the booking form, the extended-tier endpoints and their instability),
  `gitlab` and `github` / `github-app` (+ their GHES variants, sharing the same files content
  where the API is the same), `microsoft-365` (Graph conventions: `$select`, `$filter`, paging,
  mail / calendar / files entry points), `notion` (data sources, block children, the version
  header Aurora sends), `aws-keys` / `aws-role` (regional hosts, Query vs JSON protocols, what
  the catalogue covers), `postgresql` (the `psql service=` workflow, the four classes and what
  each allows, read-only habits).
- **Short guide** (one file): `jira`, `confluence`, `jira-dc`, `confluence-dc`, `gmail`,
  `calendar`, `drive`, `sentry` and its three variants, `linear`.

## Ownership

`plugins/builtin/*/plugin.yaml`, `plugins/builtin/*/agent/**`; signing stays whatever the repo
does for built-in packages today (`plugins/builtin/README.md`). Nothing else. Do not touch `plugins/builtin/*/tests/**` except to add
a case; an existing fixture that needs changing means a request-path change slipped in.

## Acceptance criteria

- [ ] `aurora plugin validate` and `aurora plugin test` pass for all 13 packages; **every
      existing fixture passes unmodified**.
- [ ] For each package, a diff of `aurora plugin compile` output before and after shows changes
      only under `apps[].agent` (`summary`, `instructions`, `guide`) and tool `description`s.
- [ ] `aurora plugin guide --app <id> --access custom` reads well for every app (attach the
      output of `indico` and `gitlab` to the PR).
- [ ] With P5-GUIDE-2 merged: the supervisor snapshot with all 23 apps connected, one account
      each, is in index mode; record the size of the *connected apps* fragment before (today's
      rendering) and after in `docs/PROGRESS.md`. It must not exceed 270 characters per app plus
      the paragraph.
- [ ] Every claim in a guide file was checked against the vendor's current API documentation or
      against the package's own fixtures; list in the PR what could not be verified, and leave
      it out of the guide rather than guess.

## Notes for the implementer

- Change files only with the Edit/Write tools. No `Co-Authored-By` lines in commits.
- This is writing for a model that will act on it: a wrong endpoint here produces wrong requests
  in production. Prefer fewer, verified statements.
- Suggested agents: one `haiku-worker` per package for summaries and the short guides, Sonnet for
  the full guides; at most three in parallel, one package per worker (disjoint directories).
  Review of every guide by the main session before merge. Size L (mostly writing).
