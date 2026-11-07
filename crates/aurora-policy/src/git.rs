//! Git smart-HTTP operation classification (07 §Matching primitives).
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitOp {
    Push,
    Fetch,
}

fn service_op(s: &str) -> Option<GitOp> {
    match s {
        "git-receive-pack" => Some(GitOp::Push),
        "git-upload-pack" => Some(GitOp::Fetch),
        _ => None,
    }
}

/// `path` is normalized; `query` is the raw query string without `?`.
/// Conflicting `service` parameters resolve to `Push` (doubt tightens).
pub fn classify(path: &str, query: Option<&str>) -> Option<GitOp> {
    if path.ends_with("/git-receive-pack") {
        return Some(GitOp::Push);
    }
    if path.ends_with("/git-upload-pack") {
        return Some(GitOp::Fetch);
    }
    if path.ends_with("/info/refs") {
        let mut found = None;
        for kv in query.unwrap_or("").split('&') {
            if let Some(op) = kv.strip_prefix("service=").and_then(service_op) {
                if op == GitOp::Push {
                    return Some(GitOp::Push);
                }
                found = Some(op);
            }
        }
        return found;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table() {
        let c = classify;
        assert_eq!(c("/o/r.git/git-receive-pack", None), Some(GitOp::Push));
        assert_eq!(c("/o/r.git/git-upload-pack", None), Some(GitOp::Fetch));
        assert_eq!(
            c("/o/r.git/info/refs", Some("service=git-receive-pack")),
            Some(GitOp::Push)
        );
        assert_eq!(
            c("/o/r.git/info/refs", Some("service=git-upload-pack")),
            Some(GitOp::Fetch)
        );
        assert_eq!(
            c(
                "/o/r.git/info/refs",
                Some("service=git-upload-pack&service=git-receive-pack")
            ),
            Some(GitOp::Push)
        );
        assert_eq!(c("/o/r.git/info/refs", None), None);
        assert_eq!(c("/o/r.git/info/refs", Some("service=other")), None);
        assert_eq!(c("/o/r", Some("service=git-receive-pack")), None);
    }
}
