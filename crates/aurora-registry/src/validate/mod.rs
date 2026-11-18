//! Validation rules (09 §6): in-manifest rules ([`manifest`]) and full package
//! rules ([`package`]). `hooks.wasm` world-shape checks: [`hooks_component`].
pub mod agent_guide_rules;
mod hooks_component;
mod library_rules;
mod package_rules;
mod params_rules;
mod wildcard;

pub use hooks_component::hooks_component;
pub use package_rules::{ValidationContext, package, zone_too_broad};
pub use params_rules::fixtures as params_fixtures;
pub use wildcard::wildcards;

use std::collections::BTreeSet;
use std::sync::OnceLock;

use aurora_core::limits::{AGENT_SNIPPET_MAX, SUMMARY_FIELDS_MAX, SUMMARY_TITLE_MAX};
use aurora_policy::HostPattern;
use regex::Regex;

use crate::Violation;
use crate::manifest::{
    Access, App, Connect, FieldType, Finalize, FormField, Hook, HostRule, INSTANCE_HOST, Inject,
    LDAP_MAX_ATTRIBUTES, LDAP_MAX_ENTRIES, Ldap, Manifest, Probe, ProbeMethod, Source, Store,
    TokenKind, Transform, UrlOrTemplate,
};
use crate::template::{CONTEXT_VARS, Declared, Place, Template};

const FORBIDDEN_HEADERS: [&str; 3] = ["authorization", "cookie", "x-api-key"];
/// The tool host pattern of a database app (`docs/06 §Database lane`, C-222).
const DATABASE_TOOL_HOST: &str = "*.db.aurora.internal";
const RESERVED_ENV: [&str; 16] = [
    "PATH",
    "HOME",
    "USER",
    "SHELL",
    "TZ",
    "LANG",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "REQUESTS_CA_BUNDLE",
    "NODE_EXTRA_CA_CERTS",
    "CURL_CA_BUNDLE",
    "FTP_PROXY",
];

/// Whether an `agent.env` name is reserved (09 §`agent`); shared with the boot path.
#[must_use]
pub fn env_name_reserved(name: &str) -> bool {
    RESERVED_ENV.contains(&name.to_ascii_uppercase().as_str())
        || ["AURORA_", "ANTHROPIC_", "OPENAI_"]
            .iter()
            .any(|p| name.starts_with(p))
}

/// Whether an `agent.env` name matches `[A-Z][A-Z0-9_]*`.
#[must_use]
pub fn env_name_valid(name: &str) -> bool {
    let mut c = name.chars();
    c.next().is_some_and(|f| f.is_ascii_uppercase())
        && c.all(|x| x.is_ascii_uppercase() || x.is_ascii_digit() || x == '_')
}

/// Whether an `agent.stubs` path is relative, normalized and confined to the home.
#[must_use]
pub fn stub_path_ok(path: &str) -> bool {
    !(path.is_empty()
        || path.starts_with('/')
        || path.contains(['\\', '\0'])
        || path
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == ".."))
}

fn re(cell: &'static OnceLock<Option<Regex>>, pat: &str) -> Option<&'static Regex> {
    cell.get_or_init(|| Regex::new(pat).ok()).as_ref()
}

/// A plain lower-case DNS name: labels of `[a-z0-9-]`, at least two labels, no IP literal, no port,
/// path or userinfo (the shape an app instance host must have, D-74).
#[must_use]
pub fn plain_dns_name(h: &str) -> bool {
    h.len() <= 253
        && h.contains('.')
        && h.parse::<std::net::IpAddr>().is_err()
        && h.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// `{instance}` may stand for a whole host only: in a URL it must be the authority and nothing
/// else in the string may contain braces (09 §6).
fn instance_url_ok(raw: &str) -> bool {
    let n = raw.matches(INSTANCE_HOST).count();
    if n == 0 {
        return true;
    }
    let Some(rest) = raw.strip_prefix("https://{instance}") else {
        return false;
    };
    n == 1 && (rest.is_empty() || rest.starts_with(['/', '?', '#']))
}

/// A host string that carries braces must be exactly `{instance}`.
fn host_braces_ok(h: &str) -> bool {
    h == INSTANCE_HOST || !h.contains(['{', '}'])
}

fn zone_ok(z: &str) -> bool {
    static R: OnceLock<Option<Regex>> = OnceLock::new();
    re(
        &R,
        r"^\*\.([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)+[a-z0-9]([a-z0-9-]*[a-z0-9])?$",
    )
    .is_some_and(|r| r.is_match(z))
        && HostPattern::parse(z).is_ok()
}

/// Host part of an `https://host[:port]/...` style string.
fn url_host(u: &str) -> Option<&str> {
    let rest = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let auth = &rest[..end];
    let auth = auth.rsplit_once('@').map_or(auth, |(_, h)| h);
    Some(auth.rsplit_once(':').map_or(auth, |(h, _)| h))
}

struct Ctx<'a> {
    out: Vec<Violation>,
    app: &'a App,
    base: String,
    declared: Declared<'a>,
}

impl Ctx<'_> {
    fn v(&mut self, code: &'static str, path: &str, msg: impl Into<String>) {
        self.out
            .push(Violation::new(code, format!("{}{path}", self.base), msg));
    }

    fn tmpl(&mut self, t: &Template, place: Place, path: &str, extra_fields: &[&str]) {
        let mut d = Declared {
            credentials: self.declared.credentials.clone(),
            metadata: self.declared.metadata.clone(),
            fields: self.declared.fields.clone(),
            context: self.declared.context.clone(),
        };
        d.fields.extend(extra_fields.iter().copied());
        for i in t.check(place, &d) {
            self.v(i.code, path, i.message);
        }
        if self.app.instances.is_none()
            && t.references()
                .any(|r| r.ns == crate::template::Ns::Instance)
        {
            self.v(
                "instance_placeholder_misuse",
                path,
                "`{instance}` needs an `instances` block on the app",
            );
        }
    }

    fn host_declared(&self, host: &str) -> bool {
        self.app.hosts.iter().any(|h| {
            HostPattern::parse(&h.matcher.host)
                .is_ok_and(|p| p.matches(host, 443) || p.as_str() == host.to_ascii_lowercase())
        }) || self
            .app
            .about
            .web_hosts
            .iter()
            .any(|w| w.eq_ignore_ascii_case(host))
    }
}

