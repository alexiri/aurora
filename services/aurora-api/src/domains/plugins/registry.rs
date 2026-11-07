//! Registry snapshot compile + activation (09 §9, 05 §Background jobs).
//!
//! All compiles run inside a transaction holding a transaction-scoped advisory
//! lock, so concurrent lifecycle changes across API replicas are serialised.

use super::ingest;
use crate::app::AppState;
use crate::audit::{self, AuditActor, AuditEvent, Outcome};
use aurora_core::{ApiError, ids};
use aurora_db::DbError;
use aurora_db::bus::{self, Channel};
use aurora_db::repo::{gateway, plugins};
use aurora_registry::claims::ConflictGroup;
use aurora_registry::manifest::CONTRACT;
use aurora_registry::snapshot::{
    ActivePackage, CompileError, CompiledSnapshot, Decision, compile, select_active,
};
use aurora_registry::{Limits, Manifest, Package};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use std::collections::BTreeSet;
use std::time::Duration;

const COMPILED_BY: &str = "aurora-api";

/// Why a compile did not produce a snapshot.
pub enum CompileFail {
    /// Unresolved host-claim conflicts (only when not allowed to hold packages).
    Conflict(Vec<ConflictGroup>),
    Api(ApiError),
}

impl From<ApiError> for CompileFail {
    fn from(e: ApiError) -> Self {
        Self::Api(e)
    }
}

impl From<DbError> for CompileFail {
    fn from(e: DbError) -> Self {
        Self::Api(e.into())
    }
}

pub fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}

async fn active_package(
    conn: &mut PgConnection,
    r: &plugins::PackageRow,
) -> Result<ActivePackage, ApiError> {
    let manifest: Manifest = serde_json::from_value(r.manifest.clone())
        .map_err(|e| ApiError::internal(format!("stored manifest unreadable: {e}")))?;
    let declares_hooks = r
        .permissions
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty());
    let mut hooks_digest = None;
    if declares_hooks {
        let bytes = plugins::get_blob(&mut *conn, &r.digest, "archive")
            .await?
            .ok_or_else(|| ApiError::internal("Package archive blob is missing."))?;
        let pkg = Package::read(&bytes, &Limits::default())
            .map_err(|_| ApiError::internal("Stored package archive is unreadable."))?;
        hooks_digest = pkg
            .file(aurora_registry::package::HOOKS_WASM)
            .map(|w| hex(&Sha256::digest(w)));
    }
    Ok(ActivePackage {
        manifest,
        digest: r.digest.clone(),
        publisher: r.publisher.clone(),
        hooks_digest,
    })
}

/// Compile every `active` package. With `hold`, packages in unresolved claim
/// conflicts (all but the earliest-approved of each group) go back to
/// `pending_approval`/`claim_conflict` and the compile is retried.
async fn compile_active(
    state: &AppState,
    conn: &mut PgConnection,
    hold: bool,
) -> Result<CompiledSnapshot, CompileFail> {
    let mut rows = plugins::list_by_status(&mut *conn, &["active"]).await?;
    let decisions: Vec<Decision> = plugins::decisions(&mut *conn)
        .await?
        .into_iter()
        .map(|d| Decision {
            claim_key: d.claim_key,
            fingerprint: d.claim_fingerprint,
            winner_package_id: d.winner_package_id,
        })
        .collect();
    let reserved = ingest::reserved_names(state);
    loop {
        let mut pkgs = Vec::with_capacity(rows.len());
        for r in &rows {
            pkgs.push(active_package(conn, r).await?);
        }
        match compile(&pkgs, &decisions, &reserved, CONTRACT) {
            Ok(c) => return Ok(c),
            Err(CompileError::UnresolvedConflicts(groups)) => {
                let mut held = BTreeSet::new();
                if hold {
                    for g in &groups {
                        let mut ids: Vec<&str> =
                            g.members.iter().map(|m| m.package_id.as_str()).collect();
                        ids.sort_unstable();
                        ids.dedup();
                        ids.sort_by_key(|p| {
                            rows.iter()
                                .find(|r| r.package_id == *p)
                                .map(|r| (r.approved_at, r.id))
                        });
                        held.extend(ids.into_iter().skip(1).map(str::to_owned));
                    }
                }
                if held.is_empty() {
                    return Err(CompileFail::Conflict(groups));
                }
                for pid in &held {
                    if let Some(r) = rows.iter().find(|r| &r.package_id == pid) {
                        plugins::transition(
                            conn,
                            r.id,
                            "pending_approval",
                            Some("claim_conflict"),
                            None,
                            false,
                        )
                        .await?;
                        audit::record(
                            state,
                            AuditEvent::new(
                                AuditActor::system("registry-compiler"),
                                "plugin.hold",
                                Outcome::Success,
                            )
                            .target("plugin_package", r.id)
                            .metadata(
                                json!({"package_id": r.package_id, "reason": "claim_conflict"}),
                            ),
                        );
                    }
                }
                rows.retain(|r| !held.contains(&r.package_id));
            }
            Err(CompileError::Serialize(m)) => return Err(ApiError::internal(m).into()),
            Err(e) => {
                return Err(ApiError::conflict("invalid_state", e.to_string()).into());
            }
        }
    }
}

/// Newest supported snapshot header, as `(hash, contract)`.
async fn current_hash(conn: &mut PgConnection) -> Result<Option<String>, DbError> {
    let min = i32::try_from(CONTRACT.saturating_sub(1)).unwrap_or(0);
    let max = i32::try_from(CONTRACT).unwrap_or(i32::MAX);
    let headers = gateway::snapshot_headers(conn, min, max).await?;
    let cands: Vec<(u32, chrono::DateTime<chrono::Utc>, String)> = headers
        .into_iter()
        .map(|h| {
            (
                u32::try_from(h.contract).unwrap_or(0),
                h.activated_at,
                h.content_hash,
            )
        })
        .collect();
    Ok(select_active(&cands, CONTRACT).map(str::to_owned))
}

