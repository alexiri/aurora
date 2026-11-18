//! `aurora plugin import-openapi` (docs/24 §10, C-650, D-172(a)): reference files and a catalogue
//! skeleton from a local OpenAPI 3.0/3.1 document. Offline, deterministic (sorted, no timestamps),
//! and every string taken from the document is untrusted: HTML tags stripped, one line, clamped.
//!
//! The parser fills one intermediate [`Model`] (operations with parameters, body fields, tags and
//! the generated tool id); the three outputs are serializers over it, so P5-GUIDE-6's
//! `operations.json` is a fourth serializer, not a second parse.

use anyhow::{Context, Result, anyhow, bail};
use aurora_core::limits::{AGENT_GUIDE_APP_MAX, AGENT_GUIDE_FILE_MAX, AGENT_GUIDE_FILES_MAX};
use aurora_core::sanitize::inline;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];
const DESC_MAX: usize = 160;
const SUMMARY_MAX: usize = 200;
const NAME_MAX: usize = 80;
const REF_DEPTH: usize = 16;
/// Room under the per-file limit for the header of a split `operations` file.
const CHUNK_MAX: usize = AGENT_GUIDE_FILE_MAX - 512;

/// One parameter or top-level body field.
#[derive(Debug, Clone)]
pub(crate) struct Field {
    pub name: String,
    /// `path`, `query`, `header`, `cookie` or `body`.
    pub loc: String,
    pub required: bool,
    pub ty: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Operation {
    pub method: String,
    /// The server's base path plus the document's path, `{param}` segments kept.
    pub path: String,
    pub tag: String,
    pub summary: String,
    pub operation_id: Option<String>,
    pub params: Vec<Field>,
    pub body: Vec<Field>,
    /// Lower snake case, unique within the model.
    pub tool_id: String,
}

#[derive(Debug)]
pub(crate) struct Model {
    pub ops: Vec<Operation>,
    pub host: Option<String>,
}

// ---------------------------------------------------------------- reading the document

fn yaml_to_json(v: serde_yaml::Value) -> Value {
    use serde_yaml::Value as Y;
    match v {
        Y::Null => Value::Null,
        Y::Bool(b) => Value::Bool(b),
        Y::Number(n) => n
            .as_i64()
            .map(Value::from)
            .or_else(|| n.as_u64().map(Value::from))
            .or_else(|| n.as_f64().map(Value::from))
            .unwrap_or(Value::Null),
        Y::String(s) => Value::String(s),
        Y::Sequence(s) => Value::Array(s.into_iter().map(yaml_to_json).collect()),
        Y::Mapping(m) => Value::Object(
            m.into_iter()
                .map(|(k, v)| {
                    let key = match k {
                        Y::String(s) => s,
                        Y::Number(n) => n.to_string(),
                        Y::Bool(b) => b.to_string(),
                        _ => String::new(),
                    };
                    (key, yaml_to_json(v))
                })
                .collect(),
        ),
        Y::Tagged(t) => yaml_to_json(t.value),
    }
}

fn load(text: &str) -> Result<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return Ok(v);
    }
    let y: serde_yaml::Value =
        serde_yaml::from_str(text).map_err(|e| anyhow!("not JSON or YAML: {e}"))?;
    Ok(yaml_to_json(y))
}

/// Follows local `$ref`s; an external one is an error naming it.
fn resolve<'a>(root: &'a Value, mut v: &'a Value) -> Result<&'a Value> {
    for _ in 0..REF_DEPTH {
        let Some(r) = v.get("$ref").and_then(Value::as_str) else {
            return Ok(v);
        };
        let Some(pointer) = r.strip_prefix('#') else {
            bail!("external $ref `{r}` is not supported (inline it first)");
        };
        v = root
            .pointer(pointer)
            .ok_or_else(|| anyhow!("$ref `{r}` does not resolve"))?;
    }
    bail!("$ref chain deeper than {REF_DEPTH} (cyclic?)")
}

