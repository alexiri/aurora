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
Viewer plus one folder's Edit the agent can only touch rules in the folder a person gave it. This
rests on Grafana's folder permissions working as documented and has **not yet been verified on a
live Grafana server**; before you rely on it, do step 3 below on your own server and try one
create from the agent.

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
would show. The gateway's shape check reads the keys and steps the script sends, but does not
require the `managed_by` label itself: a hand-built body that keeps the shape while dropping the
label is approved and sent as the card shows it, so the person approving and the folder permission
above are what keep a rule attributable to its agent. So the enforcement is layered: the script's
refusals, the person's approval, the
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

> **Who does what.** You make one thing in Grafana: the contact point. The agent makes the alert
> rules (in its own folder) and each rule names that contact point. You do not need a notification
> policy for the agent's rules, and the agent never creates or changes contact points, policies,
> mute timings or silences.

This applies to **Grafana 11 and later** (simplified routing: a rule names its contact point in
*Configure notifications*). Find your Grafana's version in the help menu (the question mark at the
top right). The steps follow Grafana's v12.4 and latest alerting documentation, whose setting names
are used below. Grafana's documentation does not describe a rule's `notification_settings` in the
provisioning API: that part is verified only against the offline fixtures, not yet on a live Grafana
server (like the Viewer account's folder permission above), so try one rule from the agent
(step 6) before you rely on it.

A contact point alone sends nothing: something must route alerts to it. For the agent's rules that
is the rule itself, which names the contact point. Aurora verifies the webhook with an
`Authorization: Bearer` header (the password kind), which Grafana's webhook contact point can send;
it cannot send Aurora's signed format, so do not use Grafana's HMAC signature settings.

1. **In Aurora:** open the agent's page, *Automations*, and the webhook ("Grafana alert" on an
   agent from the NightManager template). If it is paused, turn it on. Copy its **address** and its
   **secret**: both are shown once, when the webhook is created or a new secret is made — if you no
   longer have the secret, choose *New secret…* in the webhook's `…` menu and copy the new one; the
   old one stops working at once.
2. **In Grafana:** open *Alerts & IRM*, *Alerting*, *Notification configuration* and the
   *Contact points* tab, then choose **+ Add contact point**. Fill the form in this order:
   1. **Name**: the name the agent expects (for a NightManager agent: the one in its notes, in the
      template's example `aurora-nightmanager`). The agent's rules name the contact point by this
      name, so it must match exactly.
   2. **Integration**: **Webhook**.
   3. **URL**: the address you copied from Aurora. Never put the secret in the URL.
   4. **HTTP Method**: **POST**: the webhook receives its alerts as a POST.
   5. **Authentication Header Scheme**: `Bearer` (Grafana's default). Aurora checks an
      `Authorization: Bearer` header. Do not use Basic Authentication: Grafana takes either that or
      the header, not both.
   6. **Authentication Header Credentials**: the secret you copied from Aurora.
   7. **Max Alerts**: **10**. One notification then carries at most 10 alerts. Any alerts beyond
      that are left out, and the payload's `truncatedAlerts` says how many were left out: the
      agent reads that number and looks the rest up itself. Grafana says `0` means no limit: do
      not use it, because a flood of alerts would arrive as one very large message.
   8. **Disable resolved message**: switch it **on**. A "resolved" notice would wake the agent for
      nothing.
   9. Leave everything else at its default, including the message and title templates (the
      payload the agent reads is Grafana's default JSON) and the TLS settings. Do not use the
      HMAC signature settings: Aurora cannot verify Grafana's signature.
   10. Choose **Test** (Aurora answers the test delivery), then **Save contact point**. Back in
      Aurora, the webhook's **Deliveries** shows the test and how it was answered: accepted, or
      refused with the reason in words (wrong secret, paused, payload too large).
3. **Routing: nothing for you to do.** The agent's rules are created with the contact point's name
   in their notification settings (in Grafana's rule form: *Configure notifications*, **Default**,
   the contact point selected). Grafana then sends that rule's alerts to that contact point and
   does not use notification policies for it. The helper script checks that a contact point of that
   name exists before it creates the rule. So you make no notification policy, and the agent makes
   none either.
4. **Timing:** a rule that names a contact point is grouped and repeated by Grafana's own defaults
   for such rules (group wait 30 seconds, group interval 5 minutes, repeat interval 4 hours), and
   the agent cannot change them: the script refuses any notification setting except the contact
   point. A rule that stays firing therefore reaches the agent about every 4 hours. Aurora also
   records a repeated identical delivery as a duplicate without waking the agent, but a payload
   that changes every time is a new delivery.

   *If you use notification policies anyway:* a rule that names a contact point bypasses the
   policy tree, so your policies, their matchers and their mute timings do not apply to the
   agent's rules. Routing the agent's rules through the tree instead (a rule with no contact point
   of its own, matched by a policy) is a different setup that is not supported: the script always
   requires a contact point, and the agent cannot set grouping, timing, mute timings or silences
   on a rule.
5. **Folder and token** for the agent's rules: the folder named for the agent, the service account
   left a **Viewer** with Edit on that one folder only, and its token in the agent's Grafana
   connection — *Alert rules* above has the steps.
6. **Check the whole path:** create a harmless test rule (for example `up == 1`, `for: 1m`, on a
   throwaway label, in the agent's folder) and watch it fire: Grafana's *Alert rules* list shows
   the rule firing, and Aurora's webhook **Deliveries** shows the delivery and its thread with the
   payload inside (marked as untrusted data). Delete the test rule from the rule's menu when you
   are done.
7. **Turning it off:** pause the webhook in Aurora (its `…` menu) or delete the contact point in
   Grafana. Neither deletes the agent's rules; they expire by themselves, and you can see and
   delete them in *Alerting*, *Alert rules*, filtered by the agent's folder.

### Checklist

Follow these in order; each is one thing to look at or do.

1. Aurora: the agent's webhook exists and is switched on; its address and secret are copied.
2. Grafana: a contact point with the agent's name, integration **Webhook**, **URL** the Aurora
   address, **HTTP Method** POST.
3. **Authentication Header Scheme** `Bearer`, **Authentication Header Credentials** the Aurora
   secret.
4. **Max Alerts** 10, **Disable resolved message** on.
5. **Test** answers in Grafana, and Aurora's **Deliveries** shows the test as accepted.
6. No notification policy to make. The agent's rules name the contact point.
7. The service account is a Viewer with Edit on the agent's one folder (*Alert rules* above).
8. A harmless test rule from the agent fires, and its delivery reaches the agent's thread.

### If nothing arrives

- **The contact point's Test fails or shows nothing:** Grafana's own result next to **Test**, and
  Aurora's webhook **Deliveries** (nothing there means Grafana never sent).
- **The webhook is paused (or the agent frozen):** the webhook's row in the agent's *Automations*.
- **Wrong secret:** Deliveries shows a refused row saying so; check **Authentication Header
  Credentials**, or make a new secret with *New secret…* and paste it.
- **The rule names a different contact point:** the rule's *Configure notifications* in *Alert
  rules* (the name must equal the contact point's name exactly).
- **The rule is paused or failing:** its state in *Alert rules*.
- **The alert is still pending:** the rule's `for` duration has not passed yet; the state shows
  *Pending* until it does.
- **Grafana sent, or tried to:** the rule's *State history* (the *Alert state history* view) in
  Grafana.

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
