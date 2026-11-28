//! Identification of the serving app and tool for a request, and the pure steps of the gateway's
//! credential resolution that depend on it (docs/06 pipeline steps 5–6, FIX-159, D-329, C-930).
//!
//! This is the **one** copy: the gateway's decision path, the policy simulator (`Check what would
//! happen`) and the effective view all call it, so what a person is told and what the gateway does
//! cannot diverge in identification, in the connection candidates of an app, in the winner rule or
//! in the tool tags. Nothing here touches a database or the network.
use aurora_policy::facts::ToolTags;
use aurora_policy::graphql::{self, GraphqlKind as PolicyGql};
use aurora_policy::{HostPattern, PathPattern};

use crate::index::{HostIndex, Snapshot, path_prefix_matches, tool_serves_host};
use crate::manifest::{GitKind, GraphqlKind, ToolItem};
use crate::snapshot::{AppEntry, HostEntry};

/// The app serving the request (host entry of the snapshot, longest path prefix).
pub struct AppMatch<'a> {
    pub entry: &'a HostEntry,
    pub app: &'a AppEntry,
    /// The match comes from a registered app instance (the `{instance}` host entries), not from
    /// the static host index (D-74).
    pub instance: bool,
}

/// The winning (non-lost) static host entry of an **Active** claim for the request, and its app.
pub fn identify_app<'a>(
    snap: &'a Snapshot,
    host: &str,
    port: u16,
    path: &str,
) -> Option<AppMatch<'a>> {
    let entry = HostIndex::new(snap).lookup_port(host, port, path)?;
    let app = snap.app(&entry.app_id)?;
    Some(AppMatch {
        entry,
        app,
        instance: false,
    })
}

/// The app serving a request whose host exactly equals a registered instance's host: the app's
/// `{instance}` host entry with the longest matching path prefix (D-74).
pub fn identify_instance_app<'a>(
    snap: &'a Snapshot,
    app_id: &str,
    path: &str,
) -> Option<AppMatch<'a>> {
    let app = snap.app(app_id)?;
    app.instances.as_ref()?;
    let entry = snap
        .instance_entries(app_id)
        .filter(|e| path_prefix_matches(e.path_prefix.as_deref(), path))
        .max_by_key(|e| e.path_prefix.as_deref().map_or(0, str::len))?;
    Some(AppMatch {
        entry,
        app,
        instance: true,
    })
}

/// (D-275, SEC-109) The app of an `own_server` connection that pinned the request's origin (the
/// caller decided that; no host claim is involved): the section's synthetic entry.
pub fn identify_own_server_app<'a>(snap: &'a Snapshot, app_id: &str) -> Option<AppMatch<'a>> {
    let app = snap.app(app_id)?;
    app.own_server.as_ref()?;
    let entry = snap.own_server_entry(app_id)?;
    Some(AppMatch {
        entry,
        app,
        instance: false,
    })
}

/// The origin (`host:port`) the request's connection must have pinned: lowercase host, explicit
/// port.
pub fn origin_of(host: &str, port: u16) -> String {
    format!("{}:{port}", host.to_ascii_lowercase())
}

/// How to read a registered app instance (whose host equals the request host, already filtered
/// by the caller): its app id and base path.
pub type RegRead<R> = fn(&R) -> (&str, Option<&str>);

/// The result of [`identify_request`].
pub struct RequestApp<'a> {
    pub matched: AppMatch<'a>,
    /// Index (into the `instances` slice given) of the registration that serves the request
    /// (only when the app match came from it).
    pub instance: Option<usize>,
    /// The registration whose base path covers the request path, whether or not the app match came
    /// from it.
    pub selected: Option<usize>,
    /// The path the manifest decisions use (D-231, SEC-59): the path below the registration's
    /// base path when an instance serves the request, the request path otherwise.
    pub app_path: String,
}

