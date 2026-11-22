# P5-GRAFANA-2: the Grafana app queries through `/api/ds/query`, finds data sources, and ships query skills with a helper script (owner, 2026-11-22)

## Why

An agent working against CERN's Monit Grafana (`monit-grafana.cern.ch`) struggled. It then wrote
itself a skill recording what it had learned (owner, 2026-11-22: "improve the Grafana app so it's
easier for agents to use … it includes details on how to use the Grafana app which all agents will
struggle to relearn"). Its findings, checked against the package as merged in P5-GRAFANA:

1. **The proxy query tools fail on that instance.** Every `prometheus_*` and `loki_*` tool goes
   through `/api/datasources/proxy/uid/<uid>/…`. Monit answers `HTTP 308` with a `Location` on the
   backend host (`monit-prom-lts.cern.ch`). `aurora-agent` never follows redirects (correctly: the
   new host is not the connection's, and Grafana's backend credentials stay in Grafana), so these
   tools never work there.

   The likely cause is a data source URL that the backend redirects (http→https or a path), which
   Grafana's proxy passes through unchanged. Grafana's own UI queries Prometheus and Loki through
   `POST /api/ds/query` (the backend plugin, whose client follows the redirect), so an admin never
   notices. Other instances will have the same problem. `POST /api/ds/query` works on Monit.
2. **`/api/ds/query` is a held write.** Under a Viewer (read) connection, every query waits for
   approval. That makes the one route that works the slowest one.
3. **`aurora-agent call grafana query_datasources` fails.** `operations.json` has no entry for any
   write tool, so each of them is a catalogue entry. The agent had to write
   `--method POST --path /api/ds/query --body @…`. The same holds for `create_annotation`,
   `update_dashboard`, `create_silence` and `delete_silence`.
4. **Finding data source UIDs is slow.** `list_datasources` is empty for Viewer and Editor tokens
   (the guide says so). The agent searched dashboards and read panels. `GET /api/frontend/settings`,
   which every signed-in browser loads, lists every data source the user can **query** (name, `uid`,
   `type`), but it is not a tool. `proxy-boundaries.yaml` proves today that it selects none.
5. **Hard-won knowledge that only exists in that one agent's notes:**
   - the `ds/query` body shapes for Prometheus, Loki and OpenSearch;
   - how to read Grafana data frames (`schema.fields[].labels`, `data.values` columns, epoch-ms
     times);
   - per-refId errors (`errorSource: plugin`);
   - batching several refIds that share one `from`/`to`;
   - OpenSearch plugin quirks (below).

## Decision (D-462)

**`query_datasources` (`POST /api/ds/query`) becomes a `read` tool** (owner, 2026-11-22, chosen over
keeping it a held write and over a per-data-source allow list). It runs under a read (Viewer)
connection like the proxy GETs.

- Grafana itself grants it to the Viewer role, and it is how Grafana's own UI reads data.
- **Accepted risk:** a SQL or InfluxDB data source whose database user can write could be changed by
  a query. The gateway cannot tell from the body, because Grafana picks the plugin by the
  data-source UID, not by the body's `type`.
- The connect note and the skill say that data sources must use read-only database users, which is
  Grafana's own advice. The skill tells agents to send only read statements.
- A person who wants queries to ask first can still set the tool to Ask in the account's actions
  table.
- D-429's "`POST /api/ds/query` as a held read" is superseded.
- The proxy GET tools stay: they work on instances whose data source URLs answer directly. The
  skills send agents to `ds/query` first.

## Read first

- `plugins/builtin/grafana/**` after P5-APPSKILL-1/-2: `plugin.yaml`,
  `agent/grafana/operations.json`, `skills/grafana/grafana-api-notes/`,
  `skills/grafana/grafana-investigate-a-spike/`, `tests/*.yaml`.
- `docs/tasks/P5-GRAFANA.md` §Security review notes (the proxy boundary it proved) and §Agent text.
- `docs/09-app-plugins.md` §3.4 (tools, `approvals`) and §"Which tool a request is".
- `docs/24-app-guides.md` and `plugins/builtin/README.md` for the app-skill authoring rules
  (P5-APPSKILL-1 Part E).
- `docs/tasks/P5-APPSKILL-4.md` §Work 3 for the script rules and the stubbed-script test pattern
  (`scripts/plugins/indico_skills_test.sh`); copy that pattern.
- Grafana sources (current release; record the version you checked):
  - `pkg/api/ds_query.go` and `pkg/services/query/query.go` (status codes, per-refId errors, `from`/`to`
    forms such as `now-6h`);
  - `pkg/api/frontendsettings.go` (`datasources` entries: which fields; none secret);
  - the Prometheus and Loki query models (`pkg/promlib/models`, `pkg/tsdb/loki`).
- The OpenSearch plugin source `github.com/grafana/opensearch-datasource` (`pkg/opensearch/…`):
  `queryType` values, metric types (`raw_data`, `raw_document`, `logs`), `bucketAggs`, `orderBy`.

## Work (all in `plugins/builtin/grafana/**`)

### 1. Manifest (`plugin.yaml`)

1. Move `query_datasources` from `tools.write` to `tools.read`. Keep its id, so grants and the request
   log keep naming it. Description: "Runs queries on any data source through Grafana (Prometheus,
   Mimir, Loki, OpenSearch, Elasticsearch, SQL); only reads." Keep its `approvals` template if §3.4
   allows one on a read tool (an Ask rule can still hold it). Otherwise remove it, and say which in
   the report.
