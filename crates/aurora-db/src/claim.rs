//! The canonical claim pattern ("claims, not locks").
//!
//! Work rows carry a lease timestamp. A worker claims a batch in ONE statement:
//!
//! ```sql
//! UPDATE t SET lease = now() + make_interval(secs => $2)
//! WHERE id IN (
//!     SELECT id FROM t
//!     WHERE <due> AND (lease IS NULL OR lease < now())
//!     ORDER BY <order> LIMIT $1
//!     FOR UPDATE SKIP LOCKED)
//! RETURNING <cols>
//! ```
//!
//! `SKIP LOCKED` makes concurrent claimers pick disjoint rows; the lease makes
//! a crashed claimer's rows reclaimable after expiry. No lock is held after the
//! statement returns, and no network call happens inside it.

use sqlx::{PgExecutor, postgres::PgRow};

use crate::DbError;

/// Compile-time description of a claimable table. Fields are `&'static str` so
/// only source literals (never runtime input) can reach the generated SQL.
#[derive(Debug, Clone, Copy)]
pub struct ClaimSpec {
    pub table: &'static str,
    /// Lease column (`timestamptz null`).
    pub lease_col: &'static str,
    /// SQL boolean expression for "due", e.g. `"status = 'queued'"`.
    pub due: &'static str,
    pub order_by: &'static str,
    /// Column list for `RETURNING`.
    pub returning: &'static str,
}

impl ClaimSpec {
    /// SQL with `$1` = batch limit (bigint) and `$2` = lease seconds (float8).
    pub fn sql(&self) -> String {
        format!(
            "UPDATE {t} SET {l} = now() + make_interval(secs => $2) \
             WHERE id IN (SELECT id FROM {t} \
                 WHERE ({due}) AND ({l} IS NULL OR {l} < now()) \
                 ORDER BY {ord} LIMIT $1 FOR UPDATE SKIP LOCKED) \
             RETURNING {ret}",
            t = self.table,
            l = self.lease_col,
            due = self.due,
            ord = self.order_by,
            ret = self.returning,
        )
    }
}

/// Claim up to `limit` due rows, leasing them for `lease_secs` seconds.
pub async fn claim<'e, E: PgExecutor<'e>>(
    executor: E,
    spec: &ClaimSpec,
    limit: i64,
    lease_secs: f64,
) -> Result<Vec<PgRow>, DbError> {
    Ok(sqlx::query(&spec.sql())
        .bind(limit)
        .bind(lease_secs)
        .fetch_all(executor)
        .await?)
}
