//! Template grammar (C-909): `{{slot}}` and `{{secret:name}}`, parsed once into an AST of constant
//! parts and placeholders, so render never scans text again.
use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::model::{ContentType, Slot, SlotType};

/// Deepest JSON nesting a template may have.
const JSON_DEPTH_MAX: usize = 32;

/// A piece of a string-like value (a JSON string literal, a form value, a text body, a header).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Part {
    Lit(String),
    Slot(usize),
    Secret(String),
}

/// A JSON template node.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Node {
    /// `null`, `true`, `false` or a number, as the template spelled it (already validated).
    Raw(String),
    Str(Vec<Part>),
    /// A slot as a whole value.
    Slot(usize),
    /// A secret value as a whole value (a JSON string).
    Secret(String),
    Array(Vec<Node>),
    Object(Vec<(String, Node)>),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FormPair {
    pub key: String,
    pub value: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Body {
    Json(Node),
    Form(Vec<FormPair>),
    Text(Vec<Part>),
}

/// What the parsers know: the declared slots and secret names, and which of them were used.
pub(crate) struct Ctx<'a> {
    pub slots: &'a [Slot],
    pub index: BTreeMap<&'a str, usize>,
    pub secrets: BTreeSet<&'a str>,
    pub used_slots: BTreeSet<usize>,
    pub used_secrets: BTreeSet<String>,
}

impl<'a> Ctx<'a> {
    pub fn new(slots: &'a [Slot], secrets: &'a [String]) -> Self {
        let mut index = BTreeMap::new();
        for (i, s) in slots.iter().enumerate() {
            index.entry(s.name.as_str()).or_insert(i);
        }
        Self {
            slots,
            index,
            secrets: secrets.iter().map(String::as_str).collect(),
            used_slots: BTreeSet::new(),
            used_secrets: BTreeSet::new(),
        }
    }
}

/// `^[a-z][a-z0-9_]{0,39}$`: slot and secret value names.
pub(crate) fn name_ok(n: &str) -> bool {
    let b = n.as_bytes();
    !b.is_empty()
        && b.len() <= 40
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

pub(crate) enum Ph {
    Slot(usize),
    Secret(String),
}

/// A parse failure: the C-909 code.
pub(crate) type Fail = &'static str;

/// Where a placeholder sits, which decides what may go there.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    /// Inside a JSON string literal, a form value part, a text body: a scalar slot or a secret.
    Scalar,
    /// A whole form value: a `string_list` may go here too.
    WholeFormValue,
    /// A whole JSON value: any type.
    JsonValue,
    /// A header value: secrets only.
    Header,
}

/// Parse the placeholder starting at `s[at..]` (which begins with `{{`); returns it and the byte
/// index after the closing `}}`.
fn placeholder(s: &str, at: usize, ctx: &mut Ctx<'_>, place: Place) -> Result<(Ph, usize), Fail> {
    let rest = &s[at + 2..];
    let close = rest.find("}}").ok_or("template_syntax")?;
    let inner = &rest[..close];
    let end = at + 2 + close + 2;
    if let Some(sec) = inner.strip_prefix("secret:") {
        if !name_ok(sec) {
            return Err("template_syntax");
        }
        if !ctx.secrets.contains(sec) {
            return Err("placeholder_unknown");
        }
        ctx.used_secrets.insert(sec.to_owned());
        return Ok((Ph::Secret(sec.to_owned()), end));
    }
    if !name_ok(inner) {
        return Err("template_syntax");
    }
    if place == Place::Header {
        return Err("header_placeholder");
    }
    let &i = ctx.index.get(inner).ok_or("placeholder_unknown")?;
    let ty = ctx.slots[i].ty;
    let allowed = match place {
        Place::JsonValue => true,
        Place::WholeFormValue => ty != SlotType::Json,
        _ => !matches!(ty, SlotType::Json | SlotType::StringList),
    };
    if !allowed {
        return Err("slot_type_content_type");
    }
    ctx.used_slots.insert(i);
    Ok((Ph::Slot(i), end))
}

/// Split plain text (a text body, a form value, a header value) into parts.
pub(crate) fn parts(s: &str, ctx: &mut Ctx<'_>, place: Place) -> Result<Vec<Part>, Fail> {
    let mut out = Vec::new();
    let mut lit = String::new();
    let mut i = 0;
    while let Some(p) = s[i..].find("{{") {
        let at = i + p;
        lit.push_str(&s[i..at]);
        let (ph, end) = placeholder(s, at, ctx, place)?;
        if !lit.is_empty() {
            out.push(Part::Lit(std::mem::take(&mut lit)));
        }
        out.push(match ph {
            Ph::Slot(i) => Part::Slot(i),
            Ph::Secret(n) => Part::Secret(n),
        });
        i = end;
    }
    lit.push_str(&s[i..]);
    if !lit.is_empty() {
        out.push(Part::Lit(lit));
    }
    Ok(out)
}

pub(crate) fn text(s: &str, ctx: &mut Ctx<'_>) -> Result<Body, Fail> {
    if s.contains('\0') {
        return Err("template_syntax");
    }
    Ok(Body::Text(parts(s, ctx, Place::Scalar)?))
}

