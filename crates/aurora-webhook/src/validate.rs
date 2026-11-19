//! `validate` (C-909's rules, with its codes) and `check_values` (C-910's slot rules).
use std::collections::BTreeSet;

use aurora_core::limits::{WEBHOOK_CONFIG_MAX, WEBHOOK_SLOTS_MAX, WEBHOOK_TEMPLATE_MAX};
use serde_json::Value;

use crate::model::{
    ContentType, FieldError, RATE_PER_DAY_MAX, RATE_PER_MINUTE_MAX, Rule, Slot, SlotError,
    SlotType, WebhookConfig,
};
use crate::schema;
use crate::template::{self, Body, Ctx, Part, Place};

/// Most secret values one webhook holds.
pub const SECRET_VALUES_MAX: usize = 4;
/// Most headers one webhook sends.
pub const HEADERS_MAX: usize = 4;
/// Longest header value template.
const HEADER_VALUE_MAX: usize = 2048;
/// Longest description of the whole webhook.
const DESCRIPTION_MAX: usize = 1000;
/// Longest description of one slot.
const SLOT_DESCRIPTION_MAX: usize = 300;
/// Slot value limits when the slot sets none (C-909's maxima).
const STRING_MAX: u32 = 65_536;
const ITEMS_MAX: u32 = 100;
const ENUM_MAX: usize = 50;

/// A validated configuration with its template parsed once (an AST of constant parts and
/// placeholders), so render never parses text again.
#[derive(Debug, Clone)]
pub struct CompiledConfig {
    pub(crate) config: WebhookConfig,
    pub(crate) body: Body,
    pub(crate) headers: Vec<(String, Vec<Part>)>,
}

impl CompiledConfig {
    #[must_use]
    pub fn config(&self) -> &WebhookConfig {
        &self.config
    }

    #[must_use]
    pub fn slots(&self) -> &[Slot] {
        &self.config.slots
    }
}

fn tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

fn header_denied(lower: &str) -> bool {
    matches!(
        lower,
        "host"
            | "content-length"
            | "content-type"
            | "transfer-encoding"
            | "connection"
            | "cookie"
            | "forwarded"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "upgrade"
    ) || lower.starts_with("x-forwarded-")
        || lower.starts_with("x-aurora-")
}

fn slot_rules(i: usize, s: &Slot, ct: ContentType, out: &mut Vec<FieldError>) {
    let p = |f: &str| format!("/slots/{i}/{f}");
    if !template::name_ok(&s.name) {
        out.push(FieldError::new(p("name"), "slot_name"));
    }
    if s.description.chars().count() > SLOT_DESCRIPTION_MAX {
        out.push(FieldError::new(p("description"), "slot_limits"));
    }
    if matches!(s.ty, SlotType::Json) && ct != ContentType::Json {
        out.push(FieldError::new(p("type"), "slot_type_content_type"));
    }
    if matches!(s.ty, SlotType::StringList) && ct == ContentType::Text {
        out.push(FieldError::new(p("type"), "slot_type_content_type"));
    }
    let has_len = matches!(s.ty, SlotType::String | SlotType::StringList);
    if let Some(n) = s.max_length
        && (!has_len || !(1..=STRING_MAX).contains(&n))
    {
        out.push(FieldError::new(p("max_length"), "slot_limits"));
    }
    if let Some(n) = s.max_items
        && (s.ty != SlotType::StringList || !(1..=ITEMS_MAX).contains(&n))
    {
        out.push(FieldError::new(p("max_items"), "slot_limits"));
    }
    match (&s.r#enum, s.ty) {
        (Some(e), SlotType::Enum) => {
            let uniq: BTreeSet<&String> = e.iter().collect();
            if e.is_empty()
                || e.len() > ENUM_MAX
                || uniq.len() != e.len()
                || e.iter().any(String::is_empty)
            {
                out.push(FieldError::new(p("enum"), "slot_limits"));
            }
        }
        (None, SlotType::Enum) | (Some(_), _) => {
            out.push(FieldError::new(p("enum"), "slot_limits"));
        }
        (None, _) => {}
    }
    if let Some(sc) = &s.schema {
        let size = serde_json::to_vec(sc).map_or(usize::MAX, |b| b.len());
        if s.ty != SlotType::Json || !schema::supported(sc) {
            out.push(FieldError::new(p("schema"), "schema_unsupported"));
        } else if size > schema::SCHEMA_MAX_BYTES {
            out.push(FieldError::new(p("schema"), "too_large"));
        }
    }
}