/// Descriptions come from a third party: tags stripped, one line, clamped, no code-span breakers.
fn text(s: &str, max: usize) -> String {
    let mut plain = String::with_capacity(s.len());
    // Bidi and other format characters are removed outright, like the control characters.
    let mut chars = s
        .chars()
        .filter(|c| !aurora_core::sanitize::is_stripped(*c))
        .peekable();
    while let Some(c) = chars.next() {
        if c == '<'
            && chars
                .peek()
                .is_some_and(|n| n.is_ascii_alphabetic() || *n == '/' || *n == '!')
        {
            for t in chars.by_ref() {
                if t == '>' {
                    break;
                }
            }
            plain.push(' ');
        } else {
            plain.push(c);
        }
    }
    inline(&plain, max).replace('`', "'")
}

fn type_of(root: &Value, schema: Option<&Value>) -> String {
    let Some(s) = schema.and_then(|s| resolve(root, s).ok()) else {
        return "any".into();
    };
    let t = match s.get("type") {
        Some(Value::String(t)) => Some(t.as_str()),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).find(|t| *t != "null"),
        _ => None,
    };
    match t {
        Some("array") => format!("array<{}>", type_of(root, s.get("items"))),
        Some(t) => text(t, 24),
        None if s.get("properties").is_some() => "object".into(),
        None => "any".into(),
    }
}

fn fields_of_schema(root: &Value, schema: &Value, depth: usize, out: &mut Vec<Field>) {
    let Ok(s) = resolve(root, schema) else { return };
    if depth < 2
        && let Some(parts) = s.get("allOf").and_then(Value::as_array)
    {
        for p in parts {
            fields_of_schema(root, p, depth + 1, out);
        }
    }
    let required: BTreeSet<&str> = s
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if let Some(props) = s.get("properties").and_then(Value::as_object) {
        for (name, p) in props {
            out.push(Field {
                name: text(name, 64),
                loc: "body".into(),
                required: required.contains(name.as_str()),
                ty: type_of(root, Some(p)),
                description: text(
                    resolve(root, p)
                        .ok()
                        .and_then(|p| p.get("description"))
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    DESC_MAX,
                ),
            });
        }
    }
}

fn param_fields(root: &Value, list: Option<&Value>) -> Result<Vec<Field>> {
    let mut out = Vec::new();
    for p in list.and_then(Value::as_array).into_iter().flatten() {
        let p = resolve(root, p)?;
        let (Some(name), Some(loc)) = (
            p.get("name").and_then(Value::as_str),
            p.get("in").and_then(Value::as_str),
        ) else {
            continue;
        };
        out.push(Field {
            name: text(name, 64),
            loc: text(loc, 12),
            required: loc == "path" || p.get("required").and_then(Value::as_bool) == Some(true),
            ty: type_of(root, p.get("schema")),
            description: text(
                p.get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                DESC_MAX,
            ),
        });
    }
    Ok(out)
}

fn body_fields(root: &Value, op: &Value) -> Result<Vec<Field>> {
    let Some(rb) = op.get("requestBody") else {
        return Ok(Vec::new());
    };
    let rb = resolve(root, rb)?;
    let Some(content) = rb.get("content").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    let media = content.get("application/json").or_else(|| {
        content
            .iter()
            .find(|(k, _)| k.ends_with("+json"))
            .map(|(_, v)| v)
    });
    let mut out = Vec::new();
    if let Some(schema) = media.and_then(|m| m.get("schema")) {
        fields_of_schema(root, schema, 0, &mut out);
    }
    Ok(out)
}

