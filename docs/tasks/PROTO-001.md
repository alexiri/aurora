# PROTO-001: `aurora-protocol` wire types for W5 (runner plane), W6 (supervisor channel), W7 (platform tools)

## Goal
Every message of the runner plane, the supervisor WebSocket channel and platform tool calls as typed, schema-generated, bounded serde types — the contract that M4's runner, supervisor and API due-work engine build against in parallel.

## Linked requirements
- M4/16 `§Cross-cutting conventions` (receiver validation, versioning `runner/1` `supervisor/1`, open vocabularies, sender-side bounds, idempotency ids), `§W5 Runner plane` (work items, reports), `§W6 Supervisor channel`, `§W7 Platform tool calls`; `02 §Coding rules` (wire types derive Serialize/Deserialize/JsonSchema, `deny_unknown_fields` only for closed vocabularies, open vocab as `Known + Other(BoundedString)`), `02 §Limits registry` (`WS_FRAME_MAX`, `HOME_SYNC_PART_MAX`, etc.).

## Context
- Read `docs/16-wire-protocols.md` §Cross-cutting conventions, §W5, §W6, §W7 (normative), and only the referenced limits in `02`. Where `16` refers to `11`/`12` for a field's meaning, read just that paragraph.
- Existing crate (BE-006): `version` (`Plane`, `ProtocolVersion`), `vocab` (`open_vocab!`, `BoundedString`), `schema::all()` registry + drift test writing `schemas/*.json` (`UPDATE_SCHEMAS=1`).

## Scope (`crates/aurora-protocol/src/{runner,supervisor,tools}.rs`, `schemas/`, tests)
- All W5/W6/W7 message types (requests, responses, work-item and report enums with `kind` tags as in `16`), with validation helpers enforcing sender-side bounds (`validate()` returning typed errors) and `MAX_*` consts reusing `aurora_core::limits`.
- Register every top-level message in `schema::all()`; regenerate `schemas/`.
- Round-trip tests from the JSON examples in `16` (each example parses and re-serializes equal); unknown `kind` in open vocabularies degrades to `Other`; oversize fields rejected by `validate()`.

### Out of scope
- Any transport, runner/supervisor/API logic.

## Acceptance
- [ ] `cargo test -p aurora-protocol` (incl. schema drift), clippy, fmt.
- [ ] Report lists every ambiguity/contradiction found in `16` W5–W7 (these are spec bugs to resolve before M4 implementation).

## Dependencies: BE-006 (done). Unblocks: M4 runner, supervisor, due-work engine. Agent: rust-developer (Sonnet).
