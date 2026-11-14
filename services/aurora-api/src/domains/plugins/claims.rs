//! Host-claim conflicts and admin decisions (09 §7, C-17).

use super::dto::{HostClaimConflict, HostClaimDecision, HostClaimMember, PluginUserRef};
use super::registry::{self, CompileFail};
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use aurora_core::{ApiError, FieldError, ids};
use aurora_db::DbError;
use aurora_db::repo::plugins::{self, DecisionRow, PackageRow};
use aurora_registry::Manifest;
use aurora_registry::claims::{self, Claim, ConflictGroup};
use serde_json::json;
use sqlx::PgConnection;
use std::collections::BTreeMap;
use uuid::Uuid;

/// One manifest per package: the newest pending version if any (what approval
/// would activate), else the active one.
fn effective(rows: Vec<PackageRow>) -> Vec<PackageRow> {
    let mut by: BTreeMap<String, PackageRow> = BTreeMap::new();
    for r in rows {
        match by.get(&r.package_id) {
            None => {
                by.insert(r.package_id.clone(), r);
            }
            Some(cur) if cur.status == "active" && r.status == "pending_approval" => {
                by.insert(r.package_id.clone(), r);
            }
            _ => {}
        }
    }
    by.into_values().collect()
}

async fn groups(conn: &mut PgConnection) -> Result<Vec<ConflictGroup>, ApiError> {
    let rows = plugins::list_by_status(&mut *conn, &["active", "pending_approval"]).await?;
    let mut all: Vec<(String, Claim)> = Vec::new();
    let rows = effective(rows);
    // (FIX-123, D-215) A package whose apps a present package supersedes is on its way out: its
    // claims are not a conflict for the admin to decide (approval moves the apps and retires it).
    let retiring: Vec<&'static str> = rows
        .iter()
        .filter_map(|r| serde_json::from_value::<Manifest>(r.manifest.clone()).ok())
        .filter(|m| !aurora_registry::upgrade::supersedes(m).is_empty())
        .flat_map(|m| aurora_registry::upgrade::retired_by(&m.package.id))
        .collect();
    for r in rows
        .into_iter()
        .filter(|r| !retiring.contains(&r.package_id.as_str()))
    {
        if let Ok(m) = serde_json::from_value::<Manifest>(r.manifest) {
            all.extend(
                claims::claims(&m)
                    .into_iter()
                    .map(|c| (r.package_id.clone(), c)),
            );
        }
    }
    Ok(claims::conflict_groups(&all))
}

fn view(g: &ConflictGroup, decisions: &[DecisionRow]) -> HostClaimConflict {
    HostClaimConflict {
        group_key: g.claim_key.clone(),
        fingerprint: g.fingerprint.clone(),
        claims: g
            .members
            .iter()
            .map(|m| HostClaimMember {
                package_id: m.package_id.clone(),
                app_id: m.app_id.clone(),
                claim_key: m.claim_key.clone(),
            })
            .collect(),
        decision: decisions
            .iter()
            .find(|d| d.claim_key == g.claim_key)
            .map(|d| HostClaimDecision {
                winner_package_id: d.winner_package_id.clone(),
                winner_app_id: d.winner_app_id.clone(),
                decided_by: d.decided_by.map(|id| PluginUserRef {
                    id,
                    display_name: d.decided_by_name.clone().unwrap_or_default(),
                }),
                stale: d.claim_fingerprint != g.fingerprint,
            }),
    }
}

pub async fn list(state: &AppState) -> Result<Vec<HostClaimConflict>, ApiError> {
    let mut conn = state.pool.acquire().await.map_err(DbError::from)?;
    let gs = groups(&mut conn).await?;
    let ds = plugins::decisions(&mut *conn).await?;
    Ok(gs.iter().map(|g| view(g, &ds)).collect())
}

pub async fn decide(
    state: &AppState,
    actor: &AuditActor,
    admin: Uuid,
    group_key: &str,
    winner_package_id: &str,
    winner_app_id: &str,
    fingerprint: &str,
) -> Result<HostClaimConflict, ApiError> {
    let mut tx = audit::begin(state).await?;
    plugins::lock_compile(&mut tx).await?;
    let gs = groups(&mut tx).await?;
    let g = gs
        .iter()
        .find(|g| g.claim_key == group_key)
        .ok_or_else(|| ApiError::not_found("Host claim conflict not found."))?;
    if g.fingerprint != fingerprint {
        return Err(ApiError::conflict(
            "stale_fingerprint",
            "The conflict changed since it was loaded; reload and decide again.",
        ));
    }
    if !g
        .members
        .iter()
        .any(|m| m.package_id == winner_package_id && m.app_id == winner_app_id)
    {
        return Err(ApiError::invalid_request(
            "Winner is not part of the conflict.",
            vec![FieldError::new(
                "/winner_package_id",
                "must be a package/app taking part in the conflict",
            )],
        ));
    }
    let previous = plugins::decisions(&mut *tx)
        .await?
        .into_iter()
        .find(|d| d.claim_key == group_key)
        .map(|d| d.winner_package_id);
    plugins::upsert_decision(
        &mut tx,
        ids::new_id(),
        group_key,
        fingerprint,
        winner_package_id,
        winner_app_id,
        admin,
    )
    .await?;
    registry::activate_snapshot(state, &mut tx, true)
        .await
        .map_err(|f| match f {
            CompileFail::Api(e) => e,
            CompileFail::Conflict(_) => ApiError::internal("Unresolved host claim conflict."),
        })?;
    let ds = plugins::decisions(&mut *tx).await?;
    let out = view(g, &ds);
    let ev = AuditEvent::new(actor.clone(), "plugin.host_claim.decide", Outcome::Success)
        .target("host_claim", group_key)
        .metadata(
            json!({"group_key": group_key, "winner_package_id": winner_package_id,
            "winner_app_id": winner_app_id, "previous_winner": previous}),
        );
    audit::commit_critical(tx, &ev).await?;
    Ok(out)
}