/// Compile, store and activate (NOTIFY on change). Must run under
/// [`plugins::lock_compile`] in the caller's transaction. Returns the new
/// active hash, or `None` when the active snapshot already has this content.
pub async fn activate_snapshot(
    state: &AppState,
    conn: &mut PgConnection,
    hold: bool,
) -> Result<Option<String>, CompileFail> {
    let c = compile_active(state, conn, hold).await?;
    if current_hash(conn).await?.as_deref() == Some(c.hash.as_str()) {
        return Ok(None);
    }
    let value: Value = serde_json::from_slice(&c.json)
        .map_err(|e| ApiError::internal(format!("snapshot serialization: {e}")))?;
    let contract = i32::try_from(c.contract).unwrap_or(i32::MAX);
    plugins::put_snapshot(conn, ids::new_id(), contract, &c.hash, &value, COMPILED_BY).await?;
    bus::notify(
        &mut *conn,
        Channel::Registry,
        &json!({"hash": c.hash, "contract": contract}),
    )
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Some(c.hash))
}

fn flatten(f: CompileFail) -> ApiError {
    match f {
        CompileFail::Api(e) => e,
        CompileFail::Conflict(_) => ApiError::internal("Unresolved host claim conflict."),
    }
}

/// Recompile the active snapshot after the set of active packages (or a claim
/// decision) changed. Conflicting packages are held back as `pending_approval`.
pub async fn request_compile(state: &AppState) -> Result<(), ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    plugins::lock_compile(&mut tx).await?;
    activate_snapshot(state, &mut tx, true)
        .await
        .map_err(flatten)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Startup: activate a snapshot (empty when nothing is active) if none exists.
pub async fn ensure_initial(state: &AppState) -> Result<bool, ApiError> {
    let mut tx = state.pool.begin().await.map_err(DbError::from)?;
    plugins::lock_compile(&mut tx).await?;
    if plugins::snapshot_exists(&mut *tx).await? {
        return Ok(false);
    }
    let created = activate_snapshot(state, &mut tx, true)
        .await
        .map_err(flatten)?
        .is_some();
    tx.commit().await.map_err(DbError::from)?;
    Ok(created)
}

/// Retry [`ensure_initial`] in the background until the database is reachable.
pub fn spawn_initial(state: AppState) {
    tokio::spawn(async move {
        loop {
            match ensure_initial(&state).await {
                Ok(_) => return,
                Err(e) => {
                    tracing::warn!(error = %e.message(), "initial registry snapshot not ready; retrying");
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            }
        }
    });
}

pub struct ActiveSnapshot {
    pub hash: String,
    pub contract: i32,
    /// Canonical bytes; `sha256(body) == hash`.
    pub body: Vec<u8>,
}

impl ActiveSnapshot {
    /// `200 application/json` with the exact canonical bytes and snapshot headers.
    pub fn into_response(self) -> axum::response::Response {
        use axum::response::IntoResponse;
        let mut res = (http::StatusCode::OK, self.body).into_response();
        let h = res.headers_mut();
        h.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        if let Ok(v) = http::HeaderValue::from_str(&self.hash) {
            h.insert("x-snapshot-hash", v);
        }
        h.insert(
            "x-snapshot-contract",
            http::HeaderValue::from(self.contract),
        );
        res
    }
}

/// The snapshot consumers should use (09 §9 activation rule), limited to
/// contracts `<= contract_max`.
pub async fn active_snapshot(
    state: &AppState,
    contract_max: u32,
) -> Result<Option<ActiveSnapshot>, ApiError> {
    let max = contract_max.min(CONTRACT);
    let headers = gateway::snapshot_headers(
        &state.pool,
        i32::try_from(max.saturating_sub(1)).unwrap_or(0),
        i32::try_from(max).unwrap_or(i32::MAX),
    )
    .await?;
    let cands: Vec<(u32, chrono::DateTime<chrono::Utc>, String)> = headers
        .into_iter()
        .map(|h| {
            (
                u32::try_from(h.contract).unwrap_or(0),
                h.activated_at,
                h.content_hash,
            )
        })
        .collect();
    let Some(hash) = select_active(&cands, max).map(str::to_owned) else {
        return Ok(None);
    };
    let contract = cands
        .iter()
        .find(|c| c.2 == hash)
        .map_or(0, |c| i32::try_from(c.0).unwrap_or(0));
    let Some(v) = gateway::snapshot_body(&state.pool, &hash).await? else {
        return Ok(None);
    };
    let body = aurora_registry::snapshot::canonical_bytes(&v);
    if hex(&Sha256::digest(&body)) != hash {
        return Err(ApiError::internal(
            "Stored snapshot does not match its hash.",
        ));
    }
    Ok(Some(ActiveSnapshot {
        hash,
        contract,
        body,
    }))
}

/// The active snapshot parsed into its typed view (`None` when nothing is active).
pub async fn active_typed(
    state: &AppState,
) -> Result<Option<aurora_registry::snapshot::Snapshot>, ApiError> {
    let Some(s) = active_snapshot(state, CONTRACT).await? else {
        return Ok(None);
    };
    aurora_registry::snapshot::Snapshot::from_slice(&s.body)
        .map(Some)
        .map_err(|e| ApiError::internal(format!("active snapshot unreadable: {e}")))
}
