//! (P5-GUIDE-6, C-656, `docs/24 §3.5`) The fourth serializer over the import model: the operations
//! file `operations.json` (`aurora.operations/1`). Same sort as the other outputs, byte-stable.
//! Everything dropped to make the file valid is reported on stderr.
use std::cell::Cell;
use std::collections::BTreeMap;

use aurora_core::limits::{AGENT_OPERATIONS_FILE_MAX, AGENT_OPERATIONS_MAX};
use aurora_registry::operations::{
    Body, Field as OField, Operation as OOperation, OperationsFile, Pagination, Param, ParamIn,
    ParamType,
};
use serde_json::Value;

use super::{Model, Operation, resolve, text};

/// Headers an operations file may never declare (the linter's fixed list, `24 §3.5`).
fn header_forbidden(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(
        n.as_str(),
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "host"
            | "content-length"
            | "transfer-encoding"
            | "connection"
    ) || n.starts_with("x-aurora-")
        || n.starts_with("proxy-")
}

fn name_ok(n: &str) -> bool {
    (1..=64).contains(&n.len())
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '[' | ']' | '-'))
}

fn scalar_type(ty: &str) -> ParamType {
    match ty {
        t if t.starts_with("array") => ParamType::Array,
        "integer" => ParamType::Integer,
        "number" => ParamType::Number,
        "boolean" => ParamType::Boolean,
        _ => ParamType::String,
    }
}

fn path_ok(path: &str) -> bool {
    path.starts_with('/')
        && !path.contains("//")
        && !path.contains(['?', '#'])
        && path.split('/').skip(1).all(|s| {
            if let Some(n) = s.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                return name_ok(n);
            }
            !s.is_empty()
                && s != "."
                && s != ".."
                && s.chars().all(|c| {
                    c.is_ascii_alphanumeric()
                        || matches!(
                            c,
                            '.' | '_'
                                | '~'
                                | '!'
                                | '$'
                                | '&'
                                | '\''
                                | '('
                                | ')'
                                | '*'
                                | '+'
                                | ','
                                | ';'
                                | '='
                                | ':'
                                | '@'
                                | '%'
                                | '-'
                        )
                })
        })
}

fn template_names(path: &str) -> Vec<String> {
    path.split('/')
        .filter_map(|s| s.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
        .map(ToOwned::to_owned)
        .collect()
}

/// Scalar `enum` values and the `default` of a schema (at most 64 values).
pub(super) fn enum_default(
    root: &Value,
    schema: Option<&Value>,
) -> (Option<Vec<Value>>, Option<Value>) {
    let Some(s) = schema.and_then(|s| resolve(root, s).ok()) else {
        return (None, None);
    };
    let scalar = |v: &Value| matches!(v, Value::String(_) | Value::Number(_) | Value::Bool(_));
    let values = s
        .get("enum")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|v| scalar(v))
                .take(64)
                .map(|v| match v {
                    Value::String(x) => Value::String(text(x, 100)),
                    o => o.clone(),
                })
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty());
    let default = s.get("default").filter(|v| scalar(v)).map(|v| match v {
        Value::String(x) => Value::String(text(x, 100)),
        o => o.clone(),
    });
    (values, default)
}

/// `$` for an array response, `$.<prop>` when the object has exactly one array property.
pub(super) fn items_path(root: &Value, schema: &Value) -> Option<String> {
    let s = resolve(root, schema).ok()?;
    let is_array = |v: &Value| {
        resolve(root, v)
            .ok()
            .is_some_and(|r| r.get("type").and_then(Value::as_str) == Some("array"))
    };
    if is_array(s) {
        return Some("$".into());
    }
    let props = s.get("properties")?.as_object()?;
    let arrays: Vec<&String> = props
        .iter()
        .filter(|(_, v)| is_array(v))
        .map(|(k, _)| k)
        .collect();
    match arrays.as_slice() {
        [one]
            if !one.is_empty()
                && one
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')) =>
        {
            Some(format!("$.{one}"))
        }
        _ => None,
    }
}