/// `(host, base path)` of the first server: `https://api.example.com/v1/` → `api.example.com`, `/v1`.
fn server_of(root: &Value) -> (Option<String>, String) {
    let Some(url) = root
        .pointer("/servers/0/url")
        .and_then(Value::as_str)
        .map(str::trim)
    else {
        return (None, String::new());
    };
    let (host, path) = match url.split_once("://") {
        Some((_, rest)) => match rest.split_once('/') {
            Some((h, p)) => (Some(h.to_owned()), format!("/{p}")),
            None => (Some(rest.to_owned()), String::new()),
        },
        None => (None, url.to_owned()),
    };
    let host = host
        .map(|h| h.split(':').next().unwrap_or_default().to_owned())
        .filter(|h| !h.is_empty() && !h.contains('{'));
    (host, path.trim_end_matches('/').to_owned())
}

fn snake(s: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in s.chars() {
        if c.is_ascii_uppercase() {
            if prev_lower {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
            prev_lower = false;
        } else if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_lower = true;
        } else {
            if !out.ends_with('_') {
                out.push('_');
            }
            prev_lower = false;
        }
    }
    out.trim_matches('_').to_owned()
}

/// `[a-z][a-z0-9_-]{1,47}`, from `operationId`, else from method and path.
fn tool_id_base(op: &Operation) -> String {
    let raw = op.operation_id.clone().unwrap_or_else(|| {
        let segs: Vec<String> = op
            .path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| {
                if s.starts_with('{') {
                    format!("by_{}", s.trim_matches(['{', '}']))
                } else {
                    s.to_owned()
                }
            })
            .collect();
        format!("{}_{}", op.method.to_lowercase(), segs.join("_"))
    });
    let mut id = snake(&raw);
    if !id.starts_with(|c: char| c.is_ascii_lowercase()) {
        id = format!("op_{id}");
    }
    if id.len() < 2 {
        id.push_str("_op");
    }
    id.truncate(44);
    id.trim_end_matches('_').to_owned()
}

pub(crate) fn parse(doc: &str, host_override: Option<&str>) -> Result<Model> {
    let root = load(doc)?;
    if root.get("swagger").is_some() {
        bail!(
            "Swagger 2.0 documents are not read: convert to OpenAPI 3 first (for example with swagger2openapi)"
        );
    }
    match root.get("openapi").and_then(Value::as_str) {
        Some(v) if v.starts_with("3.") => {}
        _ => bail!("not an OpenAPI 3.0 or 3.1 document (no `openapi: 3.x` key)"),
    }
    let (server_host, base) = server_of(&root);
    let paths = root
        .get("paths")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("the document has no `paths`"))?;
    let mut ops = Vec::new();
    for (path, item) in paths {
        let item = resolve(&root, item)?;
        let shared = param_fields(&root, item.get("parameters"))?;
        for m in METHODS {
            let Some(op) = item.get(m) else { continue };
            let op = resolve(&root, op)?;
            let mut params = shared.clone();
            for p in param_fields(&root, op.get("parameters"))? {
                params.retain(|q| !(q.name == p.name && q.loc == p.loc));
                params.push(p);
            }
            let rank = |l: &str| {
                ["path", "query", "header", "cookie"]
                    .iter()
                    .position(|x| *x == l)
            };
            params.sort_by(|a, b| (rank(&a.loc), &a.name).cmp(&(rank(&b.loc), &b.name)));
            let tag = op
                .get("tags")
                .and_then(Value::as_array)
                .and_then(|t| t.first())
                .and_then(Value::as_str)
                .map(|t| text(t, 48))
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "other".into());
            let operation_id = op
                .get("operationId")
                .and_then(Value::as_str)
                .map(|s| text(s, 64))
                .filter(|s| !s.is_empty());
            let summary = text(
                op.get("summary")
                    .and_then(Value::as_str)
                    .or_else(|| op.get("description").and_then(Value::as_str))
                    .unwrap_or_default(),
                SUMMARY_MAX,
            );
            ops.push(Operation {
                method: m.to_uppercase(),
                path: format!("{base}{}", text(path, 300)),
                tag,
                summary,
                operation_id,
                params,
                body: body_fields(&root, op)?,
                tool_id: String::new(),
            });
        }
    }
    if ops.is_empty() {
        bail!("the document has no operations");
    }
    ops.sort_by(|a, b| (&a.tag, &a.path, &a.method).cmp(&(&b.tag, &b.path, &b.method)));
    let mut used: BTreeSet<String> = BTreeSet::new();
    for op in &mut ops {
        let base = tool_id_base(op);
        let mut id = base.clone();
        let mut n = 2;
        while !used.insert(id.clone()) {
            let suffix = format!("_{n}");
            id = format!("{}{suffix}", &base[..base.len().min(47 - suffix.len())]);
            n += 1;
        }
        op.tool_id = id;
    }
    Ok(Model {
        ops,
        host: host_override.map(str::to_owned).or(server_host),
    })
}

