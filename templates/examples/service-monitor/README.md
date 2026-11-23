# Service monitor

## What it does

The agent watches Grafana dashboards and turns what it sees into short,
actionable summaries:

- **Every weekday morning** it posts a three-line state report per service:
  state, since when, and what to do (or which open ticket covers it).
- **On an alert** it triages: reads the dashboards, checks whether the
  problem is already a Jira issue, and writes a one-paragraph summary with
  the deciding numbers.
- **On request** it files a Jira issue or a comment — but only the text it
  proposed and you approved.

It reads; it does not fix. It has no write access to Grafana, no write access
to GitLab, and writes to Jira only through tools that ask you first. It never
messages anyone on its own.

## Who it's for

Service owners and on-call engineers who already keep their dashboards in
Grafana and track their work in Jira. It suits a team that wants a second
pair of eyes on the dashboards, not an automated pager.

## What it needs

- **Grafana** (read): the agent reads the dashboards in the folder you choose
  and runs the Prometheus queries behind them.
- **Atlassian / Jira** (custom): reading is allowed; creating issues and
  commenting ask you first.
- **GitLab** (optional, read): useful when an alert points at a recent
  deployment; leave it out if your services are not on GitLab.
- **A skill library** (optional): if your organization provides the
  `incident-writeup` skill, the agent writes incident summaries in your
  service desk's format. Without it, everything else works the same.

## What it does on its own

A schedule **"Morning check"** runs every weekday morning and writes the
state report into its own conversation. It never messages you first.

## Adapting it

After creation you can point the instructions at different dashboards, change
the morning check's time, or add skills. The helper script
`.aurora/helpers/alert-summary.sh` is read-only; ask for changes through a
new template version so every deployment gets them reviewed.
