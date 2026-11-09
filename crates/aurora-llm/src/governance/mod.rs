//! Governance library (08 §11, D-40): org document, validation, effective view, compliance.
mod byo;
mod effective;
mod instance;
mod model;
mod validate;

pub use byo::{
    ByoInputs, ByoInstance, ByoLevel, ByoPosture, EffectiveByo, OrgByo, creation_allowed,
    effective_byo, parse_list as parse_byo_list,
};
pub use effective::{
    EffectiveEndpoint, EffectiveGovernance, EgressAllow, OpenLaneAdmission, Source, effective,
};
pub use instance::{Instance, InstanceEndpoint};
pub use model::{
    AllowEntry, BELOW_MIN, EgressPosture, GovernanceError, INVALID, LlmPosture, OpenLanePosture,
    OrgAgentTokens, OrgEgress, OrgEndpoint, OrgGovernance, OrgLlm, OrgOrigin, OriginStatus,
    StoredOrigin, effective_agent_token_max_age,
};
pub use validate::{BYO_CIDR_OUTSIDE_DEFAULT, default_for_new_org, from_stored, validate};
