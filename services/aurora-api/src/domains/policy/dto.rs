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
    Agent {
        id: Uuid,
    },
    User {
        id: Uuid,
    },
    /// (phase 4, P4-GRP-1, D-131) A directory group of the scope's org; matches only a speaker
    /// who is in the group.
    Group {
        id: Uuid,
    },
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
    /// (phase 2, RFE-011a) hold deadline preference in seconds (60 … `approvals.max_timeout_secs`);
    /// only on `require_approval` rules; `null` = no preference (`07 §Approval deadlines`).
    pub approval_timeout_secs: Option<i32>,
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
    /// (phase 2, RFE-011a) range-checked at authoring: 422 `approval_timeout_out_of_range`.
    #[serde(default)]
    pub approval_timeout_secs: Option<i64>,
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
    /// (RFE-003) workspace open-lane posture; always `enabled` at org scope.
    pub open_lane_posture: aurora_llm::governance::OpenLanePosture,
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

/// (FIX-164, C-1020, D-365) What a workspace member reads of the workspace's Internet access: the **published**
/// generation's defaults and rules, and nothing else: no draft, no revision, no generation history, no publisher, no
/// warnings, no other workspace's overrides.
#[derive(Debug, Serialize, ToSchema)]
pub struct PublishedInternetAccess {
    /// The published generation's number.
    pub number: i32,
    pub default_action: DefaultAction,
    pub uncredentialed_default: UncredentialedDefault,
    pub uncredentialed_allowlist: Vec<AllowlistEntry>,
    pub open_lane_posture: aurora_llm::governance::OpenLanePosture,
    pub rules: Vec<PolicyRule>,
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
    pub open_lane_posture: aurora_llm::governance::OpenLanePosture,
    pub rule_count: i64,
    pub draft_revision: Option<i64>,
    pub published_by: Option<CreatedBy>,
    pub published_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// (P5-REFINE-6, C-1087) What this generation changed against the one before it (at most
    /// 20; `more` counts the rest). Empty for the first generation.
    pub changes: Vec<RuleChange>,
    /// (C-1087) Changes left out after the first 20.
    pub more: i64,
    /// (C-1087) Who made it: the publisher; `null` for a draft or an unknown publisher.
    pub actor: Option<ChangeActor>,
}

