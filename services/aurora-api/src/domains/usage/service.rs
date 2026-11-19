//! The one aggregation behind the workspace, organization and instance usage routes.
use aurora_core::ApiError;
use aurora_core::limits::{USAGE_RANGE_MAX_DAYS, USAGE_TOP_MAX};
use aurora_db::DbError;
use aurora_db::repo::usage::{self, Counters, Dim, Group, ModelGroup, Scope};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Serialize;
use serde_json::json;
use sqlx::PgConnection;
use std::collections::HashMap;
use utoipa::ToSchema;
use uuid::Uuid;

/// The seven counters of C-1000 (`UsageCounters`).
#[derive(Debug, Clone, Copy, Default, Serialize, ToSchema)]
pub struct UsageCounters {
    pub turns: i64,
    /// Seconds turns ran (start to end, floored per turn).
    pub turn_seconds: i64,
    /// Seconds the agent's sandbox was awake (from the runtime rollup).
    pub awake_seconds: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
}

impl From<Counters> for UsageCounters {
    fn from(c: Counters) -> Self {
        Self {
            turns: c.turns,
            turn_seconds: c.turn_seconds,
            awake_seconds: c.awake_seconds,
            input_tokens: c.input_tokens,
            output_tokens: c.output_tokens,
            cache_read_tokens: c.cache_read_tokens,
            cache_write_tokens: c.cache_write_tokens,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct UsageDay {
    pub day: NaiveDate,
    #[serde(flatten)]
    pub counters: UsageCounters,
}

#[derive(Serialize, ToSchema)]
pub struct UsageAgent {
    pub agent_id: Uuid,
    /// `null` once the agent is deleted (the ledger keeps its id).
    pub agent_name: Option<String>,
    #[serde(flatten)]
    pub counters: UsageCounters,
}

/// Tokens and turns per model; no awake time. `turns` and `turn_seconds` count under the turn's default
/// model, so a model used only by subagents shows tokens and zero turns.
#[derive(Serialize, ToSchema)]
pub struct UsageModel {
    pub provider: Option<String>,
    pub endpoint_id: Option<String>,
    pub model: Option<String>,
    pub turns: i64,
    pub turn_seconds: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
}

#[derive(Serialize, ToSchema)]
pub struct UsageOrg {
    pub org_id: Uuid,
    /// `null` once the organization is deleted.
    pub org_name: Option<String>,
    #[serde(flatten)]
    pub counters: UsageCounters,
}

/// A workspace row of the instance view.
#[derive(Serialize, ToSchema)]
pub struct UsageInstanceWorkspace {
    pub workspace_id: Uuid,
    /// `null` once the workspace is deleted.
    pub workspace_name: Option<String>,
    pub org_id: Uuid,
    pub org_name: Option<String>,
    #[serde(flatten)]
    pub counters: UsageCounters,
}

/// A workspace row of the organization view.
#[derive(Serialize, ToSchema)]
pub struct UsageOrgWorkspace {
    pub workspace_id: Uuid,
    /// `null` once the workspace is deleted.
    pub workspace_name: Option<String>,
    #[serde(flatten)]
    pub counters: UsageCounters,
}

/// `GET /v1/workspaces/{ws}/usage` (C-1000).
#[derive(Serialize, ToSchema)]
pub struct WorkspaceUsage {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub totals: UsageCounters,
    pub by_day: Vec<UsageDay>,
    pub by_agent: Vec<UsageAgent>,
    pub by_model: Vec<UsageModel>,
    /// The last runtime rollup; awake time after it is not counted yet.
    pub runtime_as_of: Option<DateTime<Utc>>,
}

/// `GET /v1/admin/usage` (C-1003).
#[derive(Serialize, ToSchema)]
pub struct InstanceUsage {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub totals: UsageCounters,
    pub by_day: Vec<UsageDay>,
    pub by_org: Vec<UsageOrg>,
    /// The top 50 by input + output tokens.
    pub by_workspace: Vec<UsageInstanceWorkspace>,
    /// The sum of the workspaces below the top; `null` when there are none.
    pub other_workspaces: Option<UsageCounters>,
    pub by_model: Vec<UsageModel>,
    pub runtime_as_of: Option<DateTime<Utc>>,
}

/// `GET /v1/orgs/{org}/usage` (C-1006).
#[derive(Serialize, ToSchema)]
pub struct OrgUsage {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub totals: UsageCounters,
    pub by_day: Vec<UsageDay>,
    /// The top 50 by input + output tokens.
    pub by_workspace: Vec<UsageOrgWorkspace>,
    /// The sum of the workspaces below the top; `null` when there are none.
    pub other_workspaces: Option<UsageCounters>,
    pub by_model: Vec<UsageModel>,
    pub runtime_as_of: Option<DateTime<Utc>>,
}

fn bad_date(field: &str) -> ApiError {
    ApiError::invalid_field(
        "Invalid usage query.",
        format!("/{field}"),
        format!("{field} must be a date, YYYY-MM-DD"),
    )
}

fn range_error(reason: &str, message: &str) -> ApiError {
    ApiError::unprocessable("usage_range_invalid", message)
        .with_details(json!({ "reason": reason }))
}

/// (C-1000) The query range: UTC dates, both inclusive; `to` defaults to today and `from` to `to` − 29
/// days. 400 for a malformed date, 422 `usage_range_invalid` for `from_after_to` or `too_long`.
pub fn parse_range(
    from: Option<&str>,
    to: Option<&str>,
    today: NaiveDate,
) -> Result<(NaiveDate, NaiveDate), ApiError> {
    let parse = |v: &str, field: &str| {
        NaiveDate::parse_from_str(v.trim(), "%Y-%m-%d").map_err(|_| bad_date(field))
    };
    let to = match to {
        Some(v) => parse(v, "to")?,
        None => today,
    };
    let from = match from {
        Some(v) => parse(v, "from")?,
        None => to - Duration::days(29),
    };
    if from > to {
        return Err(range_error("from_after_to", "`from` is after `to`."));
    }
    if (to - from).num_days() + 1 > USAGE_RANGE_MAX_DAYS {
        return Err(range_error(
            "too_long",
            "The range is longer than 366 days.",
        ));
    }
    Ok((from, to))
}

fn db<T>(r: Result<T, DbError>) -> Result<T, ApiError> {
    r.map_err(ApiError::from)
}

/// Totals and one entry per day of the range, zero days included.
async fn totals_by_day(
    conn: &mut PgConnection,
    scope: Scope,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<(UsageCounters, Vec<UsageDay>), ApiError> {
    let rows = db(usage::grouped(conn, scope, from, to, Dim::Day).await)?;
    let mut by: HashMap<NaiveDate, Counters> = HashMap::new();
    let mut total = Counters::default();
    for g in rows {
        if let Some(d) = g.day {
            total.add(&g.counters);
            by.insert(d, g.counters);
        }
    }
    let mut days = Vec::new();
    let mut d = from;
    while d <= to {
        days.push(UsageDay {
            day: d,
            counters: by.remove(&d).unwrap_or_default().into(),
        });
        d += Duration::days(1);
    }
    Ok((total.into(), days))
}

async fn models(
    conn: &mut PgConnection,
    scope: Scope,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<UsageModel>, ApiError> {
    let mut rows: Vec<ModelGroup> = db(usage::by_model(conn, scope, from, to).await)?;
    rows.retain(|m| {
        m.turns
            + m.turn_seconds
            + m.input_tokens
            + m.output_tokens
            + m.cache_read_tokens
            + m.cache_write_tokens
            > 0
    });
    rows.sort_by(|a, b| {
        (b.input_tokens + b.output_tokens)
            .cmp(&(a.input_tokens + a.output_tokens))
            .then_with(|| a.provider.cmp(&b.provider))
            .then_with(|| a.model.cmp(&b.model))
            .then_with(|| a.endpoint_id.cmp(&b.endpoint_id))
    });
    Ok(rows
        .into_iter()
        .map(|m| UsageModel {
            provider: m.provider,
            endpoint_id: m.endpoint_id,
            model: m.model,
            turns: m.turns,
            turn_seconds: m.turn_seconds,
            input_tokens: m.input_tokens,
            output_tokens: m.output_tokens,
            cache_read_tokens: m.cache_read_tokens,
            cache_write_tokens: m.cache_write_tokens,
        })
        .collect())
}

/// Non-zero groups, biggest token users first, ties by `key`.
fn ranked(mut rows: Vec<Group>, key: impl Fn(&Group) -> Uuid) -> Vec<Group> {
    rows.retain(|g| !g.counters.is_zero());
    rows.sort_by(|a, b| {
        b.counters
            .tokens()
            .cmp(&a.counters.tokens())
            .then_with(|| key(a).cmp(&key(b)))
    });
    rows
}

fn names(v: Vec<(Uuid, String)>) -> HashMap<Uuid, String> {
    v.into_iter().collect()
}

/// (C-1000) One workspace, optionally one agent.
pub async fn workspace(
    conn: &mut PgConnection,
    ws: Uuid,
    agent: Option<Uuid>,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<WorkspaceUsage, ApiError> {
    let scope = Scope {
        org: None,
        workspace: Some(ws),
        agent,
    };
    let (totals, by_day) = totals_by_day(conn, scope, from, to).await?;
    let groups = ranked(
        db(usage::grouped(conn, scope, from, to, Dim::Agent).await)?,
        |g| g.agent_id.unwrap_or_default(),
    );
    let ids: Vec<Uuid> = groups.iter().filter_map(|g| g.agent_id).collect();
    let mut name = names(db(usage::agent_names(conn, &ids).await)?);
    let by_agent = groups
        .into_iter()
        .filter_map(|g| {
            let id = g.agent_id?;
            Some(UsageAgent {
                agent_id: id,
                agent_name: name.remove(&id),
                counters: g.counters.into(),
            })
        })
        .collect();
    Ok(WorkspaceUsage {
        from,
        to,
        totals,
        by_day,
        by_agent,
        by_model: models(conn, scope, from, to).await?,
        runtime_as_of: db(usage::runtime_as_of(conn).await)?,
    })
}

/// The top `USAGE_TOP_MAX` workspaces and the sum of the rest.
async fn workspaces(
    conn: &mut PgConnection,
    scope: Scope,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<(Vec<Group>, Option<UsageCounters>), ApiError> {
    let mut groups = ranked(
        db(usage::grouped(conn, scope, from, to, Dim::Workspace).await)?,
        |g| g.workspace_id.unwrap_or_default(),
    );
    let rest = groups.split_off(groups.len().min(USAGE_TOP_MAX));
    let other = (!rest.is_empty()).then(|| {
        let mut sum = Counters::default();
        for g in &rest {
            sum.add(&g.counters);
        }
        sum.into()
    });
    Ok((groups, other))
}

/// (C-1006) One organization, optionally one workspace of it.
pub async fn org(
    conn: &mut PgConnection,
    org: Uuid,
    workspace_id: Option<Uuid>,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<OrgUsage, ApiError> {
    let scope = Scope {
        org: Some(org),
        workspace: workspace_id,
        agent: None,
    };
    let (totals, by_day) = totals_by_day(conn, scope, from, to).await?;
    let (top, other_workspaces) = workspaces(conn, scope, from, to).await?;
    let ids: Vec<Uuid> = top.iter().filter_map(|g| g.workspace_id).collect();
    let mut name = names(db(usage::workspace_names(conn, &ids).await)?);
    let by_workspace = top
        .into_iter()
        .filter_map(|g| {
            let id = g.workspace_id?;
            Some(UsageOrgWorkspace {
                workspace_id: id,
                workspace_name: name.remove(&id),
                counters: g.counters.into(),
            })
        })
        .collect();
    Ok(OrgUsage {
        from,
        to,
        totals,
        by_day,
        by_workspace,
        other_workspaces,
        by_model: models(conn, scope, from, to).await?,
        runtime_as_of: db(usage::runtime_as_of(conn).await)?,
    })
}

/// (C-1003) The whole instance, optionally one organization.
pub async fn instance(
    conn: &mut PgConnection,
    org_id: Option<Uuid>,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<InstanceUsage, ApiError> {
    let scope = Scope {
        org: org_id,
        workspace: None,
        agent: None,
    };
    let (totals, by_day) = totals_by_day(conn, scope, from, to).await?;
    let orgs = ranked(
        db(usage::grouped(conn, scope, from, to, Dim::Org).await)?,
        |g| g.org_id.unwrap_or_default(),
    );
    let (top, other_workspaces) = workspaces(conn, scope, from, to).await?;
    let mut org_ids: Vec<Uuid> = orgs.iter().filter_map(|g| g.org_id).collect();
    org_ids.extend(top.iter().filter_map(|g| g.org_id));
    let org_name = names(db(usage::org_names(conn, &org_ids).await)?);
    let ws_ids: Vec<Uuid> = top.iter().filter_map(|g| g.workspace_id).collect();
    let ws_name = names(db(usage::workspace_names(conn, &ws_ids).await)?);
    let by_org = orgs
        .into_iter()
        .filter_map(|g| {
            let id = g.org_id?;
            Some(UsageOrg {
                org_id: id,
                org_name: org_name.get(&id).cloned(),
                counters: g.counters.into(),
            })
        })
        .collect();
    let by_workspace = top
        .into_iter()
        .filter_map(|g| {
            let (w, o) = (g.workspace_id?, g.org_id?);
            Some(UsageInstanceWorkspace {
                workspace_id: w,
                workspace_name: ws_name.get(&w).cloned(),
                org_id: o,
                org_name: org_name.get(&o).cloned(),
                counters: g.counters.into(),
            })
        })
        .collect();
    Ok(InstanceUsage {
        from,
        to,
        totals,
        by_day,
        by_org,
        by_workspace,
        other_workspaces,
        by_model: models(conn, scope, from, to).await?,
        runtime_as_of: db(usage::runtime_as_of(conn).await)?,
    })
}