2. Add the read tool **`list_queryable_datasources`**: `GET /api/frontend/settings`, name "List the data
   sources you can query", description "Lists every data source this token can query (name, UID,
   type), which `list_datasources` usually does not." Verify in `frontendsettings.go` that the
   `datasources` entries carry no secure fields and that a service-account token gets them. If either
   is false, drop this tool and say so.
3. Rewrite `agent.instructions` (≤ 600 chars):
   - query with `query_datasources` (`POST /api/ds/query`), using the `grafana-query` skill and its
     script;
   - find UIDs with `list_queryable_datasources` or a dashboard's panels;
   - the proxy GET tools are a fallback, and a `3xx` from them means "use `query_datasources`";
   - keep ranges and steps small.
4. Update `agent.summary` and the app `description` so neither says the data source proxy is the query
   route.
5. Connect note: one more sentence. Queries run as the service account, and data sources should use
   read-only database users (Grafana's advice for SQL data sources).
6. Bump the minor version.

### 2. `operations.json`

Add operations, each with a `body` block (copy the gitlab `create_issue` shape):

- `query_datasources`: fields `from`, `to` (RFC 3339, epoch ms or `now-…`) and `queries`
  (array; the description names `refId`, `datasource {uid,type}`, `expr`, `range`/`instant`,
  `intervalMs`, `maxDataPoints`).
- `list_queryable_datasources`.
- The four write tools: `create_annotation`, `update_dashboard`, `create_silence`, `delete_silence`.

`aurora-agent call grafana <tool> --body @file` must then work for each. Prove it with the
`agent_api_json` / `agent_cli` tests that already cover operations, or a fixture-level check: no
Grafana tool is listed as a catalogue entry.

### 3. Skills (`skills/grafana/`)

Follow P5-APPSKILL-1's authoring rules:
- no hosts (the host comes from the account in `app.md`);
- no secrets;
- nothing CERN-only (Monit is never named: write "some instances", not "Monit");
- scripts in `scripts/`, committed executable, bash or `python3 -I` with the standard library only,
  calling Grafana only through `aurora-agent call grafana …`.

Mark everything you could not verify in source as **seen on one instance**, with what to do if it
happens.

1. **`grafana-query`** (new; description: "Run Prometheus, Mimir, Loki, OpenSearch, Elasticsearch or
   SQL queries through Grafana and read the results; read before the first query"). Contents:
   - why `ds/query` comes before the proxy tools (the `3xx` case);
   - body shapes for Prometheus/Mimir (range and instant), Loki (log lines and metric queries) and
     OpenSearch/Elasticsearch (metrics with `date_histogram` and `terms`, and raw documents if the
     plugin source supports them; see §4), and SQL (`rawSql`, `format: table`, read statements
     only);
   - `from`/`to` forms, and `intervalMs`/`maxDataPoints` against the point limit (Prometheus refuses
     more than 11,000 points per series);
   - batching refIds that share a window;
   - how to read frames: columns in `data.values`, labels in `schema.fields[i].labels`, epoch-ms
     times, one point per frame for instant queries;
   - per-refId `status`/`error`, what a `400` with `errorSource: plugin` means, and the overall HTTP
     status Grafana returns when only some refIds fail (verify in `ds_query.go`);
   - finding UIDs: `list_queryable_datasources`, else a dashboard's panels (`datasource.uid`, and
     `${var}` resolved through `templating.list[].current`), with `check_datasource_health` to
     confirm the backend;
   - **panels are the field-name documentation**: read a panel's `targets[]` (`expr`, `metrics[].field`,
     `bucketAggs[].field`, `timeField`) before guessing a field.
