//! Group wildcard "true superset" check (09 §3.4 / §6).
//!
//! The spec gives no algorithm. Definition used here (reported as an
//! ambiguity): a wildcard has no matcher of its own; it stands for the union
//! of its group's items, `U = ⋃ (host_i, path_i, methods_i)`. It is a *true
//! superset of its group* iff
//!  1. the group has at least one item (otherwise the wildcard grants
//!     something no item describes),
//!  2. every item is well formed (host and path patterns parse; methods are
//!     plain HTTP tokens), so `U` is well defined and each item is a member
//!     of it by construction, and
//!  3. `U` stays inside the app's declared surface: for every item there is an
//!     `hosts[]` rule whose host pattern covers the item's host and whose
//!     `path_prefix` (if any) is a literal prefix of the item's path, so the
//!     wildcard never grants more than the app's own host rules reach.
//!
//! Violation code: `wildcard_not_superset`.
use aurora_policy::{HostPattern, PathPattern};

use crate::Violation;
use crate::manifest::{App, Manifest, ToolGroup};

pub fn wildcards(m: &Manifest) -> Vec<Violation> {
    let mut out = Vec::new();
    for (ai, app) in m.apps.iter().enumerate() {
        let Some(t) = &app.tools else { continue };
        for (name, g) in [("read", &t.read), ("write", &t.write)] {
            if let Some(g) = g.as_ref().filter(|g| g.wildcard.is_some()) {
                group(app, g, &format!("/apps/{ai}/tools/{name}"), &mut out);
            }
        }
    }
    out
}

fn covered(app: &App, host: &HostPattern, path: &str) -> bool {
    app.hosts.iter().any(|h| {
        let Ok(c) = HostPattern::parse(&h.matcher.host) else {
            return false;
        };
        let host_ok = c.as_str() == host.as_str()
            || (!host.as_str().contains('*') && c.matches(host.as_str(), 443));
        host_ok
            && h.matcher
                .path_prefix
                .as_deref()
                .is_none_or(|p| path.starts_with(p))
    })
}

fn group(app: &App, g: &ToolGroup, base: &str, out: &mut Vec<Violation>) {
    let mut bad = |p: String, msg: String| {
        out.push(Violation::new("wildcard_not_superset", p, msg));
    };
    if g.items.is_empty() {
        bad(
            format!("{base}/wildcard"),
            "wildcard on an empty group".into(),
        );
        return;
    }
    for (i, it) in g.items.iter().enumerate() {
        let p = format!("{base}/items/{i}");
        let host = HostPattern::parse(&it.host);
        let path_ok = PathPattern::parse(&it.path).is_ok();
        let methods_ok = it
            .methods
            .iter()
            .all(|m| !m.is_empty() && m.bytes().all(|b| b.is_ascii_alphabetic()));
        match host {
            Ok(h) if path_ok && methods_ok => {
                if !covered(app, &h, &it.path) {
                    bad(
                        p,
                        format!("`{}{}` lies outside the declared hosts", it.host, it.path),
                    );
                }
            }
            _ => bad(p, "item is malformed, wildcard coverage undefined".into()),
        }
    }
}
