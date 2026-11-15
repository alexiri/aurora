# P5-IA-5: Settings in words, and current values instead of empty forms (owner, 2026-11-15)

## Goal

The new Administration settings pages speak config:
- labels derived from keys ("Known llm hosts extra", "Default source cidrs");
- raw values (`sanctioned_only`, `credentialed_only`, an empty `[]`);
- spec references in the page text ("08 §11.1", "docs/operator/gateway-posture.md");
- a "DEFAULT" badge that does not say what the default is.

Nine sections show "No settings in this section yet" with a Save button, while their real values (from
Helm) are shown nowhere.

This task adds one **field metadata registry** with labels, help, choices, defaults and docs. Every
settings field renders from it. Sections that are not editable yet show their **current values
read-only**. A test keeps spec references and raw values out of the UI (D-341, D-340).

## Linked requirements

- D-340, D-341; contracts **C-972, C-973, C-974**.
- Normative: `14 §Information architecture` › "Settings in words", `15 §Instance settings` (sections,
  movable keys), `05` (settings section row).

## Context

- **Section registry.** `crates/aurora-config/src/settings.rs` has section-level `description` only (~75,
  ~329–390). The movable keys are listed in `deploy/helm/aurora/movable-keys.txt`, kept equal to the
  registry by a unit test.
- **Section route.** `GET /v1/admin/settings/{section}` (C-772) returns each key's effective value,
  source, lock, Helm key and default.
- **Labels in the dashboard.** `web/src/components/admin/settings/settings-section-form.tsx`, `labelOf(key)`
  (~133).
- **Unregistered keys.** The keys SET-4/5 will register are listed per section in `15 §Instance settings`
  (the movable keys table). They are read from the running configuration today.

## Scope

1. **Field registry (C-972).**
   - Add `FieldMeta { key, label, help, kind, choices: [{ value, label, description }], default_display,
     unit, doc: { title, path } }` next to the section registry. Fill it for every registered movable key,
     and for the keys `15`'s table lists for SET-4/5 sections, so C-973 can show them.
   - Write the text in the dashboard's voice: what it does and when to change it, one sentence each, no
     spec references.
   - A unit test fails if any movable key has no `FieldMeta`, if a choice lacks a label, or if a help text
     contains `§` or `docs/`.
2. **API (C-972, C-973).**
   - `GET /v1/admin/settings/{section}` adds `fields[]`.
   - For a section with no editable keys yet, it adds `editable: false` and
     `current: [{ key, value_display, source }]` from the running configuration. Secret keys read only
     "set" or "not set". Use the same redaction as the section route.
   - OpenAPI and the generated types; ITs per section, including a secret key never echoed.
3. **Dashboard.**
   - `settings-section-form.tsx` and `SettingsSectionFields` (P5-IA-4) render from `fields`:
     - labels and help;
     - `ChoiceCards` for enums, with a sentence each;
     - list editors for lists, never JSON;
     - numbers with units;
     - the **Default** badge reading "Default: <default_display>";
     - an operator-doc link by title.
   - Remove `labelOf`.
   - Read-only sections render `current` as a description list: label, value in words, a source chip
     (Helm / Default), and "Change it in your Helm values: <doc title>". No Save button.
   - Remove the spec references from the governance and LLM page text (the "Policy is set per
     organization…" notice keeps its meaning, without the doc path).
4. **UI text test (C-974).** A web test scans the message strings and the components under
   `web/src/components` and `web/src/app`, and fails on:
   - `§` followed by a digit;
   - the substring `docs/`;
   - a rendered raw enum id from a known list (`sanctioned_only`, `credentialed_only`, `owners_only`,
     `allowlist_only`, `open_lane`);
   - `[]` rendered as a value;
   - a call to a key-derived label function.

   Fix what it finds. Spec references in comments are allowed: the test reads rendered strings, not
   comments.
5. **Tests.**
   - Rust: registry completeness; ITs for `fields`, for `current` of an unregistered section, and for
     secret redaction.
   - Vitest: the form renders labels, choice cards and list editors from fixtures; read-only sections show
     no Save; the default badge text.
   - Playwright: the governance BYO choice reads in words; Directory sync (before SET-5) shows its current
     values read-only.

## Constraints / file ownership

- **Owns:**
  - `crates/aurora-config` (field registry and its test);
  - the settings section route (`fields`, `current`);
  - `settings-section-form.tsx` rendering;
  - the UI text test.
- **SET-4/5 coordination.** When they register keys, they write the `FieldMeta` rows for them, because
  the completeness test requires it. If this task lands after them, fill theirs too. Tell the Orchestrator
  which order applies.
- **Not beside P5-IA-4** (the same form component): run after it, or let P5-IA-4 leave the rendering to
  this task.

## Acceptance criteria

1. No settings field shows a key-derived label, a raw enum, a JSON value or a spec reference; the test
   enforces it.
2. Every section is either editable with words or read-only with its current values. None shows an empty
   form.
3. Defaults are shown in words.
4. Rust and web gates; axe and narrow-route suites for the settings pages, both themes.

## Dependencies

P5-IA-4 (`SettingsSectionFields`). Soft dependency on SET-4/5 (see Constraints).

## Suggested agent/model and reason

Sonnet for the registry, API and rendering. The help-text writing could be a Haiku sub-step reviewed for
accuracy against `15`.
