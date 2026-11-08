//! Host claims and conflict detection (09 §7).
//!
//! A *claim* is `(host pattern, path prefix)` from an `apps[].hosts[]` entry.
//!
//! * `claim_key` = `"<host pattern lowercased>|<path prefix>"`; an absent
//!   prefix is normalised to `/` (equivalent: both match every path).
//! * `fingerprint` = hex sha256 of `claim_key` + `\n` + the canonical JSON of
//!   the rule's injection-relevant definition (`inject`, `tenant`,
//!   `rewrite_host`, `finalize`, `transform`, `intercept`, `passthrough`).
//!   Changing what a claim does therefore changes its fingerprint.
//! * Conflicts are only reported between claims of *different packages*
//!   (a package may overlap itself).
//! * Overlap: host patterns overlap iff some host (and port) matches both;
//!   this is exact for the single-`*` pattern grammar. Path prefixes overlap
//!   iff one is a string prefix of the other. That is conservative: `/a` and
//!   `/ab` are reported as overlapping although no segment-wise path matches
//!   both.
//! * Decisions apply to a [`ConflictGroup`]: the connected component of
//!   overlapping claims. Its `claim_key` is the smallest member key and its
//!   `fingerprint` is the sha256 over the sorted member lines
//!   `package|app|claim_key|claim_fingerprint`. A decision is valid only if
//!   both match and its winner is a member package.
use std::collections::BTreeSet;

