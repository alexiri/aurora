//! Rules of the `library` block (09 §3.4 `library`; phase 4, P4-GSL, D-149, SEC-84). Codes are
//! `library_*`.
use regex::Regex;

use crate::Violation;
use crate::library::JsonPath;
use crate::manifest::{App, INSTANCE_HOST, Library, LibraryPaginate};

/// Strings a repository pattern must **not** accept (query/fragment, control characters,
/// encodings, spaces, empty segments). `.` and `..` segments are refused at runtime whatever
/// the pattern says (`library::check_repo`), so a pattern may allow `.` in names.
const HOSTILE_REPOS: [&str; 8] = [
    "a/b?c=d",
    "a/b#c",
    "a b/c",
    "a/b\r\nHost: x",
    "a/%2e%2e",
    "a//b",
    "/a/b",
    "a/b/",
];

fn v(code: &'static str, path: String, msg: impl Into<String>) -> Violation {
    Violation::new(code, path, msg)
}

/// A request path template: `/…` with only the allowed placeholders and a literal query.
fn check_template(
    tpl: &str,
    at: &str,
    required: &[&str],
    allowed: &[&str],
    out: &mut Vec<Violation>,
) {
    if !tpl.starts_with('/') || tpl.starts_with("//") {
        out.push(v(
            "library_path_placeholder",
            at.into(),
            "a library path starts with a single `/`",
        ));
        return;
    }
    let mut seen = Vec::new();
    let mut literal = String::new();
    let mut rest = tpl;
    while let Some(i) = rest.find(['{', '}']) {
        literal.push_str(&rest[..i]);
        if !rest[i..].starts_with('{') {
            out.push(v(
                "library_path_placeholder",
                at.into(),
                "unbalanced braces",
            ));
            return;
        }
        let Some(j) = rest[i..].find('}') else {
            out.push(v(
                "library_path_placeholder",
                at.into(),
                "unbalanced braces",
            ));
            return;
        };
        let name = &rest[i + 1..i + j];
        // A placeholder stands for a non-empty checked value.
        literal.push('x');
        if !allowed.contains(&name) {
            out.push(v(
                "library_path_placeholder",
                at.into(),
                format!("placeholder `{{{name}}}` is not allowed here (allowed: {allowed:?})"),
            ));
        }
        seen.push(name);
        rest = &rest[i + j + 1..];
    }
    literal.push_str(rest);
    for r in required {
        if !seen.iter().any(|s| s.split('|').next() == Some(r)) {
            out.push(v(
                "library_path_placeholder",
                at.into(),
                format!("the template must use `{{{r}}}`"),
            ));
        }
    }
    if literal.matches('?').count() > 1
        || literal.contains('#')
        || literal.contains("..")
        || literal.contains("//")
        || !literal.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'.' | b'?' | b'=' | b'&')
        })
    {
        out.push(v(
            "library_path_placeholder",
            at.into(),
            "literal text may contain only letters, digits and `/_-.?=&` (one literal query)",
        ));
    }
    if tpl.contains("?page=") || tpl.contains("&page=") {
        out.push(v(
            "library_path_placeholder",
            at.into(),
            "`page` is added by the core for paginated trees; do not template it",
        ));
    }
}

fn check_json(p: &str, at: &str, out: &mut Vec<Violation>) {
    if let Err(e) = JsonPath::parse(p) {
        out.push(v("library_json_path", at.into(), format!("`{p}`: {e}")));
    }
}

/// The path of a template up to the first placeholder or query (for host-rule matching).
fn literal_head(tpl: &str) -> &str {
    let end = tpl.find(['{', '?']).unwrap_or(tpl.len());
    &tpl[..end]
}

fn prefix_covers(prefix: &str, path: &str) -> bool {
    let p = prefix.trim_end_matches('/');
    path == p || path.starts_with(&format!("{p}/"))
}

