# P5-CATALOG: five more built-in apps — Grafana, Zoom, ServiceNow, Foreman, Rundeck (phase 5, after the UX work)

Owner request (2026-11-20): "We need to develop some more apps … Look around to see what's out there and
what would make sense for each app", then "Prepare briefs for Grafana, Zoom, ServiceNow, Foreman, Rundeck,
to be implemented after all the current UX work is done." Decisions: **D-428** (this wave), D-429 (Grafana),
D-430 (Zoom), D-431 (ServiceNow), D-432 (Foreman), D-433 (Rundeck); invariant **SEC-145**.

| Brief | Package | Hosting | Connect | Hook |
|---|---|---|---|---|
| `P5-GRAFANA.md` | `grafana` | instances, no default host | service-account token | — |
| `P5-ZOOM.md` | `zoom` | SaaS (`api.zoom.us`) | personal OAuth; account-wide Server-to-Server | `token.mint` (S2S) |
| `P5-SERVICENOW.md` | `servicenow` | instances, no default host | personal OAuth (per-instance client); integration user (basic) | — |
| `P5-FOREMAN.md` | `foreman` | instances, no default host | username + personal access token (basic) | — |
| `P5-RUNDECK.md` | `rundeck` | instances (base path allowed), no default host | API token header | — |

## Why these shapes (D-428, research of 2026-11-20)

- **Every one has a complete REST API**, so each is an ordinary package: the agent calls the API from its
  sandbox, the gateway injects the credential, tools classify requests for grants and approvals. None is an
  MCP-only app (`24 §12`, D-172(b)). The vendors' and the community's MCP servers were read **only as evidence
  of which operations agents use**; their tool lists shaped each catalogue below. Nothing attaches an MCP
  server.