// ---------------------------------------------------------------- the three outputs

/// File-name stem of a tag: lower case, `[a-z0-9_-]`, at most 40 characters.
fn slug(tag: &str) -> String {
    let mut s: String = tag
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-');
    let s = if s.is_empty() { "other" } else { s };
    s.chars().take(40).collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Detail {
    Full,
    NoBody,
    NoDescriptions,
}

fn field_line(f: &Field, detail: Detail) -> String {
    let mut l = format!(
        "- `{}` ({}, {}, {})",
        f.name,
        f.loc,
        if f.required { "required" } else { "optional" },
        f.ty
    );
    if detail != Detail::NoDescriptions && !f.description.is_empty() {
        let _ = write!(l, ": {}", f.description);
    }
    l
}

fn op_section(op: &Operation, detail: Detail) -> String {
    let mut o = format!("## {} {}\n\n", op.method, op.path);
    if !op.summary.is_empty() {
        let _ = writeln!(o, "{}\n", op.summary);
    }
    if !op.params.is_empty() {
        o.push_str("Parameters:\n");
        for p in &op.params {
            let _ = writeln!(o, "{}", field_line(p, detail));
        }
        o.push('\n');
    }
    if detail == Detail::Full && !op.body.is_empty() {
        o.push_str("Body fields (JSON, top level):\n");
        for f in &op.body {
            let _ = writeln!(o, "{}", field_line(f, detail));
        }
        o.push('\n');
    }
    o
}

fn index_line(op: &Operation) -> String {
    let mut l = format!("- {} {}", op.method, op.path);
    if !op.summary.is_empty() {
        let _ = write!(l, ": {}", op.summary);
    }
    let _ = write!(
        l,
        " [{}]",
        op.operation_id.as_deref().unwrap_or(&op.tool_id)
    );
    l
}

/// The operation index, split into files under the per-file limit when it must be.
fn index_files(app_id: &str, ops: &[Operation]) -> Vec<(String, String)> {
    let header = format!(
        "# {app_id}: operations by tag\n\nOne line per operation: `METHOD path: summary [operationId]`. Details of each tag are in its `ops-<tag>.md` file when one exists.\n"
    );
    let mut files: Vec<String> = vec![header.clone()];
    let mut tag = "";
    for op in ops {
        let mut chunk = String::new();
        if op.tag != tag {
            tag = &op.tag;
            let _ = write!(chunk, "\n## {tag}\n\n");
        }
        chunk.push_str(&index_line(op));
        chunk.push('\n');
        if files
            .last()
            .is_some_and(|f| f.len() + chunk.len() > CHUNK_MAX)
        {
            let mut next = format!("{header}\n## {}\n\n", op.tag);
            if chunk.starts_with('\n') {
                chunk = chunk
                    .split_once("\n\n")
                    .map(|(_, r)| r.to_owned())
                    .unwrap_or(chunk);
            }
            next.push_str(&chunk);
            files.push(next);
        } else if let Some(f) = files.last_mut() {
            f.push_str(&chunk);
        }
    }
    files
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let name = if i == 0 {
                "operations.md".to_owned()
            } else {
                format!("operations-{}.md", i + 1)
            };
            (name, c)
        })
        .collect()
}

struct Built {
    /// `(file, title, text)` in listing order.
    files: Vec<(String, String, String)>,
    /// Tag file names by tag, for the drop step.
    tag_files: BTreeMap<String, String>,
}

