//! Self-hosted app instances: the base path of a registration and the one place `{instance}`
//! resolves to a URL (D-74, D-231, `09 §3.4 Instance-bound hosts`).
//!
//! A registration is a host plus an optional base path (`its.cern.ch` + `/wiki`). Everything a
//! manifest writes as `https://{instance}/…` means `https://<host><base_path>/…` for that
//! registration; the gateway judges manifest paths on the **app path**, the request path with the
//! base path removed.

use crate::manifest::INSTANCE_HOST;

/// Longest accepted base path, in characters.
pub const BASE_PATH_MAX: usize = 128;
/// Most segments a base path may have.
pub const BASE_PATH_SEGMENTS_MAX: usize = 8;
/// Longest segment, in characters.
pub const BASE_PATH_SEGMENT_MAX: usize = 64;

/// Why a base path is refused (`instance_base_path_invalid`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BasePathError {
    NoLeadingSlash,
    TooLong,
    TooManySegments,
    EmptySegment,
    SegmentTooLong,
    BadCharacter,
    DotSegment,
}

impl std::fmt::Display for BasePathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoLeadingSlash => "The path must start with /.",
            Self::TooLong => "The path is too long (at most 128 characters).",
            Self::TooManySegments => "The path has too many segments (at most 8).",
            Self::EmptySegment => "The path has an empty segment.",
            Self::SegmentTooLong => "A path segment is too long (at most 64 characters).",
            Self::BadCharacter => {
                "A path segment may contain only letters, digits and the characters . _ ~ -."
            }
            Self::DotSegment => "A path segment may not be . or ..",
        })
    }
}

/// Normalize a registration's base path: `""` and `/` mean no base path, one trailing `/` is
/// dropped, case is kept. Anything else must start with `/`, have 1–8 segments of 1–64
/// characters from `[A-Za-z0-9._~-]`, none of them `.` or `..`, and be at most 128 characters.
pub fn normalize_base_path(raw: &str) -> Result<Option<String>, BasePathError> {
    let raw = raw.trim();
    if raw.is_empty() || raw == "/" {
        return Ok(None);
    }
    if !raw.starts_with('/') {
        return Err(BasePathError::NoLeadingSlash);
    }
    let p = raw.strip_suffix('/').unwrap_or(raw);
    if p.chars().count() > BASE_PATH_MAX {
        return Err(BasePathError::TooLong);
    }
    let segs: Vec<&str> = p[1..].split('/').collect();
    if segs.len() > BASE_PATH_SEGMENTS_MAX {
        return Err(BasePathError::TooManySegments);
    }
    for s in &segs {
        if s.is_empty() {
            return Err(BasePathError::EmptySegment);
        }
        if s.chars().count() > BASE_PATH_SEGMENT_MAX {
            return Err(BasePathError::SegmentTooLong);
        }
        if !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
        {
            return Err(BasePathError::BadCharacter);
        }
        if *s == "." || *s == ".." {
            return Err(BasePathError::DotSegment);
        }
    }
    Ok(Some(p.to_owned()))
}

/// Do two registrations on one host overlap? Either has no base path (it covers every path), they
/// are equal, or one is a segment prefix of the other (`/wiki` overlaps `/wiki/x`, not `/wiki2`).
#[must_use]
pub fn overlaps(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => covers(Some(a), b) || covers(Some(b), a),
        _ => true,
    }
}

/// Does a registration with `base` cover `path`? No base path covers everything; otherwise `path`
/// is the base path itself or continues it at a segment boundary. Case-sensitive.
#[must_use]
pub fn covers(base: Option<&str>, path: &str) -> bool {
    match base {
        None => true,
        Some(b) => path == b || path.strip_prefix(b).is_some_and(|r| r.starts_with('/')),
    }
}

/// The app path: `path` with the base path removed (`/wiki/rest/api/2/myself` →
/// `/rest/api/2/myself`; the base path itself → `/`). `path` must be covered by `base`.
#[must_use]
pub fn app_path<'a>(base: Option<&str>, path: &'a str) -> &'a str {
    match base.and_then(|b| path.strip_prefix(b)) {
        None => path,
        Some("") => "/",
        Some(rest) => rest,
    }
}

