//! (P5-GUIDE-6, `docs/24 §5.4`, C-652) `api.json`: the machine-readable view of one app for one
//! agent, read by `aurora-agent`. Deterministic (structs with a fixed key order), bounded by
//! `AGENT_API_JSON_MAX` with the drop order of §5.4. Advisory exactly as `tools.md` is (SEC-91):
//! the gateway decides every request.
use std::collections::BTreeMap;

use aurora_core::limits::AGENT_API_JSON_MAX;
use aurora_core::sanitize::inline;
use serde::Serialize;
use serde_json::Value;

use super::{
    GrantAccess, GuideAccount, NAME_MAX, catalogue, letter, lost, tool_access, untooled_access,
};
use crate::identify::identify_tool_in;
use crate::manifest::INSTANCE_HOST;
use crate::operations::{Body, Field, Operation, OperationsFile, Pagination, Param};
use crate::snapshot::{AppEntry, GuideKind, HostEntry};

pub const FORMAT: &str = "aurora.app-api/1";
const SUMMARY_MAX: usize = 200;

#[derive(Serialize)]
struct Doc {
    format: &'static str,
    app: AppOut,
    accounts: Vec<AccountOut>,
    operations: Vec<OpOut>,
    recipes: Vec<RecipeOut>,
    untooled: BTreeMap<String, &'static str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    truncated: bool,
}

#[derive(Serialize)]
struct AppOut {
    id: String,
    name: String,
    version: String,
}

#[derive(Serialize)]
struct AccountOut {
    letter: String,
    connection_id: String,
    label: String,
    base_url: Option<String>,
    grant: &'static str,
}

#[derive(Serialize)]
struct RecipeOut {
    file: String,
    title: String,
}

#[derive(Serialize, Clone)]
struct OpOut {
    id: String,
    method: String,
    path: String,
    host: Option<String>,
    summary: String,
    tags: Vec<String>,
    params: Vec<ParamOut>,
    body: Option<BodyOut>,
    pagination: Option<Pagination>,
    items: Option<String>,
    tool: Option<String>,
    access: BTreeMap<String, &'static str>,
    source: &'static str,
}

#[derive(Serialize, Clone)]
struct ParamOut {
    name: String,
    #[serde(rename = "in", skip_serializing_if = "Option::is_none")]
    location: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    required: Option<bool>,
    #[serde(rename = "type")]
    ty: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    values: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<Value>,
}

#[derive(Serialize, Clone)]
struct BodyOut {
    content_type: String,
    fields: Vec<ParamOut>,
}

fn ty_word(t: crate::operations::ParamType) -> &'static str {
    use crate::operations::ParamType as T;
    match t {
        T::String => "string",
        T::Integer => "integer",
        T::Number => "number",
        T::Boolean => "boolean",
        T::Array => "array",
    }
}

fn clean_value(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(inline(s, SUMMARY_MAX)),
        other => other.clone(),
    }
}

fn param_out(p: &Param) -> ParamOut {
    use crate::operations::ParamIn as I;
    ParamOut {
        name: inline(&p.name, 64),
        location: Some(match p.location {
            I::Path => "path",
            I::Query => "query",
            I::Header => "header",
        }),
        required: p.required,
        ty: ty_word(p.ty),
        description: p.description.as_deref().map(|d| inline(d, SUMMARY_MAX)),
        values: p
            .values
            .as_ref()
            .map(|v| v.iter().map(clean_value).collect()),
        default: p.default.as_ref().map(clean_value),
    }
}

fn field_out(f: &Field) -> ParamOut {
    ParamOut {
        name: inline(&f.name, 64),
        location: None,
        required: f.required,
        ty: ty_word(f.ty),
        description: f.description.as_deref().map(|d| inline(d, SUMMARY_MAX)),
        values: f
            .values
            .as_ref()
            .map(|v| v.iter().map(clean_value).collect()),
        default: f.default.as_ref().map(clean_value),
    }
}

fn body_out(b: &Body) -> BodyOut {
    BodyOut {
        content_type: inline(&b.content_type, 64),
        fields: b.fields.iter().map(field_out).collect(),
    }
}

