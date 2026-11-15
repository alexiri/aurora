//! The instance-change re-evaluation job (SET-2, `08 §11.2`, `08 §7`): after the `governance`
//! settings change, every organization's stored state is re-evaluated against its effective
//! governance (LLM secrets whose verified origin no longer complies become `mismatch`, agent
//! model overrides that are no longer allowed are cleared), the gateways drop the organization's
//! cached governance, and the instance token maximum age is applied to live agent tokens.
//!
//! One organization per transaction, in id order. The job is idempotent (a second run finds
//! nothing to change and writes nothing, not even an audit row) and resumable by rerunning: an
//! interruption loses no work and a later swap, or the next API start, runs it again. It is a
//! singleton (advisory lock) so replicas do not repeat each other's steps.

use super::service::{reevaluate_stored, retrofit_instance_pool};
use aurora_core::ApiError;
use aurora_db::repo::{credentials, governance as repo};
use aurora_db::{DbError, PgPool};
use aurora_llm::governance::{self as g, Instance};
use serde_json::json;
use uuid::Uuid;

const JOB: &str = "governance_reevaluation";
const PAGE: i64 = 100;

/// What one run changed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    pub orgs_visited: u64,
    pub orgs_changed: u64,
    pub secrets_marked: u64,
    pub agent_overrides_cleared: u64,
    pub tokens_retrofitted: u64,
}

/// Re-evaluate one organization against `inst` in its own transaction. `Ok(None)`: nothing
/// changed (nothing written).
pub async fn reevaluate_org(
    pool: &PgPool,
    inst: &Instance,
    org: Uuid,
) -> Result<Option<(usize, usize)>, ApiError> {
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    let Some((raw, revision)) = repo::lock(&mut tx, org).await? else {
        return Ok(None);
    };
    let doc = match g::from_stored(&raw, inst) {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(%org, error = %e, "malformed organization governance document; not re-evaluated");
            return Ok(None);
        }
    };
    let eff = g::effective(inst, &doc);
    let (marked, cleared) = reevaluate_stored(&mut tx, org, &eff).await?;
    if marked.is_empty() && cleared.is_empty() {
        return Ok(None);
    }
    credentials::notify_gateway_invalidate(&mut *tx, "org", org).await?;
    let ev = crate::audit::AuditEvent::new(
        crate::audit::AuditActor::system("governance-reevaluation"),
        "governance.update",
        crate::audit::Outcome::Success,
    )
    .org(org)
    .target("organization", org)
    .metadata(json!({
        "revision": revision,
        "reason": "instance_settings",
        "secrets_marked_mismatch": marked,
        "agent_overrides_cleared": cleared,
    }));
    crate::audit::commit_critical(tx, &ev).await?;
    Ok(Some((marked.len(), cleared.len())))
}

/// The whole job: every organization, then the instance token maximum age (`08 §7`). Returns
/// `None` when another replica holds the job.
pub async fn run(pool: &PgPool, inst: &Instance) -> Result<Option<Outcome>, ApiError> {
    let ran = aurora_db::jobs::try_singleton(pool, JOB, || async {
        let mut out = Outcome::default();
        let mut after: Option<Uuid> = None;
        loop {
            let ids = repo::org_ids_after(pool, after, PAGE).await?;
            let Some(last) = ids.last().copied() else {
                break;
            };
            for org in ids {
                out.orgs_visited += 1;
                match reevaluate_org(pool, inst, org).await {
                    Ok(Some((m, c))) => {
                        out.orgs_changed += 1;
                        out.secrets_marked += m as u64;
                        out.agent_overrides_cleared += c as u64;
                    }
                    Ok(None) => {}
                    // One organization failing does not stop the rest; the next run retries.
                    Err(e) => tracing::warn!(%org, error = %e.message(), "governance re-evaluation failed for an organization"),
                }
            }
            after = Some(last);
        }
        out.tokens_retrofitted =
            retrofit_instance_pool(pool, inst.agent_token_max_age_days).await?;
        Ok::<_, ApiError>(out)
    })
    .await
    .map_err(ApiError::from)?;
    ran.transpose()
}