/// (FIX-135, D-242) `about.family`: a lower-case slug id, non-empty names, the same `name` on
/// every member of a family, a unique `variant_label`, and at most one member without
/// `instances` (the SaaS deployment; the others are instance-bound).
fn family_rules(m: &Manifest, out: &mut Vec<Violation>) {
    let mut by_family: std::collections::BTreeMap<&str, Vec<(usize, &crate::manifest::App)>> =
        std::collections::BTreeMap::new();
    for (i, a) in m.apps.iter().enumerate() {
        let Some(f) = &a.about.family else { continue };
        let at = format!("/apps/{i}/about/family");
        let slug = !f.id.is_empty()
            && f.id.len() <= 64
            && f.id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !slug {
            out.push(Violation::new(
                "family_id_invalid",
                format!("{at}/id"),
                "a family id is a lower-case slug (a-z, 0-9, -)",
            ));
        }
        if f.name.trim().is_empty() || f.variant_label.trim().is_empty() {
            out.push(Violation::new(
                "family_label_empty",
                at,
                "a family needs a name and a variant_label",
            ));
        }
        by_family.entry(f.id.as_str()).or_default().push((i, a));
    }
    for members in by_family.values() {
        let first = members[0].1.about.family.as_ref();
        let mut labels = BTreeSet::new();
        let mut saas = 0;
        for (i, a) in members {
            let Some(f) = &a.about.family else { continue };
            if first.is_some_and(|x| x.name != f.name) {
                out.push(Violation::new(
                    "family_name_mismatch",
                    format!("/apps/{i}/about/family/name"),
                    "every member of a family has the same name",
                ));
            }
            if !labels.insert(f.variant_label.as_str()) {
                out.push(Violation::new(
                    "family_variant_duplicate",
                    format!("/apps/{i}/about/family/variant_label"),
                    "a variant_label is unique within its family",
                ));
            }
            if a.instances.is_none() {
                saas += 1;
                if saas > 1 {
                    out.push(Violation::new(
                        "family_two_hosted",
                        format!("/apps/{i}/about/family"),
                        "at most one member of a family is not instance-bound",
                    ));
                }
            }
        }
    }
}

pub fn manifest(m: &Manifest) -> Vec<Violation> {
    let mut out = Vec::new();
    let mut ids = BTreeSet::new();
    for (i, a) in m.apps.iter().enumerate() {
        if !ids.insert(a.about.id.as_str()) {
            out.push(Violation::new(
                "duplicate_app_id",
                format!("/apps/{i}/about/id"),
                format!("app id `{}` appears twice in the package", a.about.id),
            ));
        }
    }
    family_rules(m, &mut out);
    for c in &m.permissions.context {
        if !CONTEXT_VARS.contains(&c.as_str()) {
            out.push(Violation::new(
                "unknown_context",
                "/permissions/context",
                format!("unknown context variable `{c}`"),
            ));
        }
    }
    // (P5-UX-13, C-942) `show_context`: only `org_external_id`, and only when the package is
    // allowed to read it (`permissions.context`).
    for (i, a) in m.apps.iter().enumerate() {
        for (j, c) in a.connect.iter().enumerate() {
            for (k, v) in c.show_context().iter().enumerate() {
                let p = format!("/apps/{i}/connect/{j}/show_context/{k}");
                if v != "org_external_id" {
                    out.push(Violation::new(
                        "show_context_unknown",
                        p,
                        format!("`show_context` accepts only `org_external_id`, not `{v}`"),
                    ));
                } else if !m.permissions.context.iter().any(|c| c == v) {
                    out.push(Violation::new(
                        "show_context_not_permitted",
                        p,
                        "`org_external_id` must be listed in permissions.context",
                    ));
                }
            }
        }
    }
    if m.permissions.egress.contains_key(&Hook::ApprovalSummarize) {
        out.push(Violation::new(
            "egress_summarize",
            "/permissions/egress/approval.summarize",
            "approval.summarize may not have egress",
        ));
    }
    let clients: BTreeSet<&str> = m.oauth_clients.iter().map(|c| c.id.as_str()).collect();
    // OAuth clients used by an app that declares `instances` may template `{instance}` (D-74).
    let instance_clients: BTreeSet<&str> = m
        .apps
        .iter()
        .filter(|a| a.instances.is_some())
        .flat_map(|a| {
            a.connect
                .iter()
                .filter_map(|c| match c {
                    Connect::Oauth2(o) => Some(o.client.as_str()),
                    _ => None,
                })
                .chain(a.token.iter().filter_map(|t| t.client.as_deref()))
                .chain(
                    a.connect
                        .iter()
                        .filter_map(|c| c.token().and_then(|t| t.client.as_deref())),
                )
        })
        .collect();
    for (i, c) in m.oauth_clients.iter().enumerate() {
        for (key, url) in [
            ("authorize_url", &c.authorize_url),
            ("token_url", &c.token_url),
        ] {
            check_oauth_url(
                &mut out,
                &format!("/oauth_clients/{i}/{key}"),
                key,
                url,
                instance_clients.contains(c.id.as_str()),
            );
        }
    }
    let any_instances = m.apps.iter().any(|a| a.instances.is_some());
    for (hook, hosts) in &m.permissions.egress {
        for (i, h) in hosts.iter().enumerate() {
            if !host_braces_ok(h) || (h == INSTANCE_HOST && !any_instances) {
                out.push(Violation::new(
                    "instance_placeholder_misuse",
                    format!("/permissions/egress/{}/{i}", hook_key(*hook)),
                    "`{instance}` is only valid as a whole host in an app that declares `instances`",
                ));
            }
        }
    }

    let mut used_hooks = BTreeSet::new();
    for (i, app) in m.apps.iter().enumerate() {
        let mut cx = Ctx {
            out: Vec::new(),
            app,
            base: format!("/apps/{i}"),
            declared: Declared {
                credentials: app.credentials.iter().map(|c| c.name.as_str()).collect(),
                metadata: app.metadata.iter().map(|c| c.name.as_str()).collect(),
                fields: BTreeSet::new(),
                context: m.permissions.context.iter().map(String::as_str).collect(),
            },
        };
        app_rules(&mut cx, &clients, &mut used_hooks);
        params_rules::app_rules(app, &format!("/apps/{i}"), &mut cx.out);
        library_rules::app_rules(app, &format!("/apps/{i}"), &mut cx.out);
        out.append(&mut cx.out);
    }

    let declared: BTreeSet<Hook> = m.permissions.hooks.iter().copied().collect();
    if declared != used_hooks {
        out.push(Violation::new(
            "hooks_mismatch",
            "/permissions/hooks",
            format!("declared {declared:?} but manifest references {used_hooks:?}"),
        ));
    }
    out
}

