//! Template-database lifecycle for test databases (VEL-003).
//!
//! Every `TestDb` used to replay all 53 migrations; with ~800 it-tests the
//! per-test DDL replay dominated wall-clock (VEL-001's measurement). Instead,
//! one template database per *migration set* is created and migrated once,
//! and each test database is then cloned from it —
//! `CREATE DATABASE … TEMPLATE` is a filesystem-level copy, orders of
//! magnitude faster than replaying DDL.
//!
//! The template name is content-addressed:
//! `aurora_test_template_<migration-hash 16 hex>_<unixtime>`. The hash covers
//! every migration's version, description, SQL and checksum, so
//!
//! - **Schema always current.** A template exists under a name only after the
//!   full migration set for that exact hash was applied; a changed migration
//!   set simply computes a different hash and finds/builds its own template.
//!   A stale template is never reused (its name no longer matches), and a
//!   half-built one never carries the final name (see below).
//! - **Concurrent lanes with different trees don't interfere.** Two
//!   worktrees sharing one dev Postgres but sitting on different migration
//!   sets each get their own template; neither ever clones the other's
//!   schema. (This is also why the hash lives in the *name*, with the
//!   database comment as a human-readable cross-check.)
//! - **Isolation unchanged.** Each test still gets its own uniquely named
//!   database, dropped at test end exactly as before. Tests never connect to
//!   a template: they receive a clone, and the template itself is sealed
//!   (`ALLOW_CONNECTIONS false`) so no session can even log into it.
//! - **Concurrency-safe.** Building happens under
//!   `pg_advisory_lock(hashtext('aurora_test_template'))` with a re-check
//!   after acquiring it, so parallel test processes (nextest workers,
//!   libtest threads, CI shards) serialize; exactly one process builds a
//!   given hash while the others wait and then clone it.
//! - **Kill switch.** `AURORA_TEST_TEMPLATE_DB=0` (or `false`/`no`/`off`)
//!   reverts `TestDb` to the legacy per-test CREATE DATABASE + migrate path,
//!   so a regression here can never hard-block a lane or CI.
//!
//! Build protocol (under the advisory lock): create
//! `aurora_test_template_wip_<unixtime>_<uuid>` from `template0` (same
//! encoding/locale defaults as the legacy plain `CREATE DATABASE`), run the
//! migrations, stamp provenance in `COMMENT ON DATABASE` (not a table: clones
//! must have exactly the legacy path's schema), seal the database, then rename it to
//! the final content-addressed name. A builder that dies mid-way leaves at
//! most a `wip` database that no test will ever clone; reaping removes it once
//! old.
//! Templates of *other* hashes are left alone while they may still be in use
//! and are reaped after [`TEMPLATE_REAP_AFTER`] (24 h) — a test run is
//! minutes, never days.

use std::time::Duration;

use anyhow::Context;
use sha2::{Digest, Sha256};
use sqlx::postgres::PgConnection;
use sqlx::{Connection, PgPool};

use crate::replace_db_name;

/// Name of the env var that disables the template mechanism (`=0`).
pub const TEMPLATE_DB_ENV: &str = "AURORA_TEST_TEMPLATE_DB";

/// Prefix of every template database name.
const TEMPLATE_PREFIX: &str = "aurora_test_template";

/// In-flight (unsealed, partially built) template marker.
const WIP_MARKER: &str = "wip";

/// How long to wait for the template advisory lock (creation serializes
/// across processes; only one of N workers actually builds, the rest wait).
const TEMPLATE_LOCK_TIMEOUT: Duration = Duration::from_secs(300);

/// Age after which templates of other migration sets (and abandoned `wip`
/// builds) are dropped. A test run never holds a template anywhere near this
/// long, so reaping cannot touch a live lane's template.
const TEMPLATE_REAP_AFTER: Duration = Duration::from_secs(24 * 3600);

/// Age after which an abandoned in-flight build is reaped.
const WIP_REAP_AFTER: Duration = Duration::from_secs(3600);

/// (FIX-103) Age after which a leaked per-test database (`aurora_test_<uuid v7 simple>`, left by a
/// killed run: `Drop` never ran) with no connections is reaped. The age is read from the name: a v7
/// uuid starts with its 48-bit millisecond creation time. A test run is minutes, never hours.
pub const LEAKED_DB_REAP_AFTER: Duration = Duration::from_secs(2 * 3600);

