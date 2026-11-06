//! Host patterns (07 §Matching primitives).
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatternError {
    #[error("pattern is empty")]
    Empty,
    #[error("pattern has a trailing dot")]
    TrailingDot,
    #[error("pattern contains more than one '*'")]
    TooManyWildcards,
    #[error("invalid port")]
    BadPort,
    #[error("pattern contains invalid characters")]
    BadChars,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Shape {
    Exact(String),
    Wild { prefix: String, suffix: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Parsed {
    shape: Shape,
    port: Option<u16>,
}

/// A host pattern. Deserialization is lenient: a pattern that fails to parse
/// is kept (for display) but matches nothing at runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPattern {
    raw: String,
    parsed: Option<Parsed>,
}

impl HostPattern {
    /// Strict parse (authoring). Case is folded to lowercase.
    pub fn parse(s: &str) -> Result<Self, PatternError> {
        let parsed = parse_inner(s)?;
        Ok(Self {
            raw: s.to_ascii_lowercase(),
            parsed: Some(parsed),
        })
    }

    /// Runtime construction: invalid patterns match nothing.
    pub fn lenient(s: &str) -> Self {
        match parse_inner(s) {
            Ok(p) => Self {
                raw: s.to_ascii_lowercase(),
                parsed: Some(p),
            },
            Err(_) => Self {
                raw: s.to_owned(),
                parsed: None,
            },
        }
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn is_valid(&self) -> bool {
        self.parsed.is_some()
    }

    /// `host` has no port and no trailing dot; comparison is case-insensitive.
    pub fn matches(&self, host: &str, port: u16) -> bool {
        let Some(p) = &self.parsed else { return false };
        if p.port.is_some_and(|pp| pp != port) {
            return false;
        }
        match &p.shape {
            Shape::Exact(e) => host.eq_ignore_ascii_case(e),
            Shape::Wild { prefix, suffix } => {
                let h = host.as_bytes();
                let need = prefix.len() + suffix.len();
                h.len() > need
                    && h[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
                    && h[h.len() - suffix.len()..].eq_ignore_ascii_case(suffix.as_bytes())
            }
        }
    }
}

fn parse_inner(s: &str) -> Result<Parsed, PatternError> {
    if s.is_empty() {
        return Err(PatternError::Empty);
    }
    let (host, port) = match s.rsplit_once(':') {
        Some((h, p)) => {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return Err(PatternError::BadPort);
            }
            (
                h,
                Some(p.parse::<u16>().map_err(|_| PatternError::BadPort)?),
            )
        }
        None => (s, None),
    };
    if host.is_empty() {
        return Err(PatternError::Empty);
    }
    if host.ends_with('.') {
        return Err(PatternError::TrailingDot);
    }
    if host
        .bytes()
        .any(|b| b.is_ascii_whitespace() || b == b'/' || b == b':' || b == b'@')
    {
        return Err(PatternError::BadChars);
    }
    let host = host.to_ascii_lowercase();
    let shape = match host.matches('*').count() {
        0 => Shape::Exact(host),
        1 => {
            let (pre, suf) = host.split_once('*').unwrap_or((&host, ""));
            Shape::Wild {
                prefix: pre.to_owned(),
                suffix: suf.to_owned(),
            }
        }
        _ => return Err(PatternError::TooManyWildcards),
    };
    Ok(Parsed { shape, port })
}

impl Serialize for HostPattern {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.raw)
    }
}

impl<'de> Deserialize<'de> for HostPattern {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Self::lenient(&s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn examples() {
        let p = HostPattern::parse("*.example.com").unwrap();
        assert!(p.matches("a.example.com", 443));
        assert!(p.matches("a.b.example.com", 443));
        assert!(!p.matches("example.com", 443));
        assert!(!p.matches(".example.com", 443));
        assert!(!p.matches("evilexample.com", 443));
        let p = HostPattern::parse("api.example.com:8443").unwrap();
        assert!(p.matches("API.Example.com", 8443));
        assert!(!p.matches("api.example.com", 443));
        let p = HostPattern::parse("api.*").unwrap();
        assert!(p.matches("api.x", 1));
        assert!(!p.matches("api.", 1));
        assert!(HostPattern::parse("a.*.*").is_err());
        assert!(HostPattern::parse("a.com.").is_err());
        assert!(HostPattern::parse("").is_err());
        assert!(HostPattern::parse("a.com:x").is_err());
        let bad = HostPattern::lenient("*.*.com");
        assert!(!bad.is_valid());
        assert!(!bad.matches("a.b.com", 443));
    }

    #[test]
    fn serde_lenient() {
        let p: HostPattern = serde_json::from_str("\"*.a.*\"").unwrap();
        assert!(!p.matches("x.a.y", 80));
    }

    proptest! {
        #[test]
        fn never_panics(pat in ".{0,40}", host in ".{0,40}", port in any::<u16>()) {
            let p = HostPattern::lenient(&pat);
            let _ = p.matches(&host, port);
        }

        #[test]
        fn wildcard_needs_strictly_longer(pre in "[a-z]{0,5}", suf in "[a-z.]{0,7}[a-z]") {
            let p = HostPattern::parse(&format!("{pre}*{suf}")).unwrap();
            let exact = format!("{pre}{suf}");
            prop_assert!(!p.matches(&exact, 1));
            let longer = format!("{pre}x{suf}");
            prop_assert!(p.matches(&longer, 1));
        }
    }
}