fn convert(op: &Operation, notes: &mut Vec<String>) -> Option<OOperation> {
    let id = &op.tool_id;
    if !path_ok(&op.path) {
        notes.push(format!(
            "operations.json: skipped {} {} (the path is not a plain path template)",
            op.method, op.path
        ));
        return None;
    }
    let mut params: Vec<Param> = Vec::new();
    for p in &op.params {
        let location = match p.loc.as_str() {
            "path" => ParamIn::Path,
            "query" => ParamIn::Query,
            "header" => {
                if header_forbidden(&p.name) {
                    notes.push(format!(
                        "operations.json: dropped header parameter {} of {id} (credentials are the gateway's)",
                        p.name
                    ));
                    continue;
                }
                ParamIn::Header
            }
            _ => continue, // cookie parameters
        };
        if !name_ok(&p.name) {
            notes.push(format!(
                "operations.json: dropped parameter {:?} of {id} (not a valid name)",
                p.name
            ));
            continue;
        }
        params.push(Param {
            name: p.name.clone(),
            location,
            required: p.required.then_some(true),
            ty: scalar_type(&p.ty),
            description: Some(p.description.clone()).filter(|d| !d.is_empty()),
            values: p.values.clone(),
            default: p.default.clone(),
        });
    }
    // Every template segment is a declared path parameter, exactly once.
    for n in template_names(&op.path) {
        if !params
            .iter()
            .any(|p| p.location == ParamIn::Path && p.name == n)
        {
            params.insert(
                0,
                Param {
                    name: n,
                    location: ParamIn::Path,
                    required: Some(true),
                    ty: ParamType::String,
                    description: None,
                    values: None,
                    default: None,
                },
            );
        }
    }
    params.retain(|p| p.location != ParamIn::Path || template_names(&op.path).contains(&p.name));
    params.truncate(64);
    let body = op.op_body.as_ref().map(|(ct, fields)| Body {
        content_type: ct.clone(),
        fields: fields
            .iter()
            .filter(|f| name_ok(&f.name))
            .take(128)
            .map(|f| OField {
                name: f.name.clone(),
                required: f.required.then_some(true),
                ty: scalar_type(&f.ty),
                description: Some(f.description.clone()).filter(|d| !d.is_empty()),
                values: f.values.clone(),
                default: f.default.clone(),
            })
            .collect(),
    });
    let page = op
        .params
        .iter()
        .any(|p| p.loc == "query" && p.name == "page");
    let pagination = if op.link_header {
        Some(Pagination::LinkHeader)
    } else if page {
        Some(Pagination::PageParam {
            param: "page".into(),
            start: None,
            size_param: None,
        })
    } else {
        None
    };
    let summary = if op.summary.is_empty() {
        format!("{} {}", op.method, op.path)
    } else {
        op.summary.clone()
    };
    Some(OOperation {
        id: id.clone(),
        method: op.method.clone(),
        path: op.path.clone(),
        host: None,
        summary: summary.chars().take(200).collect(),
        tags: vec![super::slug(&op.tag).replace('-', "_")]
            .into_iter()
            .filter(|t| t.starts_with(|c: char| c.is_ascii_lowercase()) && t.len() <= 64)
            .collect(),
        params,
        body,
        pagination,
        items: op.items.clone(),
        project: None,
        tool: Some(id.clone()),
    })
}

fn render(f: &OperationsFile, compact: bool) -> String {
    let mut s = if compact {
        serde_json::to_string(f)
    } else {
        serde_json::to_string_pretty(f)
    }
    .unwrap_or_default();
    s.push('\n');
    s
}

/// The file text and what was dropped to fit the limits. `None` when no operation survives.
pub(super) fn generate(m: &Model) -> (Option<String>, Vec<String>) {
    let mut notes = Vec::new();
    let mut ops: Vec<OOperation> = m
        .ops
        .iter()
        .filter_map(|o| convert(o, &mut notes))
        .collect();
    if ops.is_empty() {
        return (None, notes);
    }
    if ops.len() > AGENT_OPERATIONS_MAX {
        notes.push(format!(
            "operations.json: kept the first {AGENT_OPERATIONS_MAX} of {} operations",
            ops.len()
        ));
        ops.truncate(AGENT_OPERATIONS_MAX);
    }
    let mut file = OperationsFile {
        format: aurora_registry::operations::FORMAT.to_owned(),
        operations: ops,
    };
    // Pretty (an author edits it) until it no longer fits, then compact before dropping detail.
    let compact = Cell::new(false);
    let fits = |f: &OperationsFile| render(f, compact.get()).len() <= AGENT_OPERATIONS_FILE_MAX;
    if !fits(&file) {
        notes.push("operations.json: written compact (one line) to fit the size limit".into());
        compact.set(true);
    }
    if !fits(&file) {
        notes.push("operations.json: dropped the parameter and field descriptions".into());
        for o in &mut file.operations {
            for p in &mut o.params {
                p.description = None;
            }
            for f in o.body.iter_mut().flat_map(|b| b.fields.iter_mut()) {
                f.description = None;
            }
        }
    }
    if !fits(&file) {
        notes.push("operations.json: dropped the top-level body fields".into());
        for o in &mut file.operations {
            if let Some(b) = &mut o.body {
                b.fields.clear();
            }
        }
    }
    if !fits(&file) {
        // `params` of the largest tags, until it fits.
        let mut weight: BTreeMap<String, usize> = BTreeMap::new();
        for o in &file.operations {
            *weight
                .entry(o.tags.first().cloned().unwrap_or_default())
                .or_default() += serde_json::to_vec(&o.params).map_or(0, |v| v.len());
        }
        let mut order: Vec<(String, usize)> = weight.into_iter().collect();
        order.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for (tag, _) in order {
            if fits(&file) {
                break;
            }
            notes.push(format!(
                "operations.json: dropped the parameters of tag {tag:?}"
            ));
            for o in &mut file.operations {
                if o.tags.first().cloned().unwrap_or_default() == tag {
                    // Path parameters stay: the linter needs every template segment declared.
                    o.params.retain(|p| p.location == ParamIn::Path);
                }
            }
        }
    }
    // The linter needs a file within the limit: if detail was not enough, the last operations go.
    let before = file.operations.len();
    while !fits(&file) && file.operations.len() > 1 {
        file.operations.pop();
    }
    if file.operations.len() < before {
        notes.push(format!(
            "operations.json: kept {} of {before} operations to fit the size limit",
            file.operations.len()
        ));
    }
    (Some(render(&file, compact.get())), notes)
}
