//! Governance library (08 §11, D-40): org document, validation, effective view, compliance.
mod effective;
mod instance;
mod model;
mod validate;

pub use effective::{
    EffectiveEndpoint, EffectiveGovernance, EgressAllow, OpenLaneAdmission, Source, effective,
};
pub use instance::{Instance, InstanceEndpoint};
pub use model::{
    AllowEntry, BELOW_MIN, EgressPosture, GovernanceError, INVALID, LlmPosture, OpenLanePosture,
    OrgEgress, OrgEndpoint, OrgGovernance, OrgLlm, OrgOrigin, OriginStatus, StoredOrigin,
};
pub use validate::{default_for_new_org, from_stored, validate};
