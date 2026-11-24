//! Manifest model, strict parsing and sanitization (09 §3).
mod client_path;
mod model;
mod sanitize;

pub use client_path::*;
pub use model::*;

use crate::Violation;

pub const CONTRACT: u32 = 1;

/// `[a-z][a-z0-9_-]{1,47}`
pub fn valid_id(s: &str) -> bool {
    let b = s.as_bytes();
    (2..=48).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
}

/// One pre-release identifier. Numeric identifiers compare numerically and rank below
/// alphanumeric ones (semver.org §11.4: derived enum order = smaller first).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PreReleaseId {
    Num(u64),
    Alpha(String),
}

/// A parsed `major.minor.patch[-pre][+build]` with real semver precedence: the numeric core
/// first, then the pre-release — a release ranks above any pre-release of the same core,
/// pre-release identifiers compare left to right (numeric numerically and below
/// alphanumeric, a longer list above its prefix), and build metadata is ignored. Plain
/// `x.y.z` orders exactly as before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemVer {
    /// The numeric core.
    pub core: (u64, u64, u64),
    /// `None`: a release (ranks above any pre-release of the same core). An empty list is
    /// the below-everything fallback for rows without a parseable version.
    pre: Option<Vec<PreReleaseId>>,
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        self.core
            .cmp(&other.core)
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Default for SemVer {
    /// Below every parseable version (the old `(0, 0, 0, false)` fallback).
    fn default() -> Self {
        SemVer {
            core: (0, 0, 0),
            pre: Some(Vec::new()),
        }
    }
}

/// Parsed `major.minor.patch[-pre][+build]` with real semver precedence (see [`SemVer`]):
/// `1.0.0-rc.1 < 1.0.0-rc.2 < 1.0.0`, so a new pre-release is a new version.
pub fn parse_semver(s: &str) -> Option<SemVer> {
    let without_build = s.split_once('+').map_or(s, |(a, _)| a);
    let (core, pre) = match without_build.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (without_build, None),
    };
    let core = parse_core(core)?;
    let pre = pre.map(|p| {
        p.split('.')
            .map(|id| {
                if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
                    // A digit-only identifier too large for u64 orders as alphanumeric
                    // (byte-wise), never equal to a different spelling.
                    match id.parse::<u64>() {
                        Ok(n) => PreReleaseId::Num(n),
                        Err(_) => PreReleaseId::Alpha(id.to_owned()),
                    }
                } else {
                    PreReleaseId::Alpha(id.to_owned())
                }
            })
            .collect::<Vec<_>>()
    });
    // An empty pre-release identifier (`1.0.0-`, `1.0.0-rc..1`) is not a version (the
    // template validator's `semver_ok` rejects it too).
    if pre
        .as_ref()
        .is_some_and(|p| p.iter().any(PreReleaseId::is_empty))
    {
        return None;
    }
    Some(SemVer { core, pre })
}

/// The numeric core `major.minor.patch` (no leading zeros, exactly three parts).
fn parse_core(core: &str) -> Option<(u64, u64, u64)> {
    let mut it = core.split('.');
    let mut num = || {
        let p = it.next()?;
        if p.is_empty()
            || (p.len() > 1 && p.starts_with('0'))
            || !p.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        p.parse::<u64>().ok()
    };
    let (a, b, c) = (num()?, num()?, num()?);
    if it.next().is_some() {
        return None;
    }
    Some((a, b, c))
}

fn classify(msg: &str) -> &'static str {
    const MECH: [&str; 5] = [
        "`bearer`",
        "`aws_sigv4`",
        "`git_commit_trailer`",
        "`oauth_token_endpoint`",
        "`registry_token_challenge`",
    ];
    if msg.contains("template_syntax:") {
        "template_syntax"
    } else if msg.contains("template_unknown_namespace:") {
        "template_unknown_namespace"
    } else if msg.contains("unknown variant") {
        if msg.contains("`base64url`") {
            "unknown_decoder"
        } else if MECH.iter().any(|m| msg.contains(m)) {
            "unknown_mechanism"
        } else {
            "unknown_variant"
        }
    } else if msg.contains("unknown field") {
        "unknown_field"
    } else {
        "schema"
    }
}

impl PreReleaseId {
    /// Whether the identifier is empty (`1.0.0-`, `1.0.0-rc..1`) — not a valid version.
    fn is_empty(&self) -> bool {
        matches!(self, PreReleaseId::Alpha(s) if s.is_empty())
    }
}

impl Manifest {
    /// Strict parse: schema (unknown keys/mechanisms/decoders), contract,
    /// ids, versions and `min_core <= running_version`; human strings are
    /// sanitized. Cross-field rules live in [`crate::validate::manifest`].
    pub fn parse(yaml: &str, running_version: &str) -> Result<Self, Vec<Violation>> {
        let mut m: Self = serde_yaml::from_str(yaml).map_err(|e| {
            let full = e.to_string();
            let path = full
                .split_once(": ")
                .map(|(p, _)| p)
                .filter(|p| !p.contains(' '))
                .unwrap_or("");
            vec![Violation::new(classify(&full), path, full.clone())]
        })?;
        let mut v = Vec::new();
        let p = &m.package;
        if p.contract != CONTRACT {
            v.push(Violation::new(
                "bad_contract",
                "/package/contract",
                format!("unsupported contract {}", p.contract),
            ));
        }
        for (path, id) in [("/package/id", &p.id), ("/package/publisher", &p.publisher)] {
            if !valid_id(id) {
                v.push(Violation::new("bad_id", path, format!("invalid id `{id}`")));
            }
        }
        for (i, a) in m.apps.iter().enumerate() {
            if !valid_id(&a.about.id) {
                v.push(Violation::new(
                    "bad_id",
                    format!("/apps/{i}/about/id"),
                    format!("invalid id `{}`", a.about.id),
                ));
            }
        }
        for (i, c) in m.oauth_clients.iter().enumerate() {
            if !valid_id(&c.id) {
                v.push(Violation::new(
                    "bad_id",
                    format!("/oauth_clients/{i}/id"),
                    format!("invalid id `{}`", c.id),
                ));
            }
        }
        if parse_semver(&p.version).is_none() {
            v.push(Violation::new(
                "bad_version",
                "/package/version",
                "not a semver version",
            ));
        }
        match (parse_semver(&p.min_core), parse_semver(running_version)) {
            (None, _) => v.push(Violation::new(
                "bad_version",
                "/package/min_core",
                "not a semver version",
            )),
            (Some(min), Some(run)) if min > run => v.push(Violation::new(
                "min_core_too_new",
                "/package/min_core",
                format!(
                    "requires Aurora {} but running {running_version}",
                    p.min_core
                ),
            )),
            _ => {}
        }
        if !v.is_empty() {
            return Err(v);
        }
        sanitize::manifest(&mut m);
        Ok(m)
    }
}