2. **Script `skills/grafana/grafana-query/scripts/grafana-query`** (python3 -I, stdlib). Subcommands:
   - `datasources [--type T]`: prints `uid  type  name`, one per line, from
     `list_queryable_datasources`. When that is refused or empty, it says so and suggests
     `panels <dashboard uid>`.
   - `panels <dashboard uid>`: prints each panel (rows expanded) as title, data source uid/type
     (variables resolved from `templating.list[].current`), and each target's `expr` / `query` /
     `rawSql`, `metrics[].type:field`, `bucketAggs[].type:field` and `timeField`.
   - `promql|logql --ds <uid> --from <t> --to <t> [--step 5m | --instant] [--limit N] <expr>…`:
     - builds the body (one refId per expression, A, B, …; `type` `prometheus` or `loki`;
       `intervalMs` from `--step`; `maxDataPoints` from the window);
     - refuses a request over 11,000 points per series with a message that names a step that fits;
     - runs `aurora-agent call grafana query_datasources --body @<tmp> --out <abs tmp>` (temp files
       under `$TMPDIR` or `/tmp/aurora-agent`, absolute paths);
     - prints the frames.
   - `run <body.json>`: sends any body (OpenSearch, SQL) and prints the frames.
   - `frames <result.json>`: prints an existing result.

   Output, shared by all the query subcommands:
   - for each refId, `# <refId> <status>` and the error when there is one;
   - per frame, a header with its labels or field names, then tab-separated rows, times in RFC 3339
     UTC;
   - long-format frames (several value columns, `terms` buckets) as one row per bucket with every
     column.

   It passes `--account` through. It exits non-zero when the call fails or any refId has an error.
   It never prints the token or headers. `--help` on each subcommand.
