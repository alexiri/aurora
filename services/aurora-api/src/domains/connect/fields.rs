//! Server-side validation of connect forms (09 §3.4 `fields`).
use aurora_core::{ApiError, FieldError};
use aurora_registry::manifest::{FieldType, FormField, Store};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

const VALUE_MAX: usize = 8 * 1024;
const FILE_MAX: usize = 256 * 1024;
/// The rename's limit (`connections/service.rs` `LABEL_MAX`, C-1041).
const LABEL_MAX: usize = 120;

#[derive(Default)]
pub struct Parsed {
    pub credentials: Map<String, Value>,
    pub metadata: Map<String, Value>,
    pub tenant_host: Option<String>,
    /// The `store: label` field's cleaned value (FIX-194): the connection's label.
    pub label: Option<String>,
    /// Submitted values by field name (for `{fields.*}` in probe URLs).
    pub raw: Map<String, Value>,
}

fn bad(errs: Vec<FieldError>) -> ApiError {
    ApiError::invalid_request("Invalid connect fields.", errs)
}

pub fn parse(fields: &[FormField], input: &BTreeMap<String, String>) -> Result<Parsed, ApiError> {
    let mut errs = Vec::new();
    for k in input.keys() {
        if !fields.iter().any(|f| &f.name == k) {
            errs.push(FieldError::new(format!("/fields/{k}"), "unknown field"));
        }
    }
    let mut out = Parsed::default();
    for f in fields {
        let path = format!("/fields/{}", f.name);
        let shown = f
            .show_when
            .as_ref()
            .is_none_or(|w| input.get(&w.field).is_some_and(|v| *v == w.equals));
        if !shown {
            continue;
        }
        // A blank label field counts as empty: the connection keeps its default or current name.
        let Some(v) = input
            .get(&f.name)
            .filter(|v| !v.is_empty() && (f.store != Store::Label || !v.trim().is_empty()))
        else {
            if f.required {
                errs.push(FieldError::new(path, "required"));
            }
            continue;
        };
        let max = if matches!(f.kind, FieldType::File | FieldType::JsonFile) {
            FILE_MAX
        } else {
            VALUE_MAX
        };
        if v.len() > max || v.contains('\0') {
            errs.push(FieldError::new(path, "invalid or too long"));
            continue;
        }
        match f.kind {
            FieldType::Select if !f.options.iter().any(|o| &o.value == v) => {
                errs.push(FieldError::new(path, "not one of the options"));
                continue;
            }
            FieldType::JsonFile => {
                match serde_json::from_str::<Value>(v) {
                    Ok(doc) => {
                        for (target, ptr) in &f.json_map {
                            match doc.pointer(ptr).and_then(Value::as_str) {
                                Some(s) => {
                                    out.credentials
                                        .insert(target.clone(), Value::String(s.into()));
                                }
                                None => errs.push(FieldError::new(
                                    path.clone(),
                                    format!("missing string at {ptr}"),
                                )),
                            }
                        }
                    }
                    Err(_) => errs.push(FieldError::new(path, "not valid JSON")),
                }
                continue;
            }
            _ => {}
        }
        out.raw.insert(f.name.clone(), Value::String(v.clone()));
        let val = Value::String(v.clone());
        match f.store {
            // (FIX-194, C-1091) Checked like a rename; stored only as the connection's label.
            Store::Label => match aurora_core::names::clean(v, LABEL_MAX) {
                Ok(l) => out.label = Some(l),
                Err(e) => errs.push(FieldError::new(path, e.to_string())),
            },
            Store::Credentials => {
                out.credentials.insert(f.name.clone(), val);
            }
            Store::Metadata => {
                out.metadata.insert(f.name.clone(), val);
            }
            Store::TenantHost => out.tenant_host = Some(v.trim().to_ascii_lowercase()),
        }
    }
    if errs.is_empty() {
        Ok(out)
    } else {
        Err(bad(errs))
    }
}
