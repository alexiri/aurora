# P5-DEDUP-1: find and remove duplicated Rust code

**Owner, 2026-11-18.** "There are probably many other things that are duplicated and could do with
shared primitives. Write briefs for searching for and fixing those duplications: one on the rust
side and another on the web side. These tasks should probably run in parallel, on their own fresh
lanes and with some parallelization, at least to identify duplicated code. The goal is to reduce
the overall lines of code." The owner's example that prompted this was FIX-173: nine hand-written
secret inputs, which now become one `SecretInput`. Decision: **D-376**. The web side is
**P5-DEDUP-2**. It runs at the same time in its own lane and shares no files with this one.

## Goal
Reduce the total lines of hand-written Rust by replacing duplicated code with shared helpers,
**without any change in behaviour**. That covers production code and tests in `crates/`,
`services/`, `connectors/` and `plugins/` (Rust only).

The measure is the net change in non-blank lines, reported per crate. A smaller codebase that
behaves differently is a failure, not a partial success.

## Context
- Size on 2026-11-18 (`wc -l`, with blank lines): about 470 k lines of Rust in total.
  - `services/` 349 k: `aurora-api` 227 k, `aurora-gateway` 52 k, `aurora-supervisor` 33 k,
    `aurora-runner` 16 k.
  - `crates/` 109 k.
  - `connectors/` 12 k.
  - Of these, about 176 k are under `tests/`.
- Shared homes that already exist. Use these before creating new ones:
  - `crates/aurora-core` (shared types and errors).
  - `crates/aurora-db` (persistence helpers).
  - `crates/aurora-protocol` (wire types).
  - `crates/aurora-testkit` (ephemeral Postgres, mock upstreams, fixtures).
  - Each crate's `tests/it/common/` (I-34: one integration-test binary per crate, shared helpers
    in `common/`).
- Generated or checked-in outputs are not targets:
  - `web/openapi.json`, `schemas/`, `docs/spec_schema.json`.
  - Anything under `target/`.
  - Migrations (`*.sql`). They are append-only; never edit a merged one.
- Layering rules are in `docs/01-architecture.md` (crate layout) and
  `docs/02-engineering-conventions.md`. `scripts/guards/dep-graph.sh` enforces the dependency
  graph. A shared helper must sit in a crate that every user may already depend on. Do not add a
  dependency edge just to share a helper.

## Scope

### Part A: discover duplication in parallel (read-only, no builds)
1. **Baseline.** On the lane's starting commit, record the non-blank line count per crate,
   split into `src` and `tests`:
   `git ls-files '<crate>/**/*.rs' | xargs grep -cv '^\s*$' | awk -F: '{s+=$2} END {print s}'`
   (or an equivalent the report shows verbatim). Record the commit sha.
2. **Clone scan.** Run a token-based clone detector over `crates services connectors`:
   - Use `jscpd` pinned to an exact version (e.g. `pnpm dlx jscpd@4.0.5 --format rust
     --min-tokens 70 --reporters json --output <scratch>`). Run it from a scratch directory.
   - Do **not** add it to any `package.json`, lockfile or CI.
   - Keep the JSON outside the repo. Summarise it in the report.
3. **Pattern sweeps, in parallel.** A token scanner misses code that has the same shape but
   different names. Run up to **five** read-only explorers (`haiku-explorer`, or `Explore` for
   the larger slices) at the same time. Each one gets one slice and the list of patterns below,
   and returns clusters, not file dumps.
   - **Slices:**
     1. `aurora-api/src/domains/` a–l.
     2. `aurora-api/src/domains/` m–z, plus the rest of `aurora-api/src`.
     3. `aurora-gateway` and `aurora-relay`.
     4. `aurora-supervisor`, `aurora-runner`, the other services, and `connectors/`.
     5. `crates/`, and every `tests/it/` (largely `aurora-api/tests/it`, 145 files).
   - **Patterns to look for:**
     - Handler boilerplate: extracting the principal, membership and role checks repeated inline,
       pagination parsing and cursor encoding, mapping errors to the API error body.
     - Repeated `sqlx` row-to-struct mapping, and copy-pasted queries that differ only in a
       filter.
     - Audit-row construction.
     - Validation of names, URLs, ids and lengths written again per domain.
     - Duplicated `From`/`Display` impls and enum↔string tables.
     - HTTP client setup and retry loops in the gateway, runner and supervisor.
     - Config parsing.
     - Tests: setup sequences (create org, workspace, agent, user, API key, login) and
       request-and-assert blocks repeated across files, when a `common/` or `aurora-testkit`
       helper would replace them.
     - Dead code: `#[allow(dead_code)]` items, and `pub` items with no reference outside their
       definition. Exclude the public API of `aurora-connector-sdk` and `aurora-plugin-sdk`,
       which outside code builds against.
   - **For each cluster, an explorer returns:**
     - the locations (`file:line`);
     - the number of copies and the lines per copy;
     - how the copies differ (for example a status code, an error message, or a permission);
     - the proposed shared home;
     - the estimated net saving;
     - a risk rating (low / medium / security-sensitive).
4. **Catalogue.** The lane lead merges the scan and the sweeps into one ranked catalogue and
   writes it into this brief's **Completion report** (Part A). Rank by net saving × safety.
   - Keep a cluster only if it has **3 or more copies**, or **2 copies with a net saving of at
     least 40 lines**, and the copies really are the same.
   - Copies that have drifted apart on purpose (different error codes or permissions) are
     listed as "not duplicates" with the reason. They are a finding, not a fix.
   - Commit the catalogue before starting Part B.

### Part B: fix, in batches
- **One batch is one cluster family** (for example "pagination parsing in aurora-api"),
  committed on its own. Its commit message gives the net line delta.
- **Order:** highest-ranked low-risk batches first. Security-sensitive clusters go last, each in
  its own batch:
  - authorization and membership checks;
  - `aurora-crypto` and `aurora-secrets`;
  - the gateway's `decide/` path;
  - anything a SEC-nn names.
  
  Each of these batches gets a `sonnet-integration-reviewer` pass (or an Opus pass) before its
  commit. The review confirms that every call site keeps exactly the same check.
