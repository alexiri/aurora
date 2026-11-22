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
use crate::snapshot::{AppEntry, HostEntry};

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
    /// (C-911) The account letter an operation is bound to (a webhook), else `null`.
    account: Option<String>,
    /// (C-911) The person's note for the agent (a webhook), else `null`.
    notes: Option<String>,
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

/// The catalogue tool every webhook operation maps to (C-909: a send-mode app has exactly one).
const WEBHOOK_TOOL: &str = "send";
const NOTES_MAX: usize = 1000;
const SLUG_MAX: usize = 40;

/// The host of a webhook connection (C-909, C-911).
pub(super) fn webhook_host(connection_id: &str) -> String {
    format!("{}.link.aurora.internal", inline(connection_id, 64))
}

/// `[a-z0-9_]`, at most [`SLUG_MAX`] characters; runs of anything else become one `_`.
pub(super) fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let out: String = out.trim_matches('_').chars().take(SLUG_MAX).collect();
    let out = out.trim_end_matches('_');
    if out.is_empty() {
        "webhook".to_owned()
    } else {
        out.to_owned()
    }
}

/// The operation ids of the webhook accounts, `send_<slug>` with `_2`, `_3`… for duplicates, in
/// account order (`None` for an account that is not a webhook).
pub(super) fn webhook_ids(accounts: &[GuideAccount]) -> Vec<Option<String>> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    accounts
        .iter()
        .map(|a| {
            let w = a.webhook.as_ref()?;
            let base = format!("send_{}", slug(&w.name));
            let n = seen.entry(base.clone()).or_default();
            *n += 1;
            Some(if *n == 1 { base } else { format!("{base}_{n}") })
        })
        .collect()
}

/// A slot's limits, as the sentence appended to its description.
pub(super) fn slot_description(s: &aurora_webhook::Slot) -> String {
    let mut parts: Vec<String> = Vec::new();
    let d = inline(&s.description, SUMMARY_MAX);
    if !d.is_empty() {
        parts.push(d);
    }
    if let Some(n) = s.max_length {
        parts.push(format!("at most {n} characters"));
    }
    if let Some(n) = s.max_items {
        parts.push(format!("at most {n} items"));
    }
    parts.join("; ")
}

fn slot_type_word(t: aurora_webhook::SlotType) -> &'static str {
    use aurora_webhook::SlotType as S;
    match t {
        S::String | S::Enum => "string",
        S::Integer => "integer",
        S::Number => "number",
        S::Boolean => "boolean",
        S::StringList => "array",
        S::Json => "json",
    }
}

fn webhook_operations(
    accounts: &[GuideAccount],
    letters: &[String],
    access_of: &dyn Fn(&str) -> BTreeMap<String, &'static str>,
) -> Vec<OpOut> {
    let ids = webhook_ids(accounts);
    let mut out = Vec::new();
    for ((a, letter), id) in accounts.iter().zip(letters).zip(ids) {
        let (Some(w), Some(id)) = (&a.webhook, id) else {
            continue;
        };
        let first = w.description.lines().find(|l| !l.trim().is_empty());
        let summary = match first {
            Some(l) => inline(
                &format!("{} — {}", inline(&w.name, 80), inline(l, SUMMARY_MAX)),
                SUMMARY_MAX,
            ),
            None => inline(&w.name, SUMMARY_MAX),
        };
        let fields = w
            .slots
            .iter()
            .map(|s| {
                let d = slot_description(s);
                ParamOut {
                    name: inline(&s.name, 64),
                    location: None,
                    required: s.required.then_some(true),
                    ty: slot_type_word(s.ty),
                    description: (!d.is_empty()).then_some(d),
                    values: s.r#enum.as_ref().map(|v| {
                        v.iter()
                            .map(|x| Value::String(inline(x, SUMMARY_MAX)))
                            .collect()
                    }),
                    default: None,
                }
            })
            .collect();
        let mut access = access_of(WEBHOOK_TOOL);
        access.retain(|k, _| k == letter);
        out.push(OpOut {
            id,
            method: inline(&w.method, 16),
            path: "/".to_owned(),
            host: Some(webhook_host(&a.id)),
            summary,
            tags: vec!["webhook".to_owned()],
            params: vec![ParamOut {
                name: "X-Aurora-Dry-Run".to_owned(),
                location: Some("header"),
                required: None,
                ty: "string",
                description: Some("Check and show the body without sending".to_owned()),
                values: Some(vec![Value::String("1".to_owned())]),
                default: None,
            }],
            body: Some(BodyOut {
                content_type: "application/json".to_owned(),
                fields,
            }),
            pagination: None,
            items: None,
            tool: Some(WEBHOOK_TOOL.to_owned()),
            access,
            account: Some(letter.clone()),
            notes: Some(inline(&w.description, NOTES_MAX)).filter(|n| !n.is_empty()),
            source: "connection",
        });
    }
    out
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
    let _ = reference_available; // P5-APPSKILL-2 replaces it with the skills of this app
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
            account: None,
            notes: None,
            source: "package",
        });
    }
    // (C-911) One operation per webhook account, in account-letter order.
    let webhook_ops = webhook_operations(accounts, &letters, &access_of);
    if !webhook_ops.is_empty() {
        mapped.insert(WEBHOOK_TOOL.to_owned());
    }
    ops.extend(webhook_ops);
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
            account: None,
            notes: None,
            source: "catalogue",
        });
    }
    // (C-1135) `recipes` stays in the file, always empty, for an `aurora-agent` one release older.
    let recipes: Vec<RecipeOut> = Vec::new();
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
                base_url: if a.webhook.is_some() {
                    Some(format!("https://{}", webhook_host(&a.id)))
                } else {
                    a.host
                        .clone()
                        .or_else(|| sole_static_host(app))
                        .map(|h| format!("https://{}", inline(&h, 253)))
                },
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
