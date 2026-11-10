//! Permission-widening diff (09 §10). Widening needs instance-admin approval;
//! an empty result means the upgrade auto-activates. First install is always
//! approval-gated by the caller (diff against an empty manifest is not needed).
//!
//! Comparison is by exact (lowercased for hosts) string: a different egress
//! pattern, even if narrower, counts as new (conservative).
use std::collections::BTreeSet;

use crate::claims::claim_key;
use crate::manifest::{Hook, Manifest, UrlOrTemplate};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Widening {
    Hook(Hook),
    Egress {
        hook: Hook,
        host: String,
    },
    Context(String),
    HostClaim(String),
    /// New zone used by `tenant`, `rewrite_host` or a templated `token_url`.
    Zone(String),
    /// (P4-GSL, D-149) a new or changed `library` block on this app id: "can serve a skill
    /// library: reads repository files from <request_host>".
    Library {
        app: String,
        host: String,
    },
}

fn zones(m: &Manifest) -> BTreeSet<String> {
    let mut z = BTreeSet::new();
    for c in &m.oauth_clients {
        if let UrlOrTemplate::Templated(t) = &c.token_url {
            z.insert(t.zone.to_ascii_lowercase());
        }
    }
    for h in m.apps.iter().flat_map(|a| &a.hosts) {
        if let Some(t) = &h.tenant {
            z.insert(t.zone.to_ascii_lowercase());
        }
        if let Some(r) = &h.rewrite_host {
            z.insert(r.zone.to_ascii_lowercase());
        }
    }
    z
}

fn claim_keys(m: &Manifest) -> BTreeSet<String> {
    m.apps
        .iter()
        .flat_map(|a| &a.hosts)
        .map(|h| claim_key(&h.matcher.host, h.matcher.path_prefix.as_deref()))
        .collect()
}

pub fn widens(old: &Manifest, new: &Manifest) -> Vec<Widening> {
    let mut out = Vec::new();
    for h in &new.permissions.hooks {
        if !old.permissions.hooks.contains(h) {
            out.push(Widening::Hook(*h));
        }
    }
    for (hook, hosts) in &new.permissions.egress {
        let had: BTreeSet<String> = old
            .permissions
            .egress
            .get(hook)
            .into_iter()
            .flatten()
            .map(|h| h.to_ascii_lowercase())
            .collect();
        for host in hosts {
            let host = host.to_ascii_lowercase();
            if !had.contains(&host) {
                out.push(Widening::Egress { hook: *hook, host });
            }
        }
    }
    for c in &new.permissions.context {
        if !old.permissions.context.contains(c) {
            out.push(Widening::Context(c.clone()));
        }
    }
    let old_keys = claim_keys(old);
    out.extend(
        claim_keys(new)
            .difference(&old_keys)
            .cloned()
            .map(Widening::HostClaim),
    );
    let old_zones = zones(old);
    out.extend(
        zones(new)
            .difference(&old_zones)
            .cloned()
            .map(Widening::Zone),
    );
    for a in &new.apps {
        let Some(lib) = &a.library else { continue };
        let same = old
            .apps
            .iter()
            .find(|o| o.about.id == a.about.id)
            .and_then(|o| o.library.as_ref())
            .is_some_and(|o| o == lib);
        if !same {
            out.push(Widening::Library {
                app: a.about.id.clone(),
                host: lib.request_host.clone(),
            });
        }
    }
    out.sort();
    out.dedup();
    out
}