- **Generic products, generic packages.** All five are built-ins in `plugins/builtin/`, not CERN addons:
  Foreman also ships as Red Hat Satellite, Grafana, Rundeck and ServiceNow are used everywhere. A CERN
  deployment detail (Judy's sign-in, `monit-grafana.cern.ch`) is the owner's to check, not a package rule.
- **Prometheus / Mimir / Loki go through Grafana**: the Grafana package reads them through Grafana's
  datasource proxy, so Grafana holds the backend credentials and the Mimir tenant header. A separate
  Prometheus/Mimir/Alertmanager app is not in this wave.
- **Not in this wave**: OpenSearch (needs a port on instances, often `:9200`) and OpenStack (needs
  service-catalog routing across hosts and ports, D-172(c)). They wait for their own specs. Private
  (internal-only) hosts work as today: `gateway.allowed_private` by name, plus a network-policy allowance.
- **No credential minting through an app (SEC-145).** An endpoint that creates or reveals a credential
  (Grafana service-account tokens, Foreman personal access tokens, Rundeck API tokens, ServiceNow OAuth
  entities and user passwords) is never a catalogue tool, and each package proves it with a `refused …`
  fixture. An agent must not be able to give itself a second, unmanaged credential.

## Common rules (every brief)

Read, and nothing else unless a brief says so:
- `plugins/builtin/README.md` (whole file): layout, checks, guide rules, placeholders, fixtures.
- `docs/09-app-plugins.md` §3.4 (`connect[]`, instance-bound hosts, `token`, `tools`, `approvals`,
  `agent`), §5 (mechanisms), §12 (fixtures). Hooks (§8) only for Zoom.
- `docs/24-app-guides.md` (renamed `24-apps-for-agents.md` by P5-APPSKILL-2) §3 (app skills) and §10a
  (operations file, `aurora-agent`).
- The reference package: `plugins/builtin/sentry/` (instances, token form, `access_levels` with
  `permission_hints`, `store: label` name field, its app skill); `plugins/builtin/indico/` for an
  instance-only app; `plugins/builtin/aws-keys/hooks/` for a `token.mint` hook.

Rules:
1. **Vendor docs, never guesses.** Every host, path, scope and token endpoint comes from the vendor's
   documentation for a current release, cited in `about.api_docs` (and `api_spec` where one exists). The
   endpoint lists in each brief are the intended **scope**, written from the research: check each one, and
   if the docs disagree or are unclear, follow the docs or leave the tool out and say so in the report.
2. **Reads are GET/HEAD.** A read that the API only offers as POST is a write tool held for approval with a
   title that says it only reads (precedent: Notion `search`). Never widen the checker.
3. **Every write tool** has an `approvals` template whose `title` completes "<Agent> wants to …" and whose
   fields show what will change. Mutating endpoints the brief does not list stay **untooled**.
4. **Refused cases** (SEC-145 and each brief's list): add the app's entry to `REFUSED` in
   `scripts/plugins/check_builtin.py` and a passing `refused <what>: …` case for each.
5. **Texts**: `about.summary`, `about.first_prompt`, tool `description`s (≤ 200 characters, plain words),
   `connect_note`, `permission_hints` in the vendor's own words, `agent.summary`, short
   `agent.instructions`, one or two **app skills** (D-453, P5-APPSKILL-1): `skills/<app>/<app>-api-notes/`
   for the reference, and where a brief names a recipe `<task>.md`, the skill `skills/<app>/<app>-<task>/`
   (scripts allowed, D-452). Wherever the five briefs say "guide file", "reference" or "recipe", read
   "app skill". Setup guide `docs/setup.md` uses the `{{aurora.…}}` placeholders, never page paths.
   These briefs therefore wait for P5-APPSKILL-1/-2 to merge.
6. **Operations file**: where the brief says so, ship `operations.json` (`24 §10a`), hand-written or from
   `aurora plugin import-openapi` on the vendor's spec, cut down to the catalogue's scope.
7. **Icon**: `plugins/builtin/ICONS.md` rules (D-99): Simple Icons 16.37.0 if the product is there
   (`grafana`, `zoom`, `servicenow` are expected; check), else the vendor's official artwork, else a
   monogram. Add the row to `ICONS.md`.
8. **Guard**: add the app names to `TERMS` in `scripts/guards/no-integration-names.sh` (`grafana|zoom|
   servicenow|foreman|rundeck`), with an allow entry for `services/aurora-api/data/common-passwords.txt`
   (it lists `zoom` and `foreman` as passwords). The first brief that merges does this; later ones rebase.
9. **No core change.** If a package needs something the manifest contract lacks, stop and report through
   Orca `ask`; do not change `crates/`, `services/` or the contract.

## Tests move with the UI

These briefs add packages and change no dashboard code, label or route. If a package addition changes
anything a test sees (a count of built-in apps, a catalogue listing, a seeded fixture, a mocked Playwright
catalogue, a CLI golden file), update **every** such test in the same commit (`web/tests`, `web/e2e` mocked
and production projects, `scripts/e2e`, Rust tests, fixtures). Never skip, mark `fixme`, delete or loosen a
test to get green. List the test files changed in the completion report.

## Acceptance (every brief)

- `bash scripts/plugins/check-builtin.sh <id>` passes (validate, test with full coverage, scopes, refused,
  hooks where present, token, texts, docs, secrets).
- `aurora plugin guide plugins/builtin/<id> --app <id>` renders, and the output is read once for sense.
- `bash scripts/guards/run-all.sh` passes.
- The dev stack lists the package (built from `scripts/plugins/build-builtin.sh`).
- No live calls in tests; recorded HTTP scrubbed (README §Fixtures).
- Completion report in the brief: files, the commands run with their results, every endpoint left out and
  why, the test files changed.

## Order and lanes

- **Starts after the current UX work has merged**: P5-REFINE-1..20 (including -20's send-backs) and
  P5-POLISH-1..11. The Orchestrator places the lanes.
- Each brief owns only `plugins/builtin/<its id>/**` plus its `REFUSED` entry and its `ICONS.md` row. Those
  two shared files (and the guard, rule 8) are one-line appends: run the briefs in **one `package` lane in
  sequence**, or in at most two lanes with those three files rebased at merge.
- Suggested order: Grafana → Foreman → Rundeck → ServiceNow → Zoom. Zoom is last because it is the only one
  with a hook.

## Security review (before each merge)

Each package gets a focused review of its tool classification and approvals against SEC-145, SEC-04/59
(credentials only on the registered host and base path) and the brief's refused list:
- **Grafana**: the datasource proxy forwards any method and path. Check that only the named backend paths
  are tools, and that encoded or dot segments (`%2F`, `..`, `%2e%2e`) and trailing variants cannot reach an
  unlisted backend path (for example the Prometheus admin API) through a read tool. Compare the gateway's path
  normalization (`06` step 3) with what Grafana forwards. Also check that no read tool returns stored
  datasource secrets.
- **Foreman**: `run_job`'s approval shows the exact target and inputs as sent; hidden parameter values are
  never readable through a tool (`show_hidden`, ENC; see the brief).
- **Rundeck** (light): `run_job`'s approval shows the options and node filter as sent; ad hoc run endpoints
  and key storage select no tool; the header is injected only under the registered base path.
- **ServiceNow**: every readable table is named (no catch-all), credential tables select no tool; the OAuth
  refresh is built for the connection's instance only.
- **Zoom**: the `token.mint` hook (secret never echoed, egress `zoom.us` only), the bounded recording
  download hosts, refresh-token rotation.

## Suggested agent/model

Sonnet for each brief (`rust-developer` for Zoom, because of the hook crate). The packages are YAML and
fixtures; the hard part is reading the vendor docs carefully and getting the classification order right,
which is why Haiku is not suggested.
