//! `render` (C-909): one pass over the parsed template. Inserted text is never scanned for
//! placeholders again, so a value containing `{{secret:x}}` stays literal; each value is escaped for
//! the place it lands in (a JSON value or string, a form value, plain text, a header).
use std::borrow::Cow;

use aurora_core::limits::WEBHOOK_BODY_MAX;
use serde_json::Value;

use crate::model::{RenderError, Rendered, SecretValues, SlotType};
use crate::template::{Body, FormPair, Node, Part};
use crate::validate::{CheckedValues, CompiledConfig};

enum Secrets<'a> {
    Real(&'a SecretValues),
    Masked,
}

struct Ctx<'a> {
    c: &'a CompiledConfig,
    v: &'a CheckedValues,
    secrets: Secrets<'a>,
}

impl Ctx<'_> {
    fn secret(&self, name: &str) -> Result<Cow<'_, str>, RenderError> {
        match &self.secrets {
            Secrets::Masked => Ok(Cow::Owned(format!("«secret:{name}»"))),
            Secrets::Real(s) => {
                s.0.get(name)
                    .map(|v| Cow::Borrowed(v.as_str()))
                    .ok_or_else(|| RenderError::MissingSecret(name.to_owned()))
            }
        }
    }

    fn value(&self, slot: usize) -> Option<&Value> {
        self.v.by_slot.get(slot).and_then(Option::as_ref)
    }

    /// A scalar slot value as text (never a `string_list` or `json`: the parser keeps those out of
    /// the places that call this).
    fn scalar(&self, slot: usize) -> String {
        match self.value(slot) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::Bool(b)) => b.to_string(),
            _ => String::new(),
        }
    }

    fn is_list(&self, slot: usize) -> bool {
        self.c
            .config
            .slots
            .get(slot)
            .is_some_and(|s| s.ty == SlotType::StringList)
    }
}

fn json_string_content(s: &str, out: &mut Vec<u8>) {
    // `to_string` of a `&str` cannot fail; the quotes are the first and last byte.
    if let Ok(q) = serde_json::to_string(s) {
        let b = q.as_bytes();
        if b.len() >= 2 {
            out.extend_from_slice(&b[1..b.len() - 1]);
        }
    }
}

fn json_value(v: &Value, out: &mut Vec<u8>) {
    if let Ok(b) = serde_json::to_vec(v) {
        out.extend_from_slice(&b);
    }
}

/// A member or element that is a left-out optional slot is dropped.
fn omitted(n: &Node, cx: &Ctx<'_>) -> bool {
    matches!(n, Node::Slot(i) if cx.value(*i).is_none())
}