pub fn app_rules(app: &App, base: &str, out: &mut Vec<Violation>) {
    let Some(lib) = &app.library else { return };
    let at = |s: &str| format!("{base}/library{s}");
    host_rules(app, lib, &at, out);
    repo_pattern(lib, &at, out);
    for (n, m) in [
        ("/resolve_ref", &lib.resolve_ref.method),
        ("/list_tree", &lib.list_tree.method),
        ("/get_blob", &lib.get_blob.method),
    ] {
        if m.as_deref().is_some_and(|m| !m.eq_ignore_ascii_case("GET")) {
            out.push(v(
                "library_method",
                at(&format!("{n}/method")),
                "a library request is always a `GET`",
            ));
        }
    }

    let r = &lib.resolve_ref;
    check_template(
        &r.path,
        &at("/resolve_ref/path"),
        &["repo", "ref"],
        &["repo", "repo|urlencode", "ref", "ref|urlencode"],
        out,
    );
    check_template(
        &r.default_path,
        &at("/resolve_ref/default_path"),
        &["repo"],
        &["repo", "repo|urlencode"],
        out,
    );
    check_json(&r.sha, &at("/resolve_ref/sha"), out);
    check_json(&r.default_branch, &at("/resolve_ref/default_branch"), out);

    let t = &lib.list_tree;
    check_template(
        &t.path,
        &at("/list_tree/path"),
        &["repo", "sha"],
        &["repo", "repo|urlencode", "sha"],
        out,
    );
    check_json(&t.entries, &at("/list_tree/entries"), out);
    let f = &t.fields;
    for (n, p) in [
        ("path", &f.path),
        ("type", &f.kind),
        ("mode", &f.mode),
        ("blob", &f.blob),
    ] {
        check_json(p, &at(&format!("/list_tree/fields/{n}")), out);
    }
    if let Some(p) = &f.size {
        check_json(p, &at("/list_tree/fields/size"), out);
    }
    if let Some(p) = &t.truncated {
        check_json(p, &at("/list_tree/truncated"), out);
    }
    if t.file_type.is_empty() || t.file_type.len() > 32 {
        out.push(v(
            "library_json_path",
            at("/list_tree/file_type"),
            "`file_type` must be 1 to 32 characters",
        ));
    }
    match (t.paginate, t.max_pages) {
        (LibraryPaginate::None, Some(_)) => out.push(v(
            "library_json_path",
            at("/list_tree/max_pages"),
            "`max_pages` needs `paginate: link_header` or `page_param`",
        )),
        (LibraryPaginate::None, None) => {}
        (_, Some(n)) if (1..=200).contains(&n) => {}
        _ => out.push(v(
            "library_json_path",
            at("/list_tree/max_pages"),
            "a paginated tree needs `max_pages` between 1 and 200",
        )),
    }

    let b = &lib.get_blob;
    check_template(
        &b.path,
        &at("/get_blob/path"),
        &["repo", "blob"],
        &["repo", "repo|urlencode", "blob"],
        out,
    );
    if b.encoding == "raw" {
        if b.content.is_some() {
            out.push(v(
                "library_json_path",
                at("/get_blob/content"),
                "a `raw` endpoint has no `content` path",
            ));
        }
    } else {
        match &b.content {
            Some(p) => check_json(p, &at("/get_blob/content"), out),
            None => out.push(v(
                "library_json_path",
                at("/get_blob/content"),
                "a JSON blob endpoint needs a `content` path",
            )),
        }
        if b.encoding != "base64" {
            check_json(&b.encoding, &at("/get_blob/encoding"), out);
        }
    }
}

fn repo_pattern(lib: &Library, at: &dyn Fn(&str) -> String, out: &mut Vec<Violation>) {
    let p = &lib.repo_pattern;
    let re = match Regex::new(p) {
        Ok(re) => re,
        Err(e) => {
            out.push(v(
                "library_repo_pattern",
                at("/repo_pattern"),
                format!("not a valid regular expression: {e}"),
            ));
            return;
        }
    };
    if !p.starts_with('^') || !p.ends_with('$') || p.len() > 300 {
        out.push(v(
            "library_repo_pattern",
            at("/repo_pattern"),
            "the pattern must be anchored (`^…$`) and at most 300 characters",
        ));
    }
    if let Some(h) = HOSTILE_REPOS.iter().find(|h| re.is_match(h)) {
        out.push(v(
            "library_repo_pattern",
            at("/repo_pattern"),
            format!(
                "the pattern accepts `{}`; it must accept only plain `owner/name` style paths",
                h.escape_debug()
            ),
        ));
    }
}

fn host_rules(app: &App, lib: &Library, at: &dyn Fn(&str) -> String, out: &mut Vec<Violation>) {
    let host = lib.request_host.as_str();
    if host.contains('*') || host.is_empty() || host.contains('/') || host.contains(':') {
        out.push(v(
            "library_host_not_exact",
            at("/request_host"),
            "`request_host` must be one exact host (no wildcard, path or port)",
        ));
        return;
    }
    if host == INSTANCE_HOST && app.instances.is_none() {
        out.push(v(
            "library_host_not_exact",
            at("/request_host"),
            "`{instance}` is only valid in an app that declares `instances`",
        ));
        return;
    }
    let rules: Vec<_> = app
        .hosts
        .iter()
        .filter(|h| h.matcher.host.eq_ignore_ascii_case(host))
        .collect();
    if rules.is_empty() {
        out.push(v(
            "library_host_not_exact",
            at("/request_host"),
            "`request_host` must equal an exact `hosts[].match.host` of the app",
        ));
        return;
    }
    if rules.iter().any(|r| r.rewrite_host.is_some()) {
        out.push(v(
            "library_host_not_exact",
            at("/request_host"),
            "a host with `rewrite_host` cannot serve a library",
        ));
    }
    let templates = [
        ("/resolve_ref/path", &lib.resolve_ref.path),
        ("/resolve_ref/default_path", &lib.resolve_ref.default_path),
        ("/list_tree/path", &lib.list_tree.path),
        ("/get_blob/path", &lib.get_blob.path),
    ];
    for (n, tpl) in templates {
        let head = literal_head(tpl);
        // The rule the gateway applies: the longest path prefix that covers the request.
        let best = rules
            .iter()
            .filter(|r| {
                r.matcher
                    .path_prefix
                    .as_deref()
                    .is_none_or(|p| prefix_covers(p, head))
            })
            .max_by_key(|r| r.matcher.path_prefix.as_deref().map_or(0, str::len));
        match best {
            Some(r) if !r.inject.is_empty() => {}
            _ => out.push(v("library_no_credential", at(n), "no host rule with an inject stage covers this request: the connection's credential would not authenticate it")),
        }
    }
}
