//! Host index over a [`Snapshot`] (06 §MITM lane step 2) and tool lookup.
//!
//! Precedence: among non-`lost` entries whose host pattern and path prefix
//! match, exact-host patterns beat wildcard patterns; among wildcards the
//! longest suffix (text after `*`), then the longest prefix, wins; ties then
//! go to the longest path prefix and finally the smaller app id. Within the
//! exact tier the longest path prefix wins. Path prefixes match whole path
//! segments ([`path_prefix_matches`], absent = every path). Governed-LLM-host
//! blocking (`reserved_hosts`) is the gateway's job, not the index's.
use aurora_policy::HostPattern;

use crate::manifest::{INSTANCE_HOST, ToolItem};
pub use crate::snapshot::{AppEntry, ClaimStatus, HostEntry, Snapshot};

/// (FIX-215, `09 §3.4`, SEC-04/SEC-59) Does a request path fall under a host entry's
/// `path_prefix`? The path is the normalized one of `06` step 3 (the app path on a registered
/// instance), without the query. Whole segments: the path equals the prefix or continues it with
/// `/`; a prefix that ends in `/` covers the paths below it; no prefix covers every path.
/// `/rest` matches `/rest` and `/rest/api/2`, not `/restricted`; `/api/` matches `/api/x`, not
/// `/api`. The one rule for the gateway, the simulator, the connect probe and the linter
/// (`scripts/plugins/check_builtin.py` mirrors it).
#[must_use]
pub fn path_prefix_matches(prefix: Option<&str>, path: &str) -> bool {
    match prefix {
        None | Some("") => true,
        Some(p) if p.ends_with('/') => path.starts_with(p),
        Some(p) => path
            .strip_prefix(p)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/')),
    }
}

/// Does the tool's host cover the request host? An `{instance}` tool covers the host of a
/// registered instance (`instance`, decided by the caller from the exact-host instance match)
/// and the app's `default_host` (D-74).
#[must_use]
pub fn tool_serves_host(
    app: &AppEntry,
    t: &ToolItem,
    host: &str,
    port: u16,
    instance: bool,
) -> bool {
    if t.host == crate::manifest::CONNECTION_HOST {
        // (D-275) Any origin the connection pinned: the caller only matches an own-server app
        // for a request whose origin a connection of it pinned (the resolver decides that).
        app.own_server.is_some()
    } else if t.host == INSTANCE_HOST {
        instance
            || app.instances.as_ref().is_some_and(|i| {
                i.default_host
                    .iter()
                    .map(String::as_str)
                    .chain(i.presets.iter().map(|p| p.host.as_str()))
                    .any(|d| d.eq_ignore_ascii_case(host))
            })
    } else {
        HostPattern::lenient(&t.host).matches(host, port)
    }
}

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
                p.matches(host, port) && path_prefix_matches(e.path_prefix.as_deref(), path)
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
        crate::identify::tools(app)
            .find(|t| crate::identify::serves(app, false, t, host, 443, path, method))
            .map(|t| (app, t))
    }
}

#[cfg(test)]
mod tests {
    use super::path_prefix_matches as m;

    #[test]
    fn a_prefix_matches_whole_segments() {
        let rest = Some("/rest");
        assert!(m(rest, "/rest"));
        assert!(m(rest, "/rest/"));
        assert!(m(rest, "/rest/api"));
        assert!(m(rest, "/rest/api/2/issue"));
        assert!(!m(rest, "/restricted"));
        assert!(
            !m(rest, "/rest%2Fx"),
            "an encoded slash is text, not a segment boundary"
        );
        assert!(!m(rest, "/res"));
        assert!(!m(rest, "/"));
        assert!(!m(rest, "/REST/api"));
        assert!(!m(Some("/ex/tracker"), "/ex/trackerxyz"));
        assert!(m(Some("/ex/tracker"), "/ex/tracker/abc/rest/api/3/myself"));
        assert!(!m(Some("/api/0"), "/api/0x"));
    }

    #[test]
    fn a_prefix_ending_in_a_slash_covers_the_paths_below_it() {
        let api = Some("/api/");
        assert!(!m(api, "/api"));
        assert!(m(api, "/api/"));
        assert!(m(api, "/api/x"));
        assert!(!m(api, "/apis/x"));
    }

    #[test]
    fn no_prefix_or_the_root_covers_every_path() {
        for p in [None, Some("/"), Some("")] {
            assert!(m(p, "/"));
            assert!(m(p, "/anything/below"));
        }
    }
}