- **Before each batch**, list the files changed by every other live lane
  (`git diff --name-only master...alexiri/<lane>` for each running lane, the Orchestrator's
  worker list). Skip clusters that touch those files. Record each skipped cluster in the report,
  so it can be done after that lane merges.
- **Within the lane**, implementation is sequential: one Rust build directory, and `cargo`
  serialises on its lock anyway. Part A is where the parallelism goes. If the Orchestrator gives
  the lane a second worktree and disk allows it (see Constraints), a second worker may take a
  batch in a **different crate** at the same time.
- **Tell the Orchestrator after every ~5 batches.** It then merges the lane into master, which
  keeps the conflict window short for the other lanes.

### Out of scope
- Any change in behaviour, including:
  - routes, request or response bodies, status codes, error `code`s and messages, log fields;
  - OpenAPI (the drift test stays byte-identical without `UPDATE_OPENAPI`);
  - SQL semantics, migrations, settings keys, Helm values, metrics names.
- Performance work, new features, and renames for taste.
- Splitting crates (that was VEL-007).
- The web side (P5-DEDUP-2).

## Rules for the shared code
- **Prefer plain functions, generics and traits** over `macro_rules!`. Use a macro only when a
  function cannot do the job (for example generating several items), and give it a doc comment
  and a test.
- **Do not widen visibility** beyond what the callers need (`pub(crate)` inside a crate).
- **Each new shared item gets** a doc comment that says what it replaces, and a unit test if it
  has logic.
- **Tests:**
  - Never delete a test case or weaken an assertion to save lines. Only setup and repeated
    request plumbing move into helpers.
  - The number of tests reported by `cargo test` (or nextest) per crate must not drop. The one
    exception is dead tests of removed dead code: list each one.
- **Dead code** is removed only when the compiler, with `-D warnings` and every target, agrees it
  is unused. Nothing an SDK crate exports is removed.
- **Edit with the Edit/Write tools** (`git mv` is fine). A script may do a genuinely bulk
  mechanical rewrite. Its diff is reviewed.

## Constraints / file ownership
- **Owns:** `crates/**/*.rs`, `services/**/*.rs`, `connectors/**/*.rs`, and the `Cargo.toml` of
  a crate whose dependencies a batch must change (only toward existing edges), batch by batch.
  It does not own `web/**`, migrations, `docs/03`, `docs/05`/`API_CONTRACTS.md`, or Helm.
- **Other lanes:** this lane yields to them. Files in another lane's live diff are off limits
  until that lane merges.
- **Disk:** 51 GB was free on 2026-11-18 (94 % used), and a Rust lane needs about 15 GB.
  Before starting, check `df` and `scripts/dev/disk-report.sh`, and clean merged lanes with
  `scripts/dev/clean-lane.sh`. Use one worktree with its own `target/`. A second one only if at
  least 30 GB is still free after the first build.

## Acceptance criteria
1. **Part A catalogue** is committed in the Completion report. It contains:
   - the baseline sha and per-crate counts;
   - the `jscpd` summary;
   - the ranked clusters, each with locations, copies, differences, home, saving and risk;
   - the "not duplicates" list;
   - the clusters skipped because of other lanes.
2. **Every batch:**
   - `cargo fmt --check`;
   - `cargo clippy --all-targets -D warnings` on the touched crates and their dependents;
   - the targeted tests of the touched crates;
   - `bash scripts/guards/run-all.sh` and `python3 scripts/extract-schema.py --check`.
3. **Every ~5 batches and at the end of the chain**, the full gate:
   - every crate's tests;
   - `openapi_matches_checked_in` green without `UPDATE_OPENAPI`;
   - `scripts/e2e/gateway.sh` and `scripts/e2e/upgrade.sh` when the gateway or api changed.
   
   The test count per crate is no lower than at the baseline, apart from listed dead tests.
4. **Final report:**
   - the per-crate line counts before and after, and the net delta (src and tests separately);
   - the delta per batch;
   - the shared items added and where they live;
   - every security-sensitive batch, with its reviewer verdict;
   - what was left and why.
5. **No item in Out of scope changed.** The OpenAPI and schema outputs are byte-identical, and
   `git diff --stat` shows no migration, `docs/05` or Helm change.

## Dependencies
- None to start Part A. Part B rebases on master before each batch.
- Runs in parallel with P5-DEDUP-2 (no shared files).

## Suggested agent/model and reason
- **Part A:** up to five Haiku/Explore explorers in parallel for the sweeps. A Sonnet lane lead
  merges them into the catalogue. Ranking needs judgement, but the input is structured.
- **Part B:** `rust-developer` (Sonnet) per batch, plus `sonnet-integration-reviewer` for the
  security-sensitive batches. Escalate to Opus for any cluster where the copies differ in a way
  that might be a bug: that is a finding to report to the owner, not something to unify silently.

## Completion report
(Part A catalogue first, then the batches.)

## Part A catalogue (2026-11-18)

Read-only discovery; no build was run. All savings are the explorers' **estimates** of net lines
(copies removed minus the new helper), not measured. Counts marked "grep" were not hand-verified per
site. Part B must re-verify each cluster on the lane's then-current master before editing.

### A.1 Baseline
Commit `f218cdb4867832d21d0f438401d70629ffee2751`. Non-blank lines of tracked `*.rs`
(`git ls-files <crate> | grep '\.rs$' | xargs grep -cv '^\s*$'`, summed; "tests" = files under a
`/tests/` directory, so inline `#[cfg(test)]` modules count as src):

