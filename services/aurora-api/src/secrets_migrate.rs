//! `aurora-api secrets migrate --to <store_id> [--purpose …] [--dry-run]` (docs/08 §Moving
//! values between stores, C-375).
//!
//! Row by row: open the old ref → `put` into the target store → in one transaction
//! `UPDATE … SET ref = new WHERE id = … AND ref = old` (compare-and-swap) → commit → only then
//! discard the old value. A lost CAS discards the NEW value and counts as skipped; any failure
//! before the commit leaves the old ref in place and valid. Reference refs (`value_source =
//! reference`) are never touched. Values never reach a log, an error or the summary.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use aurora_secrets::{Purpose, SealContext, SecretRef, SecretVault, ValueSource};
use sqlx::PgPool;
use uuid::Uuid;

/// What to do.
#[derive(Debug, Clone)]
pub struct Options {
    /// Target store id (must be managed).
    pub to: String,
    /// Only this purpose (all when `None`).
    pub purpose: Option<Purpose>,
    /// Report what would move; write nothing, not even to the store.
    pub dry_run: bool,
    /// Wait this long after a purpose's commits before discarding the old values, so gateway
    /// caches that hold an old ref have reloaded (the `aurora_gateway_invalidate` NOTIFY fires
    /// on every commit). Only matters when the old values live in an external store.
    pub discard_grace: Duration,
    /// Rows read per page.
    pub batch: i64,
}