fn emit(n: &Node, cx: &Ctx<'_>, out: &mut Vec<u8>) -> Result<(), RenderError> {
    match n {
        Node::Raw(t) => out.extend_from_slice(t.as_bytes()),
        Node::Str(parts) => {
            out.push(b'"');
            for p in parts {
                match p {
                    Part::Lit(t) => json_string_content(t, out),
                    Part::Slot(i) => json_string_content(&cx.scalar(*i), out),
                    Part::Secret(name) => json_string_content(&cx.secret(name)?, out),
                }
            }
            out.push(b'"');
        }
        Node::Slot(i) => match cx.value(*i) {
            Some(v) => json_value(v, out),
            None => out.extend_from_slice(b"null"),
        },
        Node::Secret(name) => {
            out.push(b'"');
            json_string_content(&cx.secret(name)?, out);
            out.push(b'"');
        }
        Node::Array(items) => {
            out.push(b'[');
            let mut first = true;
            for it in items.iter().filter(|it| !omitted(it, cx)) {
                if !std::mem::take(&mut first) {
                    out.push(b',');
                }
                emit(it, cx, out)?;
            }
            out.push(b']');
        }
        Node::Object(members) => {
            out.push(b'{');
            let mut first = true;
            for (k, v) in members.iter().filter(|(_, v)| !omitted(v, cx)) {
                if !std::mem::take(&mut first) {
                    out.push(b',');
                }
                out.push(b'"');
                json_string_content(k, out);
                out.extend_from_slice(b"\":");
                emit(v, cx, out)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

/// `application/x-www-form-urlencoded` encoding of one value (space as `+`).
fn form_encode(s: &str, out: &mut Vec<u8>) {
    for &b in s.as_bytes() {
        match b {
            b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'*' | b'-' | b'.' | b'_' => out.push(b),
            b' ' => out.push(b'+'),
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push(b'%');
                out.push(HEX[usize::from(b >> 4)]);
                out.push(HEX[usize::from(b & 15)]);
            }
        }
    }
}

fn emit_form(pairs: &[FormPair], cx: &Ctx<'_>, out: &mut Vec<u8>) -> Result<(), RenderError> {
    let mut first = true;
    let mut sep = |out: &mut Vec<u8>| {
        if !std::mem::take(&mut first) {
            out.push(b'&');
        }
    };
    for pair in pairs {
        // A whole-value placeholder: a left-out optional drops the pair; a list repeats the key.
        if let [Part::Slot(i)] = pair.value.as_slice() {
            let Some(v) = cx.value(*i) else { continue };
            if cx.is_list(*i) {
                for item in v.as_array().into_iter().flatten() {
                    sep(out);
                    out.extend_from_slice(pair.key.as_bytes());
                    out.push(b'=');
                    form_encode(item.as_str().unwrap_or_default(), out);
                }
                continue;
            }
        }
        sep(out);
        out.extend_from_slice(pair.key.as_bytes());
        out.push(b'=');
        for p in &pair.value {
            match p {
                Part::Lit(t) => out.extend_from_slice(t.as_bytes()),
                Part::Slot(i) => form_encode(&cx.scalar(*i), out),
                Part::Secret(name) => form_encode(&cx.secret(name)?, out),
            }
        }
    }
    Ok(())
}

fn go(
    c: &CompiledConfig,
    v: &CheckedValues,
    secrets: Secrets<'_>,
) -> Result<Rendered, RenderError> {
    let cx = Ctx { c, v, secrets };
    let mut body = Vec::new();
    match &c.body {
        Body::Json(n) => emit(n, &cx, &mut body)?,
        Body::Form(pairs) => emit_form(pairs, &cx, &mut body)?,
        Body::Text(parts) => {
            for p in parts {
                match p {
                    Part::Lit(t) => body.extend_from_slice(t.as_bytes()),
                    Part::Slot(i) => body.extend_from_slice(cx.scalar(*i).as_bytes()),
                    Part::Secret(name) => body.extend_from_slice(cx.secret(name)?.as_bytes()),
                }
            }
        }
    }
    if body.len() > WEBHOOK_BODY_MAX {
        return Err(RenderError::TooLarge);
    }
    let mut headers = Vec::with_capacity(c.headers.len());
    for (name, parts) in &c.headers {
        let mut value = String::new();
        for p in parts {
            match p {
                Part::Lit(t) => value.push_str(t),
                Part::Secret(s) => {
                    let t = cx.secret(s)?;
                    if t.chars().any(|ch| ch.is_control() && ch != '\t') {
                        return Err(RenderError::HeaderValue(name.clone()));
                    }
                    value.push_str(&t);
                }
                // The validator refuses a slot in a header.
                Part::Slot(_) => {}
            }
        }
        headers.push((name.clone(), value));
    }
    Ok(Rendered {
        content_type: c.config.content_type.as_str().to_owned(),
        body,
        headers,
    })
}

/// Render the request: body, content type and the configured headers. `secrets` are the
/// connection's secret values by name.
pub fn render(
    compiled: &CompiledConfig,
    checked: &CheckedValues,
    secrets: &SecretValues,
) -> Result<Rendered, RenderError> {
    go(compiled, checked, Secrets::Real(secrets))
}

/// The same render with every secret value shown as `«secret:name»`: for previews and dry runs,
/// where nothing is sent and no secret may appear.
pub fn render_masked(
    compiled: &CompiledConfig,
    checked: &CheckedValues,
) -> Result<Rendered, RenderError> {
    go(compiled, checked, Secrets::Masked)
}
