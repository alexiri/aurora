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
   - **Viewer**: search and read dashboards and folders, run queries on the data sources Viewers
     may query, read alert rules, alerts, silences and annotations — and, with edit permission on
     one folder, create, change and delete alert rules in that folder (*Alert rules* below).
   - **Editor**: the same reads, plus saving dashboards, adding annotations and creating or ending
     silences. By default Grafana's Editors can edit **every** folder, so an Editor token gains
     nothing from a folder permission: choose Editor only when agents must save dashboards, add
     annotations or end silences, and never to let them write alert rules.
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
- Grafana alert rules and their state, Alertmanager alerts and silences, annotations, and contact
  points' names and types (a contact point's settings and secrets never reach the agents: the list
  operation returns those two fields only).

Every request must match one of these tools or one of the held ones below, whatever permission the
agent has, full access included. Every other request is blocked, and the token is never sent with it:
other proxy paths (the Prometheus admin and status APIs, other data source types) and the rest of
Grafana's API.

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
- **Create, change or delete an alert rule** (`POST /api/v1/provisioning/alert-rules`, `PUT` and
  `DELETE …/alert-rules/<uid>`): the approval shows the rule the script builds — title, folder,
  group, the data source and query, the comparison and threshold, the `for` duration, the states,
  the labels and annotations each under its name, and the receiver. See *Alert rules* below.

## Alert rules

Agents can create, change and delete Grafana-managed alert rules and list the contact points the
server has (names and types only, never their settings or secrets). Reading rules, their state and
alerts works with Viewer.

Keep the service account's role **Viewer** and give it **Edit on one folder** — the folder named
for the agent, and no other. The role matters: by default Grafana's Editors can edit every folder,
so an Editor token is not narrowed by Edit permission on one folder, and its raw writes could
still change or delete people's rules anywhere. A Viewer has no edit anywhere by default, so with
Viewer plus one folder's Edit the agent can only touch rules in the folder a person gave it:

1. In Grafana open *Dashboards* and choose *New folder*; name it for the agent, for example
   "night agent". Give **each agent its own folder**: two agents sharing one folder could change
   or delete each other's rules and share the same caps.
2. Open the folder, choose *Permissions*, *Add a permission*, pick the service account and give it
   **Edit**.
3. Check that the other folders are not editable: open another folder's *Permissions* page and
   confirm the service account is not listed there (or is listed only with **View**). Grafana's
   folder permissions decide where a rule may live, not Aurora.

A rule an agent makes carries a `managed_by` label (which agent left it) and an `expires` label and
annotation, and the app's helper script builds the rule, refuses to touch rules without its own
label or outside its folder, and refuses to change a rule that left the shape it builds — a
recording rule (`record`), a second query, a label or annotation other than `managed_by`,
`expires`, `service` and `summary`, or a receiver setting other than `receiver`. The script is the
only supported way to write a rule. The approval card shows that shape field by field, but its
template cannot print a map's keys: a body built by hand could carry fields — a recording rule, a
second query on a SQL data source, a label a notification policy matches — that no card field
would show. So the enforcement is layered: the script's refusals, the person's approval, the
folder permission above, and Grafana's own rule permissions. Rules made by an agent appear in the
Grafana UI like any rule: people can edit and delete them, and the app sends a provenance header
so Grafana does not lock them against editing.

Where a rule's notifications go — the contact point and the notification policy tree — stays with
people: no tool creates or changes a contact point, notification policy, mute timing or
notification template, and agents can only read contact points' names and types (their settings and
secrets are not returned at all), to check that the receiver a
rule names exists. *Send alerts to an agent* below walks through making one.

Each rule is one query Grafana evaluates on its group's interval for as long as it exists, so every
rule costs evaluation time. Agent rules carry an expiry (at most 30 days) and the helper script
caps how many it manages, but a person can delete an expired or unwanted rule in the Grafana UI at
any time.

## Send alerts to an agent

An agent's alert rule notifies a **contact point**, and only a person can create one. This
walkthrough makes a contact point that calls the agent's webhook in Aurora. The steps were checked
against Grafana 13's documentation (November 2026); Grafana's wording is quoted as its current UI
uses it. Aurora verifies the webhook with an `Authorization: Bearer` header (the password kind),
which Grafana's webhook contact point can send; it cannot send Aurora's signed format, so do not
use Grafana's HMAC signature settings.

