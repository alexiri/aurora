//! (P5-GUIDE-7, `docs/24 §10a` *Platform target*, C-654, D-297, SEC-119, SEC-121) The reserved
//! target `aurora`: the platform tools, reached over the supervisor's tools socket exactly like the
//! MCP bridge does. Same registry, same descriptors, same audit; only the transport differs.
//!
//! The socket decides who may call (FIX-151): the call is bound to the conversation the harness
//! process was registered for, so a process that is not a descendant of the harness is refused
//! `session_not_caller` here as everywhere. The session id comes from `AURORA_SESSION` (set by the
//! supervisor in the harness environment only); without it nothing is sent.
use std::path::Path;

use aurora_core::limits::AGENT_CLI_PREVIEW_MAX;
use aurora_protocol::tools::{ToolContent, ToolDescriptor, ToolResult};
use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use super::call::{CallOpts, EXIT_UPSTREAM, Env, Outcome};
use super::files;
use super::preview;
use super::request::{Arg, parse_args};
use super::text::{bound, oneline};
use crate::tools::{BridgeRequest, BridgeResponse, Method};

/// Exit code of a tools-socket transport failure (C-653).
pub const EXIT_TRANSPORT: u8 = 4;
const NO_SESSION: &str = "platform tools are available inside a conversation turn (no session: AURORA_SESSION is not set)";
/// A result up to this size is printed whole; a larger one is saved and previewed.
const INLINE_MAX: usize = 1_500;
/// Largest `--body` file.
const BODY_MAX: usize = 1024 * 1024;
const SAVE_MAX: usize = 16 * 1024 * 1024;

fn session(env: &Env) -> Result<&str, Outcome> {
    env.session
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Outcome::usage(NO_SESSION))
}

async fn exchange(socket: &Path, req: &BridgeRequest) -> Result<BridgeResponse, String> {
    let mut s = UnixStream::connect(socket).await.map_err(|e| {
        format!(
            "cannot reach the platform tools socket {}: {e}",
            socket.display()
        )
    })?;
    let mut line = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    line.push(b'\n');
    s.write_all(&line)
        .await
        .map_err(|e| format!("platform tools socket: {e}"))?;
    let mut r = BufReader::new(s);
    let mut resp = String::new();
    r.read_line(&mut resp)
        .await
        .map_err(|e| format!("platform tools socket: {e}"))?;
    serde_json::from_str(&resp).map_err(|e| format!("platform tools socket: bad reply: {e}"))
}

/// A refusal the socket answered (not a transport failure): usage-level for the caller refusals,
/// transport-level otherwise.
fn refusal(e: &crate::tools::BridgeError) -> Outcome {
    let code = match e.code.as_str() {
        "session_not_caller" | "no_active_turn" => super::call::EXIT_USAGE,
        _ => EXIT_TRANSPORT,
    };
    Outcome::fail(code, format!("{}: {}", e.code, e.message))
}

/// The tools the agent's conversation may use, as the supervisor lists them.
pub async fn fetch_tools(env: &Env) -> Result<Vec<ToolDescriptor>, Outcome> {
    let session = session(env)?;
    let req = BridgeRequest {
        id: 1,
        session: session.to_owned(),
        method: Method::List,
        tool: None,
        arguments: None,
    };
    match exchange(&env.tools_socket, &req)
        .await
        .map_err(|e| Outcome::fail(EXIT_TRANSPORT, e))?
    {
        BridgeResponse {
            result: Some(v), ..
        } => serde_json::from_value(v)
            .map_err(|e| Outcome::fail(EXIT_TRANSPORT, format!("platform tools: bad list: {e}"))),
        BridgeResponse { error: Some(e), .. } => Err(refusal(&e)),
        _ => Err(Outcome::fail(EXIT_TRANSPORT, "platform tools: empty reply")),
    }
}

fn family_of(t: &ToolDescriptor) -> &str {
    t.family.as_deref().unwrap_or("other")
}

fn first_line(s: &str, max: usize) -> String {
    oneline(s.lines().next().unwrap_or(""), max)
}

/// `list aurora`: the tools by family.
#[must_use]
pub fn list(tools: &[ToolDescriptor]) -> String {
    let mut sorted: Vec<&ToolDescriptor> = tools.iter().collect();
    sorted.sort_by(|a, b| (family_of(a), &a.name).cmp(&(family_of(b), &b.name)));
    let mut o = format!("aurora (platform tools) · {} tools\n", sorted.len());
    for t in sorted {
        o.push_str(&format!(
            "{} [{}] — {}\n",
            oneline(&t.name, 64),
            oneline(family_of(t), 24),
            first_line(t.description.as_deref().unwrap_or(""), 120)
        ));
    }
    bound(&o)
}