/// (D-74, D-231, SEC-59) A host that is exactly a registered app instance serves that app's
/// `{instance}` host entries; otherwise the static host index decides. Registrations may share the
/// host under disjoint base paths: the one whose base path covers the (already normalized) path
/// serves the request. A path outside every base path matches no instance.
///
/// Returns the registration the request fell under even when no app resulted (`instance`), so the
/// caller can tell "an instance host used outside its base paths" apart.
pub fn instance_for<'a, R>(
    instances: &'a [R],
    path: &str,
    read: RegRead<R>,
) -> Option<(usize, &'a R)> {
    let sel = crate::instance::select(instances, path, |i| read(i).1)?;
    let ix = instances
        .iter()
        .position(|i| std::ptr::eq(i, sel))
        .unwrap_or(0);
    Some((ix, sel))
}

/// See [`instance_for`]; the app (if any) that serves `host:port path`.
pub fn identify_request<'a, R>(
    snap: &'a Snapshot,
    instances: &[R],
    read: RegRead<R>,
    host: &str,
    port: u16,
    path: &str,
) -> Option<RequestApp<'a>> {
    let inst = instance_for(instances, path, read);
    let inst_app_path = inst.map(|(_, i)| crate::instance::app_path(read(i).1, path).to_owned());
    let matched = inst
        .zip(inst_app_path.as_deref())
        .and_then(|((_, i), ap)| identify_instance_app(snap, read(i).0, ap))
        .or_else(|| identify_app(snap, host, port, path))?;
    let selected = inst.map(|(ix, _)| ix);
    let app_path = match inst_app_path {
        Some(ap) if matched.instance => ap,
        _ => path.to_owned(),
    };
    // An instance match carries its registration; a static match on a host that is also an
    // instance host has `instance: None` but keeps `selected`.
    Some(RequestApp {
        instance: selected.filter(|_| matched.instance),
        selected,
        matched,
        app_path,
    })
}

/// The registrations visible for a request host: those whose host equals it exactly, belonging to
/// an app that declares `instances`, and not shadowed by a static claim of **another** app on the
/// same host (fail closed: nothing is injected for it). `entries` are the Active host entries
/// whose host pattern matches the request host.
pub fn host_instances<R>(
    snap: &Snapshot,
    visible: impl IntoIterator<Item = R>,
    host: &str,
    entries: &[&HostEntry],
    host_of: impl Fn(&R) -> &str,
    app_of: impl Fn(&R) -> &str,
) -> Vec<R> {
    visible
        .into_iter()
        .filter(|i| host_of(i).eq_ignore_ascii_case(host))
        .filter(|i| snap.app(app_of(i)).is_some_and(|a| a.instances.is_some()))
        .filter(|i| !entries.iter().any(|e| e.app_id != app_of(i)))
        .collect()
}

/// Active host entries whose host pattern matches, whatever the path.
pub fn entries_for_host<'a>(snap: &'a Snapshot, host: &str, port: u16) -> Vec<&'a HostEntry> {
    snap.hosts
        .iter()
        .filter(|h| h.claim_status == crate::snapshot::ClaimStatus::Active)
        .filter(|h| HostPattern::lenient(&h.host).matches(host, port))
        .collect()
}

/// A connection as far as binding it to a request is concerned: the registration it is bound to
/// and the tenant host it was stored for.
pub type BindingRead<C, Id> = fn(&C) -> (Option<Id>, Option<&str>);