/// (P5-UX-13, C-939) Findings that do not reject a package: a tool without a description shows
/// as a bare method and host on the access screens. `aurora plugin validate` prints them;
/// `check-builtin.sh` turns them into failures for the builtin packages.
pub fn warnings(m: &Manifest) -> Vec<Violation> {
    let mut out = Vec::new();
    for (i, a) in m.apps.iter().enumerate() {
        let Some(tools) = &a.tools else { continue };
        for (gname, g) in [("read", &tools.read), ("write", &tools.write)] {
            let Some(g) = g else { continue };
            for (j, it) in g.items.iter().enumerate() {
                if it
                    .description
                    .as_deref()
                    .is_none_or(|d| d.trim().is_empty())
                {
                    out.push(Violation::new(
                        "tool_description_missing",
                        format!("/apps/{i}/tools/{gname}/items/{j}/description"),
                        format!("tool `{}` has no description", it.id),
                    ));
                }
            }
        }
    }
    out
}

fn hook_key(h: Hook) -> &'static str {
    match h {
        Hook::ConnectAuthorize => "connect.authorize",
        Hook::ConnectExchange => "connect.exchange",
        Hook::TokenMint => "token.mint",
        Hook::ApprovalSummarize => "approval.summarize",
    }
}

fn check_oauth_url(
    out: &mut Vec<Violation>,
    base: &str,
    key: &str,
    url: &UrlOrTemplate,
    instance_bound: bool,
) {
    match url {
        UrlOrTemplate::Url(u) if u.contains(INSTANCE_HOST) => {
            if !instance_bound
                || !instance_url_ok(u)
                || u.replacen(INSTANCE_HOST, "", 1).contains(['{', '}'])
            {
                out.push(Violation::new(
                    "instance_placeholder_misuse",
                    base,
                    format!(
                        "`{{instance}}` in {key} must be the whole https host, in a client used by an app that declares `instances`"
                    ),
                ));
            }
        }
        UrlOrTemplate::Url(u) => {
            if u.contains('{') || u.contains('}') {
                out.push(Violation::new(
                    if key == "token_url" {
                        "token_url_template_no_zone"
                    } else {
                        "authorize_url_template_no_zone"
                    },
                    base,
                    format!("templated {key} needs the {{template, zone}} form"),
                ));
            } else if !u.starts_with("https://") {
                out.push(Violation::new(
                    if key == "token_url" {
                        "token_url_not_https"
                    } else {
                        "authorize_url_not_https"
                    },
                    base,
                    format!("{key} must be https"),
                ));
            }
        }
        UrlOrTemplate::Templated(z) => {
            if !z.template.as_str().starts_with("https://") {
                out.push(Violation::new(
                    if key == "token_url" {
                        "token_url_not_https"
                    } else {
                        "authorize_url_not_https"
                    },
                    base,
                    format!("{key} must be https"),
                ));
            }
            if !zone_ok(&z.zone) {
                out.push(Violation::new(
                    "bad_zone",
                    format!("{base}/zone"),
                    "invalid zone",
                ));
            }
            let d = Declared::default();
            for iss in z.template.check(Place::TokenUrl, &d) {
                out.push(Violation::new(iss.code, base, iss.message));
            }
        }
    }
}

