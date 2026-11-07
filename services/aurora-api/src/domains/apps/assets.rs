//! Icon and guide extraction from the stored package archive.

use crate::app::AppState;
use aurora_core::ApiError;
use aurora_db::repo::plugins;
use aurora_registry::package::{GUIDE, ICON, ICON_DARK};
use aurora_registry::{Limits, Package};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Archives are content-addressed by digest, so `has_dark_icon` never changes per digest.
fn dark_cache() -> &'static Mutex<HashMap<String, bool>> {
    static C: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn read_file(bytes: &[u8], path: &str) -> Option<Vec<u8>> {
    let pkg = Package::read(bytes, &Limits::default()).ok()?;
    pkg.file(path).map(<[u8]>::to_vec)
}

async fn archive(state: &AppState, digest: &str) -> Result<Vec<u8>, ApiError> {
    plugins::get_blob(&state.pool, digest, "archive")
        .await?
        .ok_or_else(|| ApiError::not_found("Package archive not found."))
}

pub async fn has_dark_icon(state: &AppState, digest: &str) -> bool {
    if let Ok(c) = dark_cache().lock()
        && let Some(v) = c.get(digest)
    {
        return *v;
    }
    let Ok(bytes) = archive(state, digest).await else {
        return false;
    };
    let dark = Package::read(&bytes, &Limits::default())
        .map(|p| p.file(ICON_DARK).is_some())
        .unwrap_or(false);
    if let Ok(mut c) = dark_cache().lock() {
        c.insert(digest.to_owned(), dark);
    }
    dark
}

/// PNG bytes of the light or dark icon (dark falls back to 404 when absent).
pub async fn icon(state: &AppState, digest: &str, dark: bool) -> Result<Vec<u8>, ApiError> {
    let bytes = archive(state, digest).await?;
    read_file(&bytes, if dark { ICON_DARK } else { ICON })
        .ok_or_else(|| ApiError::not_found("Icon not found."))
}

pub async fn guide(state: &AppState, digest: &str) -> Result<Vec<u8>, ApiError> {
    let bytes = archive(state, digest).await?;
    read_file(&bytes, GUIDE).ok_or_else(|| ApiError::not_found("Guide not found."))
}