use aurora_policy::HostPattern;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::manifest::{App, HostRule, INSTANCE_HOST, Manifest};
use crate::snapshot::{canonical_bytes, hex};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Claim {
    pub app_id: String,
    pub host_pattern: String,
    pub path_prefix: String,
    pub claim_key: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub claim_key_a: String,
    pub claim_key_b: String,
    pub package_a: String,
    pub package_b: String,
    pub app_a: String,
    pub app_b: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Member {
    pub package_id: String,
    pub app_id: String,
    pub claim_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictGroup {
    pub claim_key: String,
    pub fingerprint: String,
    pub members: Vec<Member>,
}

pub fn claim_key(host: &str, path_prefix: Option<&str>) -> String {
    format!(
        "{}|{}",
        HostPattern::lenient(host).as_str(),
        path_prefix.filter(|p| !p.is_empty()).unwrap_or("/")
    )
}

pub fn rule_fingerprint(key: &str, r: &HostRule) -> String {
    let def = json!({
        "inject": r.inject, "tenant": r.tenant, "rewrite_host": r.rewrite_host,
        "finalize": r.finalize, "transform": r.transform,
        "intercept": r.intercept, "passthrough": r.passthrough,
    });
    let mut h = Sha256::new();
    h.update(key.as_bytes());
    h.update(b"\n");
    h.update(canonical_bytes(&def));
    hex(&h.finalize())
}

/// The static host a rule claims: its own host, or for an `{instance}` rule the app's
/// `instances.default_host` (none without one: instance hosts are registered, not claimed).
pub fn static_host<'a>(app: &'a App, r: &'a HostRule) -> Option<&'a str> {
    if r.matcher.host == INSTANCE_HOST {
        app.instances.as_ref()?.default_host.as_deref()
    } else {
        Some(&r.matcher.host)
    }
}

pub fn claims(m: &Manifest) -> Vec<Claim> {
    m.apps
        .iter()
        .flat_map(|a| {
            a.hosts.iter().filter_map(|r| {
                let host = static_host(a, r)?;
                let key = claim_key(host, r.matcher.path_prefix.as_deref());
                Some(Claim {
                    app_id: a.about.id.clone(),
                    host_pattern: HostPattern::lenient(host).as_str().to_owned(),
                    path_prefix: r
                        .matcher
                        .path_prefix
                        .clone()
                        .filter(|p| !p.is_empty())
                        .unwrap_or_else(|| "/".to_owned()),
                    fingerprint: rule_fingerprint(&key, r),
                    claim_key: key,
                })
            })
        })
        .collect()
}

/// `(host, port?)`; wildcard split into `(prefix, suffix)`.
enum Shape<'a> {
    Exact(&'a str),
    Wild(&'a str, &'a str),
}

fn split(p: &str) -> (Shape<'_>, Option<&str>) {
    let (h, port) = match p.rsplit_once(':') {
        Some((h, port)) => (h, Some(port)),
        None => (p, None),
    };
    match h.split_once('*') {
        Some((a, b)) => (Shape::Wild(a, b), port),
        None => (Shape::Exact(h), port),
    }
}

/// True iff some host (and port) matches both patterns. Invalid patterns
/// overlap nothing. Wildcards match one-or-more characters (as
/// [`HostPattern::matches`]).
pub fn hosts_overlap(a: &str, b: &str) -> bool {
    let (pa, pb) = (HostPattern::lenient(a), HostPattern::lenient(b));
    if !pa.is_valid() || !pb.is_valid() {
        return false;
    }
    let (sa, porta) = split(pa.as_str());
    let (sb, portb) = split(pb.as_str());
    if let (Some(x), Some(y)) = (porta, portb)
        && x != y
    {
        return false;
    }
    match (sa, sb) {
        (Shape::Exact(x), Shape::Exact(y)) => x == y,
        (Shape::Exact(x), Shape::Wild(p, s)) | (Shape::Wild(p, s), Shape::Exact(x)) => {
            x.len() > p.len() + s.len() && x.starts_with(p) && x.ends_with(s)
        }
        (Shape::Wild(p1, s1), Shape::Wild(p2, s2)) => {
            let (lp, sp) = if p1.len() >= p2.len() {
                (p1, p2)
            } else {
                (p2, p1)
            };
            let (ls, ss) = if s1.len() >= s2.len() {
                (s1, s2)
            } else {
                (s2, s1)
            };
            lp.starts_with(sp) && ls.ends_with(ss)
        }
    }
}

pub fn paths_overlap(a: &str, b: &str) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

fn overlap(a: &Claim, b: &Claim) -> bool {
    hosts_overlap(&a.host_pattern, &b.host_pattern) && paths_overlap(&a.path_prefix, &b.path_prefix)
}

/// Pairwise conflicts between claims of different packages, sorted.
pub fn conflicts(all: &[(String, Claim)]) -> Vec<Conflict> {
    let mut out = BTreeSet::new();
    for (i, (pa, a)) in all.iter().enumerate() {
        for (pb, b) in &all[i + 1..] {
            if pa != pb && overlap(a, b) {
                let (x, y) = if (pa, &a.app_id, &a.claim_key) <= (pb, &b.app_id, &b.claim_key) {
                    ((pa, a), (pb, b))
                } else {
                    ((pb, b), (pa, a))
                };
                out.insert((
                    x.1.claim_key.clone(),
                    y.1.claim_key.clone(),
                    x.0.clone(),
                    y.0.clone(),
                    x.1.app_id.clone(),
                    y.1.app_id.clone(),
                ));
            }
        }
    }
    out.into_iter()
        .map(
            |(claim_key_a, claim_key_b, package_a, package_b, app_a, app_b)| Conflict {
                claim_key_a,
                claim_key_b,
                package_a,
                package_b,
                app_a,
                app_b,
            },
        )
        .collect()
}

/// Connected components of the cross-package overlap graph, sorted by key.
pub fn conflict_groups(all: &[(String, Claim)]) -> Vec<ConflictGroup> {
    let n = all.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    let mut in_conflict = vec![false; n];
    for i in 0..n {
        for j in i + 1..n {
            if all[i].0 != all[j].0 && overlap(&all[i].1, &all[j].1) {
                in_conflict[i] = true;
                in_conflict[j] = true;
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                parent[ri] = rj;
            }
        }
    }
    let mut comps: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for i in (0..n).filter(|i| in_conflict[*i]) {
        let r = find(&mut parent, i);
        comps.entry(r).or_default().push(i);
    }
    let mut groups: Vec<ConflictGroup> = comps
        .into_values()
        .map(|idx| {
            let mut lines: Vec<(Member, &str)> = idx
                .iter()
                .map(|&i| {
                    (
                        Member {
                            package_id: all[i].0.clone(),
                            app_id: all[i].1.app_id.clone(),
                            claim_key: all[i].1.claim_key.clone(),
                        },
                        all[i].1.fingerprint.as_str(),
                    )
                })
                .collect();
            lines.sort();
            lines.dedup();
            let mut h = Sha256::new();
            for (m, fp) in &lines {
                h.update(format!(
                    "{}|{}|{}|{fp}\n",
                    m.package_id, m.app_id, m.claim_key
                ));
            }
            let members: Vec<Member> = lines.into_iter().map(|(m, _)| m).collect();
            ConflictGroup {
                claim_key: members
                    .iter()
                    .map(|m| m.claim_key.clone())
                    .min()
                    .unwrap_or_default(),
                fingerprint: hex(&h.finalize()),
                members,
            }
        })
        .collect();
    groups.sort_by(|a, b| (&a.claim_key, &a.fingerprint).cmp(&(&b.claim_key, &b.fingerprint)));
    groups
}