fn pagination_out(p: &Pagination) -> Pagination {
    match p {
        Pagination::LinkHeader => Pagination::LinkHeader,
        Pagination::PageParam {
            param,
            start,
            size_param,
        } => Pagination::PageParam {
            param: inline(param, 64),
            start: *start,
            size_param: size_param.as_deref().map(|s| inline(s, 64)),
        },
        Pagination::Cursor { param, next } => Pagination::Cursor {
            param: inline(param, 64),
            next: inline(next, 200),
        },
    }
}

/// `{param}` → a placeholder segment, as the gateway sees a concrete request path.
fn placeholder_path(path: &str) -> String {
    path.split('/')
        .map(|s| {
            if s.starts_with('{') && s.ends_with('}') {
                "x"
            } else {
                s
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The one host of a static (not instance-bound) app: every catalogue tool names the same plain
/// host. An account of such an app has no stored host, yet its requests can only go there.
fn sole_static_host(app: &AppEntry) -> Option<String> {
    if app.instances.is_some() {
        return None;
    }
    let (read, write) = catalogue(app);
    let mut hosts = read.iter().chain(&write).map(|t| t.host.as_str());
    let first = hosts.next()?;
    let plain = !first.contains(['*', '{', ':', '/']);
    (plain && hosts.all(|h| h == first)).then(|| first.to_owned())
}

/// The catalogue tool the gateway's own matcher selects for the operation (C-652). The host is
/// the operation's, else the first account's, else the app's default host, else its only host.
fn computed_tool(app: &AppEntry, op: &Operation, accounts: &[GuideAccount]) -> Option<String> {
    let instance = app.instances.is_some();
    let host: String = match op.host.as_deref() {
        Some(h) if h != INSTANCE_HOST => h.to_owned(),
        _ => accounts
            .iter()
            .find_map(|a| a.host.clone())
            .or_else(|| app.instances.as_ref().and_then(|i| i.default_host.clone()))
            .or_else(|| sole_static_host(app))
            .unwrap_or_default(),
    };
    identify_tool_in(
        app,
        instance,
        &host,
        443,
        &placeholder_path(&op.path),
        None,
        &op.method,
        &[],
        false,
        &[],
        None,
    )
    .map(|t| t.id.clone())
}

fn access_map(
    states: impl Iterator<Item = (String, super::Access)>,
) -> BTreeMap<String, &'static str> {
    states.map(|(l, s)| (l, s.word())).collect()
}

fn size_of(doc: &Doc) -> usize {
    serde_json::to_vec(doc).map_or(usize::MAX, |v| v.len() + 1)
}

fn drop_descriptions(ops: &mut [OpOut]) {
    for o in ops {
        for p in &mut o.params {
            p.description = None;
        }
        if let Some(b) = &mut o.body {
            for f in &mut b.fields {
                f.description = None;
            }
        }
    }
}

/// Render `api.json`; `None` for a database app or an app with neither operations nor catalogue.
pub(super) fn render(
    app: &AppEntry,
    lost_hosts: &[&HostEntry],
    accounts: &[GuideAccount],
    package_version: &str,
    reference_available: bool,
    operations: Option<&OperationsFile>,
) -> Option<String> {
    if app.database.is_some() {
        return None;
    }
    let (read, write) = catalogue(app);
    let tools: Vec<_> = read.iter().chain(&write).copied().collect();
    if tools.is_empty() && operations.is_none_or(|o| o.operations.is_empty()) {
        return None;
    }
    let letters: Vec<String> = (0..accounts.len()).map(letter).collect();
    let access_of = |tool: &str| {
        let l = tools
            .iter()
            .find(|t| t.id == tool)
            .is_some_and(|t| lost(t, lost_hosts));
        access_map(
            accounts
                .iter()
                .zip(&letters)
                .map(|(a, l_)| (l_.clone(), tool_access(&a.access, tool, l))),
        )
    };
    let untooled = access_map(
        accounts
            .iter()
            .zip(&letters)
            .map(|(a, l)| (l.clone(), untooled_access(app, &a.access))),
    );
    let mut ops: Vec<OpOut> = Vec::new();
    let mut mapped: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for op in operations.iter().flat_map(|o| &o.operations) {
        let tool = op.tool.clone().or_else(|| computed_tool(app, op, accounts));
        if let Some(t) = &tool {
            mapped.insert(t.clone());
        }
        let access = match &tool {
            Some(t) => access_of(t),
            None => untooled.clone(),
        };
        ops.push(OpOut {
            id: inline(&op.id, 64),
            method: inline(&op.method, 16),
            path: inline(&op.path, 400),
            host: op
                .host
                .as_deref()
                .filter(|h| *h != INSTANCE_HOST)
                .map(|h| inline(h, 253)),
            summary: inline(&op.summary, SUMMARY_MAX),
            tags: op.tags.iter().map(|t| inline(t, 64)).collect(),
            params: op.params.iter().map(param_out).collect(),
            body: op.body.as_ref().map(body_out),
            pagination: op.pagination.as_ref().map(pagination_out),
            items: op.items.as_deref().map(|i| inline(i, 200)),
            tool: tool.map(|t| inline(&t, 64)),
            access,
            source: "package",
        });
    }
    for t in &tools {
        if mapped.contains(&t.id) {
            continue;
        }
        ops.push(OpOut {
            id: inline(&t.id, 64),
            method: t
                .methods
                .first()
                .map_or_else(|| "ANY".to_owned(), |m| inline(m, 16)),
            path: inline(&t.path, 400),
            host: Some(t.host.as_str())
                .filter(|h| *h != INSTANCE_HOST && !h.contains('*'))
                .map(|h| inline(h, 253)),
            summary: inline(&t.name, SUMMARY_MAX),
            tags: Vec::new(),
            params: Vec::new(),
            body: None,
            pagination: None,
            items: None,
            tool: Some(inline(&t.id, 64)),
            access: access_of(&t.id),
            source: "catalogue",
        });
    }
    let recipes = if reference_available {
        app.agent
            .iter()
            .flat_map(|a| &a.guide)
            .filter(|f| f.kind == GuideKind::Recipe)
            .map(|f| RecipeOut {
                file: inline(&f.file, 64),
                title: inline(&f.title, NAME_MAX),
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut doc = Doc {
        format: FORMAT,
        app: AppOut {
            id: inline(&app.app_id, 64),
            name: inline(&app.about.name, NAME_MAX),
            version: inline(package_version, 40),
        },
        accounts: accounts
            .iter()
            .zip(&letters)
            .map(|(a, l)| AccountOut {
                letter: l.clone(),
                connection_id: inline(&a.id, 64),
                label: inline(&a.label, 120),
                base_url: a
                    .host
                    .clone()
                    .or_else(|| sole_static_host(app))
                    .map(|h| format!("https://{}", inline(&h, 253))),
                grant: match a.access {
                    GrantAccess::Full => "full",
                    GrantAccess::Custom { .. } => "custom",
                },
            })
            .collect(),
        operations: ops,
        recipes,
        untooled,
        truncated: false,
    };
    // Drop order of 24 §5.4: parameter descriptions, then whole `params` of the largest tags,
    // then (never reached by a sane file) body fields and catalogue entries.
    if size_of(&doc) > AGENT_API_JSON_MAX {
        doc.truncated = true;
        drop_descriptions(&mut doc.operations);
    }
    if size_of(&doc) > AGENT_API_JSON_MAX {
        let mut weight: BTreeMap<String, usize> = BTreeMap::new();
        for o in &doc.operations {
            let w = serde_json::to_vec(&o.params).map_or(0, |v| v.len());
            *weight
                .entry(o.tags.first().cloned().unwrap_or_default())
                .or_default() += w;
        }
        let mut order: Vec<(String, usize)> = weight.into_iter().collect();
        order.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for (tag, _) in order {
            if size_of(&doc) <= AGENT_API_JSON_MAX {
                break;
            }
            for o in &mut doc.operations {
                if o.tags.first().cloned().unwrap_or_default() == tag {
                    o.params.clear();
                }
            }
        }
    }
    if size_of(&doc) > AGENT_API_JSON_MAX {
        for o in &mut doc.operations {
            o.body = None;
        }
    }
    while size_of(&doc) > AGENT_API_JSON_MAX
        && doc
            .operations
            .last()
            .is_some_and(|o| o.source == "catalogue")
    {
        doc.operations.pop();
    }
    let mut out = serde_json::to_string(&doc).ok()?;
    out.push('\n');
    Some(out)
}
