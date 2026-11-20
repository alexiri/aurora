# P5-RUNDECK: built-in `rundeck` package — jobs, executions and nodes on Rundeck / Runbook Automation (phase 5, D-433, SEC-145)

Read `docs/tasks/P5-CATALOG.md` first (common rules, acceptance, order). This brief owns
`plugins/builtin/rundeck/**`, its `REFUSED` entry and its `ICONS.md` row.

## Goal

"Did last night's backup job succeed?", "show me the output of the failed execution", "which jobs are
running right now?", "run the 'clear cache' job on node X" (with approval), "stop that execution". An
agent reads projects, jobs, executions and nodes and, with approval, runs a **defined** job. It never runs
ad hoc commands or scripts.

## Hosting and connect (D-433)

- **Instance-bound, no default host** (`instances`, label "Rundeck server"). Production Rundeck usually sits
  behind a TLS proxy, often under a base path (`/rundeck`): base paths are allowed. Rundeck's own default
  port (4440) is not reachable this way; the setup guide says the server must be served on 443. Probe:
  `GET /api/<n>/system/info` expecting `[200, 401, 403]` (check what an anonymous call gets).
- Covers **Rundeck Community** (Apache-2.0, still released) and **PagerDuty Runbook Automation**
  (self-hosted and SaaS): the API is the same `/api/<n>/…` surface; enterprise-only endpoints are not used.
- **One connect method, `kind: fields`**: `access_token` (secret; a **user API token**, Profile › User API
  Tokens, with roles and an expiry), `name` (`store: label`). Injected as `header: { name:
  X-Rundeck-Auth-Token, value: "{credentials.access_token}" }` on `{instance}` under `/api`. Never the
  `authtoken` query parameter.
  - `access_levels`: `read` "Read only" / `run` "Run jobs", with `permission_hints` describing the token's
    roles and the ACL actions they need (`read`, `view`, `view_history`; plus `run`, `kill` for the second).
    Rundeck decides access with ACL policies bound to the token's roles; `connect_note` says so.
  - Probe: `GET /api/<n>/user/info` names the account (`login`). Use the API version chosen below.

## API version

Every path carries the API version (`/api/<n>/…`; current 5.x servers serve up to about 50+, the minimum
still served is 17+). Tools use `*` for the version segment (`/api/*/…`) so any version matches. The guide
tells the agent to read `system/info` once and use the version it reports (`apiversion`).

## Tool catalogue (verify against docs.rundeck.com/docs/api and the OpenAPI spec published there, beta)

**Read (GET):**
- `system/info`, `user/info`.
- Projects: `projects`, `project/*`, `project/*/readme.md` and `project/*/motd.md` if documented.
- Jobs: `project/*/jobs` (filter by group, name), `job/*` (definition; `format=yaml|json`), `job/*/info`,
  `job/*/forecast`, `job/*/executions`, `job/*/workflow`.
- Executions: `project/*/executions` (query with status, recent filter), `project/*/executions/running`,
  `execution/*`, `execution/*/state`, `execution/*/output` (with `offset`/`maxlines`, and per node
  `execution/*/output/node/*`).
- Nodes: `project/*/resources`, `project/*/resource/*`.
- Activity: `project/*/history` (if still documented), metrics endpoints only if read-only and documented.

**Write (each with an approval template):**
- `run_job` (`POST /api/*/job/*/run`): title "run a Rundeck job"; fields: job id (path), job name if the
  body or path gives it, `options` (clamped), `filter` (node filter), `loglevel`, `runAtTime`. The approval
  shows the node filter exactly as sent.
- `retry_job` (`POST /api/*/job/*/retry/*`): title "retry a Rundeck execution"; fields: job id,
  execution id, `failedNodes`.
- `abort_execution` (`POST /api/*/execution/*/abort`): title "stop a running execution".

**Refused** (`REFUSED["rundeck"]`, each a `refused …` case): **ad hoc execution** (`POST
/api/*/project/*/run/command`, `…/run/script`, `…/run/url`): they run arbitrary commands on nodes;
job import, create and delete (`POST /api/*/project/*/jobs/import`, `DELETE /api/*/job/*`,
`DELETE /api/*/jobs/delete`); bulk execution deletion; project create/delete/config; ACL policy edits
(`/api/*/system/acl/*`, `/api/*/project/*/acl/*`); **API token creation and listing**
(`/api/*/tokens`, `/api/*/tokens/*`; SEC-145); key storage (`/api/*/storage/keys/**`: private keys and
passwords for nodes).

## Agent text

- `agent.summary`: "Rundeck projects, jobs, executions and nodes: check a job's history, read execution output, run an approved job."
- `agent.instructions`: base `https://<server>[/base]/api/<version>`; Aurora adds the token; send
  `Accept: application/json`; read output in pages (`offset`, `maxlines`) and save it rather than printing
  it; running a job changes real systems, so name the job, options and nodes before asking; ad hoc commands
  are not available.
- Guide: `api-notes.md` (versions, job ids vs names and groups, execution statuses, output paging, node
  filters); recipe `why-did-a-job-fail.md` ("Find out why a job failed": the job's recent executions, the
  failed one's state per node, the failing step's output, the answer with the execution's permalink).
- `operations.json`: from `aurora plugin import-openapi` on Rundeck's published OpenAPI document if it
  imports cleanly, cut to the tools above; otherwise hand-written.

## Fixtures

Every host entry, tool (with two different API versions), approval and refused item; the header injected
only under the registered base path (`/rundeck/api/…` gets it, `/other/api/…` does not).

## Owner checks (not blocking)

Whether the owner's Rundeck is served on 443 (directly or behind a proxy). If it is only reachable on
`:4440`, it needs the instance-port change that also blocks OpenSearch (not in this wave).