| crate | src | tests |
|---|---:|---:|
| services/aurora-api | 114,374 | 101,956 |
| services/aurora-gateway | 23,933 | 25,816 |
| services/aurora-supervisor | 22,165 | 8,540 |
| services/aurora-runner | 9,100 | 5,735 |
| services/aurora-relay | 4,572 | 3,360 |
| services/aurora-cli | 4,293 | 753 |
| services/aurora-terminator | 2,010 | 1,313 |
| services/aurora-hookhost | 1,759 | 724 |
| services/aurora-connector-host | 739 | 674 |
| crates/aurora-db | 31,613 | 2,361 |
| crates/aurora-config | 9,810 | 0 |
| crates/aurora-registry | 8,667 | 4,497 |
| crates/aurora-core | 7,285 | 0 |
| crates/aurora-llm | 6,660 | 2,496 |
| crates/aurora-testkit | 4,857 | 0 |
| crates/aurora-protocol | 4,268 | 1,059 |
| crates/aurora-policy | 2,780 | 0 |
| crates/aurora-secrets | 2,085 | 1,327 |
| crates/aurora-counters | 1,168 | 1,105 |
| crates/aurora-summary | 1,024 | 0 |
| crates/aurora-connector-sdk | 963 | 0 |
| crates/aurora-channels | 799 | 361 |
| crates/aurora-client | 772 | 0 |
| crates/aurora-crypto | 674 | 0 |
| crates/aurora-plugin-sdk | 189 | 87 |
| connectors/mattermost | 8,272 | 1,164 |
| connectors/fake | 1,607 | 0 |
| connectors/builtin | 199 | 0 |
| plugins/builtin, plugins/fixtures | 2,003 / 141 | 0 |

### A.2 jscpd summary
`pnpm dlx jscpd@4.0.5 --format rust --min-tokens 70 --reporters json --gitignore --ignore
'**/target/**,**/tests/golden/**' crates services connectors`, run from the scratchpad, JSON kept
outside the repo (`<scratchpad>/jscpd/jscpd-report.json`). 8 min wall time.
1,119 files, 346,340 lines, **691 clones, 9,119 duplicated lines (2.63 %)**, 110,609 duplicated tokens
(3.13 %). So exact-token clones are a small share; most of the saving below is same-shape code the
scanner cannot see. Duplicated lines by location (both sides counted): aurora-api tests 6,025,
gateway tests 2,852, aurora-api src 2,717, supervisor src 2,577, runner tests 1,364, gateway src 547,
aurora-llm src 521, relay tests 498. Biggest pairs, all confirmed by the sweeps below: runner
`files_streams`/`ssh_streams` tests (280), gateway `ask_tool_hold`/`hold_recheck` (229), api tests
`skills`/`webhooks` (202), `hosted`/`schedules` (196), `memory`/`skills` (178), supervisor
`adapters/goose`/`opencode` `mod.rs` (171), `config.rs` (98), `launcher.rs` pairs (84, 48).

### A.3 Ranked clusters
Rank = estimated net saving × safety. IDs are used by Part B commit messages. "Home" is a crate every
user already depends on. Sources: sweeps S1 (api domains a–l), S2 (api m–z + rest), S3 (gateway,
relay), S4 (supervisor, runner, others), S5 (crates, api tests).

**Tier 1: low risk, large (tests and obvious helpers)**

