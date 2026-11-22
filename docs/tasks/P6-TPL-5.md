# P6-TPL-5: Agent templates — Save as template (owner, 2026-11-21 and 2026-11-22, phase 6, D-435(f), D-436, D-456, D-457, C-1118, C-1119, C-1141, C-1142, SEC-146)

## Why
The owner wants **Save as template** in the first version (D-435(f)): the easiest way to write a template is to set up
an agent, then save it. Templates live as source directories in a git repository (D-456(b)). So the saved file is a
zip rooted at `<id>/`, which a publisher unzips into their template repository as it is, then pushes; the
repository's CI publishes it (C-1144).

The file is the agent export with `template.yaml` added. The agent's library skills are written as **references**,
not copies (D-456(d), C-1142), and its helper files are kept. It downloads a file and publishes nothing.

Read first:
- `P6-TEMPLATES.md`;
- `API_CONTRACTS` **C-1118**, C-1141 (the format the file must pass), C-1142 (the reference format) and C-1143 (the
  helper routes you read);
- `14 §Export and import an agent` and the Manage page (D-447, FIX-213);
- `web/src/lib/export/agent-export.ts`, `agent-import.ts`, `yaml.ts`, `zip.ts` and
  `web/src/components/agents/export-import.tsx`. Reuse them; do not write a second zip or YAML writer;
- `docs/25-agent-templates.md` (P6-TPL-1).

## Scope
1. **`lib/export/agent-template.ts`**: `templateYaml(meta, apps, skillRefs)` and
   `templateFiles(export, meta, apps, skillRefs, helpers)`, on top of `exportFiles`. They follow C-1118:
   - every path under the root `<id>/`;
   - `model: default`;
   - schedules without `time_zone`, and no pending one-time schedules;
   - `README.md` = the dialog's **What it's for** text plus C-1118's closing line (D-458);
   - the ticked groups only.
   - **`apps`** come from the agent's grants: an `act`/`ask`/`read` level, or `custom` written as
     `tools: { allow, ask }` with the tool ids. The app id is the grant's connection's app. Never write the
     connection id, the account name or anything else of the account.
   - **Skills:**
     - The agent's own agent-tier skills are copied as `skills/<name>/` (as the export does).
     - Its **library-sourced** skills (`source.library_id`) become `skills:` references: `library` = that library's
       `url`, plus its `ref` and `path` when set; `skill` = the skill name; `required: true`; `why` = the skill's
       description cut to 200. Their content is never written. Read the library fields the agent's skills route
       already returns. If it lacks the `url`, use `GET /v1/skill-libraries/{id}` with the caller's rights; a library
       the caller cannot read makes that skill be left out, and the dialog says so.
     - Workspace and organization skills are never written.
   - **Helpers** come from `listAgentHelpers` and `getAgentHelper` (P6-TPL-6), written under `helpers/` with their
     executable bit (the zip writer must carry it; extend `zip.ts` if it does not, keeping exports byte-identical).
2. **Dialog**: **Save as template…** beside Export on the Manage page, in `components/agents/export-import.tsx` or a
   sibling component, for the same people and altitude.
   - **What it's for** (D-458): a required Markdown text area with a Preview toggle (the same `TemplateReadme`
     rendering as P6-TPL-4; if that is not merged yet, use the shared `Markdown` with the C-1116 rules and leave a
     note for P6-TPL-4 to switch). It is prefilled with C-1118's outline, filled in with the agent's name. Save is
     refused while the outline's placeholder lines are unchanged ("Say what <Agent> is for, so people can decide
     whether to use it").
   - The other fields and checklist are exactly C-1118's: memories unticked by default with its sentence; helper files
     ticked when there are any; Why required per app; Required per app; the id follows the name until edited and is
     checked against the C-1141 rule; the version is a semver.
   - One line lists what is left out: "<n> organization or workspace skills are not included", "<n> one-time
     schedules are not included", and library skills that could not be read.
   - Client-side checks, with one sentence each: the counts and sizes of C-1141 (the `TEMPLATE_*` limits); text
     containing `{input.`, `{agent.` or `{person.`; an executable file that is not UTF-8 text.
   - **Save** downloads `<id>-<version>.zip`.
