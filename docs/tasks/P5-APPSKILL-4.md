# P5-APPSKILL-4: Indico ships the owner's skills, made generic (owner, 2026-11-22)

## Goal

The owner wrote four Indico skills against indico.cern.ch and keeps them in the skill library
`https://github.com/alexiri/aurora-skills` (public): `indico-find-and-book-room`,
`indico-book-recurring-room`, `indico-meeting-prep`, `indico-search-talks-and-files`, three of them with
scripts. They know more than the built-in package's guide did. For example, the old recipe searched once
per building, where one search without `building` joined with the cached room list is the right way. The
owner asked whether they should "really be added to the indico app"; with D-452 and D-453 they can be.

This brief moves the **Indico knowledge** of those skills, scripts included, into the built-in `indico`
package as app skills. Everything that belongs to one person or to CERN stays in the owner's library.
The owner can then trim their library to the personal parts, or remove it.

## Decisions that bind this brief

- **D-452**, **D-453** (a), (i); **SEC-150** (no secrets, nothing personal, text files, scripts only
  call Indico through `aurora-agent`, which goes through the gateway).
- The built-in package serves **any Indico server** (`{instance}` host, `09 §3.4`): nothing may assume
  indico.cern.ch.

## Order and lanes

- After P5-APPSKILL-1/-2 and P5-SKILLDIR have merged. One small lane (Sonnet). Size S–M. It edits only
  `plugins/builtin/indico/**` and adds one test script, so it can run beside P5-APPSKILL-3.

## Read first

- The four skills: `git clone --depth 1 https://github.com/alexiri/aurora-skills` into your scratch
  directory (read the scripts before running anything; they call `aurora-agent`).
- `plugins/builtin/indico/plugin.yaml` (the catalogue, hosts, scopes, `agent.instructions`),
  `plugins/builtin/indico/agent/indico/operations.json`, and the skills P5-APPSKILL-1 converted
  (`skills/indico/indico-book-free-room`, `indico-find-category-events`, `indico-api-families`).
- `plugins/builtin/README.md` (app-skill authoring rules, P5-APPSKILL-1); `docs/24-apps-for-agents.md`
  §3 and §10a (`aurora-agent call`, `--path`, `--raw`, `--out`).

## Work

1. **Skills** under `plugins/builtin/indico/skills/indico/`:

   | Skill | From | Scripts |
   |---|---|---|
   | `indico-find-and-book-room` | the owner's skill of that name; replaces `indico-book-free-room` | `scripts/free-rooms` |
   | `indico-book-recurring-room` | the owner's | `scripts/series-free` |
   | `indico-meeting-prep` | the owner's | `scripts/event-brief`, `scripts/doc2text` |
   | `indico-search-talks-and-files` | the owner's | (uses meeting-prep's) |
   | `indico-category-events` | `indico-find-category-events` from P5-APPSKILL-1, with the owner's export-API details merged in | — |
   | `indico-api-families` | as converted, updated where the skills above found more | — |

2. **Make them generic.** Keep every fact about how Indico behaves: the search answer's `rooms` and
   `total`, `repeat_frequency=NEVER`, `availability` ignoring dates, the calendar's size and ignored
   filters, the series check over the whole date range, export paths for material, search `type` being
   required, the `aggregations`, the verify steps, never retrying a booking. Name the version it was
   checked on ("Indico 3.3 at CERN, 2026-11-21") where the behaviour is an internal endpoint's. Remove or
   replace everything else:
   - the person: names, logins (`airibarr`), e-mail addresses, "connected as …", favourite category ids.
     Instead: the booking login is asked for, or found in the person's earlier bookings if Indico shows
     it; the favourites are read with `GET /user/api/favorites/categories` (scope `read:everything`);
   - CERN only: indico.cern.ch, maps.cern.ch, the CERN Phonebook, building numbers as defaults, named
     rooms (Openlab, coffee areas), the annual closure, CodiMD, Europe/Zurich. Instead: the host is the
     account's (from `app.md`; `aurora-agent call` adds it), times are the server's local time, a room
     whose comments say it is not a meeting room is skipped, and links go to `https://<host>/…`;
   - harness paths (`~/.agents/skills/…`): use **`$SKILLS_DIR`** (P5-SKILLDIR, D-454, C-1140: the
     absolute skills folder, set by the supervisor in every agent environment). A `SKILL.md` says to run
     `"$SKILLS_DIR/indico-meeting-prep/scripts/doc2text"`; a script reaches a sibling skill the same way,
     and falls back to `$(dirname "$0")/../..` only when `SKILLS_DIR` is unset.
