//! Reading a template source repository (C-1103, C-1107, C-1108). The skill library's git
//! reader (`skill_libraries::git`, C-1125) with the template profile: the user agent
//! `aurora-templates/<version> git/2.0` and `TEMPLATE_HELPER_FILE_MAX` per blob. Preview, add,
//! patch and the poll job all read through here — never a second git client.

use std::collections::BTreeMap;

use aurora_core::{ApiError, limits, sanitize};
use serde_json::json;

use crate::app::AppState;
use crate::domains::skill_libraries::content::{ReadProfile, Reader, Resolved};
use crate::domains::skill_libraries::git::{GitRepo, Token};

/// The read profile of a template source (C-1107): its own user agent and per-blob cap.
#[must_use]
pub fn profile() -> ReadProfile {
    ReadProfile {
        user_agent: format!("aurora-templates/{}", env!("CARGO_PKG_VERSION")),
        file_max: limits::TEMPLATE_HELPER_FILE_MAX,
    }
}

/// The failure of a read, as the routes answer it: 422 `template_unreachable`, the C-1125
/// reason in words (`details.reason`, sanitized fixed text).
pub fn read_failure(e: &crate::domains::skill_libraries::content::ReadError) -> ApiError {
    let reason = sanitize::one_line(&e.reason(), 200);
    ApiError::unprocessable(
        "template_unreachable",
        format!("The repository could not be read: {reason}."),
    )
    .with_details(json!({ "reason": reason }))
}

/// One directory directly under the source's folder that holds a `template.yaml`.
#[derive(Debug)]
pub struct DirRead {
    pub dir: String,
    /// The `template.yaml` bytes (`None` when they could not be read: `problem` says why).
    pub meta: Option<Vec<u8>>,
    /// Why the `template.yaml` could not be read (sanitized, fixed text).
    pub problem: Option<String>,
}

/// One read of a source's folder: the resolved head, the repository manifest and every
/// template's `template.yaml` (everything else in the folder is ignored, C-1103).
#[derive(Debug)]
pub struct SourceRead {
    pub resolved: Resolved,
    /// The `aurora-templates.yaml` bytes (`None` when the folder has none).
    pub manifest: Option<Vec<u8>>,
    pub templates: Vec<DirRead>,
    /// More than `TEMPLATES_PER_SOURCE_MAX` template directories: only the first are read.
    pub truncated: bool,
}

/// Reads the folder of one source address once (preview, add and patch): resolve the ref, list
/// the tree, read `aurora-templates.yaml` and every `template.yaml`. One of the replica's read
/// permits, under the overall read deadline (SEC-148); errors are route errors.
pub async fn read_source(
    state: &AppState,
    url: &str,
    reference: Option<&str>,
    path: &str,
    token: Option<&Token>,
) -> Result<SourceRead, ApiError> {
    state
        .library
        .reads
        .interactive(async {
            let git = GitRepo::with_profile(
                &state.connect,
                url,
                token,
                state.library.fetch_timeout,
                profile(),
            )
            .map_err(|e| read_failure(&e))?;
            let reader = Reader::with_profile(&git, path, profile());
            read_with(&reader, reference, path)
                .await
                .map_err(|e| read_failure(&e))
        })
        .await
}

/// The reads of one listing against an open reader (also used at a recorded commit, C-1108
/// step 6). Errors are the reader's own.
pub async fn read_with(
    reader: &Reader<'_>,
    reference: Option<&str>,
    path: &str,
) -> Result<SourceRead, crate::domains::skill_libraries::content::ReadError> {
    use crate::domains::skill_libraries::content::ReadError;
    let resolved = reader.resolve_full(reference).await?;
    let entries = reader.tree(&resolved.sha).await?;
    let prefix = path.trim_matches('/');
    fn rel<'a>(prefix: &str, p: &'a str) -> Option<&'a str> {
        if prefix.is_empty() {
            Some(p)
        } else {
            p.strip_prefix(prefix)?.strip_prefix('/')
        }
    }    // The repository manifest at the folder's root (C-1103: required).
    let manifest_name = format!(
        "{}{}aurora-templates.yaml",
        prefix,
        if prefix.is_empty() { "" } else { "/" }
    );
    let manifest = match entries.iter().find(|e| e.path == manifest_name) {
        Some(e) if e.kind == "blob" => {
            let bytes = reader.blob(&e.blob).await?;
            if bytes.len() > limits::TEMPLATE_MANIFEST_MAX {
                return Err(ReadError::Unreachable(
                    "the repository manifest is larger than 64 KiB".into(),
                ));
            }
            Some(bytes)
        }
        _ => None,
    };
    // Template directories: depth 1 under the folder, in name order; a directory is a template
    // when it holds a `template.yaml` (C-1103).
    let mut dirs: Vec<String> = entries
        .iter()
        .filter(|e| e.kind == "tree")
        .filter_map(|e| rel(prefix, &e.path))
        .filter(|p| !p.contains('/'))
        .map(str::to_owned)
        .collect();
    dirs.sort();
    dirs.dedup();
    let metas: BTreeMap<&str, &crate::domains::skill_libraries::content::TreeEntry> = entries
        .iter()
        .filter_map(|e| {
            let r = rel(prefix, &e.path)?;
            let (d, f) = r.split_once('/')?;
            (f == "template.yaml" && e.kind == "blob").then_some((d, e))
        })
        .collect();
    let truncated = dirs.len() > limits::TEMPLATES_PER_SOURCE_MAX;
    dirs.truncate(limits::TEMPLATES_PER_SOURCE_MAX);
    let mut templates = Vec::with_capacity(dirs.len());
    for dir in dirs {
        match metas.get(dir.as_str()) {
            Some(e) => {
                let bytes = reader.blob(&e.blob).await?;
                let (meta, problem) = if bytes.len() > limits::TEMPLATE_HELPER_FILE_MAX {
                    (
                        None,
                        Some(format!(
                            "template.yaml is larger than {} bytes",
                            limits::TEMPLATE_HELPER_FILE_MAX
                        )),
                    )
                } else {
                    (Some(bytes), None)
                };
                templates.push(DirRead { dir, meta, problem });
            }
            None => templates.push(DirRead {
                dir,
                meta: None,
                problem: Some("template.yaml is missing".into()),
            }),
        }
    }
    Ok(SourceRead {
        resolved,
        manifest,
        templates,
        truncated,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::domains::skill_libraries::content::ReadProfile;
    use aurora_core::limits;

    /// (C-1107) The template profile: its own user agent and the 256 KiB blob cap; the library
    /// default is untouched.
    #[test]
    fn the_template_profile_carries_the_template_values() {
        let p = profile();
        assert!(p.user_agent.starts_with("aurora-templates/"));
        assert_eq!(p.file_max, limits::TEMPLATE_HELPER_FILE_MAX);
        let d = ReadProfile::default();
        assert!(d.user_agent.starts_with("aurora-skill-library/"));
        assert_eq!(d.file_max, limits::LIBRARY_FILE_MAX);
    }
}
