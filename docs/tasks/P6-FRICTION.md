# P6-FRICTION: Agents report tool problems — **proposal, spec batch (Opus) later — phase 6**

> **Status: proposal (2026-11-22).** No D-, C-, SEC- or migration numbers are taken (the spec batch takes
> them from PROGRESS's next-free note).
>
> **Owner answers, 2026-11-22 (D-472):** the recommendations of Q-P6-FRIC-1..12 are all accepted as
> written in the table below. No question is open.

## Goal

Owner, 2026-11-22: agents should have a tool for lodging complaints. An agent that gets failures from a tool,
or that has to try several times before something works, files a complaint against that tool: what the
problem was and how it got around it in the end. Tool developers then know where to round off rough edges
and make things more natural for agents. Complaints must be aggregated: not 10 000 complaints about the same
thing, but one complaint with 10 000 +1s, so the work can be prioritised.

The part that matters most is the workaround. "I tried X, got Y, and Z worked" is close to a ready-made fix.
FIX-227 and FIX-228 came from exactly this, done by hand: a review of the NightManager agent's transcripts
found where it struggled with Grafana and Jira DC, and the fixes went into the app skills. This proposal makes
that review continuous and covers every agent on the instance.

## Today

- **What fails is recorded, why is not.** The gateway writes one `request_log` row per request with `app_id`,
  `tool_id`, `turn_id`, `status`, `upstream_status`, `error_code` and the decision (`03 §request_log`, kept
  `retention.request_log_days`, default 30). The transcript has `tool.started` / `tool.finished { ok }` per
  call, with a sanitised summary and never raw arguments (`10 §Transcript events`). Nothing records what the
  agent was trying to do, what was confusing, or how it got past it.
- **Health is about accounts, not tools.** P5-UX-14 records an account's health from real use (an account that
  stops working). `TOOL_CONSECUTIVE_FAILURES` (`12 §Platform tools`) protects the bridge. Neither says that an
  operation's error text is unhelpful or that a skill describes it wrongly.
- **Who maintains what.** Aurora itself: the platform tools, `aurora-agent`, the built-in packages
  (`plugins/builtin`) and the sandbox image. Addon publishers: addon packages, from repositories an instance
  admin added (`22`). Skill library authors: library skills, from git repositories (`10 §Git skill library`).
  The agent's own, workspace and organization skills are written by the agent's people.
- **Platform-internal LLM calls** go through `PlatformLlm` (`08 §Platform-internal LLM calls`), which has one
  purpose so far, `eval_judge`.

## Proposal

### Part 1 — The report, the issue and the admin page

**The tool.** One platform tool, `friction_report`, family `feedback`, exposure `on_demand` (`24 §10b`), so
it costs nothing in the bridge's list. It is reached through `aurora-agent call aurora friction_report …`.
The platform-tools index gains the family line, and the instruction file one short paragraph (platform voice,
about 70 tokens, estimate):

> When a tool, an app operation, a skill or `aurora-agent` made you struggle (an error that did not say what
> was wrong, documentation that was wrong, an operation that was missing) and you have finished the task,
> report it once with `friction_report`: what you tried, what happened, and how you got around it. Describe
> the shape of the problem, never the data: no names, ids, addresses or content from the conversation.
> Refusals by policy and approvals are intended; do not report them.

**Arguments** (all text through the one-line sanitizer and the secret-pattern redaction at write; limits
final in the spec batch):

| Field | Content |
|---|---|
| `target` | What the report is about: `<app_id>/<operation>`, `<app_id>` (the app as a whole: connecting, its `app.md`), `aurora/<platform tool>`, `aurora-agent`, `skill/<name>`, or `sandbox` (the computer: a missing command, the image). |
| `category` | `error_unclear`, `docs_wrong`, `docs_missing`, `missing_operation`, `unexpected_response` (shape, pagination, encoding), `limits` (size, time, rate), `access_confusing`, `other`. |
| `outcome` | `worked_around`, `retried_ok` (the same call worked after a retry or a small change), `gave_up`. |
| `tried` | ≤ 400 characters. |
| `happened` | ≤ 400 characters. |
| `workaround` | ≤ 600 characters; required when `outcome` is `worked_around`. |

