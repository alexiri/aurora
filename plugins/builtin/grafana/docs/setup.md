# Connect Grafana

Grafana runs on your own server (or a Grafana Cloud stack, `<stack>.grafana.net`). Aurora adds a
service account token to the requests your agents send to that server, under `/api` only; agents
never see the token. Through Grafana, agents can also query the Prometheus, Mimir and Loki data
sources the server already has: Grafana holds those backends' credentials and tenant headers, so no
separate Prometheus or Loki connection is needed.

## Register the server

An administrator registers the server once: open {{aurora.page:self_hosted}}, choose Grafana and
enter the host name, for example `grafana.example.com` (no `https://` or port). If Grafana is served
under a path (`root_url` with `serve_from_sub_path`, for example `https://example.com/grafana`), fill
**Path** too; pasting the full address fills both. Aurora checks `GET /api/health` first.
Registering decides where credentials may be sent, so only register servers you trust.

If the server has a private address, the operator must allow it with `gateway.allowed_private` (and
the gateway's network policy).

## Create a service account token

1. In Grafana open *Administration, Users and access, Service accounts* and choose *Add service
   account*. Give it a name such as "Aurora agents".
2. Choose the role. The connect window shows what to pick for the access you chose:
   - **Viewer**: search and read dashboards and folders, run queries on the data sources Viewers may
     query, read alert rules, alerts, silences and annotations.
   - **Editor**: the same, plus saving dashboards, adding annotations and creating or ending silences.
     Reading a dashboard's saved versions also needs edit permission on that dashboard.
   Folder and data source permissions narrow both roles further. Do not give the service account the
   Admin role: agents do not need it.
3. Choose *Add service account token*, set an expiry, and copy the token (`glsa_…`). Grafana shows it
   once.

A service account belongs to **one Grafana organization**: the token reads only that organization's
dashboards, data sources and alerts. For several organizations, connect once per organization and
give each connection its own name.

Sources: Grafana's [service accounts](https://grafana.com/docs/grafana/latest/administration/service-accounts/)
and [roles and permissions](https://grafana.com/docs/grafana/latest/administration/roles-and-permissions/)
documentation.

## Connect

Choose the registered server and enter the **token**. The optional **Name** (for example "Grafana
production") is what agents call the connection; give each connection its own name when you have
several. Left empty, the connection is called "Grafana". Aurora checks the token with
`GET /api/user`.

## What agents can do

Reading is allowed by default:

- searching dashboards and folders, reading a dashboard's panels and queries, dashboard tags;
- data sources (`GET /api/datasources` lists only those the token may read settings of, which
  Grafana gives to Admin by default; agents take data source UIDs from dashboards instead) and their
  health; stored passwords and keys are never returned;
- Prometheus and Mimir queries through Grafana's data source proxy, `GET` only and only on these
  backend paths: `api/v1/query`, `query_range`, `series`, `labels`, `label/<name>/values`,
  `metadata`, `rules`, `alerts` and `query_exemplars`;
- Loki through the proxy, `GET` only: `loki/api/v1/query`, `query_range`, `labels`,
  `label/<name>/values` and `series`;
- Grafana alert rules and their state, Alertmanager alerts and silences, annotations.

Every other proxy path (the Prometheus admin and status APIs, other data source types) matches no
tool, so agents with a custom permission cannot reach it.

These are separate permissions and are held for approval:

- **Add an annotation** (`POST /api/annotations`): the approval shows the text, tags, dashboard,
  panel and times.
- **Save a dashboard** (`POST /api/dashboards/db`): the dashboard title and UID, the folder,
  whether it overwrites, and the version message.
- **Silence alerts** (`POST /api/alertmanager/grafana/api/v2/silences`): the matchers, start, end
  and comment, and the silence it changes if any. **End a silence** (`DELETE …/silence/<id>`).
- **Run a data source query** (`POST /api/ds/query`): Grafana's unified query only reads, but its
  query is in the body, so it is held like a change; the approval shows the data sources and the
  first query (PromQL, SQL or other). Agents use it for data sources without a proxy tool
  (Elasticsearch, OpenSearch, SQL). On a SQL data source the statement runs as that data source's
  database user, so give SQL data sources read-only database users.

## Limits

- Never available as tools: creating service accounts or their tokens, API keys, user and
  organization administration, creating, changing or deleting data sources (they hold the backends'
  credentials), deleting dashboards, and installing plugins.
- Not included: Grafana's newer `/apis/…` API, folders and dashboard permissions, playlists,
  library panels, reports, OnCall and Incident, and other data source proxy paths.
- Other sign-in methods are not offered: a person's password (most servers use single sign-on),
  legacy API keys (replaced by service accounts) and Grafana's JWT authentication.
- Approval links are not shown: open the dashboard or *Alerting, Silences* in Grafana yourself.
