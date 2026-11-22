# P5-GUIDE-4: Agent guides — the built-in packages adopt summaries, essentials and guide files (owner, 2026-11-13)

Fourth task of "app guides on demand": the "adapt our addons" part. Normative spec:
`docs/24-apps-for-agents.md` §3 and §10, `plugins/builtin/README.md` (authoring rules, written by
P5-GUIDE-1). Depends on **P5-GUIDE-1** (format, linter, `aurora plugin guide`); the end-to-end
check at the end needs P5-GUIDE-2. A package task: no Rust, no dashboard.

**Amendment of 2026-11-15 (D-295), applies to everything below:** the instruction file never
carries the notes, so the `summary` is the only text that makes an agent pick an app; name the
tasks the app serves. Write **recipes** (`kind: recipe`, `24 §3.1`) for the tasks people
actually ask for, at least:
- **indico**: "Find a free room and book it" (search per building with `repeat_frequency`, name
  the rooms, confirm with the person, book, verify the booking; the never-retry rule for
  bookings), and "Find the events of a category in a date range";
- **gitlab** / **github** family: "Find why a pipeline / workflow run failed and retry it";
- **microsoft-365**: "Find a meeting slot for several people";
- **jira** / **jira-dc**: "Create an issue with the right project, type and fields".
More are welcome where an app has a multi-step task with pitfalls. Recipes must use only
requests the catalogue names, and say which ones need approval. No operations files in this
task (P5-GUIDE-6 adds them where they help).

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

## Completion report (lane `p5-apps`, worker 36, 2026-11-18)

**Adapted to master** (brief of 2026-11-13/15): master has **11 packages with 17 apps**, not 13 / 23 (GitHub App,
GHES App and AWS role are connect methods of `github`, `github-ghes`, `aws-keys` since FIX-123; the four Sentry apps are one
app with instances since FIX-134; `jira`, `confluence`, `jira-dc`, `confluence-dc` are in `atlassian`; `gmail`, `calendar`, `drive`
in `google-workspace`). The tool `description`s the brief asks for were all added by P5-UX-13: none changed. Min core stays 0.1.0
(the core version). Versions: indico 1.3.0, gitlab 1.3.0, github 2.3.0, atlassian 1.2.0, microsoft-365 1.1.0, google-workspace
1.1.0, notion 1.1.0, postgresql 1.2.0, aws-keys 2.1.0, sentry 2.1.0, linear 1.1.0.

**What each app got.** Every app has a `summary` naming tasks (longest index line 179 characters) and its essentials cut to
the facts needed before the first request (all but `drive` and `sentry` before this task, now all of them, 405..931 characters;
`drive` 931 -> about 480, `sentry` 807 -> about 440; gmail, confluence, linear, calendar stay 464..691: aim was 600).
Guide files: **indico** `book-free-room.md` and `find-category-events.md` (recipes), `api-families.md`; **gitlab**
`pipeline-failed.md` (recipe), `api-conventions.md`; **github** / **github-ghes** `api-conventions.md`; **microsoft-365**
`schedule-meeting.md` (recipe), `graph-conventions.md`; **jira** / **jira-dc** `create-issue.md` (recipe); **postgresql**
`using-psql.md`; **notion** `api-notes.md`; **aws-keys** `requests.md`; **drive** `api-notes.md`; **sentry** `api-notes.md`.
No guide file (summary and essentials only): gmail, calendar, linear, confluence, confluence-dc. Not done: the short guides for
those five and the "full guide" depth the brief lists for `github-app`/`aws-role` (they are methods of the apps above now).

**Recipes the catalogue cannot support, said instead of invented.** `github` has no Actions request in its catalogue (no `/actions/`
path), so the "workflow run failed and retry" recipe was not written; `microsoft-365` reads only `/me/...`, so "find a slot for
several people" became "find a free slot in my calendar and schedule a meeting" and says that others' free/busy is not readable.
Every request in a recipe or table was checked against the package's tool paths and fixtures; claims about vendor behaviour
(pagination headers, `$select`, ADF, SigV4 and so on) come from the old notes or the vendor's public documentation as I know
it, not from a fresh fetch (no network in this lane): list for the reviewer: Indico `from`/`to` date forms and result field
names (left out), Notion `409` retry advice, Sentry `Link` `results` attribute, S3 `list-type=2` continuation.

**Evidence.** `SKIP_CARGO=1 bash scripts/plugins/check-builtin.sh` all 11 packages and `hook-example` ok (every existing fixture
unmodified: `git diff d6fd9799 -- plugins/builtin/*/tests plugins/builtin/*/hooks` empty). `aurora plugin compile` of each package
before (worktree of `d6fd9799`) and after differs only in `apps[].agent` (`summary`, `instructions`, `guide`), `version`, `digest`
and, for the three packages with hooks, `hooks_digest` (the wasm is rebuilt from the same sources at another path; the hook
sources are identical). `aurora plugin guide --app indico --access custom` read in full (guide.md lists recipes first). Connected
apps fragment: **after** 17 index lines = 2 405 characters (max 179 per app, limit 270) plus the paragraph; **before** the notes
alone were 12 620 characters for these 17 apps (sizes of the brief's table) plus the account lines. The supervisor-side snapshot
with all apps connected was not added (the supervisor does not depend on the registry); the measurement is the formula of
`apps_index` applied to the compiled summaries. Sync size (bytes and time of one home sync with every built-in app connected):
not measured; needs an idle machine.

**Tests changed because of the versions and the guide keys (aurora-registry):** `builtin_versions.rs` (shipped versions),
`snapshot.rs::the_shipped_builtin_packages_compile_without_a_host_claim_decision` (guide digests, `compile_with_guides`),
`agent_guide_hashes.txt` regenerated (`UPDATE_HASHES=1`): the 11 built-ins left it because they now use guide keys.
`cargo test -p aurora-registry`: 22 + 120 passed.

**Remainder (worker 37, 2026-11-18): the five short guides.** All five apps exist on master. Added one file each:
`google-workspace/agent/gmail/api-notes.md`, `.../calendar/api-notes.md`, `linear/agent/linear/graphql-notes.md`,
`atlassian/agent/confluence/api-notes.md`, `.../confluence-dc/api-notes.md`; versions google-workspace 1.2.0, linear 1.2.0,
atlassian 1.3.0 (`builtin_versions.rs` follows). Facts come from the package catalogues and fixtures plus the vendors' documented
APIs as known (no network): for the reviewer, unverified against a fresh fetch: Gmail `format=metadata` / base64url body parts,
Calendar `sendUpdates` and the 410 on deleted events, Linear `IssueFilter` / `isMe` and the `RATELIMITED` code, Confluence v2
`_links.next` paging and the 409 on a wrong version number. `cargo test -p aurora-registry` 22 + 120 passed;
`SKIP_CARGO=1 bash scripts/plugins/check-builtin.sh` all 11 packages ok.