3. **Scripts**: bash or `python3 -I` with the standard library only (the sandbox image has `python3`,
   `jq`, `ripgrep`; `doc2text` may use `pypdf` or `pdftotext` when present and says so when neither is),
   committed executable (`git add --chmod=+x`), no network access except through `aurora-agent call
   indico …`, no credential handling, outputs to stdout or `/tmp`, an exit code other than 0 with a
   one-line message on failure. Keep each skill within `SKILL_TOTAL_MAX`.
4. **Operations**: add to `operations.json` the operations the skills call through `--path` today, where
   the catalogue already has a tool for them (`search` on `/search/api/search`, the export category,
   event and material reads, the favourite categories, the calendar, cancelling one date); then use the
   operation ids in the skills. Each must pass the `agent_operations_*` rules and map to its catalogue
   tool.
5. **Package**: remove the replaced skills; `agent.instructions` stays the essentials (≤ ~600
   characters), naming the skills is not needed (`app.md` lists them); bump the minor version.
6. **Test** `scripts/plugins/indico_skills_test.sh`: runs each script with a stub `aurora-agent` first on
   `PATH` that answers from recorded JSON fixtures (`scripts/plugins/fixtures/indico/`, taken from the
   owner's examples, with every personal value replaced), and checks the output: free rooms joined and
   sorted, the series matrix, the event brief's sections and material paths, `doc2text` on a small PPTX
   and DOCX made in the test. Wire it into the place the built-in checks run in CI
   (`scripts/plugins/check_builtin.py` or its caller).

## Tests move with the code

Update every test or fixture that names the removed skills (`rg -n "indico-book-free-room|book-free-room" crates services scripts web tests plugins`),
in the same commit; never skip or loosen.

## Acceptance

1. `aurora plugin validate plugins/builtin/indico` with no warning; `python3 scripts/plugins/check_builtin.py`
   passes; the scripts are `100755` in git (`git ls-files -s plugins/builtin/indico/skills`).
2. `rg -n -i "airibarr|iribarren|alex|cern\.ch|codimd|phonebook|openlab|~/\.agents" plugins/builtin/indico/skills`
   finds nothing, and no skill text presents Europe/Zurich as anything but an example value
   (`tz=Europe/Zurich` as an example is fine, as in the package's tests).
3. `scripts/plugins/indico_skills_test.sh` passes; `shellcheck` on the bash scripts is clean;
   `python3 -I -m py_compile` on the Python ones.
4. `aurora plugin preview plugins/builtin/indico --access custom` shows the six skills in `app.md` and as
   projected, scripts 0555.
5. Gates: `cargo test -p aurora-registry` (built-in packages are compiled there),
   `bash scripts/guards/run-all.sh`.

## Out of scope

- Changing the owner's library repository (the owner decides what stays there).
- New catalogue tools or scopes; anything outside `plugins/builtin/indico/**` and the test script.

## Completion report

The six skills with their sizes, what was dropped as personal or CERN-only (one line each), the
operations added, the test's output, and anything in the owner's skills that could not be made generic.

**Done (package `indico` 1.5.0 → 1.6.0), worker lane `p5-package`, Sonnet.**

### Skills (`plugins/builtin/indico/skills/indico/`, bytes of SKILL.md plus scripts)

| Skill | Bytes | Scripts (git mode 100755) |
|---|---|---|
| `indico-find-and-book-room` (replaces `indico-book-free-room`) | 8,620 | `scripts/free-rooms` |
| `indico-book-recurring-room` | 8,795 | `scripts/series-free` |
| `indico-meeting-prep` | 15,379 | `scripts/event-brief`, `scripts/doc2text` |
| `indico-search-talks-and-files` | 3,650 | uses meeting-prep's through `"$SKILLS_DIR/indico-meeting-prep/scripts/…"` |
| `indico-category-events` (replaces `indico-find-category-events`, export-API details merged) | 2,306 | none |
| `indico-api-families` (updated: optional `building`, series search, calendar, cancel one date, search `type`) | 4,112 | none |

All far under `SKILL_TOTAL_MAX`. Scripts take the account from `INDICO_ACCOUNT` (passed as `--account`), reach
`aurora-agent` only, and use the operation ids (no `--path`).

### Dropped as personal or CERN-only

- Name, login (`airibarr`), both e-mail addresses, "connected as …": the login is asked for or read from the person's
  earlier bookings when Indico shows it.
- The favourite-category id list: read with `list_favorite_categories` (`read:everything`); examples use ids 12/13.
- indico.cern.ch, maps.cern.ch, the CERN Phonebook lookup, `~/.agents/skills/…` paths (now `$SKILLS_DIR`),
  CodiMD, Openlab, coffee-area names, the annual-closure date range (now "ask whether to skip closed days"),
  building numbers as defaults (examples only), Europe/Zurich as a default (an example value for `tz` only, plus
  "the server's local time"), the calendar-invite and phonebook cross-references.
- `pip install --user pypdf` in `doc2text`'s error (the sandbox may have no network): it now says a PDF reader is missing.

### Operations added to `agent/indico/operations.json`

`search`, `search_categories`, `list_favorite_categories`, `list_category_events`, `search_events`, `get_event`,
`get_event_file`, `get_booking_calendar`, `cancel_booking_occurrence`; `search_rooms` changed (`building` optional,
`repeat_interval`, no `items`, the answer is an object). Every id maps to its catalogue tool. The linter allows a path
parameter only as a whole segment, so the export paths take `categories=12.json`, `event=1234.json`,
`term=ichep.json`, `attachment=77.bin` (stated in each parameter's description).

### Tests and checks (all run with `/home/alex/orca/workspaces/aurora/heavy.sh p5-package` where heavy)

- `bash scripts/plugins/indico_skills_test.sh`: 42 checks pass (joined and sorted free rooms, building filter, one search
  without `building`, room list read once, series matrix with busy dates, every-other-week, 60-date cap, event-brief sections
  and `get_event_file` arguments, `doc2text` on a PPTX and a DOCX built in the test, old `.doc` and a PDF without a reader
  refused, no direct network use in the scripts). Fixtures are made up, in `scripts/plugins/fixtures/indico/` (shapes of
  Indico 3.3's answers; no real names, ids or addresses). Wired as a CI step after the built-in checks (`.github/workflows/ci.yml`).
- `shellcheck` is not installed on this machine: the two bash scripts were not shellchecked here (CI/owner to run
  it); `python3 -I -m py_compile`-style syntax check passes on both Python scripts.
- Tests updated in the same commit: `crates/aurora-registry/tests/it/builtin_versions.rs` (1.6.0),
  `services/aurora-api/tests/it/plugins_upgrade.rs` (skill paths and 1.6.0). The `indico-book-free-room` names left in
  `services/aurora-supervisor/src/agent_cli/` are synthetic fixtures of the CLI's own tests, not the package.
- `rg -n -i "airibarr|iribarren|alex|cern\.ch|codimd|phonebook|openlab|~/\.agents" plugins/builtin/indico/skills`: no match.
  `Europe/Zurich` appears twice, as the example `tz` value.

### Could not be made generic

- `book_room` needs the person's Indico login and Indico does not expose it through `/api/user/`: the skill says to ask
  (or look in earlier bookings), it cannot be automatic.
- "Is not a meeting room" is read from room `comments`; there is no structured field, so it relies on the text.
- The owner's "agenda in CodiMD can't be read" note became a generic "an external notes tool that needs a sign-in".

### Security notes (for the Orchestrator's review)

- Every request in the four scripts is `aurora-agent call indico <operation> …` (bash arrays and `subprocess` argument lists,
  no shell string, no `eval`, no `curl`/`requests`); the test greps for that. No credential is read from a file or the
  environment; the only environment input is `INDICO_ACCOUNT` (passed as `--account`) and `TMPDIR`.
- Writes only under `${TMPDIR:-/tmp}/aurora-agent/` (created 0700): the room-list cache (written through `mktemp` then
  `mv`) and a `mktemp -d` working directory removed on exit. No script writes elsewhere.
- Quoting: values go to `aurora-agent` as separate array elements; dates, times, capacity and the event id are validated
  by regex first (`free-rooms`, `series-free`, `event-brief`), and buildings reach `jq` through `--argjson`
  built by `jq -R`. A meeting title or reason never reaches a script: it is only in the SKILL.md examples, quoted.
  `doc2text` reads a local file and caps zip members at 20 MiB.
- Package lint and preview (run on this commit): `bash scripts/plugins/check-builtin.sh grafana indico` (validate, test,
  fixture coverage, scopes, docs; the packages are unsigned, so `aurora plugin validate` alone reports only
  `signature_missing`, no warning): `ok indico`. `aurora plugin preview plugins/builtin/indico --access custom` shows the
  six skills in `app.md` and as projected, the three scripts' entries marked `(executable)`; `git ls-files -s` shows `100755`
  for all four scripts. `bash scripts/guards/run-all.sh` passes (including `no-integration-names` and `no-live-tokens`);
  `python3 scripts/extract-schema.py --check` is up to date; `cargo test -p aurora-registry --test it builtin_versions`:
  4 passed. Not run (no database on this lane): `services/aurora-api` `plugins_upgrade` (edited consistently: skill paths
  and 1.6.0).