fn build(app_id: &str, m: &Model, detail: Detail) -> Result<Built> {
    let mut files: Vec<(String, String, String)> = Vec::new();
    for (i, (name, text)) in index_files(app_id, &m.ops).into_iter().enumerate() {
        let title = if i == 0 {
            "Operation index: every operation, grouped by tag".to_owned()
        } else {
            format!("Operation index, part {}", i + 1)
        };
        files.push((name, title, text));
    }
    // Tags, merged into `other` (smallest first) when there are more files than allowed.
    let mut by_tag: BTreeMap<String, Vec<&Operation>> = BTreeMap::new();
    for op in &m.ops {
        by_tag.entry(op.tag.clone()).or_default().push(op);
    }
    while files.len() + by_tag.len() > AGENT_GUIDE_FILES_MAX {
        let Some(smallest) = by_tag
            .iter()
            .filter(|(t, _)| *t != "other")
            .min_by_key(|(t, ops)| (ops.len(), (*t).clone()))
            .map(|(t, _)| t.clone())
        else {
            bail!(
                "too many operation index files for one app ({})",
                files.len()
            );
        };
        let moved = by_tag.remove(&smallest).unwrap_or_default();
        by_tag.entry("other".into()).or_default().extend(moved);
    }
    let mut tag_files = BTreeMap::new();
    let mut taken: BTreeSet<String> = BTreeSet::new();
    for (tag, ops) in &by_tag {
        let mut stem = format!("ops-{}", slug(tag));
        let mut n = 2;
        while !taken.insert(stem.clone()) {
            stem = format!("ops-{}-{n}", slug(tag));
            n += 1;
        }
        let mut sorted = ops.clone();
        sorted.sort_by(|a, b| (&a.path, &a.method).cmp(&(&b.path, &b.method)));
        let mut body = format!("# Operations: {tag}\n\n");
        for op in sorted {
            body.push_str(&op_section(op, detail));
        }
        let file = format!("{stem}.md");
        tag_files.insert(tag.clone(), file.clone());
        files.push((file, format!("Operations: {tag}"), body));
    }
    Ok(Built { files, tag_files })
}

/// `(file, title, text)`.
type GuideFile = (String, String, String);

fn fits(files: &[GuideFile]) -> bool {
    files.len() <= AGENT_GUIDE_FILES_MAX
        && files
            .iter()
            .all(|(_, _, t)| t.len() <= AGENT_GUIDE_FILE_MAX)
        && files.iter().map(|(_, _, t)| t.len()).sum::<usize>() <= AGENT_GUIDE_APP_MAX
}

/// Files within every limit, and what was dropped to get there (stderr).
fn generate(app_id: &str, m: &Model) -> Result<(Vec<GuideFile>, Vec<String>)> {
    let mut notes = Vec::new();
    let mut built = build(app_id, m, Detail::Full)?;
    for (detail, what) in [
        (Detail::NoBody, "dropped the top-level body fields"),
        (Detail::NoDescriptions, "dropped the parameter descriptions"),
    ] {
        if fits(&built.files) {
            break;
        }
        notes.push(what.to_owned());
        built = build(app_id, m, detail)?;
    }
    // The `ops-<tag>.md` files of the largest tags go next; their operations stay in the index.
    while !fits(&built.files) {
        let largest = built
            .files
            .iter()
            .filter(|(f, _, _)| built.tag_files.values().any(|t| t == f))
            .max_by_key(|(f, _, t)| (t.len(), std::cmp::Reverse(f.clone())))
            .map(|(f, _, _)| f.clone());
        let Some(file) = largest else {
            bail!("the operation index alone exceeds the agent guide limits of one app");
        };
        notes.push(format!(
            "dropped {file} (largest tag); its operations stay in the index"
        ));
        built.files.retain(|(f, _, _)| *f != file);
    }
    Ok((built.files, notes))
}

