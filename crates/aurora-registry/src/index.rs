//! Host index over a [`Snapshot`] (06 §MITM lane step 2) and tool lookup.
//!
//! Precedence: among non-`lost` entries whose host pattern and path prefix
//! match, exact-host patterns beat wildcard patterns; among wildcards the
//! longest suffix (text after `*`), then the longest prefix, wins; ties then
//! go to the longest path prefix and finally the smaller app id. Within the
//! exact tier the longest path prefix wins. Path prefixes are plain string
//! prefixes (absent = `/`). Governed-LLM-host blocking (`reserved_hosts`) is
//! the gateway's job, not the index's.
use aurora_policy::{HostPattern, PathPattern};

use crate::manifest::ToolItem;
pub use crate::snapshot::{AppEntry, ClaimStatus, HostEntry, Snapshot};

pub struct HostIndex<'a> {
    snap: &'a Snapshot,
    entries: Vec<(HostPattern, &'a HostEntry)>,
}

impl<'a> HostIndex<'a> {
    pub fn new(snap: &'a Snapshot) -> Self {
        let entries = snap
            .hosts
            .iter()
            .filter(|h| h.claim_status == ClaimStatus::Active)
            .map(|h| (HostPattern::lenient(&h.host), h))
            .filter(|(p, _)| p.is_valid())
            .collect();
        Self { snap, entries }
    }

    /// Lookup assuming the default TLS port 443.
    pub fn lookup(&self, host: &str, path: &str) -> Option<&'a HostEntry> {
        self.lookup_port(host, 443, path)
    }

    pub fn lookup_port(&self, host: &str, port: u16, path: &str) -> Option<&'a HostEntry> {
        self.entries
            .iter()
            .filter(|(p, e)| {
                p.matches(host, port) && path.starts_with(e.path_prefix.as_deref().unwrap_or("/"))
            })
            .max_by_key(|(p, e)| {
                let pat = p.as_str().split(':').next().unwrap_or("");
                let (pre, suf) = pat
                    .split_once('*')
                    .map_or((usize::MAX, usize::MAX), |(a, b)| (a.len(), b.len()));
                // exact = highest tier; then longer suffix, longer prefix.
                let tier = u8::from(pre == usize::MAX);
                let (suf, pre) = if tier == 1 { (0, 0) } else { (suf, pre) };
                (
                    tier,
                    suf,
                    pre,
                    e.path_prefix.as_deref().map_or(1, str::len),
                    std::cmp::Reverse(e.app_id.as_str()),
                )
            })
            .map(|(_, e)| *e)
    }

    /// The tool of the winning app whose host/path/method matches the request.
    pub fn tool(
        &self,
        host: &str,
        path: &str,
        method: &str,
    ) -> Option<(&'a AppEntry, &'a ToolItem)> {
        let entry = self.lookup(host, path)?;
        let app = self.snap.app(&entry.app_id)?;
        let tools = app.tools.as_ref()?;
        [&tools.read, &tools.write]
            .into_iter()
            .flatten()
            .flat_map(|g| &g.items)
            .find(|t| {
                HostPattern::lenient(&t.host).matches(host, 443)
                    && PathPattern::lenient(&t.path).matches(path)
                    && (t.methods.is_empty()
                        || t.methods.iter().any(|m| m.eq_ignore_ascii_case(method)))
            })
            .map(|t| (app, t))
    }
}