/// (FIX-103) Advisory-lock key of the clone/reap protocol: a clone holds it **shared** while its
/// `CREATE DATABASE … TEMPLATE` runs (Postgres refuses to copy a template that another session is
/// dropping), a reap holds it **exclusive**. It is deliberately not the template *build* lock, which a
/// builder of another migration set holds for the whole migration run: clones of an existing template
/// must not wait for that.
const REAP_LOCK: &str = "aurora_test_reap";

/// How long a reap waits for in-flight clones (each takes well under a second) before skipping.
const REAP_LOCK_WAIT: Duration = Duration::from_secs(10);

/// `false` only when `AURORA_TEST_TEMPLATE_DB` is set to a falsy value
/// (`0`, `false`, `no`, `off`) — the documented kill switch.
pub fn template_enabled() -> bool {
    match std::env::var(TEMPLATE_DB_ENV) {
        Ok(v) => !matches!(v.trim(), "0" | "false" | "no" | "off"),
        Err(_) => true,
    }
}

/// The migration-set fingerprint: the first 16 hex digits (64 bits) of a
/// SHA-256 over every migration's version, description, SQL and checksum, in
/// migration order, each field length-prefixed. Unlike `DefaultHasher` the
/// value is stable across Rust releases and processes, so templates built by
/// one toolchain are found by another.
pub fn migration_set_hash() -> String {
    let mut sha = Sha256::new();
    let mut field = |bytes: &[u8]| {
        sha.update((bytes.len() as u64).to_le_bytes());
        sha.update(bytes);
    };
    for m in aurora_db::MIGRATOR.migrations.iter() {
        field(&m.version.to_le_bytes());
        field(m.description.as_bytes());
        field(m.sql.as_bytes());
        field(&m.checksum);
    }
    sha.finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The content-addressed template name for `hash`.
fn template_name(hash: &str) -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{TEMPLATE_PREFIX}_{hash}_{ts}")
}

/// Ensure a template for the current migration set exists; returns its name.
///
/// The expensive path (create + migrate + seal) runs once per server per
/// migration set (guarded by the advisory lock and a re-check, so exactly one
/// process builds while the rest wait and then clone); every call after that
/// is one catalog lookup plus a filesystem-speed clone.
pub async fn ensure_template(server_url: &str) -> anyhow::Result<&'static str> {
    // (FIX-103) Once per process, best effort: reap leaked per-test databases of killed runs.
    static LEAK_REAP: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    LEAK_REAP
        .get_or_init(|| async {
            let _ = reap_leaked(server_url, now_millis()).await;
        })
        .await;
    // &'static leaked names are unique per hash; the set is tiny (one string
    // per distinct migration set the process ever saw — one in practice).
    let hash = migration_set_hash();
    let name: &'static str = match find_template_on_server(server_url, &hash).await? {
        Some(name) => leak_name(name),
        None => build_template(server_url, &hash).await?,
    };
    Ok(name)
}

fn leak_name(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}

/// Find the template database for `hash`, if one exists.
async fn find_template(lock_conn: &mut PgConnection, hash: &str) -> anyhow::Result<Option<String>> {
    let prefix = format!("{TEMPLATE_PREFIX}_{hash}_%");
    let row: Option<(String,)> =
        sqlx::query_as("SELECT datname FROM pg_database WHERE datname LIKE $1 ORDER BY 1 LIMIT 1")
            .bind(prefix)
            .fetch_optional(&mut *lock_conn)
            .await
            .context("looking up template database")?;
    Ok(row.map(|(name,)| name))
}

/// Fast path: look up the template on a short-lived connection.
async fn find_template_on_server(server_url: &str, hash: &str) -> anyhow::Result<Option<String>> {
    let mut conn = PgConnection::connect(server_url)
        .await
        .context("connecting to test server")?;
    let found = find_template(&mut conn, hash).await;
    conn.close().await.ok();
    found
}

