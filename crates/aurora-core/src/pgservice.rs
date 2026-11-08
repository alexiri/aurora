//! libpq service names of database connections (P2-DBL-2, C-230).
//!
//! The API names the `[service]` sections of an agent's `pg_service.conf`; the dashboard shows the
//! same name for a connection. The rule is a pure function of the connection label and id, so the
//! two cannot drift:
//!
//! - the base name is the label lowercased, every run of characters outside `[a-z0-9._]` replaced
//!   by one `-`, trimmed of `-` and `.` at both ends, at most [`SERVICE_MAX`] characters; an empty
//!   result is `db`;
//! - when two or more database connections granted to one agent share a base name, each of them
//!   becomes `<base>-<first 8 hex digits of the connection id>`.

use std::collections::BTreeMap;

use uuid::Uuid;

/// Longest base name (the suffix of a collision comes on top).
pub const SERVICE_MAX: usize = 48;

/// The base service name of a label (see the module documentation).
#[must_use]
pub fn service_base(label: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in label.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c);
        } else {
            pending_dash = true;
        }
    }
    let trimmed = out.trim_matches(['.', '-']);
    let clipped: String = trimmed.chars().take(SERVICE_MAX).collect();
    let clipped = clipped.trim_end_matches(['.', '-']);
    if clipped.is_empty() {
        "db".to_owned()
    } else {
        clipped.to_owned()
    }
}

/// The service names of one agent's database connections, in the order given.
#[must_use]
pub fn service_names<'a>(conns: impl IntoIterator<Item = (Uuid, &'a str)>) -> Vec<(Uuid, String)> {
    let bases: Vec<(Uuid, String)> = conns
        .into_iter()
        .map(|(id, label)| (id, service_base(label)))
        .collect();
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, b) in &bases {
        *count.entry(b.as_str()).or_default() += 1;
    }
    let dup: Vec<bool> = bases
        .iter()
        .map(|(_, b)| count.get(b.as_str()).copied().unwrap_or(0) > 1)
        .collect();
    bases
        .iter()
        .zip(dup)
        .map(|((id, b), dup)| {
            let name = if dup {
                format!("{b}-{}", &id.simple().to_string()[..8])
            } else {
                b.clone()
            };
            (*id, name)
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn base_names() {
        assert_eq!(service_base("Prod DB (eu)"), "prod-db-eu");
        assert_eq!(service_base("  --x--  "), "x");
        assert_eq!(service_base("a.b_c"), "a.b_c");
        assert_eq!(service_base("]\n[evil]"), "evil");
        assert_eq!(service_base("日本語"), "db");
        assert_eq!(service_base(""), "db");
        assert_eq!(service_base(&"a".repeat(100)).len(), SERVICE_MAX);
    }

    #[test]
    fn base_names_only_use_safe_characters() {
        for l in ["a=b", "x y\tz", "q\"q", "[s]", "../../etc", "ÄÖ"] {
            let s = service_base(l);
            assert!(
                s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
                "{l:?} -> {s:?}"
            );
        }
    }

    #[test]
    fn collisions_get_the_id_prefix_on_every_member() {
        let a = Uuid::parse_str("11111111-2222-7333-8444-555555555555").unwrap();
        let b = Uuid::parse_str("aaaaaaaa-2222-7333-8444-555555555555").unwrap();
        let c = Uuid::now_v7();
        let names = service_names([(a, "Prod"), (b, "prod"), (c, "other")]);
        assert_eq!(names[0].1, "prod-11111111");
        assert_eq!(names[1].1, "prod-aaaaaaaa");
        assert_eq!(names[2].1, "other");
    }
}