/// The hits of `search` for the platform tools: `(score, line)`; the caller merges them with the
/// apps' hits.
#[must_use]
pub fn search_hits(
    tools: &[ToolDescriptor],
    terms: &[String],
) -> Vec<(u32, String, String, String)> {
    let mut out = Vec::new();
    for t in tools {
        let desc = t.description.clone().unwrap_or_default();
        let s = super::show::score(terms, &[(&t.name, 6), (family_of(t), 4), (&desc, 3)]);
        if s > 0 {
            out.push((
                s,
                super::PLATFORM_TARGET.to_owned(),
                t.name.clone(),
                format!(
                    "aurora {} [{}] — {}",
                    oneline(&t.name, 64),
                    oneline(family_of(t), 24),
                    first_line(&desc, 120)
                ),
            ));
        }
    }
    out
}

fn type_of(schema: &Value) -> String {
    match schema.get("type") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .filter(|t| *t != "null")
            .collect::<Vec<_>>()
            .join("|"),
        _ => "any".to_owned(),
    }
}

/// `describe aurora <tool>`: the parameters as a table and an example call.
pub fn describe(tools: &[ToolDescriptor], name: &str) -> Result<String, Outcome> {
    let Some(t) = tools.iter().find(|t| t.name == name) else {
        return Err(Outcome::usage(format!(
            "unknown platform tool {name:?}; `aurora-agent list aurora` shows them"
        )));
    };
    let mut o = format!(
        "aurora {} [{}]\n{}\n",
        oneline(&t.name, 64),
        oneline(family_of(t), 24),
        t.description.as_deref().unwrap_or("").trim()
    );
    let props = t.input_schema.get("properties").and_then(Value::as_object);
    let required: Vec<&str> = t
        .input_schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut example = format!("aurora-agent call aurora {}", t.name);
    o.push_str("parameters:\n");
    if let Some(p) = props.filter(|p| !p.is_empty()) {
        for (k, v) in p {
            let ty = type_of(v);
            let req = required.contains(&k.as_str());
            o.push_str(&format!(
                "  {} ({}{}){}\n",
                oneline(k, 64),
                ty,
                if req { ", required" } else { "" },
                v.get("description")
                    .and_then(Value::as_str)
                    .map_or_else(String::new, |d| format!(" — {}", first_line(d, 160)))
            ));
            if req {
                match ty.as_str() {
                    "string" => example.push_str(&format!(" {k}=<text>")),
                    "integer" | "number" | "boolean" => example.push_str(&format!(" {k}=<{ty}>")),
                    _ => example.push_str(&format!(" {k}:=<json>")),
                }
            }
        }
    } else {
        o.push_str("  (none)\n");
    }
    o.push_str(&format!("example: {example}\n"));
    Ok(bound(&o))
}

/// One argument coerced by the schema's top-level type (`name=value` text becomes the declared
/// type; `name:=<json>` is taken as given).
fn coerce(schema: Option<&Value>, name: &str, raw: &str) -> Result<Value, String> {
    let ty = schema.map(type_of).unwrap_or_default();
    let bad = |what: &str| format!("argument {name}: {raw:?} is not {what}");
    match ty.as_str() {
        "integer" => raw
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| bad("an integer")),
        "number" => raw
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .ok_or_else(|| bad("a number")),
        "boolean" => match raw {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(bad("true or false")),
        },
        "array" | "object" => Err(format!(
            "argument {name} is {ty}: pass it as {name}:=<json>"
        )),
        _ => Ok(Value::String(raw.to_owned())),
    }
}

fn read_body(spec: &str) -> Result<Map<String, Value>, String> {
    use std::io::Read;
    let mut buf = Vec::new();
    let r = if spec == "-" {
        std::io::stdin()
            .take(BODY_MAX as u64 + 1)
            .read_to_end(&mut buf)
    } else {
        let p = spec.strip_prefix('@').unwrap_or(spec);
        std::fs::File::open(p)
            .map_err(|e| format!("--body {p}: {e}"))?
            .take(BODY_MAX as u64 + 1)
            .read_to_end(&mut buf)
    };
    r.map_err(|e| format!("--body: {e}"))?;
    if buf.len() > BODY_MAX {
        return Err(format!("--body is over {BODY_MAX} bytes"));
    }
    match serde_json::from_slice::<Value>(&buf) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => Err("--body must be a JSON object (the tool's arguments)".into()),
        Err(e) => Err(format!("--body is not valid JSON: {e}")),
    }
}