| ID | Cluster | Copies | Est. net | Home | Risk |
|---|---|---|---:|---|---|
| T1 | Signed+approved plugin install in api test `world()` setup (trusted-key POST, `PackageBuilder`, multipart, assert `pending_approval`, approve). Copies differ only in YAML and cookie variable. `apps_tools.rs:54`, `apps_connections.rs:55`, `grants_apps.rs:68`, `grants_connections.rs:69`, `connect.rs:153`, `connect_access.rs:114`, `sandbox_env.rs:95`, `home_render_inputs.rs:107`, `app_instances.rs:102`, `app_setup.rs:145`, `skill_library.rs:~195` (+5 trust-key-only ≈12-line copies) (S5-1; jscpd confirms) | 11 + 5 | 450-500 | `Admin::install` in `tests/it/plugins_lifecycle.rs` (already imported by 9 files) or `tests/it/common` | low |
| T2 | Hosted/runner `Rig` re-implemented per test file: `schedules.rs:13-296` ≈ `hosted.rs:13-342` (280 lines), `attachments.rs:14-345` (≈200), `memory/skills/webhooks.rs` 135-line identical block + 35-line block, partial in `tools.rs`, `fix_029.rs`, `runner_plane.rs`; `agent(handle,grant)` body copied 4×; `caps()` in 9 files (S5-2) | 6-9 files | 600-750 | `crate::hosted::{Rig, rig_with}` with `runner_id`/caps parameters | medium: `hosted` rig adds an `anthropic` endpoint and proxy URLs, the others do not; tests asserting defaults must stay green |
| T3 | Cookie+origin `call()` helpers (45 local copies of 6-10 lines, ~20 signatures) plus 67 inline 6-line `.send(` blocks; `fn code(v)` copied 24× (S5-3) | 45 + 67 + 24 | 450-550 | `Harness::call(cookie, m, uri, body)` / `call_tok` and `code()` in `tests/it/common/mod.rs` | low |
| T4 | Seed rows in api tests: `agent()` helper in ≥20 files + ~40 inline (84 `INSERT INTO agents` in 61 files, two body shapes), conversations (25), turns (18), user+member+session sequences (112) (S5-4) | many | 450 (safe share; 550-700 upper) | `Harness::{hosted_agent, byo_agent, conversation, turn, member_login}` in `common/mod.rs`; names/handles stay as is | low |
| T5 | Runner integration-test support: runner-half rig (3 copies ~38-44 lines), `connect_supervisor`, `next_text` (22), `mount_runner_api` (30), `dial`, `close_code`; `files_streams.rs`/`ssh_streams.rs` byte-identical helpers (~75 lines) (S4-1; jscpd 280) | 3-5 over 6 files | 250-300 | `tests/it/terminal_streams.rs` (already `pub(crate)`) or new `tests/it/support.rs` | low |
| T6 | Gateway test boot tail (`ca::generate`, bind, `Guard`, `gateway_tls`, `Proxy::new(ProxyParts)`, spawn) ×15; `PolicyHandler::new(PolicyHandlerParts{..})` ×11 (S3-G1) | 15 | 200 | `gateway/tests/it/common` `spawn_proxy(...)` | low |
| T7 | Gateway test `RequestIdentity` literal (15 fields) ×~21 and `Static(World)`+`impl CredentialResolver` ×9 (S3-G2) | 21 / 9 | 180 | `common::identity(org,ws,agent)`, `common::World` | low |
| T8 | Gateway test raw HTTP-through-proxy `Reply`, `dechunk` ×5, `send` ×2 identical, `Proxy-Authorization: Basic` encoding in 14 files (4 `basic()` fns are **not** identical, unify only the encoding) (S3-G3) | 5-8 | 150 | `common::{Reply, proxy_request, dechunk}` | low |
| T9 | Gateway test Postgres seeding of org/workspace/agent/credential (14 files) (S3-G4) | ~14 | 200 | `common::seed_org_ws_agent` etc. | low |
| T10 | Gateway test stubs: `Capture` LogWriter ×7 byte-identical, `Null` ×3, `Src: PolicySource` ×10-11 (parametrisable, not identical → medium), `rule()` ×7 near-copies (S3-G5) | 7 / 3 / 10 | 120 | `common::{Capture, Null, Rows, Src}` | low (Src: medium) |
| T11 | Gateway test approval `decide`/`pending` helpers (S3-G6), accept-loop for upstream stubs ×16 and `TcpListener::bind("127.0.0.1:0")` ×56 (G7), `tls_client` ×3 duplicating `common::client_config` (G8) | 3 + 16 + 3 | 70 + 100 + 40 | gateway `tests/it/common` | low |
| T12 | Supervisor adapter test fixtures: `opts()` ×5 (not identical → builder), `turn()` ×5 identical, `drain()` ×4, `tmp_opts()` ×3, `get()` (S4-5) | 5 | 150 | `#[cfg(test)] pub(crate) mod testutil` in `adapters/mod.rs` | low |
| T13 | Supervisor fake-runner `Conn` + hello/welcome handshake + `home.sync` in `turns.rs`/`previews.rs` (+`inbound.rs`) (S4-6); PTY ends in 3 tests (S4-11, 25) | 2-3 | 70 + 25 | `tests/it/common/mod.rs` | low |
| T14 | Relay test `fake_runner` (limits.rs = common, edge.rs adds eprintln diagnostics), `bind_any`, `tm()` ×3, `rig()` ≈ `rig_with` (S3-R1) | 2-3 | 130 | `relay/tests/common/mod.rs` | medium (edge.rs diagnostics must stay) |
| T15 | Mattermost test world: `AcceptAll` ×3, settings literal ×3, `Integration{..}` literal ×3, core/mm mock setup (S4-10) | 3 | 70-100 | `#[cfg(test)] test_support` in the lib (the `tests/` copy needs a feature-gated module or stays) | low |
| T16 | aurora-testkit mock-server tail `bind`+`local_addr`+`spawn(axum::serve)` ×9 +`base()`; 2 in api tests; gateway-CA temp-dir block ×16 in api tests (S5-6, S5-9) | 9 + 16 | 35 + 48 | `testkit::serve(app, ip)`; `common::gateway_ca_dir()` | low |
| T17 | Api test audit helpers: `wait_audit` ×5, `audit_count` ×4 (poll counts 50/100/200, `keys.rs` returns 0 on timeout while the others panic: parameterise) (S5-7); multipart plugin upload (S5-8) | 7 / 8 | 70 + 80 | `Harness::{audit_count, wait_audit}`, `common::multipart` | low |

**Tier 2: low risk, production code**