/// Build the template for `hash` under the advisory lock (double-checked), or
/// return the name an earlier builder already left.
async fn build_template(server_url: &str, hash: &str) -> anyhow::Result<&'static str> {
    let mut lock_conn = PgConnection::connect(server_url)
        .await
        .context("connecting to test server for template lock")?;

    // Serialize template creation across processes. The lock key is a
    // constant, so every worker process computes the same one.
    let deadline = tokio::time::Instant::now() + TEMPLATE_LOCK_TIMEOUT;
    let locked = loop {
        let row: (bool,) =
            sqlx::query_as("SELECT pg_try_advisory_lock(hashtext('aurora_test_template'))")
                .fetch_one(&mut lock_conn)
                .await
                .context("acquiring template advisory lock")?;
        if row.0 {
            break true;
        }
        if tokio::time::Instant::now() >= deadline {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    if !locked {
        anyhow::bail!(
            "timed out after {TEMPLATE_LOCK_TIMEOUT:?} waiting for the template advisory lock \
             (another process may be stuck building the template)"
        );
    }

    let result = build_locked(server_url, &mut lock_conn, hash).await;

    // Release the advisory lock (session-scoped; it also drops with the
    // connection, but release explicitly so nothing depends on close timing).
    let _ = sqlx::query("SELECT pg_advisory_unlock(hashtext('aurora_test_template'))")
        .execute(&mut lock_conn)
        .await;
    lock_conn.close().await.ok();
    result
}

/// Holding the advisory lock: re-check (another process may have built this
/// hash while we waited), otherwise build, then reap old templates.
async fn build_locked(
    server_url: &str,
    lock_conn: &mut PgConnection,
    hash: &str,
) -> anyhow::Result<&'static str> {
    if let Some(name) = find_template(lock_conn, hash).await? {
        return Ok(leak_name(name));
    }
    let name = rebuild_template(server_url, lock_conn, hash).await?;
    // Other migration sets' templates: exclusive against in-flight clones (FIX-103).
    if lock_reap_exclusive(lock_conn).await {
        reap_stale_templates(lock_conn).await;
        unlock_reap_exclusive(lock_conn).await;
    }
    Ok(leak_name(name))
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Take the reap lock exclusively (waits for in-flight clones up to [`REAP_LOCK_WAIT`]); `false`
/// when it could not be had, in which case the caller skips its reap — reaping is best effort.
async fn lock_reap_exclusive(conn: &mut PgConnection) -> bool {
    let wait = format!("SET lock_timeout = '{}ms'", REAP_LOCK_WAIT.as_millis());
    if sqlx::query(&wait).execute(&mut *conn).await.is_err() {
        return false;
    }
    let got = sqlx::query("SELECT pg_advisory_lock(hashtext($1))")
        .bind(REAP_LOCK.to_owned())
        .execute(&mut *conn)
        .await
        .is_ok();
    let _ = sqlx::query("RESET lock_timeout").execute(&mut *conn).await;
    got
}

async fn unlock_reap_exclusive(conn: &mut PgConnection) {
    let _ = sqlx::query("SELECT pg_advisory_unlock(hashtext($1))")
        .bind(REAP_LOCK.to_owned())
        .execute(&mut *conn)
        .await;
}

/// Decide whether `name` is a leaked per-test database (`aurora_test_` + 32 hex of a v7 uuid) older
/// than [`LEAKED_DB_REAP_AFTER`] at `now_ms`. Pure. Template databases (`aurora_test_template_…`) and
/// anything of another shape are never matched.
pub(crate) fn leaked_db_is_old(name: &str, now_ms: u64) -> bool {
    let Some(hex) = name.strip_prefix("aurora_test_") else {
        return false;
    };
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return false;
    }
    let Ok(created_ms) = u64::from_str_radix(&hex[..12], 16) else {
        return false;
    };
    now_ms.saturating_sub(created_ms) >= LEAKED_DB_REAP_AFTER.as_millis() as u64
}

