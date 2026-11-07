//! Policy documents: read, draft replace, publish, rollback, generations.
use super::dto::*;
use super::validate::{self, Scope};
use crate::app::AppState;
use aurora_core::ApiError;
use aurora_db::DbError;
use aurora_db::repo::credentials;
use aurora_db::repo::policy::{self as repo, GenerationRow};
use uuid::Uuid;

fn stale() -> ApiError {
    ApiError::conflict(
        "stale_revision",
        "The draft changed since you loaded it. Reload and try again.",
    )
}

/// `(scope, id)` for `aurora_gateway_invalidate`.
pub fn invalidation(s: Scope) -> (&'static str, Uuid) {
    match s.ws {
        Some(w) => ("workspace", w),
        None => ("org", s.org),
    }
}

async fn generation(
    state: &AppState,
    s: Scope,
    status: &str,
) -> Result<PolicyGeneration, ApiError> {
    let g = repo::generation_by_status(&state.pool, s.org, s.ws, status)
        .await?
        .ok_or_else(|| ApiError::internal("Internal error."))?;
    let rules = repo::rules_of(&state.pool, g.id).await?;
    generation_dto(g, rules)
}

async fn ensure(state: &AppState, s: Scope) -> Result<(), ApiError> {
    let mut c = state.pool.acquire().await.map_err(DbError::from)?;
    repo::ensure_scope(&mut c, s.org, s.ws).await?;
    Ok(())
}

pub async fn document(state: &AppState, s: Scope) -> Result<PolicyDocument, ApiError> {
    ensure(state, s).await?;
    Ok(PolicyDocument {
        scope: s.name(),
        published: generation(state, s, "published").await?,
        draft: generation(state, s, "draft").await?,
    })
}

async fn locked_draft(tx: &mut sqlx::PgConnection, s: Scope) -> Result<GenerationRow, ApiError> {
    repo::ensure_scope(tx, s.org, s.ws).await?;
    repo::lock_draft(tx, s.org, s.ws)
        .await?
        .ok_or_else(|| ApiError::internal("Internal error."))
}

pub async fn replace_draft(
    state: &AppState,
    s: Scope,
    req: DraftRequest,
) -> Result<PolicyGeneration, ApiError> {
    let revision = req.revision;
    // Cheap pre-check so stale clients get 409 before 422 noise; re-checked under lock.
    ensure(state, s).await?;
    if let Some(d) = repo::generation_by_status(&state.pool, s.org, s.ws, "draft").await?
        && d.draft_revision != revision
    {
        return Err(stale());
    }
    let checked = validate::check(state, req, s).await?;
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let draft = locked_draft(&mut tx, s).await?;
    if draft.draft_revision != revision {
        return Err(stale());
    }
    repo::replace_draft(
        &mut tx,
        draft.id,
        checked.default_action,
        checked.uncredentialed_default,
        &checked.allowlist,
        &checked.rules,
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    generation(state, s, "draft").await
}

/// Publish the draft (`expect_revision`) or, for rollback, a copy of generation `number`.
pub enum Source {
    Draft { revision: i64 },
    Generation { number: i32 },
}

/// Returns the number of the generation that became published.
pub async fn publish(
    state: &AppState,
    s: Scope,
    by: Uuid,
    source: Source,
) -> Result<i32, ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    let draft = locked_draft(&mut tx, s).await?;
    match source {
        Source::Draft { revision } => {
            if draft.draft_revision != revision {
                return Err(stale());
            }
        }
        Source::Generation { number } => {
            let target = repo::generation_by_number(&mut *tx, s.org, s.ws, number)
                .await?
                .filter(|g| g.status != "draft")
                .ok_or_else(|| ApiError::not_found("Policy generation not found."))?;
            repo::copy_content(&mut tx, target.id, draft.id).await?;
        }
    }
    repo::promote_draft(&mut tx, &draft, by).await?;
    let (scope, id) = invalidation(s);
    credentials::notify_gateway_invalidate(&mut *tx, scope, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(draft.number)
}

pub async fn generations(
    state: &AppState,
    s: Scope,
    p: crate::http::pagination::Pagination,
) -> Result<crate::http::pagination::Page<PolicyGenerationSummary>, ApiError> {
    ensure(state, s).await?;
    let rows = repo::list_generations(
        &state.pool,
        s.org,
        s.ws,
        p.after.map(|c| (c.t, c.id)),
        p.fetch(),
    )
    .await?;
    let page = crate::http::pagination::Page::from_rows(rows, p.limit, |r| {
        (r.generation.created_at, r.generation.id)
    });
    Ok(crate::http::pagination::Page {
        items: page
            .items
            .into_iter()
            .map(summary_dto)
            .collect::<Result<_, _>>()?,
        next_cursor: page.next_cursor,
    })
}