| ID | Cluster | Copies | Est. net | Home | Risk |
|---|---|---|---:|---|---|
| P1 | `AuditEvent::new(AuditActor::from_caller(c), action, Success)` (80 sites crate-wide), per-domain `event()` wrappers (`groups/service.rs:19`, `invitations/routes.rs:25`, `grants/service.rs:462` byte-identical), agent-scoped chain `.org().workspace().target("agent", id)` ×12-35, `schedules/webhooks/memory routes record` ×3, `secrets` `record`/`event` (S1-1, S2-16) | 80 / 35 | 100-130 | `crate::audit`: `AuditEvent::by(&Caller, action)`, `.in_agent(&AgentRow)` | low; Failure/Denied sites keep `Outcome` explicit; row output identical |
| P2 | Single-field `ApiError::invalid_request(msg, vec![FieldError::new(..)])`: ~58-70 inline + ~20 local `bad()` wrappers (message "Invalid request." in 9, domain messages in the rest) (S1-2, S2-2) | ~90 | 150-200 | `ApiError::invalid_field(message, path, detail)` in `aurora-core/src/error.rs` | low (JSON body identical; 422 `invalid`/`grant_invalid` copies are not part of it) |
| P3 | `Page { items: ..map(X::from).collect(), next_cursor }` ×~28 and `p.after.map(\|c\| (c.t, c.id))` ×33; `governance/standing.rs:183` hand-rolls `Page::from_rows` (S1-3, S2-1) | ~28 / 33 | 100-130 | `Page::map`, `Pagination::after_key` in `http/pagination.rs` | low; must not reorder parse-vs-authorise |
| P4 | Dead code verified by one-occurrence grep: `previews::service::set_org_enabled` (31), `scim/rate_limit` `body_limit_middleware`+`refill_secs` (27), `ShareView`, `ImportPlan`, `reject_public`, `PathId`, `ERROR_SCHEMA`, `SCIM_EXPIRY_SWEEP_EVERY`, `let _ = SESSION_COOKIE;`; `grants::grant_connection` (24), `files::drop_session`, `channels/cards/build.rs turn_controls` (**mirror.rs is in `fix-167`, re-check there**), `addon_sources/poll.rs actor()` (**check P5 lanes**), `groups role_str`; aurora-db: 13 unreferenced repo fns (≈150 lines, list in S5-5), `repo/policy.rs workspace_org` (also a 3rd copy of the same SQL), `aurora-core/dash_paths.rs` 11 fns (55), `with_card`, `regenerate_agent_token`, `inner_backend`, `is_wildcard`, testkit `set_default_branch`/`expect_header`/`respond_once`; mattermost `lookups_for`, `send_json`; runner `send_terminal_data`; supervisor test `Rig::next_data` (14). **Delete only when `cargo clippy --all-targets -D warnings` agrees** (aurora-api is a lib: dead `pub` never warns, so the grep evidence is the gate and the reviewer confirms). Dead `can_use()` branches (`WorkspaceAuth::can_use()` is always `true`) are listed under "not duplicates"/findings, not removed | ~45 items | 400-450 | n/a | low; SDK exports untouched |
| P5 | `hex` encoders: 8 hand-written copies replaceable by the already-used `hex::encode` (`plugins/ingest.rs:47`, `skill_libraries/content.rs:617`, `terminal/service.rs:747`, `webhooks/receive.rs:287`, `previews/service.rs:121`, `addon_sources/*`) (S2-4) | 8 | 60-70 | `hex` crate | low (leave `unhex`: `from_str_radix` accepts `+`) |
| P6 | Hand-written char-boundary truncation ×5-9 (`conversations/service.rs:319`, `hosted/reports.rs:32`, `evals/judge.rs:84`, `files/service.rs:677`, `schedules/fire.rs:28`, `webhooks/receive.rs:387`, ...) → `str::floor_char_boundary` (rust-version 1.98, stable) (S1-4) | 5-9 | 40-60 | std | low |
| P7 | aurora-llm `Endpoint` has no `Default`: 12 full 20-field literals; add `#[derive(Default)]`. Also `default_model()` ×7 (default trait method), `list_models` static fallback ×4 (S5-10) | 12 / 7 / 4 | 110 + 18 + 25 | `aurora-llm` `endpoint.rs`, `types.rs` | low |
| P8 | Periodic job loops (`interval` + `Delay` + `loop` + `match Ok(n) if n>0`): 12 in `jobs.rs` + ~10 elsewhere; 17 `Ok(_) => {}` arms (S1-6, S2-3) | ~22 | 100 | `jobs::every(period, f)`; the tracing **field names must stay** (use a macro only for the log arm) | medium (advisory-lock singleton jobs, `ssh` enabled gate keep their semantics) |
| P9 | Agent lookup `visible_agent` (load agent, 404, `c.workspace()`) ×8 (`agents/routes.rs:190`, `avatar.rs:97`, `grants/service.rs:23`, `conversations/service.rs:40`, `freeze/routes.rs:38`, `hosted/routes.rs:80`, `schedules/skills/webhooks/memory routes`); `workspace()`+`require_own` two-liner ×23 (S1-5, S2-8, S2-12) | 8 / 23 | 35 + 20 | `grants::service::visible_agent` or a `Caller` method; keep the dead `can_use` check literal; `freeze` keeps `require_own`; `evals` variant excluded | low |
| P10 | Supervisor adapters: `trunc` ×5 identical, `fn err` ×5, `purge_session` ×3, `start_session` prologue ×3, `Default`/`with_launcher`/`steer`/`abort`/`release`, `run_turn` tail ×3, `safe_ref` ×2 (S4-3); `build_env` prologue ×4 (credential scrubbing: medium) and `effort()` ×3 (codex differs), `pretty` ×2, `role_model`/`subagent_model`/`context_window` ×2 (S4-4) | 3-5 | 120 + 75 | `adapters/mod.rs`, `harness.rs`, new `adapters/{env,model}.rs` | low; `build_env` low-medium |
| P11 | Supervisor launchers: spawn+pipes+kill task+stdin writer+stderr tail ×5 (`adapters/*/launcher.rs`); move `forward_lines`/`MAX_LINE_BYTES` out of `codex/launcher.rs` (S4-2) | 5 | 110-130 | new `adapters/child.rs` | medium (see findings B1, B2: parameterise, do not "fix") |
| P12 | aurora-config `validate.rs`: required-token block ×5 (11 lines), api-url+listen check ×3 (S5-11) | 5 / 3 | 45 + 24 | same file; keep error order; do **not** reuse `check_url` (stricter) | low |
| P13 | Runner: `stream_listener.rs` open-route+wait-for-Opened ×3 (≈55 lines), `release_stream_as`/`release_pty_as` (S4-8); `api.rs` request building ×3, `map_err` ×6, retry loop ×2 (S4-15) | 3 / 3 | 80 + 25 | same files | medium (caps, release paths) |
| P14 | Wire framing `frame_data` ×4-6 and `encode_stream_frame` ×2 (runner/supervisor) (S4-7) | 6 / 2 | 35 | `aurora_protocol::supervisor` | low-medium (wire format; golden frames must match) |
| P15 | Binary bootstrap: `Cli`+`Command`+`load`+`init_telemetry`+exit-code tail in 6 bins here (api, gateway, relay too: ≈9); `host:port` listen parse ×3; runtime-exit log ×2 (S4-9) | 6-9 | 70-120 | `aurora_core::service` (check it can name `TelemetryConfig` without a cycle) | low; terminator's config-error branch prints differently: keep |
| P16 | `lock()` poison-ignoring free fn ×6 (+~50 inline sites) (S4-14); `db(sqlx::Error)` ×4 (S1-5b); `parts()` | 6 / 4 | 12 + 12 | `aurora_core` / `aurora_db::api_error` | low |
| P17 | Internal HTTP client for gateway/hookhost: `accounts/gateway.rs:58`, `skill_libraries/gateway.rs:78`, `connect/hooks.rs:6` (S2-5); runner/terminator/connector-host plane clients (S4-16, keep estimate low) | 3 / 3 | 45 + 25 | `http::internal_client::InternalClient` | low-medium (`send_internal` stays the final call) |
| P18 | Gateway production: `Ctx` request-log struct built ×5 with 17 fields (`decide.rs:1121,2379,2404,2572,2774`) (S3-D1) → **security-sensitive tier** | 5 | 55 | `Ctx::new(..)` | audit rows |
| P19 | aurora-db: retention `prune_*` ×8 (≈17 lines each, same `DELETE .. SKIP LOCKED`), approval_notify/push twin modules (`batch_siblings` byte-identical), keyset `list_*` triples, advisory-lock `lock_text` ×5, SAVEPOINT trio ×2 (S5-13..15, S1-10) | 8 / 2 / 6 / 5 | 40-60 / 15-50 / 25-40 / 15 / 14 | `aurora-db` | low-medium (SQL text must stay literally the same) |
| P20 | `str_enum!`-style `as_str`/`parse` tables: ≥14 pairs in llm/db/protocol and ~9 in api dtos (S5-12, S1-9). **Plain functions are preferred (D-376): a macro only if a function cannot do it. `parse` fallbacks differ per enum, so generate `as_str` only** | 14 + 9 | 110-150 | `aurora-core` | medium; decide before starting |
| P21 | Smaller: `SCIM` deprovision effect chain ×4 in `resource_users.rs` (security-sensitive: revocation order) (S2-10); app.rs live-settings prefix closure ×3 (20) and org `previews_enabled` query ×3 (18, one in a skipped file); `Set-Cookie` append ×5; settings marker predicates ×2; `valid_handle` vs `aurora_core::ids::is_valid_handle` (byte-equal, core fn unused: the api copy goes); `registry` etc. under-threshold items stay out | | ≈100 total | per item | low |

