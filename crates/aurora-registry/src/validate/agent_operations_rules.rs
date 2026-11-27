//! (P5-GUIDE-6, `docs/24 §3.5`, C-651) Operations-file rules. The file is rejected as a whole;
//! every violation names its JSON pointer inside the file (`<path>#/operations/<i>/…`).
use std::collections::BTreeSet;

use aurora_core::limits::{AGENT_OPERATIONS_FILE_MAX, AGENT_OPERATIONS_MAX};
use serde_json::Value;

use crate::Violation;
use crate::library::JsonPath;
use crate::manifest::{App, INSTANCE_HOST, Inject};
use crate::operations::{self, Body, Operation, Pagination, Param, ParamIn};

const METHODS: [&str; 6] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"];
const CONTENT_TYPES: [&str; 3] = [
    "application/json",
    "application/x-www-form-urlencoded",
    "multipart/form-data",
];
const FORBIDDEN_HEADERS: [&str; 6] = [
    "authorization",
    "proxy-authorization",
    "cookie",
    "host",
    "content-length",
    "transfer-encoding",
];
const SUMMARY_MAX: usize = 200;
const DESCRIPTION_MAX: usize = 200;
const NAME_MAX: usize = 64;

fn v(code: &'static str, at: String, msg: impl Into<String>) -> Violation {
    Violation::new(code, at, msg)
}

/// `[a-z][a-z0-9_]{0,63}` (operation ids and tags).
#[must_use]
pub fn id_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

fn one_line(s: &str, max: usize) -> bool {
    let n = s.chars().count();
    n >= 1 && n <= max && !s.chars().any(char::is_control) && !s.trim().is_empty()
}

fn param_name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= NAME_MAX
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'[' | b']' | b'-'))
}

fn literal_segment_ok(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'.' | b'_'
                        | b'~'
                        | b'!'
                        | b'$'
                        | b'&'
                        | b'\''
                        | b'('
                        | b')'
                        | b'*'
                        | b'+'
                        | b','
                        | b';'
                        | b'='
                        | b':'
                        | b'@'
                        | b'%'
                        | b'-'
                )
        })
}

/// The header names the app's `hosts[]` inject stages set or remove (lower case).
fn inject_headers(app: &App) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for h in &app.hosts {
        for i in &h.inject {
            match i {
                Inject::Header { name, .. } | Inject::RemoveHeader(name) => {
                    out.insert(name.to_ascii_lowercase());
                }
                _ => {}
            }
        }
    }
    out
}

fn tool_ids(app: &App) -> BTreeSet<&str> {
    let mut out = BTreeSet::new();
    if let Some(t) = &app.tools {
        for g in [&t.read, &t.write].into_iter().flatten() {
            if let Some(w) = &g.wildcard {
                out.insert(w.id.as_str());
            }
            out.extend(g.items.iter().map(|i| i.id.as_str()));
        }
    }
    out
}

fn check_scalar_list(values: Option<&Vec<Value>>, at: &str, out: &mut Vec<Violation>) {
    if let Some(e) = values
        && (e.len() > 64 || e.iter().any(|x| x.is_array() || x.is_object()))
    {
        out.push(v(
            "agent_operations_param",
            format!("{at}/enum"),
            "enum holds at most 64 scalar values",
        ));
    }
}

fn check_param(p: &Param, at: &str, forbidden: &BTreeSet<String>, out: &mut Vec<Violation>) {
    if !param_name_ok(&p.name) {
        out.push(v(
            "agent_operations_param",
            format!("{at}/name"),
            "name is 1 to 64 characters of [A-Za-z0-9_.[]-]",
        ));
    }
    if let Some(d) = &p.description
        && !one_line(d, DESCRIPTION_MAX)
    {
        out.push(v(
            "agent_operations_param",
            format!("{at}/description"),
            "description is one line of at most 200 characters",
        ));
    }
    check_scalar_list(p.values.as_ref(), at, out);
    if p.location == ParamIn::Header {
        let l = p.name.to_ascii_lowercase();
        if FORBIDDEN_HEADERS.contains(&l.as_str())
            || l == "connection"
            || l.starts_with("x-aurora-")
            || l.starts_with("proxy-")
            || forbidden.contains(&l)
        {
            out.push(v(
                "agent_operations_header",
                format!("{at}/name"),
                format!(
                    "header `{}` belongs to the gateway, never to the agent",
                    p.name
                ),
            ));
        }
    }
}