**What the server adds.** The call carries the turn (FIX-110), so the API knows the organization, workspace,
agent, conversation and turn. It adds the harness and the model id (the turn's `ModelChoice`). It resolves
the target against **what this agent can use**: an app granted to it and an operation in that app's
catalogue, a platform tool available to it, a skill in its effective set, `aurora-agent`, `sandbox`. For each
target it records who maintains it and which version: the package id and version, the library source and
commit, or the Aurora release. A target the agent cannot use, and an own, workspace or organization skill
(Q-P6-FRIC-1), is refused with one fixed error that does not say whether the target exists elsewhere.

It also links the **evidence** automatically, without trusting the agent: up to 10 `request_log` ids of this
turn for the same app and operation with a status ≥ 400, and up to 10 platform tool calls of this turn that
ended `ok: false`. These are ids only. Rows whose decision is a policy one (`blocked`, `approval_*`,
`governance_refused`, `read_only_connection`, …) are linked but flagged, because a "workaround" for a
refusal is a finding for the admin, not friction (§Security).

**Limits.** One report per agent, target and category per day. A second one adds to that report's
`repeats` and returns `already_reported`. At most `FRICTION_REPORTS_PER_AGENT_DAY` (20) a day in all. Eval
turns may report; the daily rule keeps repeated runs from inflating the counts.

**What the agent gets back.** `{ recorded, issue: "<ref>", agents: <distinct agents that reported it>,
known_issue?: "<admin text>" }`. It never includes another report's text, an issue title or anything else
written by another agent.

**The issue (grouping).** Every report joins exactly one issue. The **fingerprint** is
`(target kind, target, maintainer, major version, category, signal)`. The signal is the most frequent error
among the evidence: `http_<status>`, the gateway's `error_code` or the platform tool's error code, else
`none`. A report joins the open issue with its fingerprint, or opens one. This is deterministic and needs no
model (Q-P6-FRIC-5). It tends to put too much in one issue rather than too little, and admins fix that with
two actions:

- **Merge** moves an issue's reports into another one. The merged issue becomes an alias, so later reports
  with its fingerprint go to the target.
- **Split** moves selected reports into a new issue. New reports still follow the fingerprint, so a split
  issue only receives reports that an admin moves there, or (Part 4) that the matcher sends.

An issue holds:

- **Counts:** reports, repeats, distinct agents, distinct workspaces, distinct organizations, first and last
  seen, and the versions seen.
- **A status:** `open`, `acknowledged`, `fixed` (in version X of the package, the library or Aurora),
  `wont_fix`, or `stale` (no report in 60 days; hidden by default). A report on a version at or after the fix
  reopens the issue and marks it `regression`. Later reports on older versions only add to the counts.
- **A title and description.** The **automatic title** is built from the structured fields only, for
  example "jira-dc/search_issues — error unclear (http_400)", never from report text. Admins edit both.

Lists are ranked by **distinct agents** by default, not by raw reports, so that one agent repeating itself
does not outrank 300 agents that each hit the problem once.

**Who sees what** (Q-P6-FRIC-3, -4):

- **Instance admins:** every issue with its counts and structured fields; the report text of every
  organization whose setting `share_tool_problems` is on (default on). For an organization that turned it
  off, they see counts and structured fields only.
- **Organization owners and admins:** the issues their organization's agents reported, with their own
  organization's report text and the global counts. They never see another organization's name or text.
- **The agent's people:** nothing in this version (Q-P6-FRIC-9).
- **Evidence** opens through the request log and transcript views that already exist, with their existing
  permission checks. It is never copied into the issue.

**Retention.** A report is deleted after `FRICTION_REPORT_RETENTION` (90 days), and with its conversation,
agent or organization. The issue's counters keep their totals. An issue lives until an admin deletes it.

**Dashboard.** Administration › **Tool problems** (following `14 §Visual design`: primitives, both themes,
390 px):

- **The list:** filters by kind, app or package, status, and "has a known issue". Columns: title, target,
  agents, organizations, reports, last seen, versions, status. Sorted by agents.
- **The issue page:** title and status, the numbers (with Part 2's failed-call counts beside them), the 20
  newest shared reports as plain text (category, outcome, tried, happened, workaround, harness, model,
  version, date, organization label), and the evidence links. Actions: Acknowledge, Fixed in version…, Won't
  fix, Merge into…, Split, Copy as Markdown, Delete.

An organization-level page lists the same issues for the organization (its place in the IA is decided at spec
time). Instance settings section `tool_problems`: on/off (default on) and retention. Organization setting:
`share_tool_problems`.

**Export** (Q-P6-FRIC-8). Copy as Markdown and `GET /v1/admin/tool-problems/{id}/export` produce the issue's
fields, counts, versions and up to 20 shared example reports, with no organization names and no agent,
conversation or request ids. This is what an admin pastes into the addon publisher's repository, the skill
library's repository or Aurora's own repository, or hands to a coding agent, as the owner did for FIX-227/228.
Nothing is ever sent anywhere automatically.

**Audit.** Status changes, merge, split, delete, export and the organization setting are audited. Report
filing writes no audit row: the report is itself the record.

### Part 2 — Failed calls without a report

Agents will not always report, and they sometimes blame a tool for their own mistake. A daily job counts,
without any text, what the request log and the tool registry already know. Rows go per day, target, version
and signal:

- failed calls;
- **recovered:** a later call to the same operation in the same turn succeeded;
- **unrecovered:** the turn's last call to it failed;
- distinct agents and distinct organizations.

Refusals by policy are excluded. The admin page gains "Most failed operations (7 days)", each row linked to
the issue with the same fingerprint when there is one. A row with many failures but no reports is something
to look at. An issue with reports and many recovered failures is friction that agents get past, at a cost.
This part reads only existing data and adds no text anywhere (Q-P6-FRIC-7).

### Part 3 — Known issues back to agents

An admin can **publish a known issue** on an issue: a text the admin writes (≤ 400 characters, sanitised).
It can be a confirmed workaround from the reports, rewritten. Agents then read it where they look:

- in `tools.md`, under the operation's line, as "Known issue (from this instance's administrators): …";
  app-level issues go in a short section at the top;
- in the rebuilt `SKILL.md` of an app or library skill, after the provenance line;
- in `aurora-agent describe` for the operation or platform tool;
- printed by `aurora-agent call` after an error on that operation, at the moment it helps.

At most `FRICTION_KNOWN_ISSUES_PER_APP` (5) per app. A published known issue disappears by itself when the
issue is `fixed` and the agent's active version is at or after the fix, or when the admin unpublishes it. The
renderer stays a pure function: the render inputs gain `known_issues` per app and skill, and a change
resyncs the homes of agents that have the app. `tools.md` and `SKILL.md` are read on demand, so this releases
no idle session (`24 §10b`). Publishing is audited.

Only admin-written text reaches agents (Q-P6-FRIC-6). Publishing an agent's workaround as it stands would
carry text written in one organization, possibly shaped by content that agent read, into every other
organization's agents.

### Part 4 — Matching by a model (optional, after experience)

When the fingerprint proves too coarse, `PlatformLlm` gains a purpose `friction_match`. It runs on a key an
instance admin chooses in the `tool_problems` settings, checked at every call like the judge key, and only for
reports from organizations that share. Its input is the new report and at most 20 open issues of the same
target (their admin titles and structured fields, never other reports' text). Its output is
`{ "issue": "<one of the listed ids>" | "new" }`, and anything else falls back to the fingerprint. The
model can therefore only choose; it cannot write anything that someone reads. Usage is recorded on the
instance, not on any turn.

## Security (one SEC invariant at spec time)

1. **Report text is untrusted.** An agent wrote it, possibly shaped by untrusted content. It is sanitised and
   redacted at write, and shown only as plain text, never as Markdown or HTML. It never goes into any agent's
   sandbox, instructions or tool result. Part 4's model can only return an id.
2. **It stays inside its audience.** The organization's owners and admins, and instance admins only when the
   organization shares. The automatic issue title uses structured fields only. The filing agent's response
   carries no other report's text.
3. **The only text that reaches agents is admin-written** (Part 3): sanitised, fenced and labelled with its
   author.
4. **No new reach and no spam.** A report can only name a target the agent can use, and the refusal does not
   reveal whether a target exists elsewhere. Filing is an ordinary platform tool call in an active turn.
   Daily limits apply.
5. **Evidence is ids.** Opening it goes through the existing permission checks.
6. **Refusals stay refusals.** Evidence that holds a policy decision is flagged on the report. A report that
   describes a way around a refusal is shown to the instance admin as a possible policy finding. It never
   becomes a known issue as it stands.
7. **Nothing leaves the instance by itself.** Export is a person's action.

## Rejected

- **Opening issues in the publishers' repositories automatically.** It would send organizations' text outside
  the instance without a person deciding, it needs credentials for every repository, and it would flood
  publishers.
- **Issue summaries written by a model from the reports.** That would put one organization's text into what
  other admins, and through Part 3 other agents, read. The summary is written by a person.
- **A core (always listed) tool.** It would cost tokens in every session. The instruction paragraph is what
  makes agents report; the tool's schema is only needed when they do.
- **Asking the agent at the end of every turn whether it struggled.** That is one more model call on every
  turn.
- **Detecting failures only (Part 2 alone).** It finds that something fails, but not why or how the agent got
  past it, which is the useful part.
- **+1 buttons for people.** The agents hit the problems, and their reports already count. People act
  through the admin actions.

## Deliverables of the spec batch (when phase 6 starts)

- D-numbers for the answers; one SEC invariant (§Security); C-numbers for the tool, the routes and the
  render inputs.
- `24` (the tool, the instruction paragraph, Part 3 in `tools.md`, `SKILL.md`, `describe` and `call`), `12`
  (the registry entry), `03` (`friction_reports`, `friction_issues`, `friction_signals` under §Planned
  changes, with their migrations), `05` and `API_CONTRACTS` (admin and organization routes, export, settings),
  `08` (`PlatformLlm` purpose, Part 4), `10` (retention and deletion), `14` (the pages), `15` (settings
  section), `17`, `18`.
- Proposed briefs: **P6-FRIC-1** (Rust: tool, tables, fingerprint, visibility, admin and organization
  routes, settings, audit; L), **P6-FRIC-2** (Rust: the daily failed-call job; S–M), **P6-FRIC-3** (Next.js:
  the pages and settings; M–L), **P6-FRIC-4** (both: known issues in the renderer and `aurora-agent`, publish
  in the issue page; M), **P6-FRIC-5** (optional, Rust: `friction_match`; S–M).

## Questions for the owner (all answered 2026-11-22, D-472)

| # | Question | Recommendation |
|---|---|---|
| Q-P6-FRIC-1 | What can a report be about? | **Platform tools, `aurora-agent`, app operations and apps, app skills, library skills and the sandbox.** Not the agent's own, workspace or organization skills: their authors are the agent's people, who already read its conversations. |
| Q-P6-FRIC-2 | Is `friction_report` a core tool or on demand? | **On demand, plus the instruction paragraph** (about 70 tokens per session). |
| Q-P6-FRIC-3 | Who reads report text? | **The organization's owners and admins for their organization; instance admins for organizations that share.** Nobody else. Counts are visible to both. |
| Q-P6-FRIC-4 | Does an organization share its report text with instance admins by default? | **Yes**, through an organization setting it can turn off. When it is off, text stays in the organization and only counts and structured fields go up. |
| Q-P6-FRIC-5 | How are reports grouped in the first version? | **By the deterministic fingerprint, with admin merge and split.** Model matching (Part 4) only once the fingerprint is seen to fall short. |
| Q-P6-FRIC-6 | Do agents get known issues back, and who writes them? | **Yes, in the first version (Part 3), written by instance admins only.** Never an agent's workaround as it stands. |
| Q-P6-FRIC-7 | Count failed calls without reports (Part 2)? | **Yes**, counts only, from data that already exists. |
| Q-P6-FRIC-8 | Send issues to publishers' repositories? | **Never automatically.** Copy as Markdown and an export route; a person files them. |
| Q-P6-FRIC-9 | Do the agent's people see its reports (for example on the agent's timeline)? | **Not in this version.** Revisit after the admin pages have been used. |
| Q-P6-FRIC-10 | How long are reports kept? | **90 days**, and deleted with their conversation, agent or organization. Issues and their counts stay until an admin deletes them. |
| Q-P6-FRIC-11 | On by default for the instance? | **Yes**, an instance setting. |
| Q-P6-FRIC-12 | When in phase 6? | **Parts 1 and 2 early** (right after P6-WARMPOOL Parts 1–2, beside other lanes: they touch new tables and a new page), so data collects while the rest of phase 6 is built. Part 3 once there are issues worth publishing; Part 4 only if needed. |
