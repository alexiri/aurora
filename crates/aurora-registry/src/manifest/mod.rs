//! Manifest model, strict parsing and sanitization (09 §3).
mod model;
mod sanitize;

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

/// Parsed `major.minor.patch[-pre][+build]`; ordering puts pre-releases first.
pub fn parse_semver(s: &str) -> Option<(u64, u64, u64, bool)> {
    let (core, pre) = match s.split_once('+').map_or(s, |(a, _)| a).split_once('-') {
        Some((c, p)) => (c, !p.is_empty()),
        None => (s.split_once('+').map_or(s, |(a, _)| a), false),
    };
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
    Some((a, b, c, !pre))
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
