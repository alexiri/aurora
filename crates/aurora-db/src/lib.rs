//! aurora-db — Postgres access: pool, embedded migrations, LISTEN/NOTIFY bus,
//! singleton-job and claim helpers. See docs/02-engineering-conventions.md and
//! docs/03-data-model.md (owning document for every table).
#![forbid(unsafe_code)]

mod api_error;
pub mod authz;
pub mod bus;
pub mod claim;
mod error;
pub mod grants;
pub mod jobs;
pub mod repo;

pub use api_error::conflict_code;
pub use error::DbError;
pub use sqlx::PgPool;

/// Embedded, forward-only migrations (`crates/aurora-db/migrations`).
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Open a pool against `url`.
pub async fn connect(url: &str, max_connections: u32) -> Result<PgPool, DbError> {
    Ok(sqlx::postgres::PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(url)
        .await?)
}

/// Apply all pending migrations (idempotent).
pub async fn migrate(pool: &PgPool) -> Result<(), DbError> {
    MIGRATOR.run(pool).await.map_err(DbError::Migrate)
}