/// The connections of the serving app that are candidates for the request (`same` = every
/// connection of that app in scope). `on_instance` is the registration serving the request, when
/// the match came from an instance.
///
/// Tenant-bound entries only offer connections stored for this host; when none does, the
/// mismatching ones stay so that the guard answers 421. Instance host: only connections bound to
/// this very instance; connections of other instances stay so the guard answers 421, default-host
/// ones never ride here. Static (default) host of an instance-bound app: connections without an
/// instance; instance-bound ones stay only so the guard answers 421.
pub fn bind_connections<C: Clone, Id: PartialEq + Copy>(
    m: &AppMatch<'_>,
    on_instance: Option<Id>,
    host: &str,
    same: Vec<C>,
    read: BindingRead<C, Id>,
) -> Vec<C> {
    if let Some(iid) = on_instance {
        let ok: Vec<_> = same
            .iter()
            .filter(|c| read(c).0 == Some(iid))
            .cloned()
            .collect();
        if ok.is_empty() {
            same.into_iter().filter(|c| read(c).0.is_some()).collect()
        } else {
            ok
        }
    } else if m.app.instances.is_some() {
        let ok: Vec<_> = same
            .iter()
            .filter(|c| read(c).0.is_none())
            .cloned()
            .collect();
        if ok.is_empty() { same } else { ok }
    } else if m.entry.tenant.is_some() {
        let ok: Vec<_> = same
            .iter()
            .filter(|c| read(c).1.is_some_and(|t| t.eq_ignore_ascii_case(host)))
            .cloned()
            .collect();
        if ok.is_empty() { same } else { ok }
    } else {
        same
    }
}

/// (FIX-104, D-180, SEC-04) A credential claimed by a package's host entry or a generic secret's
/// host pattern is injected only into a request that leaves over TLS to the claimed port (443
/// when the claim names none). Plain HTTP or another port: an uncredentialed request.
pub fn claim_transport_ok(claim_port: Option<u16>, https: bool, port: u16) -> bool {
    https && port == claim_port.unwrap_or(443)
}

/// The port a host-entry claim names (`host:port`), if any.
pub fn claim_port(claim: &str) -> Option<u16> {
    HostPattern::lenient(claim).port()
}

/// What the winner rule (06 step 6) looks at in one granted candidate.
#[derive(Debug, Clone, Copy)]
pub struct Rank {
    pub is_app: bool,
    /// A generic secret limited to a path.
    pub path_limited: bool,
    pub workspace: bool,
}

/// Step 6: the indices that survive the precedence before ambiguity is declared: app connection
/// over generic secret; path-limited over whole-host secret; workspace over organization. One
/// survivor wins; several are refused as ambiguous (`multiple_connections` /
/// `multiple_providers`); none is "no credential".
pub fn narrow(c: &[Rank]) -> Vec<usize> {
    let mut sel: Vec<usize> = (0..c.len()).collect();
    if sel.iter().any(|&i| c[i].is_app) {
        sel.retain(|&i| c[i].is_app);
    }
    if sel.iter().any(|&i| c[i].path_limited) {
        sel.retain(|&i| c[i].path_limited);
    }
    if sel.iter().any(|&i| c[i].workspace) {
        sel.retain(|&i| c[i].workspace);
    }
    sel
}

/// Every tool the app's catalogue lists, read group first.
pub fn tools(app: &AppEntry) -> impl Iterator<Item = &ToolItem> {
    app.tools
        .iter()
        .flat_map(|t| [&t.read, &t.write])
        .flatten()
        .flat_map(|g| &g.items)
}

/// The tool's host, path and HTTP method cover the request.
pub fn serves(
    app: &AppEntry,
    instance: bool,
    t: &ToolItem,
    host: &str,
    port: u16,
    path: &str,
    method: &str,
) -> bool {
    tool_serves_host(app, t, host, port, instance)
        && PathPattern::lenient(&t.path).matches(path)
        && (t.methods.is_empty() || t.methods.iter().any(|m| m.eq_ignore_ascii_case(method)))
}

/// A GraphQL-tagged tool, a tool with a `form` parameter condition (Q8, D-124), or a tool with
/// a declared body shape (the gateway checks the whole body) may match: the body prefix is
/// needed to classify or to check. Other requests are unaffected.
pub fn needs_body(m: &AppMatch<'_>, host: &str, port: u16, path: &str, method: &str) -> bool {
    tools(m.app).any(|t| {
        (t.graphql.is_some()
            || t.params
                .as_deref()
                .is_some_and(aurora_policy::params::needs_body)
            || t.body_check.is_some())
            && serves(m.app, m.instance, t, host, port, path, method)
    })
}