/// (FIX-103) Drop leaked per-test databases and their gateway test roles: names of the
/// `aurora_test_<uuid>` shape whose creation time (in the uuid) is older than
/// [`LEAKED_DB_REAP_AFTER`] **and** that have no connection. Takes the reap lock exclusively (waiting
/// out in-flight clones), and only when there is something to reap. Returns the number of databases
/// dropped. Never touches the `aurora` database or any name outside the test namespace.
pub async fn reap_leaked(server_url: &str, now_ms: u64) -> anyhow::Result<usize> {
    let mut conn = PgConnection::connect(server_url)
        .await
        .context("connecting to test server to reap leaked databases")?;
    let names: Vec<(String,)> =
        sqlx::query_as("SELECT datname FROM pg_database WHERE datname LIKE $1")
            .bind("aurora\\_test\\_%")
            .fetch_all(&mut conn)
            .await
            .context("listing test databases")?;
    let leaked: Vec<String> = names
        .into_iter()
        .map(|(n,)| n)
        .filter(|n| leaked_db_is_old(n, now_ms))
        .collect();
    let roles: Vec<(String,)> =
        sqlx::query_as("SELECT rolname FROM pg_roles WHERE rolname LIKE $1")
            .bind("aurora\\_test\\_gw\\_%")
            .fetch_all(&mut conn)
            .await
            .unwrap_or_default();
    let old_roles: Vec<String> = roles
        .into_iter()
        .map(|(n,)| n)
        .filter(|n| {
            n.strip_prefix("aurora_test_gw_")
                .is_some_and(|hex| leaked_db_is_old(&format!("aurora_test_{hex}"), now_ms))
        })
        .collect();
    if leaked.is_empty() && old_roles.is_empty() {
        conn.close().await.ok();
        return Ok(0);
    }
    if !lock_reap_exclusive(&mut conn).await {
        conn.close().await.ok();
        return Ok(0);
    }
    let mut dropped = 0;
    for name in &leaked {
        // No connection at all, then a plain DROP (no FORCE): a session that arrives meanwhile makes
        // the DROP fail, which is the right outcome.
        let busy: Result<(i64,), _> =
            sqlx::query_as("SELECT count(*) FROM pg_stat_activity WHERE datname = $1")
                .bind(name)
                .fetch_one(&mut conn)
                .await;
        if !matches!(busy, Ok((0,))) {
            continue;
        }
        if sqlx::query(&format!("DROP DATABASE IF EXISTS \"{name}\""))
            .execute(&mut conn)
            .await
            .is_ok()
        {
            dropped += 1;
            eprintln!("aurora-testkit: reaped leaked test database `{name}`");
        }
    }
    for role in &old_roles {
        // Fails harmlessly while the role still has privileges in a live database.
        let _ = sqlx::query(&format!("DROP ROLE IF EXISTS \"{role}\""))
            .execute(&mut conn)
            .await;
    }
    unlock_reap_exclusive(&mut conn).await;
    conn.close().await.ok();
    Ok(dropped)
}

/// Reap templates of other migration sets (and abandoned in-flight builds) that are past their grace
/// period, exclusive against in-flight clones. Normally this runs after a template build; exposed so
/// the clone/reap protocol can be exercised directly. Returns `false` when the reap lock was not had.
pub async fn reap_templates(server_url: &str) -> anyhow::Result<bool> {
    let mut conn = PgConnection::connect(server_url)
        .await
        .context("connecting to test server to reap templates")?;
    let ok = lock_reap_exclusive(&mut conn).await;
    if ok {
        reap_stale_templates(&mut conn).await;
        unlock_reap_exclusive(&mut conn).await;
    }
    conn.close().await.ok();
    Ok(ok)
}

