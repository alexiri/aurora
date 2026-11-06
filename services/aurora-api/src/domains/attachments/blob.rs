//! `BlobStore` (docs/10 §Attachments): `put` / `get` / `delete` by opaque key. Phase 0 backend:
//! `postgres` over the `blobs` table; an S3-compatible backend is a later registry entry. The
//! backend name is stored per attachment row (`blob_backend`), so rows keep working after a switch.

use aurora_core::ApiError;
use aurora_db::PgPool;
use aurora_db::repo::blobs;
use std::sync::Arc;
use uuid::Uuid;

/// `attachments.blob_backend` of the Phase 0 backend.
pub const POSTGRES: &str = "postgres";

#[async_trait::async_trait]
pub trait BlobStore: Send + Sync {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), ApiError>;
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, ApiError>;
    /// Idempotent.
    async fn delete(&self, key: &str) -> Result<(), ApiError>;
}

pub struct PostgresBlobStore {
    pool: PgPool,
}

impl PostgresBlobStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl BlobStore for PostgresBlobStore {
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), ApiError> {
        Ok(blobs::put(&self.pool, key, bytes).await?)
    }
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, ApiError> {
        Ok(blobs::get(&self.pool, key).await?)
    }
    async fn delete(&self, key: &str) -> Result<(), ApiError> {
        Ok(blobs::delete(&self.pool, key).await?)
    }
}

/// Backend that new blobs are written to.
pub fn current(pool: &PgPool) -> (&'static str, Arc<dyn BlobStore>) {
    (POSTGRES, Arc::new(PostgresBlobStore::new(pool.clone())))
}

/// Backend named by a stored row; `None` for an unknown name.
pub fn for_backend(name: &str, pool: &PgPool) -> Option<Arc<dyn BlobStore>> {
    (name == POSTGRES).then(|| Arc::new(PostgresBlobStore::new(pool.clone())) as Arc<dyn BlobStore>)
}

/// Fresh opaque blob key.
pub fn new_key() -> String {
    Uuid::now_v7().to_string()
}