/// Among registrations on one host, the one whose base path covers `path` — the longest base path
/// wins. `base_of` reads a registration's base path.
pub fn select<'a, T>(
    instances: &'a [T],
    path: &str,
    base_of: impl Fn(&T) -> Option<&str>,
) -> Option<&'a T> {
    instances
        .iter()
        .filter(|i| covers(base_of(i), path))
        .max_by_key(|i| base_of(i).map_or(0, str::len))
}

/// One registration's address: its host and base path. The single place `{instance}` becomes a
/// URL (registration probe, connect probes, OAuth authorize and token URLs, token refresh and
/// mint, skill-library fetch, offline fixtures).
#[derive(Debug, Clone, Copy)]
pub struct InstanceBase<'a> {
    pub host: &'a str,
    pub base_path: Option<&'a str>,
}

impl<'a> InstanceBase<'a> {
    #[must_use]
    pub fn new(host: &'a str, base_path: Option<&'a str>) -> Self {
        Self { host, base_path }
    }

    /// `https://<host><base_path>` (no trailing slash).
    #[must_use]
    pub fn base_url(&self) -> String {
        format!("https://{}{}", self.host, self.base_path.unwrap_or(""))
    }

    fn host_ok(&self) -> bool {
        let h = self.host;
        !h.is_empty()
            && h.parse::<std::net::IpAddr>().is_err()
            && h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
            && !h.starts_with('.')
            && !h.contains("..")
    }

    fn base_ok(&self) -> bool {
        self.base_path
            .is_none_or(|b| normalize_base_path(b).is_ok_and(|n| n.as_deref() == Some(b)))
    }

