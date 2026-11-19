//! The JSON Schema subset of C-909 (`type`, `properties`, `required`, `items`, `enum`, `maxLength`,
//! `minimum`, `maximum`, `additionalProperties`): a static check of a slot's schema and a small
//! validator for a value against it. No schema engine dependency.
use serde_json::Value;

/// Deepest schema nesting accepted.
const DEPTH_MAX: usize = 8;
/// Largest schema, serialized.
pub(crate) const SCHEMA_MAX_BYTES: usize = 8 * 1024;

const TYPES: [&str; 7] = [
    "string", "number", "integer", "boolean", "object", "array", "null",
];

/// Is `schema` inside the supported subset (and well formed)?
pub(crate) fn supported(schema: &Value) -> bool {
    walk(schema, 0)
}

fn walk(s: &Value, depth: usize) -> bool {
    if depth > DEPTH_MAX {
        return false;
    }
    let Some(o) = s.as_object() else {
        return false;
    };
    o.iter().all(|(k, v)| match k.as_str() {
        "type" => match v {
            Value::String(t) => TYPES.contains(&t.as_str()),
            Value::Array(a) => {
                !a.is_empty()
                    && a.iter()
                        .all(|t| t.as_str().is_some_and(|t| TYPES.contains(&t)))
            }
            _ => false,
        },
        "properties" => v
            .as_object()
            .is_some_and(|p| p.values().all(|c| walk(c, depth + 1))),
        "required" => v.as_array().is_some_and(|a| a.iter().all(Value::is_string)),
        "items" => walk(v, depth + 1),
        "enum" => v.as_array().is_some_and(|a| !a.is_empty()),
        "maxLength" => v.is_u64(),
        "minimum" | "maximum" => v.is_number(),
        "additionalProperties" => v.is_boolean() || walk(v, depth + 1),
        _ => false,
    })
}

fn type_ok(t: &str, v: &Value) -> bool {
    match t {
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => {
            v.is_i64()
                || v.is_u64()
                || v.as_f64()
                    .is_some_and(|f| f.is_finite() && f.fract() == 0.0)
        }
        "boolean" => v.is_boolean(),
        "object" => v.is_object(),
        "array" => v.is_array(),
        "null" => v.is_null(),
        _ => false,
    }
}

/// Does `v` satisfy `schema` (which [`supported`] accepted)?
pub(crate) fn matches(schema: &Value, v: &Value) -> bool {
    let Some(o) = schema.as_object() else {
        return false;
    };
    for (k, s) in o {
        let ok = match k.as_str() {
            "type" => match s {
                Value::String(t) => type_ok(t, v),
                Value::Array(a) => a.iter().any(|t| t.as_str().is_some_and(|t| type_ok(t, v))),
                _ => false,
            },
            "enum" => s.as_array().is_some_and(|a| a.contains(v)),
            "maxLength" => match (v.as_str(), s.as_u64()) {
                (Some(t), Some(n)) => (t.chars().count() as u64) <= n,
                _ => true,
            },
            "minimum" => match (v.as_f64(), s.as_f64()) {
                (Some(x), Some(m)) => x >= m,
                _ => true,
            },
            "maximum" => match (v.as_f64(), s.as_f64()) {
                (Some(x), Some(m)) => x <= m,
                _ => true,
            },
            "required" => match (v.as_object(), s.as_array()) {
                (Some(obj), Some(req)) => req
                    .iter()
                    .all(|r| r.as_str().is_some_and(|r| obj.contains_key(r))),
                _ => true,
            },
            "properties" => match (v.as_object(), s.as_object()) {
                (Some(obj), Some(props)) => props
                    .iter()
                    .all(|(name, sub)| obj.get(name).is_none_or(|x| matches(sub, x))),
                _ => true,
            },
            "additionalProperties" => match (v.as_object(), o.get("properties")) {
                (Some(obj), props) => {
                    let known = props.and_then(Value::as_object);
                    obj.iter()
                        .filter(|(name, _)| !known.is_some_and(|k| k.contains_key(*name)))
                        .all(|(_, x)| match s {
                            Value::Bool(allow) => *allow,
                            sub => matches(sub, x),
                        })
                }
                _ => true,
            },
            "items" => match v.as_array() {
                Some(a) => a.iter().all(|x| matches(s, x)),
                None => true,
            },
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    true
}