**Tier 3: security-sensitive. Go last, one batch each, with a `sonnet-integration-reviewer` pass**

| ID | Cluster | Copies | Est. net | Notes |
|---|---|---|---:|---|
| X1 | Anchor-token extractors (relay, runner_plane, ssh, connectors): bearer parse ×5, `X-Aurora-Replica` ×5, `check_protocol` ×3; `replica_unknown` wording differs per plane and is client-visible → helper takes the message (S2-6). Case-insensitive caller bearer ×5 (`scim/auth.rs`, `scim/rate_limit.rs` byte-identical, `http/auth.rs`, `apilimit.rs`) (S2-11). **Keep two helpers: the anchor ones are case-sensitive, the caller ones are not** | 5+5+3 / 5 | 35-55 / 25 | auth |
| X2 | Stream-grant redemption: `terminal/service.rs:319`, `ssh/sessions.rs:691`, `files/service.rs:459` (sandbox query, boot check, hosted, frozen, response literal) (S2-7). Differs on frozen handling (423 vs 409) and hot-reload flag, see findings F3/F4 | 3 | 40 | access |
| X3 | Connect SSRF guard: `check_url`, `refuse`, `URL_MAX`, send/redirect/capped read in `connect/service/link.rs` and `own_server.rs` (S1-7). Parameterise on the error set, never loosen. `connect/service/*` is on no live lane's file list today; re-check before starting | 2 files, 40 lines each | 45-60 | SSRF |
| X4 | Gateway `decide/`: `Ctx` (P18), `HoldOutcome` → refusal mapping ×3 (35 lines) and the peek-then-evaluate snippet ×3 (S3-D2); **findings F1/F2 first** | 3 | 55 + 80 | decide path |
| X5 | Gateway internal endpoints: `err()` ×2-3, bearer check ×3 (`account_test.rs:100`, `library_fetch.rs:204`, `status.rs:96`), `app_connection_in_org` ×2, `pkg_ctx` ×2 (S3-S1); `Invalidatable` impls differ on purpose | 2-3 | 55 | internal auth, preserve 401 body |
| X6 | Supervisor `PtyEngine`/`Streams` credit window and violation accounting (≈35 lines) and process-group `Drop` reaper ×2 (22) (S4-12, S4-13); the supervisor streaming engine is in `p5-apps`' neighbourhood | 2 | 40 + 22 | credit/kill |
| X7 | Crypto/secrets: `valid_key_id` in `aurora-crypto/inline.rs:47` = `ring.rs:25 valid_ring_id` (in-crate merge only, 6 lines); dead `active_key_id`, `swap_registry`; `ct_eq` see findings F5 (S5-16, S5-5) | | ≈20 | owner decides API removal |

**Estimated totals (not measured):** tests ≈ 3,000-3,600 lines (T1-T17), production ≈ 1,600-2,000
(P1-P21) and ≈ 450 in the security tier. Counting the duplicated-line clusters only once where they
overlap (T1/T4, T3/T17, P1/P3/P2 touch the same files), I expect **roughly 4-5 k net lines (about 1 %
of the 470 k)**. That is far below "nine secret inputs everywhere"; most of the volume is test setup in
`aurora-api/tests/it`. Biggest levers for the line count: T2, T1, T3, T4, T5, then P4, P2, P1, P3.