/// (C-1087) One rule added, removed or edited.
#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
pub struct RuleChange {
    pub kind: RuleChangeKind,
    pub rule_name: String,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleChangeKind {
    Added,
    Removed,
    Edited,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ChangeActor {
    pub display_name: String,
}

/// The most changes listed per generation (C-1087).
pub const CHANGES_MAX: usize = 20;

/// (C-1087) The rule changes from `old` to `new`, matched by `logical_id`, in `new`'s order
/// (removed ones after, in `old`'s order); the first [`CHANGES_MAX`] and how many more.
#[must_use]
pub fn rule_changes(
    old: &[aurora_db::repo::policy::RuleRow],
    new: &[aurora_db::repo::policy::RuleRow],
) -> (Vec<RuleChange>, i64) {
    use aurora_db::repo::policy::RuleRow;
    fn same(a: &RuleRow, b: &RuleRow) -> bool {
        a.priority == b.priority
            && a.name == b.name
            && a.description == b.description
            && a.enabled == b.enabled
            && a.identities == b.identities
            && a.targets == b.targets
            && a.action == b.action
            && a.require_approval == b.require_approval
            && a.approval_timeout_secs == b.approval_timeout_secs
            && a.rate_limit == b.rate_limit
            && a.conditions == b.conditions
    }
    let mut all = Vec::new();
    for n in new {
        match old.iter().find(|o| o.logical_id == n.logical_id) {
            None => all.push((RuleChangeKind::Added, &n.name)),
            Some(o) if !same(o, n) => all.push((RuleChangeKind::Edited, &n.name)),
            Some(_) => {}
        }
    }
    for o in old {
        if !new.iter().any(|n| n.logical_id == o.logical_id) {
            all.push((RuleChangeKind::Removed, &o.name));
        }
    }
    let more = i64::try_from(all.len().saturating_sub(CHANGES_MAX)).unwrap_or(i64::MAX);
    let listed = all
        .into_iter()
        .take(CHANGES_MAX)
        .map(|(kind, name)| RuleChange {
            kind,
            rule_name: name.clone(),
        })
        .collect();
    (listed, more)
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
    /// (RFE-003) workspace scope only; absent = `enabled` (whole-draft replace). A
    /// workspace can only tighten, so `enabled` adds no restriction beyond org and instance.
    #[serde(default)]
    pub open_lane_posture: Option<aurora_llm::governance::OpenLanePosture>,
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
    /// (FIX-109) The draft revision the caller last read: rollback replaces the current draft,
    /// so a draft a colleague saved since is not discarded silently (409 `stale_revision`).
    pub expect_revision: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WarningCode {
    Shadowed,
    Inert,
    OverriddenByOrg,
    /// (phase 2, RFE-025) an allow rule with empty identities selects credentials for every
    /// agent, including agents created later.
    RuleAppliesToAllAgents,
    /// (phase 2, RFE-005) informational: a rule with `user` identities matches only turns
    /// spoken by that person.
    UserRuleNeedsSpeaker,
    /// (phase 2, RFE-005) an allow rule that grants credentials names only `user` identities, so
    /// speakerless traffic gets no credential from it.
    UserRuleNoAgentFallback,
    /// (phase 4, P4-GRP-1) informational: a rule with `group` identities matches only turns
    /// spoken by a member of the group.
    GroupRuleNeedsSpeaker,
    /// (phase 4, P4-GRP-1) an allow rule that grants credentials names only `group` identities.
    GroupRuleNoAgentFallback,
    /// (phase 4, P4-GRP-1) informational: a named group has no members.
    GroupRuleEmpty,
    /// (phase 4, P4-GRP-1, SEC-71) the rule names a group that no longer exists and matches
    /// nobody (`details.group_id`).
    RuleNamesRemovedGroup,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PolicyWarning {
    pub code: WarningCode,
    pub rule_logical_id: Uuid,
    pub message: String,
    /// (phase 2, RFE-025) `warning`; present on `rule_applies_to_all_agents`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    /// (phase 2, RFE-025) `{ rule_id, targets }` for `rule_applies_to_all_agents`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub details: Option<serde_json::Value>,
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
    /// (RFE-003) `request` (default) or `open`: `open` evaluates a browser tunnel under the
    /// open-lane posture and the CONNECT-time semantics (`07`); `url` is `https://host[:port]`
    /// and `method`/`body` are ignored.
    #[serde(default)]
    pub lane: Option<SimLane>,
    /// (phase 2, RFE-005) The person whose turn is simulated; omitted or null = no speaker, so
    /// `user` identities never match. Must hold `can_use` on the workspace (400 field error
    /// otherwise). Ignored for `lane: "open"` (tunnels carry no speaker).
    #[serde(default)]
    pub speaker_user_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SimLane {
    Request,
    Open,
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
    /// (RFE-003) the open-lane posture refused the tunnel (`disabled`).
    OpenLane,
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

/// (P4-011B-2, C-829) `GET /v1/policy/approval-limits`: the live effective values, no other key.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ApprovalLimits {
    /// `approvals.timeout_secs`
    pub timeout_secs: u64,
    /// `approvals.max_timeout_secs`
    pub max_timeout_secs: u64,
    pub park: ApprovalLimitsPark,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ApprovalLimitsPark {
    pub enabled: bool,
    pub threshold_secs: u64,
    pub max_timeout_secs: u64,
    pub resume_window_secs: u64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Simulation {
    /// (phase 2, RFE-011a) Only for `needs_approval`: the hold timeout in seconds the request
    /// would get (minimum of the set rule timeouts, else `approvals.timeout_secs`).
    pub approval_timeout_secs: Option<u64>,
    /// (P4-011B-2, C-829) Only for `needs_approval`: parking is on and the timeout is above its
    /// threshold, so the agent is refused at once and resumes when a person approves.
    pub approval_parks: bool,
    /// (P4-011B-2, C-829) Only for `needs_approval`: the wanted timeout was cut to the instance's
    /// current maximum.
    pub approval_timeout_clamped: bool,
    pub verdict: SimVerdict,
    pub reason: Option<SimReason>,
    pub guidance_code: Option<String>,
    pub winning_credential: Option<WinningCredential>,
    pub candidates: Vec<SimCandidate>,
    /// (FIX-159, C-930) The app the request was recognized as (host, port and path against Active
    /// host claims), and the catalogue tool when one matched. Absent for a request no app serves.
    pub app: Option<SimApp>,
    pub trace: SimTrace,
}

/// (FIX-159, C-930) What the request was recognized as.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SimApp {
    pub app_id: String,
    /// The catalogue tool the request matched; null when none did.
    pub tool_id: Option<String>,
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
        approval_timeout_secs: r.approval_timeout_secs,
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
        open_lane_posture: parse_str(&g.open_lane_posture)?,
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
        open_lane_posture: parse_str(&g.open_lane_posture)?,
        rule_count: s.rule_count,
        draft_revision: (g.status == "draft").then_some(g.draft_revision),
        actor: g
            .published_by_name
            .clone()
            .map(|display_name| ChangeActor { display_name }),
        published_by: published_by(&g),
        published_at: g.published_at,
        created_at: g.created_at,
        changes: Vec::new(),
        more: 0,
    })
}
