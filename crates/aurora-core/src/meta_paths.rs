//! Paths that are never agent content when Aurora reads a git repository, a template archive or
//! a plugin package (owner request, 2026-11-25).
//!
//! A `.github` folder is a repository's own metadata (workflows, issue templates): every
//! discovery or walk over a repository or archive ignores it at any depth. It is never a skill
//! folder, never listed, never counted toward a size or file limit, and never an error — the one
//! predicate here is what every walk site calls, so none of them can drift.

/// `true` when `path` (segments joined by `/`) has a `.github` segment at any depth.
#[must_use]
pub fn has_github_segment(path: &str) -> bool {
    path.split('/').any(|s| s == ".github")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::has_github_segment;

    #[test]
    fn matches_at_any_depth_and_nowhere_else() {
        assert!(has_github_segment(".github"));
        assert!(has_github_segment(".github/workflows/ci.yml"));
        assert!(has_github_segment(
            "service-monitor/.github/workflows/ci.yml"
        ));
        assert!(has_github_segment("skills/x/.github/helper.py"));
        assert!(!has_github_segment("skills/.githubious/SKILL.md"));
        assert!(!has_github_segment("github/workflows/ci.yml"));
        assert!(!has_github_segment("skills/x/SKILL.md"));
        assert!(!has_github_segment(""));
    }
}