fn app_rules(cx: &mut Ctx<'_>, clients: &BTreeSet<&str>, used: &mut BTreeSet<Hook>) {
    let app = cx.app;
    // instances (D-74)
    if let Some(ins) = &app.instances {
        if ins.label.trim().is_empty() {
            cx.v(
                "instances_label_empty",
                "/instances/label",
                "label is empty",
            );
        }
        if let Some(d) = &ins.default_host
            && !plain_dns_name(d)
        {
            cx.v(
                "instance_default_host_invalid",
                "/instances/default_host",
                "default_host must be a plain lower-case DNS name",
            );
        }
        // (FIX-134, D-243) Preset instances are static claims like `default_host`: plain DNS
        // names inside a registrable domain, distinct from it and from each other.
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        seen.extend(ins.default_host.as_deref());
        for (i, p) in ins.presets.iter().enumerate() {
            let at = format!("/instances/presets/{i}");
            if !plain_dns_name(&p.host) {
                cx.v(
                    "instance_preset_invalid",
                    &format!("{at}/host"),
                    "a preset host must be a plain lower-case DNS name",
                );
            } else if psl::domain(p.host.as_bytes()).is_none() {
                cx.v(
                    "instance_preset_not_registrable",
                    &format!("{at}/host"),
                    "a preset host must lie inside a registrable domain, not be a public suffix",
                );
            }
            if !seen.insert(p.host.as_str()) {
                cx.v(
                    "instance_preset_duplicate",
                    &format!("{at}/host"),
                    "a preset host repeats default_host or another preset",
                );
            }
            if p.label.trim().is_empty() {
                cx.v(
                    "instance_preset_label_empty",
                    &format!("{at}/label"),
                    "a preset needs a label",
                );
            }
        }
        if let Some(p) = &ins.probe
            && (!p.path.starts_with('/')
                || p.path.contains(['{', '}', '#'])
                || p.path.starts_with("//"))
        {
            cx.v(
                "instance_probe_invalid",
                "/instances/probe/path",
                "probe path must start with a single `/` and carry no placeholder",
            );
        }
        if !app.hosts.iter().any(|h| h.matcher.host == INSTANCE_HOST) {
            cx.v(
                "instances_without_instance_host",
                "/instances",
                "an app with `instances` needs a hosts[] entry for `{instance}`",
            );
        }
    }
    for (i, w) in app.about.web_hosts.iter().enumerate() {
        if w.contains(['{', '}']) {
            cx.v(
                "instance_placeholder_misuse",
                &format!("/about/web_hosts/{i}"),
                "web_hosts are plain hosts",
            );
        }
    }
    // hosts
    for (i, h) in app.hosts.iter().enumerate() {
        host_rule(cx, h, &format!("/hosts/{i}"));
    }
    // connect
    for (i, c) in app.connect.iter().enumerate() {
        let p = format!("/connect/{i}");
        match c {
            Connect::Oauth2(o) => {
                if !clients.contains(o.client.as_str()) {
                    cx.v(
                        "unknown_oauth_client",
                        &format!("{p}/client"),
                        format!("no oauth client `{}`", o.client),
                    );
                }
                for (j, f) in o.fields.iter().enumerate() {
                    if f.store == Store::Credentials || f.kind == FieldType::JsonFile {
                        cx.v(
                            "oauth_field_store",
                            &format!("{p}/fields/{j}"),
                            "oauth2 fields may only use `store: metadata | tenant_host` (credentials come from the token response)",
                        );
                    }
                }
                if let Some(pr) = &o.probe {
                    probe(cx, pr, &format!("{p}/probe"), &[], false);
                }
                for (k, _) in o.token_response.iter().filter(|(k, _)| {
                    !["expires_in", "scope"].contains(&k.as_str())
                        && !app.credentials.iter().any(|c| &c.name == *k)
                }) {
                    cx.v(
                        "undeclared_credential",
                        &format!("{p}/token_response/{k}"),
                        format!("credential `{k}` is not declared"),
                    );
                }
            }
            Connect::Fields(f) => {
                if let Some(pr) = &f.probe {
                    let names: Vec<&str> = f
                        .fields
                        .iter()
                        .map(|x: &FormField| x.name.as_str())
                        .collect();
                    probe(cx, pr, &format!("{p}/probe"), &names, true);
                }
            }
            Connect::Hook(h) => {
                used.insert(Hook::ConnectExchange);
                if h.authorize {
                    used.insert(Hook::ConnectAuthorize);
                }
            }
        }
    }
    // token: the app's, then each connect method's own (FIX-123, D-214)
    let method_tokens = app
        .connect
        .iter()
        .enumerate()
        .filter_map(|(i, c)| c.token().map(|t| (format!("/connect/{i}/token"), t)));
    for (tp, t) in app
        .token
        .iter()
        .map(|t| ("/token".to_owned(), t))
        .chain(method_tokens)
    {
        if let Some(c) = &t.client
            && !clients.contains(c.as_str())
        {
            cx.v(
                "unknown_oauth_client",
                &format!("{tp}/client"),
                format!("no oauth client `{c}`"),
            );
        }
        if t.kind == TokenKind::Hook {
            used.insert(Hook::TokenMint);
        }
        for (k, v) in &t.extra_params {
            cx.tmpl(
                v,
                Place::TokenParams,
                &format!("{tp}/extra_params/{k}"),
                &[],
            );
        }
    }
    method_rules(cx);
    // tools
    if let Some(t) = &app.tools {
        let mut ids = BTreeSet::new();
        for (gname, g) in [("read", &t.read), ("write", &t.write)] {
            let Some(g) = g else { continue };
            if let Some(w) = &g.wildcard
                && !ids.insert(w.id.as_str())
            {
                cx.v(
                    "duplicate_tool_id",
                    &format!("/tools/{gname}/wildcard/id"),
                    format!("duplicate tool id `{}`", w.id),
                );
            }
            for (j, it) in g.items.iter().enumerate() {
                let p = format!("/tools/{gname}/items/{j}");
                if !ids.insert(it.id.as_str()) {
                    cx.v(
                        "duplicate_tool_id",
                        &format!("{p}/id"),
                        format!("duplicate tool id `{}`", it.id),
                    );
                }
                if !host_braces_ok(&it.host)
                    || (it.host == INSTANCE_HOST && app.instances.is_none())
                {
                    cx.v(
                        "instance_placeholder_misuse",
                        &format!("{p}/host"),
                        "`{instance}` is only valid as a whole host in an app that declares `instances`",
                    );
                }
                match HostPattern::parse(&it.host) {
                    Err(e) => cx.v("host_pattern_invalid", &format!("{p}/host"), e.to_string()),
                    Ok(hp) => {
                        // (P2-DBL-2) The tools of a database app are matched on the synthetic
                        // host of the database lane (docs/06), which no host entry can claim.
                        let covered = (app.database.is_some() && hp.as_str() == DATABASE_TOOL_HOST)
                            || app.hosts.iter().any(|h| {
                                HostPattern::parse(&h.matcher.host).is_ok_and(|c| {
                                    c.as_str() == hp.as_str()
                                        || (!hp.as_str().contains('*')
                                            && c.matches(hp.as_str(), 443))
                                })
                            });
                        if !covered {
                            cx.v(
                                "tool_host_uncovered",
                                &format!("{p}/host"),
                                format!("host `{}` is not covered by hosts[].match", it.host),
                            );
                        }
                    }
                }
                if it.path.contains(['{', '}']) {
                    cx.v(
                        "instance_placeholder_misuse",
                        &format!("{p}/path"),
                        "`{instance}` never appears in a path",
                    );
                }
                if aurora_policy::PathPattern::parse(&it.path).is_err() {
                    cx.v(
                        "tool_path_invalid",
                        &format!("{p}/path"),
                        "invalid path pattern",
                    );
                }
            }
        }
    }
    // approvals
    let injected: BTreeSet<String> = app
        .hosts
        .iter()
        .flat_map(|h| h.inject.iter())
        .filter_map(|i| match i {
            Inject::Header { name, .. } | Inject::RemoveHeader(name) => {
                Some(name.to_ascii_lowercase())
            }
            Inject::Bearer(_) | Inject::Basic { .. } => Some("authorization".to_owned()),
            _ => None,
        })
        .collect();
    for (k, a) in &app.approvals {
        let p = format!("/approvals/{k}");
        if a.title.chars().count() > SUMMARY_TITLE_MAX {
            cx.v(
                "summary_title_too_long",
                &format!("{p}/title"),
                "title too long",
            );
        }
        if a.fields.len() > SUMMARY_FIELDS_MAX as usize {
            cx.v(
                "summary_too_many_fields",
                &format!("{p}/fields"),
                "too many summary fields",
            );
        }
        for (j, f) in a.fields.iter().enumerate() {
            if let Source::Header { name } = &f.source {
                let n = name.to_ascii_lowercase();
                if FORBIDDEN_HEADERS.contains(&n.as_str())
                    || n.starts_with("proxy-")
                    || injected.contains(&n)
                {
                    cx.v(
                        "approval_header_forbidden",
                        &format!("{p}/fields/{j}/name"),
                        format!("header `{name}` may not be shown"),
                    );
                }
            }
        }
        if let Some(l) = &a.link {
            cx.tmpl(l, Place::Link, &format!("{p}/link"), &[]);
            let raw = l.as_str();
            if !raw.starts_with("https://") {
                cx.v(
                    "link_not_https",
                    &format!("{p}/link"),
                    "approval link must be https",
                );
            } else if let Some(h) = url_host(raw).filter(|h| !h.contains('{'))
                && !cx.host_declared(h)
            {
                cx.v(
                    "link_host_undeclared",
                    &format!("{p}/link"),
                    format!("link host `{h}` is not declared"),
                );
            }
        }
        if a.summarize.is_some() {
            used.insert(Hook::ApprovalSummarize);
        }
    }
    // agent
    if let Some(ag) = &app.agent {
        // (P5-GUIDE-1) the guide keys; the base path is the app's, without the `/agent` part.
        let base = cx.base.clone();
        agent_guide_rules::manifest_rules(ag, &base, &mut cx.out);
        if ag.instructions.chars().count() > AGENT_SNIPPET_MAX {
            cx.v(
                "agent_instructions_too_long",
                "/agent/instructions",
                "instructions too long",
            );
        }
        for (i, e) in ag.env.iter().enumerate() {
            if env_name_reserved(&e.name) {
                cx.v(
                    "env_name_reserved",
                    &format!("/agent/env/{i}/name"),
                    format!("`{}` is reserved", e.name),
                );
            } else if !env_name_valid(&e.name) {
                cx.v(
                    "env_name_invalid",
                    &format!("/agent/env/{i}/name"),
                    format!("`{}` is not a valid name", e.name),
                );
            }
        }
        for (i, s) in ag.stubs.iter().enumerate() {
            if !stub_path_ok(&s.path) {
                cx.v(
                    "stub_path_invalid",
                    &format!("/agent/stubs/{i}/path"),
                    "stub path must be relative, normalized and inside home",
                );
            }
        }
    }
    if let Some(d) = &app.database
        && d.engine != "postgres"
    {
        cx.v(
            "database_engine",
            "/database/engine",
            "only postgres is supported",
        );
    }
    if let Some(l) = &app.ldap {
        ldap_rules(app, l, cx);
    }
}
/// (D-251) The shape of an LDAP app (`docs/09 §3 ldap`): a plain public server name, LDAPS, a
/// subtree, a bounded attribute allow-list, one synthetic host entry and nothing that carries a
/// credential — the lane binds anonymously and can only search.
fn ldap_rules(app: &App, l: &Ldap, cx: &mut Ctx<'_>) {
    let server = l.server.as_str();
    let plain_name = !server.is_empty()
        && server.len() <= 253
        && server.contains('.')
        && server
            .split('.')
            .all(|p| !p.is_empty() && p.len() <= 63 && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        // An IPv4 literal ends in a number; a registrable name's last label never does.
        && !server.rsplit('.').next().is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit()));
    if !plain_name
        || server.ends_with(".internal")
        || server.ends_with(".local")
        || server.ends_with(".localhost")
    {
        cx.v(
            "ldap_server_invalid",
            "/ldap/server",
            "the server is an exact public DNS name (no IP literal, wildcard, port or internal name)",
        );
    }
    if l.port == 0 {
        cx.v("ldap_port_invalid", "/ldap/port", "the port is 1..65535");
    }
    if l.base_dn.trim().is_empty()
        || l.base_dn.len() > 256
        || l.base_dn.chars().any(char::is_control)
    {
        cx.v(
            "ldap_base_dn_invalid",
            "/ldap/base_dn",
            "the base DN is 1..256 characters without control characters",
        );
    }
    if l.attributes.is_empty() || l.attributes.len() > LDAP_MAX_ATTRIBUTES {
        cx.v(
            "ldap_attributes_invalid",
            "/ldap/attributes",
            format!("list 1..{LDAP_MAX_ATTRIBUTES} attributes"),
        );
    }
    for (i, a) in l.attributes.iter().enumerate() {
        let ok = a.len() <= 64
            && a.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && a.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        if !ok {
            cx.v(
                "ldap_attributes_invalid",
                &format!("/ldap/attributes/{i}"),
                "an attribute is a plain name (letters, digits, `-`); `*`, `+` and options are not allowed",
            );
        }
    }
    if l.max_entries == 0 || l.max_entries > LDAP_MAX_ENTRIES {
        cx.v(
            "ldap_max_entries_invalid",
            "/ldap/max_entries",
            format!("max_entries is 1..{LDAP_MAX_ENTRIES}"),
        );
    }
    let host = Ldap::host_of(&app.about.id);
    if app.database.is_some() {
        cx.v(
            "ldap_database_exclusive",
            "/ldap",
            "an app is either a database or an LDAP app",
        );
    }
    // Anonymous bind: no credential is declared, so none can be requested or injected.
    if !app.credentials.is_empty() {
        cx.v(
            "ldap_credentials_declared",
            "/credentials",
            "an LDAP app binds anonymously and declares no credentials",
        );
    }
    let own = |h: &HostRule| h.matcher.host.eq_ignore_ascii_case(&host);
    if app.hosts.iter().filter(|h| own(h)).count() != 1 {
        cx.v(
            "ldap_host_missing",
            "/hosts",
            format!("declare exactly one hosts[] entry for `{host}`"),
        );
    }
    for (i, h) in app.hosts.iter().enumerate() {
        if !own(h) {
            cx.v(
                "ldap_host_foreign",
                &format!("/hosts/{i}/match/host"),
                format!("an LDAP app claims only `{host}`"),
            );
        } else if !h.inject.is_empty() {
            cx.v(
                "ldap_host_inject",
                &format!("/hosts/{i}/inject"),
                "the LDAP host injects nothing",
            );
        }
    }
}

