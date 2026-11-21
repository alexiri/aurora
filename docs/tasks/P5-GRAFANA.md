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

## Completion report (2026-11-21, lane `p5-platform`, Opus)

**Done: every item of the brief.** Nothing waits. The other four apps of P5-CATALOG were not touched
(owner: on hold).

### Files

- New package `plugins/builtin/grafana/` (1.0.0): `plugin.yaml`; `docs/setup.md`;
  `assets/icon.png` and `assets/icon-dark.png`; `agent/grafana/` with `api-notes.md`, the recipe
  `investigate-a-spike.md` and `operations.json` (31 read operations); `tests/` with `read.yaml`,
  `write.yaml`, `refused.yaml`, `proxy-boundaries.yaml` and `instance.yaml` (131 cases).
- `scripts/plugins/check_builtin.py`: `REFUSED["grafana"]`, 11 entries: service-account token
  creation, service-account creation, API key creation, user admin, organization admin, datasource
  creation, update and deletion, dashboard deletion, plugin install, and the Prometheus admin API.
- `scripts/guards/no-integration-names.sh`: `grafana` added to `TERMS`. Nothing in `crates/` or
  `services/` names it, so no allow entry was needed. The `common-passwords.txt` allow entry of rule 8
  is for `zoom` and `foreman` and comes with their briefs.
- `plugins/builtin/ICONS.md`: the `grafana` row.

Test files changed or removed outside the package: none. No dashboard code, label, route, mock,
golden file or count of built-in apps changes. I searched `web/tests`, `web/e2e` (`mock-api.ts` reads
named packages only), `scripts/e2e` and the Rust ITs. The tests that read every built-in package
(`aurora-registry` `snapshot`, `manifests`, `agent_guide`, `builtin_versions`) pass with it.

### What was verified, and against what

- **Routes and methods**: Grafana **v13.3.1** (latest release, 2026-11-21) source code:
  `pkg/api/api.go`, `pkg/api/folder.go`, `pkg/services/serviceaccounts/api/api.go`, and the ngalert
  generated routes (`generated_base_api_{alertmanager,prometheus,provisioning}.go`). Parameters and
  bodies were checked against `public/openapi3.json` (cited as `about.api_spec`) and
  `pkg/services/ngalert/api/tooling/post.json` (`postableSilence`). The docs are cited as
  `about.api_docs`: the legacy HTTP API, deprecated from Grafana 13 but "not being disabled for the
  moment" (`apis-migration.md`). Grafana 13's docs no longer describe `/api/dashboards/*` and
  `/api/folders`, so their shapes come from the v12.4.15 docs; the routes are unchanged in v13.3.1.
- **Every endpoint in the brief is a tool.** Nothing was left out. Notes:
  - `GET /api/datasources` requires `datasources:read`. Grafana gives that only to **Admin** by
    default (`rbac-fixed-basic-role-definitions`, `fixed:datasources:reader`). A Viewer or Editor
    token gets an empty list, or only the built-in `grafana` source (`GetDataSources` filters by read
    permission), although queries work (`datasources:query`).

    The agent text therefore takes data source UIDs from dashboard panels first. The tool
    descriptions, `connect_note` and the setup guide say so. A `GET /api/frontend/settings` tool would
    list the queryable data sources for Viewers. It is not in D-429, so it stays untooled (a
    `proxy boundary` case shows that). **Owner's call** whether to add it.
  - `GET /api/dashboards/uid/*/versions` requires `dashboards:write`, so Editor or edit permission on
    the dashboard. The description and the guide say so.
  - `POST /api/auth/keys` is **not routed in v13.3.1** (API keys were removed). The refused case
    stays for older servers.
  - `POST /api/plugins/*/install` is routed only with `plugin_admin_enabled`. Refused either way.
- **Connect**: service account token as bearer on `{instance}` under **`/api/`**. The brief says
  `/api`, but the gateway compares `path_prefix` as a plain string (`aurora-registry`
  `identify_instance_app`, `path.starts_with(prefix)`), so `/api` would also inject the token on
  Grafana's `/apis/…` layer. The fixture "boundary: the newer /apis layer …" caught that.
  `check_builtin.py`'s `entry_matches` compares by segment, so its coverage check and the gateway
  disagree here; see Findings.

  Probes: `GET /api/health` for registration and `GET /api/user` for connect, both `[200]`. There
  are no access-level scopes, only hints ("Role · Viewer", "Role · Editor").
- **Approval links**: none. No link variable carries an instance's base path (`{request.host}`
  only, `09 §3.4 approvals`), so a link would be wrong for a Grafana under `/grafana`. The setup
  guide says so.
- **Not offered** (brief): basic auth with a person's password, legacy API keys, JWT auth. None
  looks worth adding: service accounts are Grafana's documented way for automation.
- **Icon**: Simple Icons 16.37.0. The tarball sha256 matches the pin (`cd9faf0e…`). Slug `grafana`,
  `#F46800`, SVG sha256 `e6da4ab9…`, rendered like the others. I re-rendered `gitlab` the same way
  and it is pixel-identical. Contrast on the dark surface is 5.74:1, so the dark icon is the same file.