/// `tools:` skeleton: reads (`GET`/`HEAD`), then writes; more specific paths first within a group.
fn skeleton(m: &Model) -> String {
    let host = m.host.clone().unwrap_or_else(|| "TODO-host".to_owned());
    let pattern = |p: &str| -> String {
        let segs: Vec<&str> = p
            .split('/')
            .map(|s| if s.contains('{') { "*" } else { s })
            .collect();
        segs.join("/")
    };
    let mut out = String::from("tools:\n");
    for (group, is_read) in [("read", true), ("write", false)] {
        let mut ops: Vec<(String, &Operation)> = m
            .ops
            .iter()
            .filter(|o| (o.method == "GET" || o.method == "HEAD") == is_read)
            .map(|o| (pattern(&o.path), o))
            .collect();
        if ops.is_empty() {
            continue;
        }
        let literal = |p: &str| p.split('/').filter(|s| !s.is_empty() && *s != "*").count();
        ops.sort_by(|(pa, a), (pb, b)| {
            literal(pb)
                .cmp(&literal(pa))
                .then(pb.len().cmp(&pa.len()))
                .then((pa, &a.method).cmp(&(pb, &b.method)))
        });
        let _ = writeln!(out, "  {group}:\n    items:");
        for (p, o) in ops {
            let name = if o.summary.is_empty() {
                format!("{} {}", o.method, o.path)
            } else {
                o.summary.clone()
            };
            let name: String = name.chars().take(NAME_MAX).collect();
            let _ = writeln!(
                out,
                "      - {{ id: {}, name: {}, host: {host}, path: {}, methods: [{}] }}",
                o.tool_id,
                serde_json::to_string(&name).unwrap_or_default(),
                serde_json::to_string(&p).unwrap_or_default(),
                o.method
            );
        }
    }
    out
}

fn valid_app_id(id: &str) -> bool {
    let b = id.as_bytes();
    (2..=48).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_')
}

/// Removes the files an earlier run wrote, so a regeneration never leaves an unlisted file behind.
fn clear_generated(dir: &Path) -> Result<()> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let generated = n.starts_with("ops-")
            || n == "operations.md"
            || (n.starts_with("operations-") && n.ends_with(".md"));
        if generated && e.path().is_file() {
            std::fs::remove_file(e.path())
                .with_context(|| format!("removing {}", e.path().display()))?;
        }
    }
    Ok(())
}

pub fn import(file: &Path, app: &str, host: Option<&str>, dir: &Path) -> Result<i32> {
    if !valid_app_id(app) {
        bail!("`{app}` is not an app id ([a-z][a-z0-9_-]{{1,47}})");
    }
    let doc =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let model = parse(&doc, host)?;
    let (files, notes) = generate(app, &model)?;
    let out_dir = dir.join("agent").join(app);
    std::fs::create_dir_all(&out_dir)?;
    clear_generated(&out_dir)?;
    for (name, _, content) in &files {
        std::fs::write(out_dir.join(name), content).with_context(|| format!("writing {name}"))?;
    }
    for n in &notes {
        eprintln!("import-openapi: {n}");
    }
    eprintln!(
        "import-openapi: wrote {} files for {} operations to {}",
        files.len(),
        model.ops.len(),
        out_dir.display()
    );
    if model.host.is_none() {
        eprintln!(
            "import-openapi: no host (--host or servers[].url): the skeleton carries TODO-host"
        );
    }
    let mut out = String::from(
        "# Add under apps[].agent in plugin.yaml (summary is required with a guide):\nguide:\n",
    );
    for (name, title, _) in &files {
        let _ = writeln!(
            out,
            "  - {{ file: {name}, title: {} }}",
            serde_json::to_string(title).unwrap_or_default()
        );
    }
    out.push_str(
        "\n# Catalogue skeleton for apps[]: review overlaps, approvals and `params` classifiers.\n",
    );
    out.push_str(&skeleton(&model));
    print!("{out}");
    Ok(0)
}
