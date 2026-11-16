//! Request paths of the Files routes (C-899): lexical validation before any SFTP call, and the
//! scope predicates applied to a *resolved* path. Pure functions only.
use aurora_core::ApiError;
use aurora_core::limits::FILES_PATH_MAX;

pub const ROOT: &str = "/workspace";
pub const AURORA_DIR: &str = "/workspace/.aurora";

pub fn invalid_path(why: &str) -> ApiError {
    ApiError::new(
        axum::http::StatusCode::BAD_REQUEST,
        "invalid_path",
        format!("Invalid path: {why}."),
    )
}

/// C-899: valid UTF-8 (a `&str` already), absolute, `/workspace` or under it, no empty, `.` or
/// `..` component, no NUL, one trailing `/` stripped, each component ≤ 255 bytes, the whole ≤
/// `FILES_PATH_MAX` bytes. Returns the normalised path.
pub fn validate(raw: &str) -> Result<String, ApiError> {
    if raw.len() > FILES_PATH_MAX {
        return Err(invalid_path("it is too long"));
    }
    if raw.contains('\0') {
        return Err(invalid_path("it contains a NUL byte"));
    }
    let p = raw.strip_suffix('/').unwrap_or(raw);
    let Some(rest) = p.strip_prefix(ROOT) else {
        return Err(invalid_path("it must be under /workspace"));
    };
    if rest.is_empty() {
        return Ok(ROOT.to_owned());
    }
    let Some(rest) = rest.strip_prefix('/') else {
        return Err(invalid_path("it must be under /workspace"));
    };
    for c in rest.split('/') {
        if c.is_empty() {
            return Err(invalid_path("it has an empty component"));
        }
        if c == "." || c == ".." {
            return Err(invalid_path("`.` and `..` are not allowed"));
        }
        if c.len() > 255 {
            return Err(invalid_path("a name is longer than 255 bytes"));
        }
    }
    Ok(p.to_owned())
}

/// A resolved path is inside the scope (`/workspace` or below).
pub fn in_scope(resolved: &str) -> bool {
    resolved == ROOT
        || resolved
            .strip_prefix(ROOT)
            .is_some_and(|r| r.starts_with('/'))
}

/// A resolved path equal to or under `/workspace/.aurora`, or `/workspace` itself for the
/// operations that may not touch it (callers decide which).
pub fn is_aurora(resolved: &str) -> bool {
    resolved == AURORA_DIR
        || resolved
            .strip_prefix(AURORA_DIR)
            .is_some_and(|r| r.starts_with('/'))
}

/// `(parent, name)` of a validated non-root path.
pub fn split(path: &str) -> Option<(&str, &str)> {
    let i = path.rfind('/')?;
    let (parent, name) = (&path[..i], &path[i + 1..]);
    (!name.is_empty()).then_some((if parent.is_empty() { "/" } else { parent }, name))
}

pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// `inner` is `outer` or below it (component-wise).
pub fn is_within(inner: &str, outer: &str) -> bool {
    inner == outer
        || inner
            .strip_prefix(outer)
            .is_some_and(|r| r.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_scope_and_strips_one_trailing_slash() {
        assert_eq!(validate("/workspace").unwrap(), "/workspace");
        assert_eq!(validate("/workspace/").unwrap(), "/workspace");
        assert_eq!(validate("/workspace/a/b/").unwrap(), "/workspace/a/b");
        assert_eq!(
            validate("/workspace/.hidden").unwrap(),
            "/workspace/.hidden"
        );
    }

    #[test]
    fn refuses_the_documented_shapes() {
        let long_component = format!("/workspace/{}", "a".repeat(256));
        let long_path = format!("/workspace/{}", "a/".repeat(2100));
        for bad in [
            "",
            "workspace/a",
            "/etc/passwd",
            "/workspaces",
            "/workspace2/a",
            "/workspace//a",
            "/workspace/a//b",
            "/workspace//",
            "/workspace/./a",
            "/workspace/a/..",
            "/workspace/../etc",
            "/workspace/a\0b",
            long_component.as_str(),
            long_path.as_str(),
        ] {
            let e = validate(bad).unwrap_err();
            assert_eq!(e.code(), "invalid_path", "{bad:?}");
        }
    }

    #[test]
    fn scope_predicates() {
        assert!(in_scope("/workspace"));
        assert!(in_scope("/workspace/x"));
        assert!(!in_scope("/workspace2"));
        assert!(!in_scope("/etc"));
        assert!(is_aurora("/workspace/.aurora"));
        assert!(is_aurora("/workspace/.aurora/x/y"));
        assert!(!is_aurora("/workspace/.aurorax"));
        assert!(!is_aurora("/workspace"));
        assert_eq!(split("/workspace/a/b"), Some(("/workspace/a", "b")));
        assert_eq!(
            split("/workspace"),
            Some(("", "workspace")).map(|_| ("/", "workspace"))
        );
        assert!(is_within("/workspace/a/b", "/workspace/a"));
        assert!(!is_within("/workspace/ab", "/workspace/a"));
    }
}
