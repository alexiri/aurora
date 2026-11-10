//! aurora-policy — the single implementation of policy matching and
//! evaluation (docs/07-policy-approvals.md). Pure: no I/O, no database.
#![forbid(unsafe_code)]

pub mod eval;
pub mod facts;
pub mod git;
pub mod graphql;
pub mod host;
pub mod methods;
pub mod params;
pub mod path;
pub mod rule;
pub mod select;

pub use eval::{
    BlockReason, Decision, MemoryCounter, RateCounter, RateLimitHit, ScopePolicy, Verdict,
    connect_allow_admits, evaluate,
};
pub use facts::{Credential, CredentialKey, RequestFacts, RequestIdentity};
pub use git::GitOp;
pub use graphql::GraphqlKind;
pub use host::HostPattern;
pub use params::{ParamCondition, ParamInput, ParamMatch, ParamSource};
pub use path::PathPattern;
pub use rule::Rule;
pub use select::selected_candidates;
