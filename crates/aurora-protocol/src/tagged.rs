//! Open tagged unions: known kinds map to typed bodies, unknown kinds degrade
//! to `Other(UnknownMessage)` instead of failing (`16 §Cross-cutting conventions`).

use schemars::JsonSchema;
use serde::de::Error as _;
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::vocab::{BoundedString, OPEN_VOCAB_MAX};

/// A message whose kind this build does not know. Consumers log and skip it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UnknownMessage {
    pub kind: BoundedString<OPEN_VOCAB_MAX>,
    pub body: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `{ "<tag>": "x", ...body fields }`
    Internal,
    /// `{ "<tag>": "x", "body": {...} }`
    Adjacent,
}

pub fn serialize_tagged<S: Serializer>(
    mode: Mode,
    tag: &str,
    kind: &str,
    body: Value,
    s: S,
) -> Result<S::Ok, S::Error> {
    let mut map = Map::new();
    match mode {
        Mode::Internal => {
            let Value::Object(obj) = body else {
                return Err(S::Error::custom("message body must be a JSON object"));
            };
            map = obj;
            map.insert(tag.to_owned(), Value::String(kind.to_owned()));
        }
        Mode::Adjacent => {
            map.insert(tag.to_owned(), Value::String(kind.to_owned()));
            map.insert("body".to_owned(), body);
        }
    }
    map.serialize(s)
}

pub fn deserialize_tagged<'de, D: Deserializer<'de>>(
    mode: Mode,
    tag: &str,
    d: D,
) -> Result<(String, Value), D::Error> {
    let mut map = Map::<String, Value>::deserialize(d)?;
    let kind = match map.remove(tag) {
        Some(Value::String(k)) => k,
        Some(_) => return Err(D::Error::custom(format!("`{tag}` must be a string"))),
        None => return Err(D::Error::custom(format!("missing `{tag}`"))),
    };
    let body = match mode {
        Mode::Internal => Value::Object(map),
        Mode::Adjacent => map
            .remove("body")
            .unwrap_or_else(|| Value::Object(Map::new())),
    };
    Ok((kind, body))
}

pub fn variant_schema(mode: Mode, tag: &str, wire: &str, sub: Value) -> Value {
    let tag_prop = serde_json::json!({ "const": wire });
    match mode {
        Mode::Internal => serde_json::json!({
            "allOf": [
                sub,
                { "type": "object", "properties": { tag: tag_prop }, "required": [tag] }
            ]
        }),
        Mode::Adjacent => serde_json::json!({
            "type": "object",
            "properties": { tag: tag_prop, "body": sub },
            "required": [tag]
        }),
    }
}

pub fn union_schema(mode: Mode, tag: &str, mut variants: Vec<Value>) -> schemars::Schema {
    let mut props = Map::new();
    props.insert(
        tag.to_owned(),
        serde_json::json!({ "type": "string", "maxLength": OPEN_VOCAB_MAX }),
    );
    let _ = mode;
    variants.push(serde_json::json!({
        "description": "unknown kind; consumers log and skip",
        "type": "object",
        "properties": props,
        "required": [tag]
    }));
    schemars::Schema::try_from(serde_json::json!({ "anyOf": variants }))
        .unwrap_or_else(|_| schemars::Schema::from(true))
}

/// Declares an open tagged union. `mode` is `Internal` or `Adjacent`.
macro_rules! tagged_open {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident, mode = $mode:ident, tag = $tag:literal {
            $( $(#[$vmeta:meta])* $var:ident($ty:ty) = $wire:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq)]
        $vis enum $name {
            $( $(#[$vmeta])* $var($ty), )+
            /// Kind not known to this build; logged and skipped.
            Other($crate::tagged::UnknownMessage),
        }

        impl $name {
            /// Wire kind string.
            pub fn kind(&self) -> &str {
                match self {
                    $( Self::$var(_) => $wire, )+
                    Self::Other(u) => u.kind.as_str(),
                }
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, s: S) -> ::core::result::Result<S::Ok, S::Error> {
                use ::serde::ser::Error as _;
                let body = match self {
                    $( Self::$var(v) => ::serde_json::to_value(v).map_err(S::Error::custom)?, )+
                    Self::Other(u) => u.body.clone(),
                };
                $crate::tagged::serialize_tagged($crate::tagged::Mode::$mode, $tag, self.kind(), body, s)
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> ::core::result::Result<Self, D::Error> {
                use ::serde::de::Error as _;
                let (kind, body) = $crate::tagged::deserialize_tagged($crate::tagged::Mode::$mode, $tag, d)?;
                match kind.as_str() {
                    $( $wire => Ok(Self::$var(::serde_json::from_value(body).map_err(D::Error::custom)?)), )+
                    _ => Ok(Self::Other($crate::tagged::UnknownMessage {
                        kind: $crate::vocab::BoundedString::new(kind).map_err(D::Error::custom)?,
                        body,
                    })),
                }
            }
        }

        impl ::schemars::JsonSchema for $name {
            fn schema_name() -> ::std::borrow::Cow<'static, str> {
                ::std::borrow::Cow::Borrowed(stringify!($name))
            }

            fn json_schema(g: &mut ::schemars::SchemaGenerator) -> ::schemars::Schema {
                let variants = vec![
                    $( $crate::tagged::variant_schema(
                        $crate::tagged::Mode::$mode,
                        $tag,
                        $wire,
                        ::serde_json::to_value(g.subschema_for::<$ty>()).unwrap_or_default(),
                    ), )+
                ];
                $crate::tagged::union_schema($crate::tagged::Mode::$mode, $tag, variants)
            }
        }
    };
}