fn check_body(b: &Body, at: &str, out: &mut Vec<Violation>) {
    if !CONTENT_TYPES.contains(&b.content_type.as_str()) {
        out.push(v(
            "agent_operations_body",
            format!("{at}/content_type"),
            "content_type is application/json, application/x-www-form-urlencoded or multipart/form-data",
        ));
    }
    if b.fields.len() > 128 {
        out.push(v(
            "agent_operations_body",
            format!("{at}/fields"),
            "at most 128 body fields",
        ));
    }
    for (i, f) in b.fields.iter().enumerate() {
        let fat = format!("{at}/fields/{i}");
        if !param_name_ok(&f.name) {
            out.push(v(
                "agent_operations_body",
                format!("{fat}/name"),
                "field name is 1 to 64 characters of [A-Za-z0-9_.[]-]",
            ));
        }
        if let Some(d) = &f.description
            && !one_line(d, DESCRIPTION_MAX)
        {
            out.push(v(
                "agent_operations_body",
                format!("{fat}/description"),
                "description is one line of at most 200 characters",
            ));
        }
        check_scalar_list(f.values.as_ref(), &fat, out);
    }
}

fn check_json_path(p: &str, at: String, out: &mut Vec<Violation>) {
    if let Err(e) = JsonPath::parse(p) {
        out.push(v("agent_operations_json_path", at, format!("`{p}`: {e}")));
    }
}

fn check_pagination(p: &Pagination, at: &str, out: &mut Vec<Violation>) {
    let bad = |what: &str, out: &mut Vec<Violation>| {
        out.push(v(
            "agent_operations_pagination",
            format!("{at}/{what}"),
            "query parameter name expected",
        ));
    };
    match p {
        Pagination::LinkHeader => {}
        Pagination::PageParam {
            param,
            start,
            size_param,
        } => {
            if !param_name_ok(param) {
                bad("param", out);
            }
            if size_param.as_deref().is_some_and(|s| !param_name_ok(s)) {
                bad("size_param", out);
            }
            if start.is_some_and(|s| s > 1) {
                out.push(v(
                    "agent_operations_pagination",
                    format!("{at}/start"),
                    "start is 0 or 1",
                ));
            }
        }
        Pagination::Cursor { param, next } => {
            if !param_name_ok(param) {
                bad("param", out);
            }
            check_json_path(next, format!("{at}/next"), out);
        }
    }
}

fn check_path(o: &Operation, at: &str, out: &mut Vec<Violation>) {
    let p = &o.path;
    let path_at = format!("{at}/path");
    if !p.starts_with('/') || p.contains(['?', '#']) || p.contains("//") {
        out.push(v(
            "agent_operations_path",
            path_at.clone(),
            "path starts with `/`, has no `//`, no query string and no fragment",
        ));
        return;
    }
    let declared: Vec<&str> = o
        .params
        .iter()
        .filter(|x| x.location == ParamIn::Path)
        .map(|x| x.name.as_str())
        .collect();
    let mut used: Vec<&str> = Vec::new();
    for seg in p.split('/').skip(1) {
        if let Some(name) = seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            if declared.contains(&name) {
                used.push(name);
            } else {
                out.push(v(
                    "agent_operations_path_param",
                    path_at.clone(),
                    format!("`{{{name}}}` is not a declared path parameter"),
                ));
            }
        } else if seg.is_empty() {
            // a trailing slash is a legal empty last segment
        } else if !literal_segment_ok(seg) {
            out.push(v(
                "agent_operations_path",
                path_at.clone(),
                format!("segment `{seg}` is not allowed"),
            ));
        }
    }
    for d in declared {
        if used.iter().filter(|u| **u == d).count() != 1 {
            out.push(v(
                "agent_operations_path_param",
                path_at.clone(),
                format!("path parameter `{d}` must appear exactly once in the path"),
            ));
        }
    }
}