/// Every template of a host rule that can read a credential.
fn host_templates(h: &HostRule) -> Vec<&Template> {
    let mut out: Vec<&Template> = Vec::new();
    for i in &h.inject {
        match i {
            Inject::Bearer(t)
            | Inject::Header { value: t, .. }
            | Inject::Query { value: t, .. }
            | Inject::PathTemplate { value: t, .. }
            | Inject::PathRegex { replacement: t, .. } => out.push(t),
            Inject::Basic { user, password } => out.extend([user, password]),
            Inject::RemoveHeader(_) => {}
        }
    }
    if let Some(r) = &h.rewrite_host {
        out.push(&r.template);
    }
    if let Some(Finalize::AwsSigv4(a)) = &h.finalize {
        out.extend([&a.region, &a.access_key, &a.secret_key]);
        out.extend(a.session_token.iter());
    }
    if let Some(Transform::GitCommitTrailer(t)) = &h.transform {
        out.push(&t.trailer);
    }
    out
}

/// Connect-method rules (FIX-123, D-195, D-214, 09 §3.4 `connect[]`): unique ids, presentation
/// text, at most one `recommended`, `tools[].connect_methods` naming real methods, and (for an
/// app with several methods) every credential its host templates read, unless optional, is one
/// that each method produces.
fn method_rules(cx: &mut Ctx<'_>) {
    let app = cx.app;
    let mut ids = BTreeSet::new();
    let mut recommended = 0;
    for (i, c) in app.connect.iter().enumerate() {
        let p = format!("/connect/{i}");
        if !ids.insert(c.id()) {
            cx.v(
                "duplicate_connect_method",
                &format!("{p}/id"),
                format!("connect method id `{}` appears twice", c.id()),
            );
        }
        // (FIX-133, D-240) `aurora_*` form keys are Aurora's own channel to the hooks
        // (`aurora_access`): a package may not declare one, so a user can never supply it.
        for (j, f) in c.form_fields().iter().enumerate() {
            if f.name.starts_with("aurora_") {
                cx.v(
                    "field_name_reserved",
                    &format!("{p}/fields/{j}/name"),
                    format!(
                        "`{}` is reserved: field names may not start with `aurora_`",
                        f.name
                    ),
                );
            }
        }
        for (key, v, max) in [
            ("label", c.label(), METHOD_LABEL_MAX),
            ("description", c.description(), METHOD_DESCRIPTION_MAX),
        ] {
            if let Some(v) = v
                && (v.trim().is_empty()
                    || v.chars().count() > max
                    || v.chars().any(char::is_control))
            {
                cx.v(
                    "method_text_invalid",
                    &format!("{p}/{key}"),
                    format!("`{key}` must be 1 to {max} characters without control characters"),
                );
            }
        }
        if c.recommended() {
            recommended += 1;
        }
    }
    if recommended > 1 {
        cx.v(
            "recommended_multiple",
            "/connect",
            "at most one connect method may be `recommended`",
        );
    }
    if let Some(t) = &app.tools {
        for (gname, g) in [("read", &t.read), ("write", &t.write)] {
            let Some(g) = g else { continue };
            for (j, it) in g.items.iter().enumerate() {
                for m in &it.connect_methods {
                    if !ids.contains(m.as_str()) {
                        cx.v(
                            "tool_method_unknown",
                            &format!("/tools/{gname}/items/{j}/connect_methods"),
                            format!("tool `{}` names unknown connect method `{m}`", it.id),
                        );
                    }
                }
                // (FIX-136, D-241) A tool's `scopes` are ids some connect method declares.
                for s in &it.scopes {
                    if !app
                        .connect
                        .iter()
                        .any(|c| c.declared_scopes().iter().any(|d| &d.scope == s))
                    {
                        cx.v(
                            "tool_scope_unknown",
                            &format!("/tools/{gname}/items/{j}/scopes"),
                            format!(
                                "tool `{}` needs scope `{s}`, which no connect method declares",
                                it.id
                            ),
                        );
                    }
                }
            }
        }
    }
    // (P5-UX-2, C-956) `scope_presets`: each entry is a scope the method declares, and `read`
    // holds only read scopes (so "Read only" can never request a write scope).
    for (i, c) in app.connect.iter().enumerate() {
        let presets = match c {
            Connect::Oauth2(o) => o.scope_presets.as_ref(),
            Connect::Fields(f) => f.scope_presets.as_ref(),
            Connect::Hook(_) => None,
        };
        let Some(presets) = presets else { continue };
        for (key, list) in [("read", &presets.read), ("write", &presets.write)] {
            for (j, s) in list.iter().enumerate() {
                let pp = format!("/connect/{i}/scope_presets/{key}/{j}");
                match c.declared_scopes().iter().find(|d| &d.scope == s) {
                    None => cx.v(
                        "scope_preset_unknown",
                        &pp,
                        format!("`scope_presets.{key}` names scope `{s}`, which the method does not declare"),
                    ),
                    Some(d) if key == "read" && d.access != Access::Read => cx.v(
                        "scope_preset_write",
                        &pp,
                        format!("scope `{s}` is a write scope: `scope_presets.read` may hold only read scopes"),
                    ),
                    Some(_) => {}
                }
            }
        }
    }
    // (FIX-136) `fields` method scope declarations and their read probes.
    for (i, c) in app.connect.iter().enumerate() {
        let Connect::Fields(f) = c else { continue };
        let mut seen = BTreeSet::new();
        for (j, s) in f.scopes.iter().enumerate() {
            if !seen.insert(s.scope.as_str()) {
                cx.v(
                    "duplicate_scope",
                    &format!("/connect/{i}/scopes/{j}"),
                    format!("scope `{}` is declared twice", s.scope),
                );
            }
        }
        let names: Vec<&str> = f.fields.iter().map(|x| x.name.as_str()).collect();
        for (k, pr) in &f.probe_scopes {
            let pp = format!("/connect/{i}/probe_scopes/{k}");
            match f.scopes.iter().find(|s| &s.scope == k) {
                None => cx.v(
                    "probe_scope_unknown",
                    &pp,
                    format!("`probe_scopes` names scope `{k}`, which the method does not declare"),
                ),
                // Write scopes are never probed: a probe must not change anything, and a
                // successful read says nothing about a write scope.
                Some(s) if s.access != Access::Read => cx.v(
                    "probe_scope_write",
                    &pp,
                    format!("scope `{k}` is a write scope: write scopes are never probed"),
                ),
                Some(_) => {}
            }
            if pr.method != ProbeMethod::Get {
                cx.v("probe_scope_method", &pp, "a scope probe must be a GET");
            }
            probe(cx, pr, &pp, &names, true);
        }
    }
    // Only an app that uses the per-method `token` is held to the union rule: a package written
    // before it (several methods, one shared token) stays valid (the key is additive).
    if app.connect.len() < 2 || !app.connect.iter().any(|c| c.token().is_some()) {
        return;
    }
    let optional: BTreeSet<&str> = app
        .credentials
        .iter()
        .filter(|c| c.optional)
        .map(|c| c.name.as_str())
        .collect();
    let mut read: BTreeSet<&str> = BTreeSet::new();
    for t in app.hosts.iter().flat_map(host_templates) {
        read.extend(
            t.references()
                .filter(|r| r.ns == crate::template::Ns::Credentials)
                .map(|r| r.name.as_str()),
        );
    }
    for (i, c) in app.connect.iter().enumerate() {
        // A hook method's credentials come from the hook, which the manifest cannot see.
        let produced: BTreeSet<&str> = match c {
            Connect::Hook(_) => continue,
            Connect::Fields(f) => f
                .fields
                .iter()
                .filter(|x| x.store == Store::Credentials)
                .map(|x| x.name.as_str())
                .collect(),
            Connect::Oauth2(o) => o
                .token_response
                .keys()
                .map(String::as_str)
                .filter(|k| !["expires_in", "scope"].contains(k))
                .collect(),
        };
        // What this method's own token templates read counts too.
        let mut needs = read.clone();
        if let Some(t) = c.token() {
            for tpl in t.extra_params.values() {
                needs.extend(
                    tpl.references()
                        .filter(|r| r.ns == crate::template::Ns::Credentials)
                        .map(|r| r.name.as_str()),
                );
            }
        }
        for r in needs
            .iter()
            .filter(|r| !optional.contains(*r) && !produced.contains(*r))
        {
            cx.v(
                "method_credential_unproduced",
                &format!("/connect/{i}"),
                format!(
                    "connect method `{}` does not produce credential `{r}`, which host templates read (declare it `optional` or produce it)",
                    c.id()
                ),
            );
        }
    }
}

