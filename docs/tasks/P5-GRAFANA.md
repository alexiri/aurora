# P5-GRAFANA: built-in `grafana` package — dashboards, metrics and logs through self-hosted Grafana (phase 5, D-429, SEC-145)

Read `docs/tasks/P5-CATALOG.md` first (common rules, acceptance, order). This brief owns
`plugins/builtin/grafana/**`, its `REFUSED` entry and its `ICONS.md` row.

## Goal

An agent answers "why did the error rate jump at 14:00?", "what does the MONIT dashboard for X show?",
"which alerts are firing?" by searching dashboards, reading panel queries, and running PromQL and LogQL
against the datasources a Grafana instance already has, **including Prometheus and Mimir**. One Grafana
connection reaches every datasource its service account may query, so no separate Prometheus or Mimir app
is needed (D-428).

## Hosting and connect (D-429)

- **Instance-bound, no default host** (`instances`, like `indico`): Grafana is self-hosted (Grafana Cloud
  stacks are `<stack>.grafana.net`, which an admin can register as an instance too). Base paths are
  allowed (Grafana behind `root_url` with `serve_from_sub_path`). Registration probe:
  `GET /api/health` expecting `[200]`.
- **One connect method, `kind: fields`**: a **service account token** (Grafana › Administration › Service
  accounts › Add token), injected as `bearer` on `{instance}` under `/api`. Fields: `name`
  (`store: label`, optional), `access_token` (secret). Connect probe: `GET /api/user` expecting `[200]`.
  - `access_levels`: `read` "Viewer" (`permission_hints`: "Role · Viewer"), `write` "Editor" ("Role ·
    Editor"). Say in `connect_note` that the token's role and folder permissions decide what the agent can
    see, and that a Viewer token is enough for everything except the write tools.
  - Not offered: basic auth with a person's password (most instances use single sign-on, and a password
    is the wrong thing to store), legacy API keys (deprecated, migrated to service accounts), Grafana's
    JWT auth (an instance-specific setup). Say in the report if a method looks worth adding.
- A service account belongs to **one Grafana organization**; the token decides which. The guide says so.

## Tool catalogue (verify each against the Grafana HTTP API docs, current release)

Use the legacy `/api/...` paths (still served; the `/apis/...` Kubernetes-style API is newer and optional).
Order matters: specific datasource-proxy patterns come before anything broader.

**Read (GET only):**
- Identity and search: `/api/user`, `/api/org`, `/api/search` (dashboards and folders), `/api/folders`,
  `/api/folders/*`.
- Dashboards: `/api/dashboards/uid/*`, `/api/dashboards/uid/*/versions`, `/api/dashboards/tags`.
- Datasources: `/api/datasources`, `/api/datasources/uid/*`, `/api/datasources/uid/*/health`.
- **Prometheus/Mimir through the proxy** (`/api/datasources/proxy/uid/*/…`), each its own tool:
  `api/v1/query`, `api/v1/query_range`, `api/v1/series`, `api/v1/labels`, `api/v1/label/*/values`,
  `api/v1/metadata`, `api/v1/rules`, `api/v1/alerts`, `api/v1/query_exemplars`.
- **Loki through the proxy**: `loki/api/v1/query`, `loki/api/v1/query_range`, `loki/api/v1/labels`,
  `loki/api/v1/label/*/values`, `loki/api/v1/series`.
- Alerting: `/api/prometheus/grafana/api/v1/rules` (Grafana-managed rules and their state),
  `/api/v1/provisioning/alert-rules`, `/api/v1/provisioning/alert-rules/*`,
  `/api/alertmanager/grafana/api/v2/alerts`, `/api/alertmanager/grafana/api/v2/silences`.
- Annotations: `/api/annotations`.

The proxy forwards any method and any path to the backend, so **only the listed backend paths are tools**.
Everything else under `/api/datasources/proxy/` stays untooled (refused under custom grants and
`block_untooled`), including the Prometheus admin API (`api/v1/admin/**`).

**Write (each with an approval template):**
- `create_annotation` (`POST /api/annotations`): title "add an annotation"; fields: text, tags, dashboard
  UID, time.
- `update_dashboard` (`POST /api/dashboards/db`): title "save a dashboard"; fields: dashboard title, UID,
  folder UID, `overwrite`, message. The guide tells the agent to read the current version first and to keep
  `overwrite: false`.
- `create_silence` (`POST /api/alertmanager/grafana/api/v2/silences`): title "silence alerts"; fields:
  matchers, starts/ends, comment.
- `delete_silence` (`DELETE /api/alertmanager/grafana/api/v2/silence/*`): title "end a silence".
- `query_datasources` (`POST /api/ds/query`): Grafana's unified query endpoint. It only reads, but its body is
  JSON, so it is a held write (rule 2). Title "run a datasource query (read only)"; fields: datasource UIDs,
  the first query's expression. The guide sends agents to the GET proxy tools first and here only for
  datasources without one (Elasticsearch/OpenSearch, SQL).

**Refused** (`REFUSED["grafana"]`, each a `refused …` case): service-account token creation
(`POST /api/serviceaccounts/*/tokens`, SEC-145), service-account creation, API key creation
(`POST /api/auth/keys`), user and organization admin (`/api/admin/users`, `/api/orgs`), datasource
create/update/delete (`/api/datasources` POST, `/api/datasources/uid/*` PUT/DELETE: datasources hold
backend credentials), dashboard deletion (`DELETE /api/dashboards/uid/*`), plugin install.

## Agent text

- `agent.summary`: "Grafana dashboards, Prometheus/Mimir metrics, Loki logs and alerts: find a dashboard, run a query, see what fires."
- `agent.instructions` (≤ 600 chars): the base URL is the account's address plus `/api`; Aurora adds the
  token; find a datasource UID with `/api/datasources` before querying; query Prometheus/Mimir/Loki
  through `/api/datasources/proxy/uid/<uid>/…` with GET; keep time ranges and `step` small and use
  `aurora-agent` so large results are saved, not printed.
- Guide files: `api-notes.md` (search, dashboards, datasource UIDs, the proxy paths, alerting,
  annotations, errors); recipe `investigate-a-spike.md` ("Find out why a metric jumped": find the
  dashboard, read the panel's queries, run them over the window, compare with the previous day, look for
  annotations and firing alerts, answer with a deep link built from the dashboard URL).
- `operations.json`: hand-written for the read tools above (path parameters: datasource `uid`, label
  `name`; query parameters: `query`, `start`, `end`, `step`, `limit`). Grafana serves an OpenAPI document
  on the instance (`/swagger`); use it to check the shapes if one is at hand, not as the source of the
  operation list.

## Fixtures

Every host entry and tool (positive and `negative <tool>:` cases), every approval template, each refused
item. In particular: a proxy GET to `api/v1/query` selects the Prometheus query tool; a proxy `POST
api/v1/admin/tsdb/delete_series` and a proxy GET to an unlisted backend path select no tool; the bearer is
injected only on the registered instance host and under its base path (`expect: { refused:
host_mismatch }` for another host).

## Owner checks (not blocking)

Whether CERN's `monit-grafana.cern.ch` lets a group create service-account tokens, and for which Grafana
organizations. A private (internal-only) Grafana needs `gateway.allowed_private` and a network-policy
allowance like any private host.