### A.4 Not duplicates (differ on purpose) and findings for the owner
Not duplicates (keep separate):
- `stale()` ×4 (`stale_revision` vs `stale_version`, different texts); `not_found()` helpers (each its own resource text); `evals/service.rs:39 visible_agent` (404 mapping, hosted-only 422).
- `approvals` vs `action_approvals` `decide` (different token-owner rule, two-phase execute); `ListQuery`/`validate_status` differ in allowed sets.
- Name validators (`agents`, `keys`, `host_tokens`, `groups`, `app_instances`, `evals`, ...): same predicate shape, different limits, messages and semantics (host_tokens rejects control chars but says only "1-80 characters"; `clean_label` strips; groups allow `\n`): a shared predicate saves ≈0 lines.
- `derive_handle` (`agents/service.rs:175`) vs `tenancy::agent_handle`: the order of trim/prefix/truncate differs, edge-case handles differ.
- `form_encode` in `aurora-llm/mint/entra.rs` vs `sts.rs` (different unreserved sets); `litellm.rs` vs `openai_compatible.rs` (different origin methods and env names; only the 4-line pieces in P7); `entra`/`gcp` parse; `repo/gateway.rs` approval inserts (extra parked columns); `AuditRow` vs `InstanceAuditRow`; `aurora_config::*Posture` vs `aurora_llm::governance::*` (serde/ToSchema).
- Test helpers with different timeouts or failure behaviour: `wait_audit` variants, `plugins.rs::upload` (no 201 assert) vs `upload_with`, hosted `caps()` (asserted image) vs fake-only `caps()`, runner `terminal_streams::close_code` vs `leg_limits::close_code`, relay `limits.rs fake_app`/`fake_api`, gateway `refresh`/`refresh_proxy` `token_endpoint`, `byo.rs`/`outage.rs` `Capture` (other types).
- Cache `Invalidatable` impls (`seclink`, `plaintext`, `llm_mint`, `resolve`, `resolver`, `byo`): per-scope logic differs. `NoVerify`/`ChainOnly` TLS verifiers: different verification. `decode_stream_data` vs `decode_binary_data`: different tags and errors. classify_failure (claude_code/codex/goose/opencode/pi) differ. `code()` in supervisor `process.rs:808` vs `supervisor.rs:181` (different fallback). `policy/routes.rs` pairs (differ in `#[utoipa::path]`). `placement` roles mirror tenancy roles (OpenAPI schema names). Plugin hook crates (standalone wasm workspaces, <40 lines). `query` structs repeating `limit`/`cursor` can't take `#[serde(flatten)] PageParams` (changes OpenAPI parameters); the four that can (`RunsQuery`, `DeliveriesQuery`, `oauth_clients/dto.rs`, `freeze/dto.rs`) lack `into_params(parameter_in = Query)`: check the generated spec first (≈24 lines, in P3 only if `openapi_matches_checked_in` stays green).
- Poll helpers `eventually`/`wait_for`/`until` across crates: different timings, no shared dev-dependency.

