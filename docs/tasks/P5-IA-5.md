# P5-IA-5: Settings in words, and current values instead of empty forms (owner, 2026-11-15)

## Goal

The Administration settings pages speak config:
- labels derived from keys ("Known llm hosts extra", "Default source cidrs");
- raw values (`sanctioned_only`, `credentialed_only`, an empty `[]`);
- spec references in the page text ("08 §11.1", "docs/operator/gateway-posture.md");
- a "DEFAULT" badge that does not say what the default is.

Nine sections showed "No settings in this section yet" with a Save button, while their real values (from
Helm) were shown nowhere.

This task adds one **field registry** with labels, help, choices, defaults and docs; renders every
settings field from it; shows **current values read-only** for sections that are not editable yet; and
adds a test that keeps spec references, raw values and retired words out of the interface (D-340, D-341).

## Linked requirements

- **Design** (D-354): the canvas “Aurora — agents, accounts and access”, https://claude.ai/artifact/3UwmUgrhh6MLCsfqdhTFmQ, boards Administration › People › Sign-in, Platform › Email, Agents › Defaults, All settings (labels, help, “Default:” badges, Helm locks); Read me › Patterns. Read each board's note; where a board and the spec disagree, the spec wins.
- D-340, D-341; contracts **C-972, C-973, C-974**.
- Normative: `14 §Information architecture` › "Settings in words" and the vocabulary table,
  `15 §Instance settings` (sections, movable keys), `05` (settings section row).

## Wave and order

- **Wave 2.** Starts after P5-IA-1 (`SettingsSectionFields`).
- Disjoint from P5-IA-3 and P5-IA-4, which compose pages around the form this task renders.
- **SET-4/5** register keys in the same registry file. From this task on, a new movable key needs its
  field metadata (the completeness test requires it). The Orchestrator orders this task after SET-4.

## Context

- **Section registry.** `crates/aurora-config/src/settings.rs` has section-level `description` only (~75,
  ~329–390). Movable keys are listed in `deploy/helm/aurora/movable-keys.txt`, kept equal to the registry
  by a unit test.
- **Section route.** `GET /v1/admin/settings/{section}` (C-772) returns each key's effective value,
  source, lock, Helm key and default.
- **Rendering.** `web/src/components/admin/settings/settings-section-form.tsx`: `labelOf(key)` (~133)
  derives labels from keys.
- **Keys not editable yet.** The keys SET-4/5 register are listed per section in `15 §Instance settings`;
  today they are read from the running configuration.

## Scope

1. **Field registry (C-972).**
   - Add `FieldMeta { key, label, help, kind, choices: [{ value, label, description }], default_display,
     unit, doc: { title, path } }` next to the section registry. Fill it for every movable key, and for
     the keys `15` lists for sections that are not editable yet.
   - Write in the dashboard's voice: what the setting does and when to change it, one sentence, in the
     words of the vocabulary table.
   - A unit test fails if a movable key has no `FieldMeta`, if a choice lacks a label, or if a help text
     contains `§` or `docs/`.
2. **API (C-972, C-973).**
   - `GET /v1/admin/settings/{section}` adds `fields[]`.
   - For a section with no editable keys yet, it adds `editable: false` and
     `current: [{ key, value_display, source }]` from the running configuration. Secret keys read only
     "set" or "not set", with the section route's existing redaction.
   - OpenAPI and the generated types.
3. **Dashboard.**
   - `SettingsSectionFields` renders from `fields`:
     - labels and help;
     - `ChoiceCards` for enumerations, with a sentence each;
     - list editors for lists, never JSON;
     - numbers with units;
     - the **Default** badge reading "Default: <default_display>";
     - an operator-doc link by title.
   - Remove `labelOf`.
   - Read-only sections render `current` as a description list: label, value in words, a source chip
     (Helm / Default), and "Change it in your Helm values: <doc title>". No Save button.
   - All settings' search covers field labels and help.
   - Remove the spec references from the governance and AI-endpoint page texts.