const METHOD_LABEL_MAX: usize = 60;
const METHOD_DESCRIPTION_MAX: usize = 400;

fn probe(cx: &mut Ctx<'_>, pr: &Probe, p: &str, fields: &[&str], fields_connect: bool) {
    cx.tmpl(
        &pr.url,
        Place::Probe { fields_connect },
        &format!("{p}/url"),
        fields,
    );
    if pr.url.as_str().contains(INSTANCE_HOST) && !instance_url_ok(pr.url.as_str()) {
        cx.v(
            "instance_placeholder_misuse",
            &format!("{p}/url"),
            "`{instance}` must be the whole https host of the probe URL",
        );
    }
    if let Some(h) = url_host(pr.url.as_str()).filter(|h| !h.contains('{')) {
        if !cx
            .app
            .hosts
            .iter()
            .any(|r| HostPattern::parse(&r.matcher.host).is_ok_and(|c| c.matches(h, 443)))
            && !cx
                .app
                .about
                .web_hosts
                .iter()
                .any(|w| w.eq_ignore_ascii_case(h))
        {
            cx.v(
                "probe_host_undeclared",
                &format!("{p}/url"),
                format!("probe host `{h}` is not declared"),
            );
        }
    } else if url_host(pr.url.as_str()).is_none() {
        cx.v(
            "probe_host_undeclared",
            &format!("{p}/url"),
            "probe url must be an absolute URL",
        );
    }
    for k in pr.metadata.keys() {
        if !cx.declared.metadata.contains(k.as_str()) {
            cx.v(
                "undeclared_metadata",
                &format!("{p}/metadata/{k}"),
                format!("metadata `{k}` is not declared"),
            );
        }
    }
}