1. **In Aurora:** open the agent's page, *Automations*, and the webhook ("Grafana alert" on an
   agent from the NightManager template). If it is paused, turn it on. Copy its **address** and its
   **secret**: both are shown once, when the webhook is created or a new secret is made — if you no
   longer have the secret, choose *New secret…* in the webhook's `…` menu and copy the new one; the
   old one stops working at once.
2. **In Grafana:** open *Alerts & IRM*, *Alerting*, *Notification configuration* and the
   *Contact points* tab, then choose **+ Add contact point**.
   - **Name**: the name the agent expects (for a NightManager agent: the one in its notes, in the
     template's example `aurora-nightmanager`).
   - **Integration**: **Webhook**. **URL**: the Aurora address. **HTTP Method**: **POST**.
   - **Authentication Header Scheme**: `Bearer` (the default), and **Authentication Header
     Credentials**: the Aurora secret. Do not use Basic Authentication, and do not put the secret
     in the URL.
   - Optional settings: switch **Disable resolved message** on (a resolved notice would wake the
     agent for nothing); set **Max alerts** (for example 10) so one notification does not carry a
     hundred alerts; leave the message template at the default — the payload the agent reads is the
     default JSON.
   - Choose **Test** (Aurora answers the test delivery), then **Save contact point**. Back in
     Aurora, the webhook's **Deliveries** shows the test and how it was answered — accepted, or
     refused with the reason in words (wrong secret, paused, payload too large).
3. **Route notifications to it.** Either
   - the rule names the contact point itself (in the rule form, *Select contact point*; Grafana 11
     and later) — this is what the agent's rules do: the receiver a rule names **is** a contact
     point's name, and the helper script checks it exists before it creates the rule; or
   - on older Grafana, a **notification policy**: in *Notification configuration*, *Notification
     policies*, add a policy whose matcher is the label the agent's rules carry
     (`managed_by = nightmanager` in the template's example) and whose contact point is the one you
     made. Tell which Grafana you have from its version, shown in the help menu.
4. **Timing that protects you:** on the notification policy (or the rule's route settings), set a
   **repeat interval of at least 4 hours**, so a rule that stays firing does not wake the agent
   every few minutes, and leave the group wait and group interval at their defaults unless you know
   otherwise. Aurora also records a repeated identical delivery as a duplicate without waking the
   agent — but a payload that changes every time is a new delivery.
5. **Folder and token** for the agent's rules: the folder named for the agent, the service account
   left a **Viewer** with Edit on that one folder only, and its token in the agent's Grafana
   connection — *Alert rules* above has the steps.
6. **Check the whole path:** create a harmless test rule (for example `up == 1`, `for: 1m`, on a
   throwaway label, in the agent's folder) and watch it fire: Grafana's *Alert rules* list shows
   the rule firing, and Aurora's webhook **Deliveries** shows the delivery and its thread with the
   payload inside (marked as untrusted data). Delete the test rule from the rule's menu when you
   are done. When nothing arrives, look at, in this order: the contact point's **Test** result in
   Grafana, the rule's state in *Alert rules* (firing? or is the rule paused or failing?), and in
   Aurora the webhook's **Deliveries** (nothing at all means Grafana never sent; a refused row says
   why) and whether the webhook is paused or the agent frozen.
7. **Turning it off:** pause the webhook in Aurora (its `…` menu) or delete the contact point in
   Grafana. Neither deletes the agent's rules; they expire by themselves, and you can see and
   delete them in *Alerting*, *Alert rules*, filtered by the agent's folder.

## Limits

- Never available as tools: creating service accounts or their tokens, API keys, user and
  organization administration, creating, changing or deleting data sources (they hold the backends'
  credentials), deleting dashboards, installing plugins, and creating or changing contact points,
  notification policies, mute timings or notification templates.
- Not included: Grafana's newer `/apis/…` API, folders and dashboard permissions, playlists,
  library panels, reports, OnCall and Incident, and other data source proxy paths.
- Other sign-in methods are not offered: a person's password (most servers use single sign-on),
  legacy API keys (replaced by service accounts) and Grafana's JWT authentication.
- Approval links are not shown: open the dashboard or *Alerting, Silences* in Grafana yourself.