4. **UI text test (C-974).** A web test scans the rendered strings of `web/src/components` and
   `web/src/app` and fails on:
   - `§` followed by a digit;
   - the substring `docs/`;
   - a raw enumeration id shown as a label (`sanctioned_only`, `credentialed_only`, `owners_only`,
     `allowlist_only`, `open_lane`);
   - `[]` shown as a value;
   - a key-derived label function;
   - a retired word of the vocabulary table used as a page name or label ("Governance", "Policy",
     "Credentials", "Simulate", and "grant", "secret" or "connection" for an account), and the button
     labels "Disable" and "Enable".

   The test reads rendered strings, not comments or identifiers. "Secret stores", the role "Member" and
   the organization's "Members" tab are allowed (`14` vocabulary notes).

   **Who fixes what.** Briefs of waves 2 and 3 are rewriting some of the files this test flags, and
   some run beside this task. So:
   - This task fixes every flagged string in a file that no brief of wave 2 or 3 owns.
   - For the vocabulary check only, the test keeps a list of the other flagged files, each with the
     brief that owns it (read the "Constraints / file ownership" of P5-UX-3, -5, -6, -7, -9, -10, -11,
     -14, -16, -17 and P5-IA-3, -4). Do not edit those files.
   - The list can only shrink: the test also fails when a listed file no longer contains a retired
     word, so the brief that cleans a file removes its entry.
   - The list must be empty when wave 3 is done; PROGRESS records that check.
5. **Tests.**
   - Rust: registry completeness; `fields` per section; `current` for a section that is not editable; a
     secret key never echoed.
   - Vitest: labels, choice cards and list editors from fixtures; read-only sections have no Save; the
     Default badge text; All settings' search finds a field by its help text.
   - Playwright: the BYO choice on Administration › Agents reads in words; a section that is not
     editable shows its current values read-only.

## Constraints / file ownership

- Owns: `crates/aurora-config` (the field registry and its test); the settings section route (`fields`,
  `current`); the rendering inside `SettingsSectionFields`; the UI text test and the strings it flags.
- The help-text writing is a good Haiku sub-step, reviewed for accuracy against `15`.

## Acceptance criteria

1. No settings field shows a key-derived label, a raw enumeration, a JSON value or a spec reference; the
   test enforces it, along with the vocabulary.
2. Every section is either editable in words or read-only with its current values; none is an empty
   form.
3. Defaults are shown in words.
4. Rust and web gates; axe and narrow-route suites for the settings pages, both themes.

## Suggested agent/model and reason

Sonnet for the registry, the API and the rendering; Haiku for the help texts under review.

## Completion report (worker 19, 2026-11-16)

Commits: `35a34fbc` (aurora-config `fields.rs`, `fields`/`editable`/`current` on the section routes and `fields` on the index, OpenAPI, `schema.d.ts`), `c644f9b3` (web rendering, All settings search), `36da6f98` (UI text test, unowned strings, Playwright mocks and specs), plus the gate fixes.

- **Registry:** 73 movable keys with label, help, kind, choices, default in words, unit, doc title. Durations stay seconds on the wire; `unit` is the edit unit. `float` kind added (rate multiplier). Section "Governance" is titled "Instance rules".
- **`editable: false` / `current`:** every section is editable since SET-4/5, so no live section answers it; the mechanism is `READ_ONLY_SECTIONS` (empty) in `service.rs`, proven by a unit test (secret never echoed) and the Playwright mock.
- **UI text test:** `web/tests/ui-text.test.ts`; `VOCABULARY_EXEMPT` (16 files, each with its brief) and `OTHER_EXEMPT` (setup-view → P5-UX-6, governance-view → P5-IA-3/2) can only shrink.
- **Not done:** the stacked two-form pages (not in the brief); `OrgsStanding` shows de-underscored posture ids (P5-IA-3's file); doc titles are text, not links.
- **Gate:** fmt, workspace clippy, guards, schema check 0; `aurora-config` 30, `-core` 74, `-db` 38 (+8), `aurora-api` all targets 1194/1195 (the one failure, `channels_inbox::a_channel_event_turn_belongs_to_the_inbox_job_trace`, passes alone: load flake); OpenAPI drift and `gen:api` clean; web typecheck 0, lint 0 errors, vitest 1920 passed, build ok; Playwright mocked+tracing 892 passed / 10 failed, the 10 were my own new/changed specs (dl markup axe, "1800" → "30 minutes"), fixed and rerun green; production project 8 passed. `connect-access.spec` fails only on a cold dev server (the redirect target compiles in >5 s); it passes warm and I raised that one assertion's timeout to 30 s. Fail-before not re-run by reverting (new Rust tests call `field()`, which did not exist).
- Rule slips: one heredoc append (the `service.rs` test module); a stray `sed -i` on `/dev/null` (failed, harmless).