/// The catalogue tool for the request; GraphQL tools match on the fail-closed operation
/// kind, git tools on the smart-HTTP operation. Unmatched requests have `tool = None`.
#[allow(clippy::too_many_arguments)]
pub fn identify_tool<'a>(
    m: &AppMatch<'a>,
    host: &str,
    port: u16,
    path: &str,
    query: Option<&str>,
    method: &str,
    body_prefix: &[u8],
    truncated: bool,
    headers: &[(&str, &str)],
    connect_method: Option<&str>,
) -> Option<&'a ToolItem> {
    identify_tool_in(
        m.app,
        m.instance,
        host,
        port,
        path,
        query,
        method,
        body_prefix,
        truncated,
        headers,
        connect_method,
    )
}

/// [`identify_tool`] for a known app without a host entry: the database lane resolves the app
/// from the connection and matches its tools on the synthetic `<id>.db.aurora.internal` host
/// (`instance` is `false` there: database apps are never instance-bound).
#[allow(clippy::too_many_arguments)]
pub fn identify_tool_in<'a>(
    app: &'a AppEntry,
    instance: bool,
    host: &str,
    port: u16,
    path: &str,
    query: Option<&str>,
    method: &str,
    body_prefix: &[u8],
    truncated: bool,
    headers: &[(&str, &str)],
    connect_method: Option<&str>,
) -> Option<&'a ToolItem> {
    let gql = graphql::classify(body_prefix, truncated, query);
    let git = aurora_policy::git::classify(path, query);
    let params = aurora_policy::ParamInput {
        query,
        headers,
        body: body_prefix,
        truncated,
    };
    tools(app).find(|t| {
        // (FIX-123, D-214) A tool restricted to other connect methods is not this connection's.
        t.applies_to(connect_method)
            && serves(app, instance, t, host, port, path, method)
            // (P4-GRANTS, Q8, D-124, SEC-68) fail closed: any doubt means "not this tool" and
            // classification continues down the catalogue.
            && t.params
                .as_deref()
                .is_none_or(|p| aurora_policy::params::matches(p, &params))
            && t.graphql.is_none_or(|k| {
                matches!(
                    (k, gql),
                    (GraphqlKind::Query, PolicyGql::Query)
                        | (GraphqlKind::Mutation, PolicyGql::Mutation)
                )
            })
            && t.git.is_none_or(|k| {
                matches!(
                    (k, git),
                    (GitKind::Fetch, Some(aurora_policy::GitOp::Fetch))
                        | (GitKind::Push, Some(aurora_policy::GitOp::Push))
                )
            })
    })
}

/// (FIX-136, D-241) The tool needs scopes (any of) and the connection's known scopes hold none of
/// them. Unknown scopes (empty) never refuse: they are trusted only to refuse, never to widen, and
/// an undeclared token may well have them.
pub fn scope_missing(tool_scopes: &[String], known: &[String]) -> bool {
    !tool_scopes.is_empty() && !known.is_empty() && !tool_scopes.iter().any(|s| known.contains(s))
}

/// The tags the policy evaluator looks at, from the parts of a tool. `write`: the catalogue lists
/// the tool in the write group (FIX-133).
pub fn tags(
    write: bool,
    graphql: bool,
    git: bool,
    scope_missing: bool,
    ask_default: bool,
) -> ToolTags {
    ToolTags {
        scope_missing,
        graphql,
        git,
        write,
        // (P5-TOOLASK, C-1042) Only a write tool can default to Ask (the linter refuses the rest).
        ask_default: ask_default && write,
    }
}

/// [`tags`] of a catalogue tool of `app`; `known_scopes` are the winning connection's scopes
/// (`None`: no connection won).
pub fn tool_tags(app: &AppEntry, t: &ToolItem, known_scopes: Option<&[String]>) -> ToolTags {
    tags(
        app.tools
            .as_ref()
            .and_then(|g| g.write.as_ref())
            .is_some_and(|g| g.items.iter().any(|i| i.id == t.id)),
        t.graphql.is_some(),
        t.git.is_some(),
        known_scopes.is_some_and(|k| scope_missing(&t.scopes, k)),
        t.approval.is_some(),
    )
}
