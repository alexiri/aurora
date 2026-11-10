//! Per-user onboarding progress (P4-ONB-2, D-141): welcome seen, dismissed guides, tour done.

use crate::DbError;
use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OnboardingRow {
    pub welcome_seen: bool,
    pub guide_dismissed: Vec<Uuid>,
    pub tour_completed_at: Option<DateTime<Utc>>,
}

pub async fn get<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
) -> Result<Option<OnboardingRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT welcome_seen, guide_dismissed, tour_completed_at
         FROM user_onboarding WHERE user_id = $1",
    )
    .bind(user)
    .fetch_optional(exec)
    .await?)
}

/// Counts the instance setup wizard reports (`GET /v1/admin/setup`, C-533). Numbers only.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SetupCounts {
    /// Gateways seen within the threshold and not stopped.
    pub gateways_live: i64,
    /// Active org-level LLM keys of the default organization (0 without one).
    pub org_llm_keys: i64,
    /// Instance-wide OAuth clients (any state).
    pub instance_oauth_clients: i64,
    /// Managed connectors added from the dashboard.
    pub managed_connectors: i64,
    /// Plugin versions waiting for an instance admin's decision.
    pub plugins_pending: i64,
}

pub async fn setup_counts<'e, E: PgExecutor<'e>>(
    exec: E,
    default_org: Option<Uuid>,
    gateway_online_secs: f64,
) -> Result<SetupCounts, DbError> {
    Ok(sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM gateway_instances
              WHERE stopped_at IS NULL
                AND last_seen_at >= now() - make_interval(secs => $2)) AS gateways_live,
            (SELECT count(*) FROM secrets
              WHERE $1::uuid IS NOT NULL AND org_id = $1 AND workspace_id IS NULL
                AND kind = 'llm' AND status = 'active') AS org_llm_keys,
            (SELECT count(*) FROM app_oauth_clients
              WHERE org_id IS NULL AND workspace_id IS NULL) AS instance_oauth_clients,
            (SELECT count(*) FROM connectors WHERE managed) AS managed_connectors,
            (SELECT count(*) FROM plugin_packages
              WHERE status = 'pending_approval') AS plugins_pending",
    )
    .bind(default_org)
    .bind(gateway_online_secs)
    .fetch_one(exec)
    .await?)
}

/// What to change. `None` leaves a field alone; `tour_completed_at` is `Some(None)` to clear.
#[derive(Debug, Default, Clone)]
pub struct Patch {
    pub welcome_seen: Option<bool>,
    pub guide_dismissed: Option<Vec<Uuid>>,
    pub tour_completed_at: Option<Option<DateTime<Utc>>>,
}

/// Upsert the row, applying only the fields in `patch`; returns the stored row.
pub async fn apply<'e, E: PgExecutor<'e>>(
    exec: E,
    user: Uuid,
    patch: &Patch,
) -> Result<OnboardingRow, DbError> {
    let (set_tour, tour) = match patch.tour_completed_at {
        Some(v) => (true, v),
        None => (false, None),
    };
    Ok(sqlx::query_as(
        "INSERT INTO user_onboarding (user_id, welcome_seen, guide_dismissed, tour_completed_at)
         VALUES ($1, COALESCE($2, false), COALESCE($3, '{}'), $5)
         ON CONFLICT (user_id) DO UPDATE SET
            welcome_seen = COALESCE($2, user_onboarding.welcome_seen),
            guide_dismissed = COALESCE($3, user_onboarding.guide_dismissed),
            tour_completed_at = CASE WHEN $4 THEN $5 ELSE user_onboarding.tour_completed_at END,
            updated_at = now()
         RETURNING welcome_seen, guide_dismissed, tour_completed_at",
    )
    .bind(user)
    .bind(patch.welcome_seen)
    .bind(patch.guide_dismissed.as_deref())
    .bind(set_tour)
    .bind(tour)
    .fetch_one(exec)
    .await?)
}