### Security review notes (P5-CATALOG §Security review, SEC-145, SEC-04/59)

1. **Only the named backend paths are tools.** The 14 proxy tools are exact patterns
   `/api/datasources/proxy/uid/*/<backend path>`, GET only, with no `**`. `PathPattern::matches`
   (`aurora-policy/src/path.rs`) needs exactly as many segments as the pattern, and `*` covers one
   non-empty segment. So an extra, encoded or dot segment after a tool path never selects that tool.

   Evidence: `tests/proxy-boundaries.yaml`, 25 cases, all passing. Each of these selects no tool:
   - unlisted GETs: `status/config`, `status/flags`, `targets`, `admin/tsdb/snapshot`;
   - dot and encoded variants after a tool path: `..`, `%2e%2e`, `%2E%2E`, `%252e%252e`, `%2F`, `//`;
   - suffix variants: `/admin`, `.json`, `;x=1`, an encoded `?`, upper case;
   - other routes: the numeric-id proxy route, the proxy root, DELETE on a tool path, Mimir
     cardinality, an Elasticsearch index API, the data source `resources` route (a second proxy), the
     plugin proxy, `frontend/settings`.

   Two cases show what stays the same tool: one trailing slash (FIX-109), and a query string, which
   never changes the backend path. `refused.yaml` adds the brief's proxy `POST
   api/v1/admin/tsdb/delete_series`, and `snapshot`.
2. **Gateway normalization (`06` step 3) against what Grafana forwards.**
   - **Gateway.** `hygiene::normalize_path` decodes percent-encoded unreserved characters (so `%2e`
     becomes `.`). It refuses `.` and `..` segments, `%2F`, `%5C`, `\` and NUL with 400
     `ambiguous_path`, and collapses `//`. Other escapes stay encoded (upper-cased), so `%252e`
     stays a literal segment.

     The gateway forwards **the normalized path** (`proxy/mod.rs` passes it to `decide`, and
     `PreparedRequest.path` is built from it). So the matcher and Grafana see the same segments.

     Evidence, run here: `cargo test -p aurora-gateway --lib hygiene` (4 passed: `/a/../b`,
     `/a/./b`, `/%2e%2e/x`, `/a/%2E`, `/a/%2Fb`, `/a%5Cb`, `/a%00` refused) and
     `cargo test -p aurora-gateway --test it method_override_and_ambiguous_paths` (1 passed: 400
     `ambiguous_path` end to end).
   - **Grafana** (v13.3.1, `pkg/services/datasourceproxy/datasourceproxy.go` and
     `pkg/api/pluginproxy/ds_proxy.go`):
     - The data source UID must match `^[a-zA-Z0-9_-]*$` (`IsValidShortUID`, else 400). So the uid
       segment cannot carry `.`, `%` or `;`, and the regex `^/api/datasources/proxy/(\d+|uid/[\w-]+)/?`
       strips exactly that segment.
     - The rest of the **escaped** path is appended to the data source URL
       (`JoinURLFragments(targetUrl.Path, proxyPath)`) and unescaped **once** into `URL.Path`. Go
       then sends `RawPath` because it is a valid encoding of `Path`, so nothing is decoded twice.
       A `%252e%252e` reaches the backend as the literal `%2e%2e`, not as a dot segment.
     - What the gateway accepts therefore arrives at the backend as the same segments, under the
       data source's own base URL (for Mimir, `/prometheus`).
   - **Defence in depth.** Grafana itself refuses POST, PUT and DELETE on any proxy path of a
     Prometheus, Amazon or Azure Prometheus or Loki data source that matches no plugin route
     (`validateRequest`). So the TSDB admin API (all POST/PUT) is unreachable through Grafana even
     under a `full` grant. Mimir data sources are of type `prometheus`.
   - **Gap in the offline runner** (not a package defect; see Findings). `aurora plugin test`
     (`check_snapshot.rs`) matches the **raw** request path, without step 3. A fixture with a dot or
     `%2F` segment *inside* a `*` position would select a tool offline although production refuses it
     with 400. Example: `…/proxy/uid/x%2Fy/api/v1/query`, also refused by Grafana's UID check. No
     such case was written. The cases above hold with or without normalization.
3. **No read tool returns stored data source secrets.** `GET /api/datasources` builds
   `DataSourceListItemDTO`, and `GET /api/datasources/uid/*` uses `convertModelToDtos` through
   `getK8sDataSourceByUIDHandler`. Neither has a password field. Secrets appear only as
   `secureJsonFields` booleans ("is set"). `user` and `basicAuthUser` are user names, and `jsonData`
   holds non-secret settings.

   The other reads return no credentials either: the user, the org, dashboards, alert rule
   definitions, alerts, silences and annotations. The proxy tools do not include Prometheus
   `status/config`. Contact points and their `…/export?decrypt=true`
   (`alert.provisioning.secrets:read`) are not tooled.