fn host_rule(cx: &mut Ctx<'_>, h: &HostRule, p: &str) {
    if !host_braces_ok(&h.matcher.host) {
        cx.v(
            "instance_placeholder_misuse",
            &format!("{p}/match/host"),
            "`{instance}` stands for a whole host only, never part of one",
        );
    } else if h.matcher.host == INSTANCE_HOST {
        if cx.app.instances.is_none() {
            cx.v(
                "instance_placeholder_misuse",
                &format!("{p}/match/host"),
                "`{instance}` needs an `instances` block on the app",
            );
        }
        if h.tenant.is_some() || h.rewrite_host.is_some() {
            cx.v(
                "instance_placeholder_misuse",
                p,
                "an `{instance}` host rule cannot also be tenant-bound or rewrite the host",
            );
        }
    }
    if h.matcher
        .path_prefix
        .as_deref()
        .is_some_and(|x| x.contains(['{', '}']))
    {
        cx.v(
            "instance_placeholder_misuse",
            &format!("{p}/match/path_prefix"),
            "`{instance}` never appears in a path",
        );
    }
    if let Err(e) = HostPattern::parse(&h.matcher.host) {
        cx.v(
            "host_pattern_invalid",
            &format!("{p}/match/host"),
            e.to_string(),
        );
    }
    for (i, inj) in h.inject.iter().enumerate() {
        let ip = format!("{p}/inject/{i}");
        let ts: Vec<&Template> = match inj {
            Inject::Bearer(t) => vec![t],
            Inject::Basic { user, password } => vec![user, password],
            Inject::Header { value, .. }
            | Inject::Query { value, .. }
            | Inject::PathTemplate { value, .. } => vec![value],
            Inject::RemoveHeader(_) => vec![],
            Inject::PathRegex {
                pattern,
                replacement,
            } => {
                let ok = pattern.len() <= 256
                    && pattern.starts_with('^')
                    && pattern.ends_with('$')
                    && Regex::new(pattern).is_ok();
                if !ok {
                    cx.v(
                        "bad_path_regex",
                        &ip,
                        "path_regex must be anchored (^...$), <= 256 chars and compile",
                    );
                }
                vec![replacement]
            }
        };
        for t in ts {
            cx.tmpl(t, Place::Inject, &ip, &[]);
        }
    }
    if let Some(t) = &h.tenant {
        if t.field != "connection.tenant_host" {
            cx.v(
                "bad_tenant_field",
                &format!("{p}/tenant/field"),
                "tenant field must be connection.tenant_host",
            );
        }
        if !zone_ok(&t.zone) {
            cx.v("bad_zone", &format!("{p}/tenant/zone"), "invalid zone");
        }
    }
    if let Some(r) = &h.rewrite_host {
        if !zone_ok(&r.zone) {
            cx.v(
                "bad_zone",
                &format!("{p}/rewrite_host/zone"),
                "invalid zone",
            );
        }
        cx.tmpl(
            &r.template,
            Place::RewriteHost,
            &format!("{p}/rewrite_host/template"),
            &[],
        );
    }
    if let Some(crate::manifest::Finalize::AwsSigv4(a)) = &h.finalize {
        let fp = format!("{p}/finalize/aws_sigv4");
        for t in [&a.region, &a.access_key, &a.secret_key]
            .into_iter()
            .chain(a.session_token.as_ref())
        {
            cx.tmpl(t, Place::Finalize, &fp, &[]);
        }
    }
    if let Some(crate::manifest::Transform::GitCommitTrailer(t)) = &h.transform {
        cx.tmpl(&t.trailer, Place::Transform, &format!("{p}/transform"), &[]);
    }
}