**Findings: copies that differ in a way that may be a bug. Report to the owner; not unified, not fixed here.**
- **F1 gateway `decide/link.rs:310` (Restarting)** builds `ApiError::unavailable("gateway_restarting", ..)` + `guidance::response`, while `decide.rs` uses `guidance::restarting()` which adds `Retry-After: 1`. The link lane has no `Retry-After`.
- **F2 gateway StoreUnavailable arm:** only the request lane (`decide.rs:1750`) counts `health::count_refused("approval_store_unavailable")` and adds `details.reason = database_unavailable`; the link lane and DB lane (`link.rs:322`, `decide.rs:2966`) do neither.
- **F3 hot-reload flag:** `terminal/service.rs:140` reads the live `s.ops().terminal.enabled`, while `files/service.rs:171,477` read `s.config.terminal.enabled` and `ssh/plane.rs:50`, `ssh/sessions.rs:213,429` read `s.config.ssh.enabled` (boot config).
- **F4 frozen agent on stream redemption:** `terminal::open` propagates the 423 from `ensure_agent_active`; ssh redeem and files open map every error to 409 `not_current`.
- **F5 `ct_eq` timing:** `connectors/mattermost/src/callbacks.rs:136` returns early on length mismatch (leaks token length); `aurora-cli/forward.rs:176` and `aurora-crypto/ring.rs:246` pad to the longer. Mattermost does not depend on `aurora-crypto`.
- **F6 name validation:** `tenancy::validate_name` rejects invisible formatting characters (`sanitize::is_stripped`), while `groups`, `agents::name_ok`, `secrets::clean_name`, `app_instances::clean_label` reject control characters only, so invisible-character group, agent and secret names are possible.
- **F7 pagination order:** `keys/routes.rs:46,98` and `groups/routes.rs:75` parse pagination before authorising (bad cursor → 400 even for the unauthorised); `approvals`, `action_approvals`, `invitations` authorise first.
- **F8 claude_code stdout** (`adapters/claude_code/launcher.rs`) uses unbounded `BufReader::lines()`, the other four launchers use `forward_lines` capped at 16 MiB.
- **F9 FIX-151 registry:** `HarnessRegistry::global().register(pid, conversation_id)` is called only by the claude_code and codex launchers, not goose, opencode or pi. If tool authorisation depends on it, platform tools from those three harnesses may be refused.
- **F10 dead branches:** `WorkspaceAuth::can_use()` is always `true` (`aurora-db/src/authz.rs:95`), so every `if !ws.can_use() { 403 }` (`skills/routes.rs:38,49`, `schedules/routes.rs:39`, `webhooks/routes.rs:40`, `peer_tasks/service.rs:69`, and six more in other lanes' files) is unreachable.
- **F11** `summarize_client.rs:~150` calls `.headers(key_id_headers(..))` twice in a row (probably a no-op); `config/validate.rs`: `check_dev_switches` runs for connector_host but not relay/terminator/connector, and their `starts_with("http://")` URL checks are weaker than `check_url`.

### A.5 Skipped because of other lanes
Live diffs checked on 2026-11-18 with `git diff --name-only master...alexiri/<lane>`:
`p5-apps` (registry, `aurora-core/limits.rs`, supervisor `agent_cli/*`+`home.rs`+`lib.rs`+`main.rs`+`supervisor.rs`, `aurora-cli/plugin/{guide,openapi}*`, `hosted/home.rs`, `plugins/{registry,service}.rs`, 7 api tests), `fix-167` (api `domains/channels/*` ×8, `domains/connectors/*` ×6, `user_tokens/service.rs`, 5 api tests), `p5-agent` (none), `p5-platform` (docs only), `p5-dedup-2` (none yet). Queued briefs: P5-GUIDE-7 (`domains/tools/`), FIX-170, P5-WEBHOOK-1/2 (`crates/aurora-webhook`, `aurora-registry`, api `webhooks` routes, gateway `link` send path, connect path), FIX-169, FIX-171..173 (web).

| Skipped | Where | Do after |
|---|---|---|
| P1 audit wrapper `connectors/admin.rs:83` | fix-167 | fix-167 merge |
| P2 `bad`/`invalid` wrappers in `channels/{integrations,presences,mod}.rs`, `connectors/{host,ops,plane}.rs` | fix-167 | fix-167 merge |
| P3 page rebuild in `channels/{links,reach,integrations}.rs` | fix-167 | fix-167 merge |
| P5 `hex` in `plugins/registry.rs:47`, `connectors/manifest.rs:27`, `addon_sources/*` | p5-apps / fix-167 | merges |
| P4 dead `channels/cards/build.rs turn_controls` (mirror.rs is fix-167's), `addon_sources/poll.rs actor()` | fix-167 / unknown lane | merge; re-check references |
| P8 tick loops `approvals/notify.rs:301,320`, `hosted/readiness.rs:445`, `addon_sources/poll.rs:16` | other | other lanes merge |
| X1 `connectors/{auth,host}.rs` bearer/replica/protocol copies | fix-167 | fix-167 merge |
| `previews_enabled` query `hosted/home.rs:660`; `valid_id` in `connectors/admin.rs:74`, `valid_name` `connectors/setup.rs:49` | p5-apps / fix-167 | merges |
| P11/T12 adapters, supervisor `agent_cli/*`, `home.rs`, `supervisor.rs` (`code()`), `tests/it/{home,tools,agent_cli}.rs` fake-runner copies | p5-apps | p5-apps merge |
| T1 copies in `home_agent_guides.rs:78`, `channels_inbox.rs`, `connectors.rs`, `connectors_managed.rs`, `plugins_upgrade.rs`; T3 `code()` in `user_tokens.rs:441` | p5-apps / fix-167 | merges |
| `aurora-cli` `tests/openapi.rs` `aurora_bin()`/`tmp()` | p5-apps | p5-apps merge |
| All of `domains/webhooks/*`, `tests/it/webhooks.rs`, gateway `decide/link.rs` (X4), `skills`/`webhooks` rig blocks of T2 | P5-WEBHOOK-2 (api webhook routes, gateway link) | WEBHOOK-2 merge; do the T2 `memory.rs`/`skills.rs` part now only if `skills.rs` is untouched, else wait |
| `domains/tools/*` (`tools/messaging.rs` has an 85-line jscpd self-clone), `tools.rs` rig | P5-GUIDE-7 | GUIDE-7 merge |
| `aurora-registry` clusters, `aurora-core/limits.rs` | P5-WEBHOOK-1, p5-apps | merges |
| Terminal wrappers in `terminal/*` (X2), FIX-170 (`aurora-llm` model choice, P7 touches `Endpoint`: **rebase first**) | FIX-169 / FIX-170 | their merges |

`web/**` is P5-DEDUP-2's. Part B clusters that sit **only** in non-skipped files and can start now:
T3, T4, T5, T6-T11, T13, T15-T17, P6, P7 (after a FIX-170 file check), P9 (non-skipped files), P12, P14, P16, P19.

## Part B batches (lane p5-platform, worker 41, 2026-11-19)
Lines are `git diff --shortstat` of the batch commit (insertions/deletions, test code only; counts include blank lines). Gate per batch: `cargo clippy -p aurora-api --tests -- -D warnings`, the tests of every touched file by module filter, guards; no full suites (owner 2026-11-19, GitHub CI is the full gate). Test counts: the same tests run as before (they were run per module, none removed).

| Batch | Commit | Δ lines | Tests run |
|---|---|---:|---|
| T2 hosted/runner rigs: `schedules.rs` uses `hosted::Rig`; new `tests/it/runner_rig.rs` (seeded-sandbox rig) shared by `memory.rs`, `skills.rs` | fe15ea6a | +189 / −646 (−457) | schedules, memory, skills: 30 passed |
| T1 plugin install block: `Admin::trust`/`install`/`install_with` (`plugins_lifecycle.rs`) replace nine copies | 760057b7 | +84 / −478 (−394) | 10 modules + `plugins_lifecycle`: 149 passed |
| T3 `Harness::call`/`call_tok` replace the cookie+origin bodies of 43 per-file `call()` helpers (signatures kept) | b3bf2814 | +70 / −281 (−211) | 42 modules: 430 passed |
| T4 `Harness::hosted_agent`/`byo_agent` replace 33 `INSERT INTO agents` sequences | da209b15 | +53 / −131 (−78) | 28 modules: 421 passed |
| **Net, four batches** | | **−1,140** | |

Scripted bulk rewrites (stated in each commit, diffs reviewed): T2 line-range deletions; T1 six identical blocks replaced by line range; T3 and T4 regex rewrites over exact-shape copies (T3 first run over-matched into off-limits files and a mis-anchored regex, was reverted with `git checkout` and re-run with a `[^;]` anchor and a skip list).

Left, and why: `webhooks.rs`, `tools.rs` rigs (P5-WEBHOOK-2, P5-GUIDE-7); `app_setup.rs`, `apps_connections.rs`, `plugins*.rs`, `secrets.rs`, `permission_matrix.rs`, `model_roles.rs` (p5-polish-b), `settings.rs`, `settings_ops.rs` (p5-agent) skipped by T3/T4; `attachments.rs` rig and `fix_029.rs`/`runner_plane.rs` partial rigs (different shape: own `Live`, second runner, admin cookie; need a parameterised rig, next); hosted `agent()` with grant rows and the `$1::text`-handle (11) and `'ada'` (8) agent shapes (T4 remainder); inline conversation/turn seed rows (T4 rest); T3's 67 inline `.send(` blocks. Next: T5 runner test support, T16/T17, T6–T11 gateway test support, then the P clusters (P4 dead code first).
FIX-182 (priority, separate commit 52d1602b) is not a dedup batch; its report is in `docs/tasks/FIX-182.md`.