/// Create, migrate, stamp, seal and rename the template. Caller holds the
/// advisory lock.
async fn rebuild_template(
    server_url: &str,
    lock_conn: &mut PgConnection,
    hash: &str,
) -> anyhow::Result<String> {
    // In-flight name: no test ever looks this up (find_template matches only
    // the content-addressed shape), so a crash here leaves nothing harmful.
    // The unixtime lets the reaper age abandoned builds without parsing a
    // uuid.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let wip = format!(
        "{TEMPLATE_PREFIX}_{WIP_MARKER}_{now}_{}",
        uuid::Uuid::now_v7().simple()
    );

    // From template0: encoding/locale/collation end up exactly what the
    // legacy plain `CREATE DATABASE` produced. Starts unsealed; it is sealed
    // below once the migrations are in, before it becomes visible under its
    // final name.
    sqlx::query(&format!("CREATE DATABASE \"{wip}\" TEMPLATE template0"))
        .execute(&mut *lock_conn)
        .await
        .context("creating template database")?;

    // Provenance goes into the database comment (a shared-catalog entry that
    // `CREATE DATABASE … TEMPLATE` does not copy), so cloned test databases
    // carry exactly the schema of the legacy create + migrate path.
    let built = migrate(server_url, &wip).await;
    let built = match built {
        Ok(()) => sqlx::query(&format!(
            "COMMENT ON DATABASE \"{wip}\" IS 'aurora-testkit migration template: hash={hash} migrations={}'",
            aurora_db::MIGRATOR.migrations.len()
        ))
        .execute(&mut *lock_conn)
        .await
        .map(|_| ())
        .context("stamping template provenance"),
        Err(e) => Err(e),
    };
    if let Err(e) = built {
        // Leave nothing half-built behind.
        let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{wip}\" WITH (FORCE)"))
            .execute(&mut *lock_conn)
            .await;
        return Err(e);
    }

    // Seal before renaming: from now on the template is guaranteed a quiet
    // source for every clone (no session can log into it), for its whole
    // life.
    sqlx::query(&format!(
        "ALTER DATABASE \"{wip}\" WITH ALLOW_CONNECTIONS false"
    ))
    .execute(&mut *lock_conn)
    .await
    .context("sealing template")?;

    let name = template_name(hash);
    sqlx::query(&format!("ALTER DATABASE \"{wip}\" RENAME TO \"{name}\""))
        .execute(&mut *lock_conn)
        .await
        .context("publishing template under its content-addressed name")?;
    Ok(name)
}

/// Run the full migration set on the fresh template.
async fn migrate(server_url: &str, wip: &str) -> anyhow::Result<()> {
    let url = replace_db_name(server_url, wip)?;
    let mut conn = PgConnection::connect(&url)
        .await
        .context("connecting to fresh template")?;
    aurora_db::MIGRATOR
        .run(&mut conn)
        .await
        .map_err(|e| anyhow::anyhow!("running migrations on template: {e}"))?;
    conn.close().await.ok();
    Ok(())
}

/// Best-effort reaping, caller holds the advisory lock: drop templates of
/// other migration sets (and abandoned in-flight builds) older than their
/// grace period. Never touches the template for the current hash — that one
/// may be in active use by this very run — and never anything outside the
/// `aurora_test_template_*` namespace.
async fn reap_stale_templates(lock_conn: &mut PgConnection) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let Ok(rows) =
        sqlx::query_as::<_, (String,)>("SELECT datname FROM pg_database WHERE datname LIKE $1")
            .bind(format!("{TEMPLATE_PREFIX}_%"))
            .fetch_all(&mut *lock_conn)
            .await
    else {
        return;
    };
    let current_hash = migration_set_hash();
    for (name,) in rows {
        let Some(reason) = reap_decision(&name, &current_hash, now) else {
            continue;
        };
        let dropped = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
            .execute(&mut *lock_conn)
            .await;
        if dropped.is_ok() {
            eprintln!("aurora-testkit: reaped {reason} `{name}`");
        }
    }
}

/// Decide what to do with one database in the `aurora_test_template_*`
/// namespace: return the reap reason, or `None` to leave it alone. Pure so it
/// can be unit-tested against every name shape without a server.
fn reap_decision(name: &str, current_hash: &str, now: u64) -> Option<&'static str> {
    let wip = name.starts_with(&format!("{TEMPLATE_PREFIX}_{WIP_MARKER}_"));
    let tail = name.strip_prefix(&format!("{TEMPLATE_PREFIX}_"))?;
    // Both name shapes carry their creation unixtime right after the fixed
    // fields: `aurora_test_template_<hash16>_<unixtime>` (published) and
    // `aurora_test_template_wip_<unixtime>_<uuid>` (in-flight). Only `wip`
    // names carry the marker, so the two cannot be confused.
    let ts = if wip {
        tail.split('_').nth(1)
    } else {
        tail.split('_').next_back()
    }
    .and_then(|t| t.parse::<u64>().ok())?;

    if wip {
        (now.saturating_sub(ts) >= WIP_REAP_AFTER.as_secs()).then_some("stale in-flight template")
    } else if name.contains(&format!("_{current_hash}_")) {
        None // the template this run may be cloning right now
    } else if now.saturating_sub(ts) >= TEMPLATE_REAP_AFTER.as_secs() {
        Some("old template (other migration set)")
    } else {
        None
    }
}