3. **`grafana-opensearch`** (new; description: "Query OpenSearch or Elasticsearch data sources through
   Grafana: body format, aggregations and the plugin's quirks"). Take each quirk from the agent's
   notes, check it against the plugin source, and keep it only as what is true, phrased generally:
   - `queryType` casing (`lucene`/`ppl`, and the misleading error text);
   - whether a `logs` / `raw_data` / `raw_document` metric returns raw documents, and the exact body
     that does (if none does through `ds/query`, say so plainly);
   - that a count with no `bucketAggs` is refused, and that a total over a window is the **sum of every
     `date_histogram` bucket** (a 12 h window at `interval: 12h` gives two buckets);
   - `cardinality` on an IP field returning 0 or null, and the `terms`-with-large-`size` alternative;
   - `orderBy` by a metric id falling back to term order (use `_count`);
   - `sum` on a missing field returning 0, not an error (verify the field in a panel first).

   Include one complete body example (`terms` under `date_histogram`) and how its frame reads.
4. **`grafana-investigate-a-spike`**: rewrite steps 2–5 to use `grafana-query panels`, `promql` /
   `logql` (both windows in one call where they share `from`/`to`; the day before as a second call)
   and `list_annotations`. Drop "Only the proxy paths … work".
5. **`grafana-api-notes`**:
   - the Common requests table leads with `query_datasources` and `list_queryable_datasources`;
   - the data source proxy section says when it fails (`3xx` to the backend host) and to switch;
   - `POST /api/ds/query` is no longer "held for approval";
   - add `aurora-agent call grafana <write tool> --body @file` for the write tools;
   - keep everything else.

### 4. Fixtures (`tests/`)

- Move the three `query_datasources` cases from `write.yaml` to `read.yaml`. They are classified
  `query_datasources` as a read and are no longer held. Keep the summary expectations if the template
  stays.
- Keep the negative `resources` case.
- `list_queryable_datasources`: a positive case. Change `frontend/settings` in `proxy-boundaries.yaml`
  from "selects no tool" to that tool, and keep a negative for `POST /api/frontend/settings` and for a
  path under it.
- A refused or negative case still proves that `/api/ds/query` with `GET`, and `/api/ds/query/…`,
  select no tool.
- `aurora plugin test plugins/builtin/grafana`: everything passes.

### 5. Script test

`scripts/plugins/grafana_skills_test.sh` (same pattern as `indico_skills_test.sh`). A stub
`aurora-agent` on `PATH` records its arguments and answers canned files. Cover:
- `datasources` from a frontend-settings sample;
- `panels` from a dashboard with a row and a `${datasource}` variable;
- `promql` building the expected body (refIds, `intervalMs`, `maxDataPoints`) and printing a
  two-series range frame and an instant frame;
- the over-11,000-points refusal;
- a per-refId error giving a non-zero exit;
- `run` with an OpenSearch `terms` frame.

The canned frames come from the Grafana sources' test data or documentation shapes; say which. Wire
it where `indico_skills_test.sh` is wired.

## Tests move with the code

Update every test or fixture that names `query_datasources` as a write, the old instructions text, or
the `frontend/settings` boundary case, in the same commit:

```
rg -n "query_datasources|frontend/settings|datasource proxy" plugins/builtin/grafana crates services scripts tests web
```

Never skip or loosen a test.

## Security review (Orchestrator, before merge)

- `query_datasources` is a read only at `POST /api/ds/query` exactly. `GET`, sub-paths, the
  `resources` route and the proxy keep selecting no tool, and `block_untooled` is unchanged.
- `list_queryable_datasources` is `GET /api/frontend/settings` only. The report quotes the
  `frontendsettings.go` lines that show no secure field is sent.
- Scripts:
  - call only `aurora-agent call grafana`;
  - no `curl`, no other host, no token handling, no `eval` of response text;
  - temp files are created with `mkstemp`.
- No refused tool (SEC-145) became reachable.

## Acceptance

1. `aurora plugin validate plugins/builtin/grafana` gives no warning, and
   `python3 scripts/plugins/check_builtin.py` passes.
2. `aurora plugin test plugins/builtin/grafana` passes, with the moved and new cases.
3. `scripts/plugins/grafana_skills_test.sh` passes. `python3 -I -m py_compile` passes on the script,
   and `shellcheck` is clean on the test.
4. `aurora plugin preview plugins/builtin/grafana` shows:
   - the three skills plus the api-notes skill;
   - `app.md` listing them;
   - no Grafana tool as a catalogue entry in `api.json`.
5. `bash scripts/guards/run-all.sh` passes, including `no-integration-names`.
6. The report lists every quirk kept, changed or dropped, with the source line or test that decided
   it, and the Grafana and OpenSearch-plugin versions checked.

## Not in this brief

- Following redirects in `aurora-agent` or the gateway. Never: the new host is not the connection's.
- A per-data-source allow list for `ds/query`. This was the owner's third option and was not chosen.
  It would be gateway work.
- Jira's base-path note in the agent's skill (`its.cern.ch/jira`, `--path /jira/rest/…`). It belongs
  to the atlassian app; check separately whether named operations miss the account's base path.
- The agent's claim that a relative `--out` is written "relative to the tool's cwd": `files::check_out`
  joins the `aurora-agent` process's own working directory, which is the shell's. Do not repeat the
  claim. The script uses absolute paths anyway.

## Placement

- **Depends on** P5-APPSKILL-1 and P5-APPSKILL-2 being merged: the skills folder layout,
  `$SKILLS_DIR`, `app.md` and `aurora plugin preview`.
- **Owns** `plugins/builtin/grafana/**` and `scripts/plugins/grafana_skills_test.sh`, plus the
  one-line wiring of that test.
- It can run beside P5-APPSKILL-3 (web) and P5-APPSKILL-4 (`plugins/builtin/indico/**`). If -4 adds the
  shared test wiring at the same time, rebase that one line.
- One `package` lane, Sonnet. Security review by the Orchestrator before merge.
- Owner check (not blocking): run `grafana-query datasources` and one `promql` against Monit Grafana
  with a Viewer token.