fn header_rules(
    i: usize,
    h: &crate::model::HeaderTemplate,
    ctx: &mut Ctx<'_>,
    out: &mut Vec<FieldError>,
) -> Vec<Part> {
    let name_path = format!("/headers/{i}/name");
    let value_path = format!("/headers/{i}/value");
    if h.name.is_empty() || h.name.len() > 100 || !h.name.bytes().all(tchar) {
        out.push(FieldError::new(name_path.clone(), "header_name_denied"));
    } else if header_denied(&h.name.to_ascii_lowercase()) {
        out.push(FieldError::new(name_path, "header_name_denied"));
    }
    if h.value.len() > HEADER_VALUE_MAX {
        out.push(FieldError::new(value_path, "too_large"));
        return Vec::new();
    }
    if h.value.chars().any(|c| c.is_control() && c != '\t') {
        out.push(FieldError::new(value_path, "template_syntax"));
        return Vec::new();
    }
    match template::parts(&h.value, ctx, Place::Header) {
        Ok(p) => p,
        Err(code) => {
            out.push(FieldError::new(value_path, code));
            Vec::new()
        }
    }
}

/// Check every rule of C-909 (without secret values, which the caller holds): names, duplicates,
/// limits, JSON validity with placeholders, slot types against the content type, optional slots at
/// the root, the schema subset, headers and sizes. `secret_value_names` are the names of the
/// connection's secret values.
pub fn validate(
    config: &WebhookConfig,
    secret_value_names: &[String],
) -> Result<CompiledConfig, Vec<FieldError>> {
    let mut out: Vec<FieldError> = Vec::new();
    if serde_json::to_vec(config).map_or(true, |b| b.len() > WEBHOOK_CONFIG_MAX) {
        out.push(FieldError::new("", "too_large"));
    }
    if config.template.len() > WEBHOOK_TEMPLATE_MAX {
        out.push(FieldError::new("/template", "too_large"));
    }
    if config.description.chars().count() > DESCRIPTION_MAX {
        out.push(FieldError::new("/description", "too_large"));
    }
    if config
        .rate
        .per_minute
        .is_some_and(|n| n > RATE_PER_MINUTE_MAX)
    {
        out.push(FieldError::new("/rate/per_minute", "rate_above_ceiling"));
    }
    if config.rate.per_day.is_some_and(|n| n > RATE_PER_DAY_MAX) {
        out.push(FieldError::new("/rate/per_day", "rate_above_ceiling"));
    }
    // Secret value names.
    if secret_value_names.len() > SECRET_VALUES_MAX {
        out.push(FieldError::new("/secret_value_names", "slot_limits"));
    }
    let mut seen = BTreeSet::new();
    for (i, n) in secret_value_names.iter().enumerate() {
        if !template::name_ok(n) {
            out.push(FieldError::new(
                format!("/secret_value_names/{i}"),
                "slot_name",
            ));
        } else if !seen.insert(n.as_str()) {
            out.push(FieldError::new(
                format!("/secret_value_names/{i}"),
                "slot_duplicate",
            ));
        }
    }
    // Slots.
    if config.slots.len() > WEBHOOK_SLOTS_MAX {
        out.push(FieldError::new("/slots", "slot_limits"));
    }
    let mut names = BTreeSet::new();
    for (i, s) in config.slots.iter().enumerate() {
        slot_rules(i, s, config.content_type, &mut out);
        if !names.insert(s.name.as_str()) {
            out.push(FieldError::new(
                format!("/slots/{i}/name"),
                "slot_duplicate",
            ));
        }
    }
    // Headers.
    if config.headers.len() > HEADERS_MAX {
        out.push(FieldError::new("/headers", "too_large"));
    }
    let mut ctx = Ctx::new(&config.slots, secret_value_names);
    let mut header_names = BTreeSet::new();
    let mut headers = Vec::new();
    for (i, h) in config.headers.iter().enumerate() {
        let parts = header_rules(i, h, &mut ctx, &mut out);
        if !header_names.insert(h.name.to_ascii_lowercase()) {
            out.push(FieldError::new(
                format!("/headers/{i}/name"),
                "slot_duplicate",
            ));
        }
        headers.push((h.name.clone(), parts));
    }
    // The template.
    let body = if config.template.len() > WEBHOOK_TEMPLATE_MAX {
        None
    } else {
        match template::body(config.content_type, &config.template, &mut ctx) {
            Ok(b) => Some(b),
            Err(code) => {
                out.push(FieldError::new("/template", code));
                None
            }
        }
    };
    if let Some(Body::Json(template::Node::Slot(i))) = &body
        && !config.slots[*i].required
    {
        out.push(FieldError::new("/template", "optional_slot_at_root"));
    }
    if body.is_some() {
        for i in 0..config.slots.len() {
            // A duplicate name resolves to its first declaration: the later one is unused too.
            if !ctx.used_slots.contains(&i) {
                out.push(FieldError::new(format!("/slots/{i}/name"), "slot_unused"));
            }
        }
        for (i, n) in secret_value_names.iter().enumerate() {
            if !ctx.used_secrets.contains(n) {
                out.push(FieldError::new(
                    format!("/secret_value_names/{i}"),
                    "secret_unused",
                ));
            }
        }
    }
    match (out.is_empty(), body) {
        (true, Some(body)) => Ok(CompiledConfig {
            config: config.clone(),
            body,
            headers,
        }),
        _ => Err(out),
    }
}