pub(crate) fn form(s: &str, ctx: &mut Ctx<'_>) -> Result<Body, Fail> {
    if s.is_empty() || s.contains(['\0', '\r', '\n']) {
        return Err("template_syntax");
    }
    let mut pairs = Vec::new();
    for pair in s.split('&') {
        let (key, value) = pair.split_once('=').ok_or("template_syntax")?;
        if key.is_empty() || key.contains("{{") {
            return Err("template_syntax");
        }
        let whole =
            value.starts_with("{{") && value.ends_with("}}") && value[2..].find("{{").is_none();
        let place = if whole {
            Place::WholeFormValue
        } else {
            Place::Scalar
        };
        pairs.push(FormPair {
            key: key.to_owned(),
            value: parts(value, ctx, place)?,
        });
    }
    Ok(Body::Form(pairs))
}

pub(crate) fn json(s: &str, ctx: &mut Ctx<'_>) -> Result<Body, Fail> {
    let mut p = JsonParser { s, i: 0, ctx };
    p.ws();
    let n = p.value(0)?;
    p.ws();
    if p.i != s.len() {
        return Err("template_json");
    }
    Ok(Body::Json(n))
}

struct JsonParser<'s, 'c, 'x> {
    s: &'s str,
    i: usize,
    ctx: &'c mut Ctx<'x>,
}

impl JsonParser<'_, '_, '_> {
    fn ws(&mut self) {
        while self
            .s
            .as_bytes()
            .get(self.i)
            .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
        {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.i).copied()
    }

    fn value(&mut self, depth: usize) -> Result<Node, Fail> {
        if depth > JSON_DEPTH_MAX {
            return Err("template_json");
        }
        match self.peek().ok_or("template_json")? {
            b'{' if self.s[self.i..].starts_with("{{") => {
                let (ph, end) = placeholder(self.s, self.i, self.ctx, Place::JsonValue)?;
                self.i = end;
                Ok(match ph {
                    Ph::Slot(i) => Node::Slot(i),
                    Ph::Secret(n) => Node::Secret(n),
                })
            }
            b'{' => {
                self.i += 1;
                let mut members = Vec::new();
                self.ws();
                if self.peek() == Some(b'}') {
                    self.i += 1;
                    return Ok(Node::Object(members));
                }
                loop {
                    self.ws();
                    if self.peek() != Some(b'"') {
                        return Err("template_json");
                    }
                    let key = self.string()?;
                    // A key is constant: a placeholder there could rewrite the structure.
                    let key = match key.as_slice() {
                        [] => String::new(),
                        [Part::Lit(k)] => k.clone(),
                        _ => return Err("template_json"),
                    };
                    self.ws();
                    if self.peek() != Some(b':') {
                        return Err("template_json");
                    }
                    self.i += 1;
                    self.ws();
                    let v = self.value(depth + 1)?;
                    members.push((key, v));
                    self.ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Node::Object(members));
                        }
                        _ => return Err("template_json"),
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.peek() == Some(b']') {
                    self.i += 1;
                    return Ok(Node::Array(items));
                }
                loop {
                    self.ws();
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Node::Array(items));
                        }
                        _ => return Err("template_json"),
                    }
                }
            }
            b'"' => Ok(Node::Str(self.string()?)),
            _ => {
                let start = self.i;
                while self.peek().is_some_and(|b| {
                    !matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b',' | b']' | b'}' | b':')
                }) {
                    self.i += 1;
                }
                let tok = &self.s[start..self.i];
                match serde_json::from_str::<Value>(tok) {
                    Ok(Value::Null | Value::Bool(_) | Value::Number(_)) => {
                        Ok(Node::Raw(tok.to_owned()))
                    }
                    _ => Err("template_json"),
                }
            }
        }
    }

    /// A string literal (at the opening quote) as parts: constants decoded, placeholders resolved.
    fn string(&mut self) -> Result<Vec<Part>, Fail> {
        self.i += 1;
        let mut out = Vec::new();
        let mut seg = String::new();
        loop {
            let rest = &self.s[self.i..];
            let c = rest.chars().next().ok_or("template_json")?;
            if c == '"' {
                self.i += 1;
                break;
            }
            if c == '\\' {
                seg.push('\\');
                self.i += 1;
                let n = self.s[self.i..].chars().next().ok_or("template_json")?;
                seg.push(n);
                self.i += n.len_utf8();
                continue;
            }
            if rest.starts_with("{{") {
                flush(&mut seg, &mut out)?;
                let (ph, end) = placeholder(self.s, self.i, self.ctx, Place::Scalar)?;
                out.push(match ph {
                    Ph::Slot(i) => Part::Slot(i),
                    Ph::Secret(n) => Part::Secret(n),
                });
                self.i = end;
                continue;
            }
            seg.push(c);
            self.i += c.len_utf8();
        }
        flush(&mut seg, &mut out)?;
        Ok(out)
    }
}

/// Decode a raw string-literal segment (JSON escapes) into a constant part.
fn flush(seg: &mut String, out: &mut Vec<Part>) -> Result<(), Fail> {
    if seg.is_empty() {
        return Ok(());
    }
    let decoded: String =
        serde_json::from_str(&format!("\"{seg}\"")).map_err(|_| "template_json")?;
    seg.clear();
    if !decoded.is_empty() {
        out.push(Part::Lit(decoded));
    }
    Ok(())
}

pub(crate) fn body(ct: ContentType, s: &str, ctx: &mut Ctx<'_>) -> Result<Body, Fail> {
    match ct {
        ContentType::Json => json(s, ctx),
        ContentType::Form => form(s, ctx),
        ContentType::Text => text(s, ctx),
    }
}
