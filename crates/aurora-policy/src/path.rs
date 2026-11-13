//! Path patterns (07 §Matching primitives). Query strings never participate.
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathPatternError {
    #[error("path pattern must start with '/'")]
    NoLeadingSlash,
    #[error("'**' is only allowed as the last segment")]
    DoubleStarNotLast,
    #[error("'*' is only allowed alone or as a trailing segment-prefix wildcard")]
    BadWildcard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    Lit(String),
    Prefix(String),
    One,
    Rest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPattern {
    raw: String,
    segs: Option<Vec<Seg>>,
}

fn split(path: &str) -> Vec<&str> {
    let path = path.split('?').next().unwrap_or("");
    let rest = path.strip_prefix('/').unwrap_or(path);
    // (FIX-109, 07 §Matching primitives) One trailing `/` is not significant: `/a/b` and `/a/b/`
    // are the same path to most frameworks, so a block on `/v1/users` must also stop
    // `/v1/users/` ("doubt only tightens"). A second empty segment (`/a//`) still counts.
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.is_empty() {
        Vec::new()
    } else {
        rest.split('/').collect()
    }
}

fn compile(s: &str) -> Result<Vec<Seg>, PathPatternError> {
    if !s.starts_with('/') {
        return Err(PathPatternError::NoLeadingSlash);
    }
    let parts = split(s);
    let n = parts.len();
    let mut out = Vec::with_capacity(n);
    for (i, p) in parts.into_iter().enumerate() {
        out.push(match p {
            "**" if i + 1 == n => Seg::Rest,
            "**" => return Err(PathPatternError::DoubleStarNotLast),
            "*" => Seg::One,
            _ if p.contains('*') => match p.strip_suffix('*') {
                Some(pre) if !pre.contains('*') => Seg::Prefix(pre.to_owned()),
                _ => return Err(PathPatternError::BadWildcard),
            },
            _ => Seg::Lit(p.to_owned()),
        });
    }
    Ok(out)
}

impl PathPattern {
    pub fn parse(s: &str) -> Result<Self, PathPatternError> {
        Ok(Self {
            raw: s.to_owned(),
            segs: Some(compile(s)?),
        })
    }

    /// Runtime construction: invalid patterns match nothing.
    pub fn lenient(s: &str) -> Self {
        Self {
            raw: s.to_owned(),
            segs: compile(s).ok(),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn is_valid(&self) -> bool {
        self.segs.is_some()
    }

    /// `path` is already normalized; a query suffix is ignored defensively.
    pub fn matches(&self, path: &str) -> bool {
        let Some(segs) = &self.segs else { return false };
        let parts = split(path);
        let mut i = 0;
        for (k, seg) in segs.iter().enumerate() {
            match seg {
                Seg::Rest => return k + 1 == segs.len(),
                Seg::One => {
                    if parts.get(i).is_none_or(|s| s.is_empty()) {
                        return false;
                    }
                }
                Seg::Lit(l) => {
                    if parts.get(i) != Some(&l.as_str()) {
                        return false;
                    }
                }
                Seg::Prefix(pre) => {
                    if !parts.get(i).is_some_and(|s| s.starts_with(pre.as_str())) {
                        return false;
                    }
                }
            }
            i += 1;
        }
        i == parts.len()
    }
}

impl Serialize for PathPattern {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.raw)
    }
}

impl<'de> Deserialize<'de> for PathPattern {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self::lenient(&String::deserialize(d)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn m(p: &str, path: &str) -> bool {
        PathPattern::parse(p).unwrap().matches(path)
    }

    #[test]
    fn examples() {
        assert!(m("/v1/users", "/v1/users"));
        // (FIX-109) A trailing `/` is not significant, on the path and on the pattern.
        assert!(m("/v1/users", "/v1/users/"));
        assert!(m("/v1/users/", "/v1/users"));
        assert!(m("/v1/users/", "/v1/users/"));
        assert!(
            !m("/v1/users", "/v1/users//"),
            "a second empty segment still counts"
        );
        assert!(
            !m("/v1/*", "/v1/"),
            "a wildcard segment still needs a segment"
        );
        assert!(!m("/v1/users", "/v1/users/1"));
        assert!(m("/v1/*/x", "/v1/a/x"));
        assert!(!m("/v1/*/x", "/v1//x"));
        assert!(!m("/v1/*", "/v1"));
        assert!(m("/v1/**", "/v1"));
        assert!(m("/v1/**", "/v1/a/b/c"));
        assert!(!m("/v1/**", "/v2/a"));
        assert!(m("/**", "/"));
        assert!(m("/", "/"));
        assert!(m("/v1/file*", "/v1/file"));
        assert!(m("/v1/file*", "/v1/file.txt"));
        assert!(!m("/v1/file*", "/v1/fil"));
        assert!(!m("/v1/file*", "/v1/file/x"));
        assert!(m("/v1/users", "/v1/users?x=1"));
        assert!(PathPattern::parse("/a/**/b").is_err());
        assert!(PathPattern::parse("/a/f*o").is_err());
        assert!(PathPattern::parse("a").is_err());
        assert!(!PathPattern::lenient("/a/**/b").matches("/a/x/b"));
    }

    proptest! {
        #[test]
        fn never_panics(pat in ".{0,40}", path in ".{0,60}") {
            let _ = PathPattern::lenient(&pat).matches(&path);
        }

        #[test]
        fn literal_matches_itself(segs in proptest::collection::vec("[a-z0-9]{1,6}", 0..5)) {
            let p = format!("/{}", segs.join("/"));
            prop_assert!(PathPattern::parse(&p).unwrap().matches(&p));
            let all = PathPattern::parse("/**").unwrap();
            prop_assert!(all.matches(&p));
        }
    }
}
