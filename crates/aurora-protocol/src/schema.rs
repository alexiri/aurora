//! Registry of every protocol JSON Schema. Each entry is written to
//! `schemas/<name>.json` at the repo root; `tests/schemas.rs` fails on drift.

use schemars::Schema;

use crate::version::ProtocolVersion;
use crate::vocab::ExampleKind;

/// All registered schemas as `(file stem, schema)` pairs.
pub fn all() -> Vec<(&'static str, Schema)> {
    vec![
        ("protocol_version", schemars::schema_for!(ProtocolVersion)),
        ("example_vocab", schemars::schema_for!(ExampleKind)),
    ]
}