    /// Resolve a manifest URL written `https://{instance}…` to this registration's URL:
    /// `https://<host><base_path>…`. `None` (fail closed) unless the template has `{instance}`
    /// exactly once as the whole host right after `https://`, and the host and base path are
    /// well formed (DNS characters only, no IP literal; a normalized base path).
    #[must_use]
    pub fn resolve(&self, raw_template: &str) -> Option<String> {
        let rest = raw_template.strip_prefix("https://{instance}")?;
        if rest.contains(INSTANCE_HOST) || !(rest.is_empty() || rest.starts_with(['/', '?', '#'])) {
            return None;
        }
        if !self.host_ok() || !self.base_ok() {
            return None;
        }
        Some(format!("{}{}", self.base_url(), rest))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_validates() {
        assert_eq!(normalize_base_path("/wiki/"), Ok(Some("/wiki".into())));
        assert_eq!(normalize_base_path("/"), Ok(None));
        assert_eq!(normalize_base_path(""), Ok(None));
        assert_eq!(normalize_base_path("/Wiki"), Ok(Some("/Wiki".into())));
        assert_eq!(
            normalize_base_path("/a/b.c_d~e-f"),
            Ok(Some("/a/b.c_d~e-f".into()))
        );
        assert_eq!(
            normalize_base_path("wiki"),
            Err(BasePathError::NoLeadingSlash)
        );
        assert_eq!(
            normalize_base_path("//wiki"),
            Err(BasePathError::EmptySegment)
        );
        assert_eq!(
            normalize_base_path("/a//b"),
            Err(BasePathError::EmptySegment)
        );
        assert_eq!(
            normalize_base_path("/a//"),
            Err(BasePathError::EmptySegment)
        );
        assert_eq!(normalize_base_path("/."), Err(BasePathError::DotSegment));
        assert_eq!(normalize_base_path("/a/.."), Err(BasePathError::DotSegment));
        assert_eq!(
            normalize_base_path("/a b"),
            Err(BasePathError::BadCharacter)
        );
        assert_eq!(
            normalize_base_path("/a?x=1"),
            Err(BasePathError::BadCharacter)
        );
        assert_eq!(
            normalize_base_path("/a%2Fb"),
            Err(BasePathError::BadCharacter)
        );
        assert_eq!(
            normalize_base_path("/wiki:8080"),
            Err(BasePathError::BadCharacter)
        );
        assert_eq!(
            normalize_base_path(&format!("/{}", "a".repeat(65))),
            Err(BasePathError::SegmentTooLong)
        );
        assert_eq!(
            normalize_base_path("/1/2/3/4/5/6/7/8/9"),
            Err(BasePathError::TooManySegments)
        );
        assert!(normalize_base_path("/1/2/3/4/5/6/7/8").is_ok());
        let long = format!(
            "/{}",
            ["a".repeat(60), "b".repeat(60), "c".repeat(10)].join("/")
        );
        assert_eq!(normalize_base_path(&long), Err(BasePathError::TooLong));
    }

    #[test]
    fn overlap_is_a_segment_prefix() {
        assert!(overlaps(None, None));
        assert!(overlaps(None, Some("/wiki")));
        assert!(overlaps(Some("/wiki"), None));
        assert!(overlaps(Some("/wiki"), Some("/wiki")));
        assert!(overlaps(Some("/wiki"), Some("/wiki/x")));
        assert!(overlaps(Some("/wiki/x"), Some("/wiki")));
        assert!(!overlaps(Some("/wiki"), Some("/wiki2")));
        assert!(!overlaps(Some("/wiki"), Some("/docs")));
        assert!(!overlaps(Some("/wiki"), Some("/WIKI")));
    }

    #[test]
    fn covers_and_app_path() {
        assert!(covers(None, "/anything"));
        assert!(covers(Some("/wiki"), "/wiki"));
        assert!(covers(Some("/wiki"), "/wiki/rest/api/2/myself"));
        assert!(!covers(Some("/wiki"), "/wiki2/rest"));
        assert!(!covers(Some("/wiki"), "/WIKI/rest"));
        assert!(!covers(Some("/wiki"), "/rest/api/2/myself"));
        assert!(!covers(Some("/wiki"), "/"));
        assert_eq!(
            app_path(Some("/wiki"), "/wiki/rest/api/2/myself"),
            "/rest/api/2/myself"
        );
        assert_eq!(app_path(Some("/wiki"), "/wiki"), "/");
        assert_eq!(app_path(Some("/wiki"), "/wiki/"), "/");
        assert_eq!(app_path(None, "/a/b"), "/a/b");
    }

    #[test]
    fn the_longest_base_path_wins() {
        let v = [Some("/a"), None, Some("/a/b")];
        let pick = |p: &str| select(&v, p, |b| *b).copied();
        assert_eq!(pick("/a/b/c"), Some(Some("/a/b")));
        assert_eq!(pick("/a/x"), Some(Some("/a")));
        assert_eq!(pick("/z"), Some(None));
        let only = [Some("/wiki")];
        assert!(select(&only, "/z", |b| *b).is_none());
    }

    #[test]
    fn resolves_the_placeholder_to_host_and_base_path() {
        let b = InstanceBase::new("its.cern.ch", Some("/wiki"));
        assert_eq!(b.base_url(), "https://its.cern.ch/wiki");
        assert_eq!(
            b.resolve("https://{instance}/rest/api/2/serverInfo")
                .as_deref(),
            Some("https://its.cern.ch/wiki/rest/api/2/serverInfo")
        );
        assert_eq!(
            b.resolve("https://{instance}").as_deref(),
            Some("https://its.cern.ch/wiki")
        );
        assert_eq!(
            b.resolve("https://{instance}?x=1").as_deref(),
            Some("https://its.cern.ch/wiki?x=1")
        );
        let plain = InstanceBase::new("forge.example.com", None);
        assert_eq!(
            plain.resolve("https://{instance}/oauth/token").as_deref(),
            Some("https://forge.example.com/oauth/token")
        );
        // fail closed
        for bad in [
            "http://{instance}/x",
            "https://evil.example/{instance}",
            "https://{instance}.evil.example/x",
            "https://{instance}@evil.example/x",
            "https://{instance}:8443/x",
            "https://{instance}/{instance}",
            "https://x",
        ] {
            assert_eq!(b.resolve(bad), None, "{bad}");
        }
        assert_eq!(
            InstanceBase::new("10.0.0.1", None).resolve("https://{instance}/x"),
            None
        );
        assert_eq!(
            InstanceBase::new("a/b.example", None).resolve("https://{instance}/x"),
            None
        );
        assert_eq!(
            InstanceBase::new("a.example", Some("/x/../y")).resolve("https://{instance}/x"),
            None
        );
        assert_eq!(
            InstanceBase::new("a.example", Some("/x/")).resolve("https://{instance}/x"),
            None
        );
    }
}