/// The slot values that passed [`check_values`], in declaration order. Holds values: do not log it.
#[derive(Clone)]
pub struct CheckedValues {
    pub(crate) by_slot: Vec<Option<Value>>,
    names: Vec<String>,
}

impl std::fmt::Debug for CheckedValues {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CheckedValues({:?})", self.given())
    }
}

impl CheckedValues {
    /// The names of the slots that were given (safe to log).
    pub fn given(&self) -> Vec<&str> {
        self.iter().map(|(n, _)| n).collect()
    }

    /// The given values by slot name, in declaration order (for an approval card or a preview).
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.names
            .iter()
            .zip(&self.by_slot)
            .filter_map(|(n, v)| v.as_ref().map(|v| (n.as_str(), v)))
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.iter().find(|(n, _)| *n == name).map(|(_, v)| v)
    }
}

fn err(slot: &str, rule: Rule) -> SlotError {
    SlotError {
        slot: slot.to_owned(),
        rule,
    }
}

fn check_one(s: &Slot, v: &Value) -> Result<(), Rule> {
    let max_len = s.max_length.unwrap_or(STRING_MAX) as usize;
    match s.ty {
        SlotType::String => {
            let t = v.as_str().ok_or(Rule::Type)?;
            if t.chars().count() > max_len {
                return Err(Rule::MaxLength);
            }
        }
        SlotType::StringList => {
            let a = v.as_array().ok_or(Rule::Type)?;
            if a.len() > s.max_items.unwrap_or(ITEMS_MAX) as usize {
                return Err(Rule::MaxItems);
            }
            for item in a {
                let t = item.as_str().ok_or(Rule::Type)?;
                if t.chars().count() > max_len {
                    return Err(Rule::MaxLength);
                }
            }
        }
        SlotType::Enum => {
            let t = v.as_str().ok_or(Rule::Type)?;
            if !s.r#enum.as_ref().is_some_and(|e| e.iter().any(|x| x == t)) {
                return Err(Rule::Enum);
            }
        }
        SlotType::Integer => {
            if !(v.is_i64() || v.is_u64()) {
                return Err(Rule::Type);
            }
        }
        SlotType::Number => {
            if !v.is_number() {
                return Err(Rule::Type);
            }
        }
        SlotType::Boolean => {
            if !v.is_boolean() {
                return Err(Rule::Type);
            }
        }
        SlotType::Json => {
            if let Some(sc) = &s.schema
                && !schema::matches(sc, v)
            {
                return Err(Rule::Schema);
            }
        }
    }
    Ok(())
}

/// Validate the agent's slot values (an object) against the slots. A `null` counts as left out.
/// Lengths are counted in Unicode scalar values.
pub fn check_values(compiled: &CompiledConfig, values: &Value) -> Result<CheckedValues, SlotError> {
    let obj = values.as_object().ok_or_else(|| err("", Rule::Type))?;
    let slots = &compiled.config.slots;
    if let Some(k) = obj.keys().find(|k| !slots.iter().any(|s| &s.name == *k)) {
        return Err(err(k, Rule::Unknown));
    }
    let mut by_slot = Vec::with_capacity(slots.len());
    for s in slots {
        match obj.get(&s.name) {
            None | Some(Value::Null) => {
                if s.required {
                    return Err(err(&s.name, Rule::Missing));
                }
                by_slot.push(None);
            }
            Some(v) => {
                check_one(s, v).map_err(|r| err(&s.name, r))?;
                by_slot.push(Some(v.clone()));
            }
        }
    }
    Ok(CheckedValues {
        by_slot,
        names: slots.iter().map(|s| s.name.clone()).collect(),
    })
}
