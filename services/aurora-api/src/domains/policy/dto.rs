use crate::domains::secrets::dto::CreatedBy;
use aurora_db::repo::policy::{GenerationRow, GenerationSummaryRow, RuleRow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Identity {
    Agent { id: Uuid },
    User { id: Uuid },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScopeSel {
    Workspace,
    Organization,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SecretKindSel {
    Generic,
    Llm,
}

/// Rule target (I-12: tagged with `type`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    App {
        app_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tools: Option<Vec<String>>,
    },
    Connection {
        id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tools: Option<Vec<String>>,
    },
    AppConnections {
        app_id: String,
        scope: ScopeSel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tools: Option<Vec<String>>,
    },
    Secret {
        id: Uuid,
    },
    Secrets {
        scope: ScopeSel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<SecretKindSel>,
    },
    Network {
        host: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        methods: Option<Vec<String>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Per {
    Minute,
    Hour,
    Day,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RateLimit {
    pub count: u32,
    pub per: Per,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConditionOn {
    Body,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConditionOp {
    Contains,
    NotContains,
    Equals,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub on: ConditionOn,
    pub op: ConditionOp,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_pointer: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Allow,
    Block,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Block => "block",
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PolicyRule {
    pub logical_id: Uuid,
    pub priority: i32,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub identities: Vec<Identity>,
    pub targets: Vec<Target>,
    pub action: Action,
    pub require_approval: bool,
    pub rate_limit: Option<RateLimit>,
    pub conditions: Vec<Condition>,
}

/// A rule in a draft body: no `priority` (array order), optional `logical_id` (C-08).
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RuleInput {
    pub logical_id: Option<Uuid>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub identities: Vec<Identity>,
    #[serde(default)]
    pub targets: Vec<Target>,
    pub action: Action,
    #[serde(default)]
    pub require_approval: bool,
    #[serde(default)]
    pub rate_limit: Option<RateLimit>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DefaultAction {
    None,
    Allow,
    Block,
}

impl DefaultAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Allow => "allow",
            Self::Block => "block",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UncredentialedDefault {
    None,
    Block,
}

impl UncredentialedDefault {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Block => "block",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AllowlistEntry {
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GenerationStatus {
    Draft,
    Published,
    Superseded,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PolicyGeneration {
    pub id: Uuid,
    pub number: i32,
    pub status: GenerationStatus,
    pub default_action: DefaultAction,
    pub uncredentialed_default: UncredentialedDefault,
    pub uncredentialed_allowlist: Vec<AllowlistEntry>,
    pub rules: Vec<PolicyRule>,
    /// Drafts only.
    pub draft_revision: Option<i64>,
    pub published_by: Option<CreatedBy>,
    pub published_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScopeName {
    Organization,
    Workspace,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PolicyDocument {
    pub scope: ScopeName,
    pub published: PolicyGeneration,
    pub draft: PolicyGeneration,
}

/// A generation without its rules.
#[derive(Debug, Serialize, ToSchema)]
pub struct PolicyGenerationSummary {
    pub id: Uuid,
    pub number: i32,
    pub status: GenerationStatus,
    pub default_action: DefaultAction,
    pub uncredentialed_default: UncredentialedDefault,
    pub uncredentialed_allowlist: Vec<AllowlistEntry>,
    pub rule_count: i64,
    pub draft_revision: Option<i64>,
    pub published_by: Option<CreatedBy>,
    pub published_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftRequest {
    pub revision: i64,
    pub default_action: DefaultAction,
    #[serde(default)]
    pub uncredentialed_default: Option<UncredentialedDefault>,
    #[serde(default)]
    pub uncredentialed_allowlist: Option<Vec<AllowlistEntry>>,
    pub rules: Vec<RuleInput>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PublishRequest {
    pub revision: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RollbackRequest {
    /// Generation number to copy.
    pub generation: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WarningCode {
    Shadowed,
    Inert,
    OverriddenByOrg,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PolicyWarning {
    pub code: WarningCode,
    pub rule_logical_id: Uuid,
    pub message: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PolicyWarnings {
    pub items: Vec<PolicyWarning>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SimulateRequest {
    pub agent_id: Uuid,
    pub method: String,
    pub url: String,
    /// Request body (UTF-8 text) for body conditions.
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub use_draft: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SimVerdict {
    Allow,
    NeedsApproval,
    RateLimited,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SimReason {
    Rule,
    DefaultDenied,
    EgressDenied,
    Governance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    Secret,
    Connection,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WinningCredential {
    pub kind: CredentialKind,
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SimCandidate {
    pub kind: CredentialKind,
    pub id: Uuid,
    pub name: String,
    pub selected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TraceSource {
    Hand,
    Derived,
    Default,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TraceEntry {
    pub rule_logical_id: Uuid,
    pub rule_name: String,
    pub source: TraceSource,
    pub matched: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ScopeTrace {
    pub verdict: Option<SimVerdict>,
    pub considered: Vec<TraceEntry>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SimTrace {
    pub organization: ScopeTrace,
    pub workspace: ScopeTrace,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Simulation {
    pub verdict: SimVerdict,
    pub reason: Option<SimReason>,
    pub guidance_code: Option<String>,
    pub winning_credential: Option<WinningCredential>,
    pub candidates: Vec<SimCandidate>,
    pub trace: SimTrace,
}

// ---- row conversions ----

fn parse<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, aurora_core::ApiError> {
    serde_json::from_value(v).map_err(|e| {
        tracing::error!(error = %e, "stored policy document is malformed");
        aurora_core::ApiError::internal("Internal error.")
    })
}

fn parse_str<T: serde::de::DeserializeOwned>(s: &str) -> Result<T, aurora_core::ApiError> {
    parse(Value::String(s.to_owned()))
}

pub fn rule_dto(r: RuleRow) -> Result<PolicyRule, aurora_core::ApiError> {
    Ok(PolicyRule {
        logical_id: r.logical_id,
        priority: r.priority,
        name: r.name,
        description: r.description,
        enabled: r.enabled,
        identities: parse(r.identities)?,
        targets: parse(r.targets)?,
        action: parse_str(&r.action)?,
        require_approval: r.require_approval,
        rate_limit: r.rate_limit.map(parse).transpose()?,
        conditions: parse(r.conditions)?,
    })
}

fn published_by(g: &GenerationRow) -> Option<CreatedBy> {
    g.published_by.map(|id| CreatedBy {
        id,
        display_name: g.published_by_name.clone().unwrap_or_default(),
    })
}

pub fn generation_dto(
    g: GenerationRow,
    rules: Vec<RuleRow>,
) -> Result<PolicyGeneration, aurora_core::ApiError> {
    Ok(PolicyGeneration {
        id: g.id,
        number: g.number,
        status: parse_str(&g.status)?,
        default_action: parse_str(&g.default_action)?,
        uncredentialed_default: parse_str(&g.uncredentialed_default)?,
        uncredentialed_allowlist: parse(g.uncredentialed_allowlist.clone())?,
        rules: rules.into_iter().map(rule_dto).collect::<Result<_, _>>()?,
        draft_revision: (g.status == "draft").then_some(g.draft_revision),
        published_by: published_by(&g),
        published_at: g.published_at,
        created_at: g.created_at,
    })
}

pub fn summary_dto(
    s: GenerationSummaryRow,
) -> Result<PolicyGenerationSummary, aurora_core::ApiError> {
    let g = s.generation;
    Ok(PolicyGenerationSummary {
        id: g.id,
        number: g.number,
        status: parse_str(&g.status)?,
        default_action: parse_str(&g.default_action)?,
        uncredentialed_default: parse_str(&g.uncredentialed_default)?,
        uncredentialed_allowlist: parse(g.uncredentialed_allowlist.clone())?,
        rule_count: s.rule_count,
        draft_revision: (g.status == "draft").then_some(g.draft_revision),
        published_by: published_by(&g),
        published_at: g.published_at,
        created_at: g.created_at,
    })
}