3. **Golden archive**: a vitest builds a fixed template (a fixed export with one own skill, one library skill, one
   helper, fixed apps and a fixed date) and compares the bytes with
   `crates/aurora-template/tests/fixtures/web-saved-template.zip`.
   - Commit that file; the test generates it with an env switch, documented in the test.
   - Add **one Rust test** in `crates/aurora-template` that reads it with `read_archive` and asserts the parsed
     content, the reference and the helper's executable bit included. This is the only Rust edit.
   - The web export stays byte-identical for exports: its existing tests stay green, untouched.
4. **Docs**:
   - `docs/14 §Export and import an agent` gets a paragraph for Save as template.
   - `docs/25`'s "start from Save as template" section names the button as it really is, and gives the steps: unzip
     into the repository folder, raise nothing (the version is `1.0.0`), open a pull request, merge.

## Not in this brief
Any API change; publishing into the installation from the dashboard (not in this version).

## Tests move with the UI (owner, 2026-11-20 — binding)
In every commit, find every test that sees what you change, and update it in the same commit. That means
`web/tests/**`, `web/e2e/**` (the mocked **and** the `production` projects), `scripts/e2e/*.sh`, fixtures and mocks.
At least run `rg -n "<old text>|<old selector>" web/tests web/e2e scripts/e2e services crates` per change.

Never loosen, `.skip`, `test.fixme` or delete a test to get green.

Run through `heavy.sh`:
- every vitest file you changed, and every one that renders a component you changed;
- every mocked Playwright spec that visits a page you changed.

Production specs are updated by reading, and listed in the completion report.

## Acceptance criteria
- [ ] Vitest:
  - `templateFiles` for each group combination;
  - every path under `<id>/`;
  - apps by level and custom;
  - a library skill written as a reference, with no `skills/<name>/` for it;
  - helpers with executable bits;
  - **no connection id, account label, secret, webhook address, library token or model name in any written byte**
    (assert over the whole zip);
  - pending one-time schedules left out and counted;
  - organization skills left out and counted;
  - `README.md` is the What it's for text plus the closing line, and an unchanged outline is refused;
  - each client-side refusal;
  - the golden archive byte-identical.
- [ ] The Rust test reads the golden archive with `aurora_template::read_archive` (no problems), and
  `aurora template check` on it prints `ok`.
- [ ] Playwright (mocked project): open Save as template on an agent with two apps, an own skill, a library skill, a
  helper, a memory and a schedule; untick nothing but memories; Save is refused until What it's for is written;
  write it; save. The download is named `<id>-1.0.0.zip`.
- [ ] `pnpm exec tsc --noEmit`, lint, `pnpm build`; `cargo test -p aurora-template`. All through `heavy.sh`.

## Ownership and sequencing
- **Owns:** `web/src/lib/export/agent-template.ts` (new); the Save as template dialog and its tests; the golden
  fixture and its Rust test; the two doc paragraphs. The Export and Import code is read, not changed. A small export
  of a helper is fine, and so is an executable-bit option in `zip.ts` that leaves export bytes unchanged.
- Starts after P6-TPL-1 is merged. The helper part needs P6-TPL-6's routes: mock them from C-1143 until it merges,
  then finish. It can run beside P6-TPL-2 (no shared files).
- If P6-TPL-4 runs at the same time, it owns the Manage page's new Template section. Keep this change inside the
  Export and import part.

## Numbers
D-435, D-436, D-456, D-457, D-458, C-1118, C-1119, C-1141, C-1142, SEC-146. No migration.

## Suggested agent/model and reason
`nextjs-developer` (Sonnet). A dialog and a pure file builder on the existing export code. The cross-check with the
Rust reader is one small test.