fn text_of(r: &ToolResult) -> String {
    r.content
        .iter()
        .flatten()
        .filter_map(|c| match c {
            ToolContent::Text(t) => Some(t.text.clone()),
            ToolContent::Json(j) => Some(j.value.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `call aurora <tool> [name=value | name:=<json> …] [--body @file] [--out F] [--raw]`.
pub async fn call(env: &Env, rest: &[String], opts: &CallOpts) -> Outcome {
    let Some((tool, args)) = rest.split_first() else {
        return Outcome::usage("call aurora: name a tool (`aurora-agent list aurora`)");
    };
    let session = match session(env) {
        Ok(s) => s,
        Err(o) => return o,
    };
    let parsed = match parse_args(args) {
        Ok(a) => a,
        Err(e) => return Outcome::usage(e),
    };
    let mut arguments = match &opts.body {
        Some(b) => match read_body(b) {
            Ok(m) => m,
            Err(e) => return Outcome::usage(e),
        },
        None => Map::new(),
    };
    // The schema is only help for coercion; the registry validates on the platform side. A tool
    // the list does not show is still sent (it is refused there, SEC-121).
    let schema = if parsed.iter().any(|a| matches!(a, Arg::Plain(..))) {
        fetch_tools(env)
            .await
            .ok()
            .and_then(|t| t.into_iter().find(|t| &t.name == tool))
            .map(|t| t.input_schema)
    } else {
        None
    };
    for a in parsed {
        match a {
            Arg::Raw(n, v) => {
                arguments.insert(n, v);
            }
            Arg::Plain(n, raw) => {
                let prop = schema.as_ref().and_then(|s| s.get("properties")?.get(&n));
                match coerce(prop, &n, &raw) {
                    Ok(v) => {
                        arguments.insert(n, v);
                    }
                    Err(e) => return Outcome::usage(e),
                }
            }
        }
    }
    let out_path = match &opts.out {
        Some(p) => match files::check_out(p, &env.cwd, &env.protected) {
            Ok(p) => Some(p),
            Err(e) => return Outcome::usage(e),
        },
        None => None,
    };
    let req = BridgeRequest {
        id: 1,
        session: session.to_owned(),
        method: Method::Call,
        tool: Some(tool.clone()),
        arguments: Some(Value::Object(arguments)),
    };
    let resp = match exchange(&env.tools_socket, &req).await {
        Ok(r) => r,
        Err(e) => return Outcome::fail(EXIT_TRANSPORT, e),
    };
    if let Some(e) = &resp.error {
        return refusal(e);
    }
    let Some(v) = resp.result else {
        return Outcome::fail(EXIT_TRANSPORT, "platform tools: empty reply");
    };
    let result: ToolResult = match serde_json::from_value(v) {
        Ok(r) => r,
        Err(e) => {
            return Outcome::fail(EXIT_TRANSPORT, format!("platform tools: bad result: {e}"));
        }
    };
    if !result.ok {
        let m = result.error.as_ref().map_or_else(
            || "tool failed".to_owned(),
            |e| format!("{}: {}", e.code.as_str(), e.message),
        );
        return Outcome::fail(EXIT_UPSTREAM, m);
    }
    let text = text_of(&result);
    if opts.raw {
        if let Some(p) = &out_path
            && let Err(e) = files::save(
                text.as_bytes(),
                Some(p),
                &env.out_dir,
                "aurora",
                tool,
                "json",
            )
        {
            return Outcome::fail(EXIT_UPSTREAM, e);
        }
        return Outcome {
            code: super::call::EXIT_OK,
            out: text.into_bytes(),
            err: String::new(),
        };
    }
    if out_path.is_none() && text.chars().count() <= INLINE_MAX {
        let mut o = text;
        if !o.ends_with('\n') {
            o.push('\n');
        }
        return Outcome::ok(o);
    }
    let bytes = if text.len() > SAVE_MAX {
        &text.as_bytes()[..SAVE_MAX]
    } else {
        text.as_bytes()
    };
    let parsed: Option<Value> = serde_json::from_slice(bytes).ok();
    let ext = files::extension("", parsed.is_some());
    let saved = match files::save(
        bytes,
        out_path.as_deref(),
        &env.out_dir,
        "aurora",
        tool,
        ext,
    ) {
        Ok(p) => p,
        Err(e) => return Outcome::fail(EXIT_UPSTREAM, e),
    };
    let mut o = format!("ok · {} bytes · saved: {}\n", bytes.len(), saved.display());
    match parsed {
        Some(v) => o.push_str(&preview::json_part(&v)),
        None => {
            for l in text.lines().take(12) {
                o.push_str(&oneline(l, 160));
                o.push('\n');
            }
        }
    }
    let mut o = o;
    if o.chars().count() > AGENT_CLI_PREVIEW_MAX {
        o = bound(&o);
    }
    Outcome::ok(o)
}