/// Clone the template into a fresh, uniquely named test database.
///
/// `admin` must be a connection to any database on the target server.
pub async fn clone_from_template(
    admin: &mut PgConnection,
    template: &str,
    name: &str,
) -> anyhow::Result<()> {
    // (FIX-103) Shared against reaps, which take the same lock exclusively: a reap never drops a template
    // while a clone is copying it. Clones do not exclude each other.
    sqlx::query("SELECT pg_advisory_lock_shared(hashtext($1))")
        .bind(REAP_LOCK.to_owned())
        .execute(&mut *admin)
        .await
        .context("taking the clone lock")?;
    let cloned = sqlx::query(&format!(
        "CREATE DATABASE \"{name}\" TEMPLATE \"{template}\""
    ))
    .execute(&mut *admin)
    .await
    .context("cloning test database from template");
    let _ = sqlx::query("SELECT pg_advisory_unlock_shared(hashtext($1))")
        .bind(REAP_LOCK.to_owned())
        .execute(&mut *admin)
        .await;
    cloned?;
    Ok(())
}

/// Run the migrations against a fresh pool on `url` (legacy `TestDb` path).
pub async fn migrate_pool(pool: &PgPool) -> anyhow::Result<()> {
    aurora_db::MIGRATOR
        .run(pool)
        .await
        .context("running migrations")
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;
    const HASH: &str = "853b0f112cc4dfb4";

    #[test]
    fn published_template_of_the_current_hash_is_never_reaped() {
        let name = format!("{TEMPLATE_PREFIX}_{HASH}_{NOW}");
        assert_eq!(reap_decision(&name, HASH, NOW), None);
        // Even ancient — a long-running lane may still be cloning it.
        assert_eq!(reap_decision(&name, HASH, NOW + 100 * 86_400), None);
    }

    #[test]
    fn other_hash_templates_are_reaped_after_the_ttl_only() {
        let name = format!("{TEMPLATE_PREFIX}_00000000000000ff_{NOW}");
        assert_eq!(reap_decision(&name, HASH, NOW), None);
        assert_eq!(
            reap_decision(&name, HASH, NOW + TEMPLATE_REAP_AFTER.as_secs()),
            Some("old template (other migration set)")
        );
    }

    #[test]
    fn abandoned_wip_builds_are_reaped_after_their_ttl() {
        let fresh = format!("{TEMPLATE_PREFIX}_wip_{NOW}_019abcdeadbeef00");
        let old = format!("{TEMPLATE_PREFIX}_wip_{}_019abcdeadbeef00", NOW - 3600);
        assert_eq!(reap_decision(&fresh, HASH, NOW), None);
        assert_eq!(
            reap_decision(&old, HASH, NOW),
            Some("stale in-flight template")
        );
    }

    #[test]
    fn leaked_database_names_are_aged_by_their_uuid_timestamp() {
        let ttl = LEAKED_DB_REAP_AFTER.as_millis() as u64;
        let created: u64 = 1_800_000_000_000;
        let name = format!("aurora_test_{created:012x}{}", "ab".repeat(10));
        assert_eq!(name.len(), "aurora_test_".len() + 32);
        assert!(!leaked_db_is_old(&name, created + ttl - 1));
        assert!(leaked_db_is_old(&name, created + ttl));
        // Only the exact shape: templates, the dev database, short or non-hex names are never matched.
        assert!(!leaked_db_is_old("aurora", created + 10 * ttl));
        assert!(!leaked_db_is_old(
            "aurora_test_template_00000000000000ff_1",
            created + 10 * ttl
        ));
        assert!(!leaked_db_is_old("aurora_test_abc", created + 10 * ttl));
        assert!(!leaked_db_is_old(
            &format!("aurora_test_{}", "z".repeat(32)),
            created + 10 * ttl
        ));
    }

    #[test]
    fn unparseable_names_are_left_alone() {
        assert_eq!(
            reap_decision(&format!("{TEMPLATE_PREFIX}_garbage"), HASH, NOW),
            None
        );
        assert_eq!(
            reap_decision("{TEMPLATE_PREFIX}_wip_notime_x", HASH, NOW),
            None
        );
        assert_eq!(reap_decision("some_other_database", HASH, NOW), None);
    }
}
