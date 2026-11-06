//! JSON Schemas of the channel wire types, written to `schemas/channels_*.json`.

use schemars::{Schema, schema_for};

use crate::{CapabilitySet, Claims, Content};

/// `(file stem, schema)` pairs.
pub fn all() -> Vec<(&'static str, Schema)> {
    vec![
        ("channels_content", schema_for!(Content)),
        ("channels_action_claims", schema_for!(Claims)),
        ("channels_capabilities", schema_for!(CapabilitySet)),
    ]
}