impl Options {
    #[must_use]
    pub fn new(to: &str) -> Self {
        Self {
            to: to.to_owned(),
            purpose: None,
            dry_run: false,
            discard_grace: Duration::from_secs(2),
            batch: 200,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    /// Moved (dry run: would move).
    pub moved: u64,
    /// The row changed concurrently (CAS lost); nothing overwritten, the new value discarded.
    pub skipped: u64,
    /// The row could not be moved (store error, context mismatch, …); it keeps its old ref.
    pub failed: u64,
    /// Already in the target store.
    pub already: u64,
}

#[derive(Debug, Default)]
pub struct Summary {
    pub per_purpose: BTreeMap<&'static str, Counts>,
}

impl Summary {
    #[must_use]
    pub fn failed(&self) -> u64 {
        self.per_purpose.values().map(|c| c.failed).sum()
    }

    #[must_use]
    pub fn moved(&self) -> u64 {
        self.per_purpose.values().map(|c| c.moved).sum()
    }

    #[must_use]
    pub fn render(&self, dry_run: bool) -> String {
        let mut s = String::new();
        let verb = if dry_run { "would move" } else { "moved" };
        for (p, c) in &self.per_purpose {
            let _ = writeln!(
                s,
                "{p}: {verb} {}, skipped {}, failed {}, already in target {}",
                c.moved, c.skipped, c.failed, c.already
            );
        }
        s
    }
}

/// Parse a purpose name (`Purpose::as_str`).
pub fn parse_purpose(s: &str) -> Result<Purpose, String> {
    Purpose::ALL
        .into_iter()
        .find(|p| p.as_str() == s)
        .ok_or_else(|| {
            format!(
                "unknown purpose '{s}'; one of {}",
                Purpose::ALL.map(Purpose::as_str).join(", ")
            )
        })
}

struct Source {
    purpose: Purpose,
    /// `SELECT id, org_id, workspace_id, <ref> … WHERE id > $1 ORDER BY id LIMIT $2`.
    select: &'static str,
    /// `UPDATE … WHERE id = $1 AND <ref> = $3` setting the ref to `$2`.
    update: &'static str,
}

const SOURCES: [Source; 6] = [
    Source {
        purpose: Purpose::SecretValue,
        select: "SELECT id, org_id, workspace_id, value_ref FROM secrets \
                 WHERE value_source = 'managed' AND id > $1 ORDER BY id LIMIT $2",
        update: "UPDATE secrets SET value_ref = $2, updated_at = now() \
                 WHERE id = $1 AND value_source = 'managed' AND value_ref = $3",
    },
    Source {
        purpose: Purpose::ConnectionCredentials,
        select: "SELECT id, org_id, workspace_id, credentials_ref FROM app_connections \
                 WHERE id > $1 ORDER BY id LIMIT $2",
        // The version bump makes a concurrent token refresh notice the change.
        update: "UPDATE app_connections SET credentials_ref = $2, \
                 credentials_version = credentials_version + 1, updated_at = now() \
                 WHERE id = $1 AND credentials_ref = $3",
    },
    Source {
        purpose: Purpose::OauthClientSecret,
        select: "SELECT id, org_id, workspace_id, secret_ref FROM app_oauth_clients \
                 WHERE id > $1 ORDER BY id LIMIT $2",
        update: "UPDATE app_oauth_clients SET secret_ref = $2, updated_at = now() \
                 WHERE id = $1 AND secret_ref = $3",
    },
    Source {
        purpose: Purpose::IntegrationCredentials,
        select: "SELECT id, org_id, NULL::uuid, credentials_ref FROM channel_integrations \
                 WHERE id > $1 ORDER BY id LIMIT $2",
        update: "UPDATE channel_integrations SET credentials_ref = $2, \
                 credentials_version = credentials_version + 1, updated_at = now() \
                 WHERE id = $1 AND credentials_ref = $3",
    },
    Source {
        purpose: Purpose::PresenceCredentials,
        select: "SELECT p.id, i.org_id, NULL::uuid, p.credentials_ref \
                 FROM channel_presences p JOIN channel_integrations i ON i.id = p.integration_id \
                 WHERE p.credentials_ref IS NOT NULL AND p.id > $1 ORDER BY p.id LIMIT $2",
        update: "UPDATE channel_presences SET credentials_ref = $2, \
                 credentials_version = credentials_version + 1, updated_at = now() \
                 WHERE id = $1 AND credentials_ref = $3",
    },
    Source {
        purpose: Purpose::WebhookVerification,
        select: "SELECT w.id, a.org_id, a.workspace_id, w.verification->>'secret_ref' \
                 FROM webhooks w JOIN agents a ON a.id = w.agent_id \
                 WHERE w.verification->>'secret_ref' IS NOT NULL AND w.id > $1 \
                 ORDER BY w.id LIMIT $2",
        update: "UPDATE webhooks SET \
                 verification = jsonb_set(verification, '{secret_ref}', to_jsonb($2::text)), \
                 updated_at = now() \
                 WHERE id = $1 AND verification->>'secret_ref' = $3",
    },
];

type Row = (Uuid, Option<Uuid>, Option<Uuid>, String);

/// Outcome of one row.
enum Moved {
    Done,
    Lost,
}

/// Run the migration. Errors only for problems that stop the whole run (bad target store, the
/// database); per-row problems are counted in the summary.
pub async fn run(pool: &PgPool, vault: &SecretVault, opts: &Options) -> Result<Summary, String> {
    let target = vault
        .registry()
        .get(&opts.to)
        .ok_or_else(|| format!("unknown store '{}'", opts.to))?;
    if !target.capabilities().managed {
        return Err(format!(
            "store '{}' does not support managed values",
            opts.to
        ));
    }
    let mut summary = Summary::default();
    for src in &SOURCES {
        if opts.purpose.is_some_and(|p| p != src.purpose) {
            continue;
        }
        let counts = migrate_purpose(pool, vault, opts, src).await?;
        summary.per_purpose.insert(src.purpose.as_str(), counts);
    }
    Ok(summary)
}

async fn migrate_purpose(
    pool: &PgPool,
    vault: &SecretVault,
    opts: &Options,
    src: &Source,
) -> Result<Counts, String> {
    let mut counts = Counts::default();
    let mut discards: Vec<(SealContext, SecretRef)> = Vec::new();
    let mut after = Uuid::nil();
    loop {
        let rows: Vec<Row> = sqlx::query_as(src.select)
            .bind(after)
            .bind(opts.batch)
            .fetch_all(pool)
            .await
            .map_err(|e| format!("{}: reading rows: {e}", src.purpose.as_str()))?;
        let Some(last) = rows.last() else { break };
        after = last.0;
        for (id, org, ws, col) in rows {
            let Ok(old) = col.parse::<SecretRef>() else {
                tracing::warn!(purpose = src.purpose.as_str(), %id, "unparseable ref; row left as is");
                counts.failed += 1;
                continue;
            };
            let in_target = old.store_id().unwrap_or("inline") == opts.to;
            if in_target {
                counts.already += 1;
                continue;
            }
            if opts.dry_run {
                counts.moved += 1;
                continue;
            }
            let ctx = SealContext {
                purpose: src.purpose,
                owner_id: id,
                org_id: org.unwrap_or_else(Uuid::nil),
                workspace_id: ws,
            };
            match move_row(pool, vault, opts, src, &ctx, &old, &col).await {
                Ok(Moved::Done) => {
                    counts.moved += 1;
                    discards.push((ctx, old));
                }
                Ok(Moved::Lost) => {
                    tracing::warn!(purpose = src.purpose.as_str(), %id, "row changed concurrently; skipped");
                    counts.skipped += 1;
                }
                Err(why) => {
                    tracing::warn!(purpose = src.purpose.as_str(), %id, error = %why, "row not moved");
                    counts.failed += 1;
                }
            }
        }
    }
    // The new refs are committed. Give caches a moment, then drop the old values.
    if !discards.is_empty() {
        if !opts.discard_grace.is_zero() {
            tokio::time::sleep(opts.discard_grace).await;
        }
        for (ctx, old) in discards {
            if let Err(e) = vault.discard(&ctx, &old, ValueSource::Managed).await {
                // The row already points at the new value; the old one is an orphan.
                tracing::warn!(
                    purpose = src.purpose.as_str(), id = %ctx.owner_id, error = %e,
                    "old value could not be discarded (orphaned in its store)"
                );
            }
        }
    }
    Ok(counts)
}

async fn move_row(
    pool: &PgPool,
    vault: &SecretVault,
    opts: &Options,
    src: &Source,
    ctx: &SealContext,
    old: &SecretRef,
    old_col: &str,
) -> Result<Moved, String> {
    // read old → put new (the old ref stays valid throughout)
    let new = vault
        .reseal(ctx, old, &opts.to)
        .await
        .map_err(|e| e.to_string())?;
    let swapped: Result<bool, String> = async {
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        let n = sqlx::query(src.update)
            .bind(ctx.owner_id)
            .bind(new.to_column())
            .bind(old_col)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?
            .rows_affected();
        if n != 1 {
            // Dropping the transaction rolls it back.
            return Ok(false);
        }
        // Gateways reload the row (and with it the new ref) on commit.
        let (scope, sid) = if ctx.org_id.is_nil() {
            ("all", Uuid::nil())
        } else {
            ("org", ctx.org_id)
        };
        aurora_db::repo::credentials::notify_gateway_invalidate(&mut *tx, scope, sid)
            .await
            .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(true)
    }
    .await;
    match swapped {
        Ok(true) => Ok(Moved::Done),
        Ok(false) => {
            discard_new(vault, ctx, &new).await;
            Ok(Moved::Lost)
        }
        Err(e) => {
            // Nothing committed (or the commit outcome is unknown: only discard when we know the
            // row does not point at the new ref).
            if row_points_to(pool, src, ctx.owner_id, &new).await == Some(false) {
                discard_new(vault, ctx, &new).await;
            }
            Err(e)
        }
    }
}

async fn discard_new(vault: &SecretVault, ctx: &SealContext, new: &SecretRef) {
    if let Err(e) = vault.discard(ctx, new, ValueSource::Managed).await {
        tracing::warn!(id = %ctx.owner_id, error = %e, "unused new value could not be discarded");
    }
}

/// Whether the row currently holds `r` (None when that cannot be determined).
async fn row_points_to(pool: &PgPool, src: &Source, id: Uuid, r: &SecretRef) -> Option<bool> {
    // Re-use the select by walking from just before `id`.
    let before = Uuid::from_u128(id.as_u128().checked_sub(1)?);
    let rows: Vec<Row> = sqlx::query_as(src.select)
        .bind(before)
        .bind(1_i64)
        .fetch_all(pool)
        .await
        .ok()?;
    let row = rows.into_iter().find(|row| row.0 == id)?;
    Some(row.3 == r.to_column())
}