/// Lint one operations file of `app`. `path` is the archive path, used in the violations.
pub fn rules(app: &App, path: &str, bytes: &[u8], out: &mut Vec<Violation>) {
    if bytes.len() > AGENT_OPERATIONS_FILE_MAX {
        out.push(v(
            "agent_operations_too_large",
            path.to_owned(),
            format!("operations file exceeds {AGENT_OPERATIONS_FILE_MAX} bytes"),
        ));
        return;
    }
    let file = match operations::parse(bytes) {
        Ok(f) => f,
        Err(e) => {
            out.push(v(e.code(), path.to_owned(), e.message().to_owned()));
            return;
        }
    };
    if file.operations.is_empty() || file.operations.len() > AGENT_OPERATIONS_MAX {
        out.push(v(
            "agent_operations_too_many",
            format!("{path}#/operations"),
            format!("1 to {AGENT_OPERATIONS_MAX} operations"),
        ));
        return;
    }
    let forbidden = inject_headers(app);
    let tools = tool_ids(app);
    let hosts: BTreeSet<&str> = app
        .hosts
        .iter()
        .map(|h| h.matcher.host.as_str())
        .chain(std::iter::once(INSTANCE_HOST).filter(|_| app.instances.is_some()))
        .collect();
    let mut ids = BTreeSet::new();
    for (i, o) in file.operations.iter().enumerate() {
        let at = format!("{path}#/operations/{i}");
        if !id_ok(&o.id) {
            out.push(v(
                "agent_operations_id",
                format!("{at}/id"),
                "id is [a-z][a-z0-9_]{0,63}",
            ));
        } else if !ids.insert(o.id.as_str()) {
            out.push(v(
                "agent_operations_duplicate_id",
                format!("{at}/id"),
                format!("id `{}` appears twice", o.id),
            ));
        }
        if !METHODS.contains(&o.method.as_str()) {
            out.push(v(
                "agent_operations_method",
                format!("{at}/method"),
                "method is GET, HEAD, POST, PUT, PATCH or DELETE",
            ));
        }
        if !one_line(&o.summary, SUMMARY_MAX) {
            out.push(v(
                "agent_operations_param",
                format!("{at}/summary"),
                "summary is one line of 1 to 200 characters",
            ));
        }
        if o.tags.len() > 8 || o.tags.iter().any(|t| !id_ok(t)) {
            out.push(v(
                "agent_operations_param",
                format!("{at}/tags"),
                "at most 8 tags of the id grammar",
            ));
        }
        if o.params.len() > 64 {
            out.push(v(
                "agent_operations_param",
                format!("{at}/params"),
                "at most 64 parameters",
            ));
        }
        check_path(o, &at, out);
        if let Some(h) = &o.host
            && !hosts.contains(h.as_str())
        {
            out.push(v(
                "agent_operations_host",
                format!("{at}/host"),
                format!("`{h}` is not one of the app's hosts"),
            ));
        }
        for (j, p) in o.params.iter().enumerate() {
            check_param(p, &format!("{at}/params/{j}"), &forbidden, out);
        }
        if let Some(b) = &o.body {
            check_body(b, &format!("{at}/body"), out);
        }
        if let Some(p) = &o.pagination {
            check_pagination(p, &format!("{at}/pagination"), out);
        }
        if let Some(it) = &o.items {
            check_json_path(it, format!("{at}/items"), out);
        }
        if let Some(p) = &o.project {
            let at = format!("{at}/project");
            if p.is_empty() || p.len() > 8 {
                out.push(v(
                    "agent_operations_project",
                    at.clone(),
                    "1 to 8 paths expected",
                ));
            }
            for (j, ptr) in p.iter().enumerate() {
                match JsonPath::parse(ptr) {
                    Ok(path) if path.simple() => {}
                    Ok(_) => out.push(v(
                        "agent_operations_project",
                        format!("{at}/{j}"),
                        format!("`{ptr}`: only plain `.name` segments, no wildcards"),
                    )),
                    Err(e) => out.push(v(
                        "agent_operations_project",
                        format!("{at}/{j}"),
                        format!("`{ptr}`: {e}"),
                    )),
                }
            }
        }
        if let Some(t) = &o.tool
            && !tools.contains(t.as_str())
        {
            out.push(v(
                "agent_operations_tool",
                format!("{at}/tool"),
                format!("`{t}` is not a tool of the app's catalogue"),
            ));
        }
    }
}