4. **SEC-145.** No tool creates or reveals a credential. Each case in `refused.yaml` passes with "no
   tool": service-account token creation and token listing, service-account creation, API key
   creation, user admin (create, set a password), organization admin (create, add a user), data
   source create, update and delete, dashboard deletion, plugin install, and the Prometheus admin
   API. `check_builtin.py` enforces one passing case per `REFUSED` entry.
5. **Credentials only on the registered host and base path** (SEC-04/59, D-231). Evidence:
   `instance.yaml`.
   - Injected on the registered host under `/api/`, including a Grafana Cloud stack registered as an
     instance.
   - Unmatched (nothing injected) on: another host, a look-alike host
     (`grafana.example.org.attacker.example`), an unregistered host, `/login`, `/apis/…` and the web
     interface's service-account pages.
   - Under a base path `/grafana`: matched on the app path, and `{path.7}` of `delete_silence` is read
     from the app path. Without the base path, on the sibling `/grafana2` and on another host:
     unmatched.
   - The brief asked for `refused: host_mismatch` on another host. An instance-only app has no
     claim there, so the request is not a Grafana request at all and **nothing** is injected
     (`unmatched`, as for Indico). `host_mismatch` needs a second claim on the host.
6. **Writes.** All 5 write tools are held, each with an approval template.
   - `create_silence` shows every matcher's name, value, `isEqual` and `isRegex`. An `isEqual: false`
     matcher silences everything *except* the value, so the approver must see it. It also shows the
     body `id`, which turns a create into a change of that silence.
   - `query_datasources` shows the first query's `expr`, `rawSql` or `query` and every data source
     UID. On a SQL data source the statement runs as that data source's database user, so it can
     write. The setup guide tells admins to give SQL data sources read-only users.
   - No `grants` flags (`custom_only` / `block_untooled`), as for sentry and gitlab. Untooled
     requests are refused under custom grants. Under a `full` grant the token's role is the limit.
     The credential, user, organization, data source and plugin endpoints need the organization
     Admin role or Grafana server admin, which a Viewer or Editor token lacks. Dashboard deletion
     is within an Editor's reach under a `full` grant, as with any app whose token can delete. The
     setup guide says not to give the service account Admin.

     If the reviewer wants untooled requests blocked under every grant, set `block_untooled: true`.
     That is a one-line change, and it is a widening-free minor bump later.

### Findings for the Orchestrator (outside this lane's files)

- **F1.** The gateway matches `hosts[].match.path_prefix` as a string prefix
  (`crates/aurora-registry/src/identify.rs:52`), while `scripts/plugins/check_builtin.py`
  `entry_matches` reads it as a segment prefix; `09 §3.4` does not say which. A prefix without a
  trailing slash therefore also covers sibling paths: `/api` covers `/apis` (fixed here with `/api/`),
  `/api/v4` covers `/api/v4x`, and `/rest` covers `/restricted`. These are the same host and the same
  token, so it is not a cross-host leak, but it widens what a host entry claims. Worth a FIX: segment
  matching in the core, or a linter rule requiring the trailing slash.
- **F2.** `aurora plugin test` (`check_snapshot.rs`) does not apply step-3 path hygiene. A fixture
  cannot show `ambiguous_path`, and dot or `%2F` segments in a `*` position classify differently
  offline than in production. Worth a FIX: run `hygiene::check_request_line` first and allow
  `expect: { refused: ambiguous_path }`.

### Commands run (all passed)

- `heavy.sh p5-platform bash scripts/plugins/check-builtin.sh grafana`: `ok grafana` (validate;
  test 131 passed, 0 failed, 0 skipped; coverage of every host entry, tool, negative and approval
  field; scopes; refused, 11 of 11; texts; docs; secrets). The first run failed on the `/apis` case,
  which led to the `/api/` change above.
- `aurora plugin guide plugins/builtin/grafana --app grafana --out <scratch>`: renders. `guide.md`,
  `tools.md` (31 read, 5 held writes, untooled blocked under a custom grant), `api.json`, the
  recipe and the reference file were read once for sense.
- `heavy.sh p5-platform cargo test -p aurora-registry --test it`: 173 passed. This covers
  `builtin_versions`, `manifests::every_builtin_and_fixture_validates…`,
  `snapshot::the_shipped_builtin_packages_compile…` and `agent_guide`.
- `heavy.sh p5-platform cargo test -p aurora-gateway --lib hygiene`: 4 passed.
  `… --test it method_override_and_ambiguous_paths`: 1 passed.
- `heavy.sh p5-platform bash scripts/plugins/build-builtin.sh --out <scratch>`: 15 packages,
  `grafana-1.0.0.aurora-plugin` unsigned. The dev stack was not started (lane rule: no aurora-dev).
- `bash scripts/guards/run-all.sh`: all guards passed, with `grafana` in `TERMS`.
- `python3 scripts/extract-schema.py --check`: up to date. No migration was made.
- Links checked by hand, all `200`: `api_docs`, `api_spec`, the service-accounts and roles docs, and
  `homepage`. The anchor `#add-a-token-to-a-service-account-in-grafana` exists.
- No Rust or web code changed, so `cargo fmt`, clippy, `pnpm typecheck`, eslint and vitest do not
  apply.
