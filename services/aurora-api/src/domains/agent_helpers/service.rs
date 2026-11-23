//! The writer of C-1143 (P6-TPL-6): `replace_all` swaps an agent's whole helper set in the
//! caller's transaction (P6-TPL-3 calls it from template create/update), `delete` removes one
//! or all. Both validate paths with the crate's C-1141 helper rules (never re-implemented),
//! enforce the per-agent caps, and bump the agent's home-sync generation the way a skill edit
//! does. No agent tool reaches either.

use aurora_core::{ApiError, limits};
use aurora_db::repo::{agent_helpers as repo, skills};
use aurora_template::helper_path_ok;
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use uuid::Uuid;

/// One file to store: the path below `helpers/` and its bytes.
pub struct HelperFile<'a> {
    pub path: &'a str,
    pub content: &'a [u8],
    pub executable: bool,
}

fn invalid(path: &str, why: &str) -> ApiError {
    ApiError::unprocessable(
        "template_helper_invalid",
        format!("`{path}` is not a valid helper file: {why}."),
    )
}

fn cap(message: String) -> ApiError {
    ApiError::unprocessable("helpers_limit", message)
}

/// Replace an agent's whole helper set with `files` (C-1143), inside the caller's transaction:
/// everything of the agent is removed first, then the files are inserted with their SHA-256
/// digests, and the agent's home-sync generation is bumped. The C-1141 path rules
/// ([`helper_path_ok`]) and the per-agent caps (`TEMPLATE_HELPERS_MAX` files,
/// `TEMPLATE_HELPER_FILE_MAX` per file, `TEMPLATE_HELPERS_TOTAL_MAX` together) are enforced
/// here, before anything is written.
pub async fn replace_all(
    tx: &mut PgConnection,
    agent: Uuid,
    files: &[HelperFile<'_>],
) -> Result<(), ApiError> {
    if files.len() > limits::TEMPLATE_HELPERS_MAX {
        return Err(cap(format!(
            "An agent can have at most {} helper files.",
            limits::TEMPLATE_HELPERS_MAX
        )));
    }
    let mut total = 0usize;
    let mut seen = std::collections::HashSet::new();
    for f in files {
        if !helper_path_ok(f.path) {
            return Err(invalid(
                f.path,
                "at most 4 segments below helpers/, letters, digits, . _ / - only, no hidden or empty segment",
            ));
        }
        if f.content.len() > limits::TEMPLATE_HELPER_FILE_MAX {
            return Err(invalid(
                f.path,
                &format!("larger than {} bytes", limits::TEMPLATE_HELPER_FILE_MAX),
            ));
        }
        total += f.content.len();
        if total > limits::TEMPLATE_HELPERS_TOTAL_MAX {
            return Err(cap(format!(
                "An agent's helper files together can be at most {} bytes.",
                limits::TEMPLATE_HELPERS_TOTAL_MAX
            )));
        }
        if !seen.insert(f.path) {
            return Err(invalid(f.path, "the same path twice"));
        }
    }
    repo::delete_all(&mut *tx, agent).await?;
    for f in files {
        let sha256 = hex::encode(Sha256::digest(f.content));
        repo::insert(&mut *tx, agent, f.path, f.content, f.executable, &sha256).await?;
    }
    skills::bump_agent(&mut *tx, agent).await?;
    Ok(())
}

/// Delete one helper file (`Some(path)`) or every helper file (`None`) of an agent, inside the
/// caller's transaction, and bump the home-sync generation when anything was removed. The
/// number of files removed.
pub async fn delete(
    tx: &mut PgConnection,
    agent: Uuid,
    path: Option<&str>,
) -> Result<u64, ApiError> {
    let removed = match path {
        Some(p) => {
            if !helper_path_ok(p) {
                return Err(invalid(
                    p,
                    "at most 4 segments below helpers/, letters, digits, . _ / - only, no hidden or empty segment",
                ));
            }
            u64::from(repo::delete_one(&mut *tx, agent, p).await?)
        }
        None => repo::delete_all(&mut *tx, agent).await?,
    };
    if removed > 0 {
        skills::bump_agent(&mut *tx, agent).await?;
    }
    Ok(removed)
}
