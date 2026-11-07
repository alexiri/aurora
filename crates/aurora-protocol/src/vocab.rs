//! Bounded strings and open vocabularies.
//! See `docs/02-engineering-conventions.md` §Coding rules: open vocabularies are
//! `Known + Other(String)` with bounded length; unknown values are logged and
//! skipped by consumers, never fatal.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// Maximum length (in characters) of the `Other` payload of an open vocabulary.
pub const OPEN_VOCAB_MAX: usize = 64;

/// A string of at most `N` characters. Over-long input fails to deserialize.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BoundedString<const N: usize>(String);

/// Error returned when a string exceeds its bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("string exceeds {max} characters")]
pub struct TooLong {
    pub max: usize,
}

impl<const N: usize> BoundedString<N> {
    /// Wraps `s` if it has at most `N` characters.
    pub fn new(s: impl Into<String>) -> Result<Self, TooLong> {
        let s = s.into();
        if s.chars().count() > N {
            Err(TooLong { max: N })
        } else {
            Ok(Self(s))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<const N: usize> Serialize for BoundedString<N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de, const N: usize> Deserialize<'de> for BoundedString<N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

impl<const N: usize> JsonSchema for BoundedString<N> {
    fn schema_name() -> Cow<'static, str> {
        Cow::Owned(format!("BoundedString{N}"))
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type": "string", "maxLength": N})
    }
}

/// Declares an open vocabulary: `Known` variants map to fixed wire strings and
/// any other string becomes `Other(BoundedString<OPEN_VOCAB_MAX>)`.
///
/// Serde (de)serializes the enum as a plain string; the JSON Schema is a
/// bounded string. Over-long unknown values fail to deserialize.
#[macro_export]
macro_rules! open_vocab {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident = $wire:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        $vis enum $name {
            $( $(#[$vmeta])* $variant, )+
            /// A value not known to this build; logged and skipped by consumers.
            Other($crate::vocab::BoundedString<{ $crate::vocab::OPEN_VOCAB_MAX }>),
        }

        impl $name {
            /// Wire string for this value.
            pub fn as_str(&self) -> &str {
                match self {
                    $( Self::$variant => $wire, )+
                    Self::Other(s) => s.as_str(),
                }
            }

            /// Maps a wire string to a value; fails only if an unknown string is too long.
            pub fn from_wire(s: &str) -> ::core::result::Result<Self, $crate::vocab::TooLong> {
                Ok(match s {
                    $( $wire => Self::$variant, )+
                    other => Self::Other($crate::vocab::BoundedString::new(other)?),
                })
            }
        }

        impl $crate::__private::serde::Serialize for $name {
            fn serialize<S: $crate::__private::serde::Serializer>(
                &self,
                serializer: S,
            ) -> ::core::result::Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> $crate::__private::serde::Deserialize<'de> for $name {
            fn deserialize<D: $crate::__private::serde::Deserializer<'de>>(
                deserializer: D,
            ) -> ::core::result::Result<Self, D::Error> {
                let s: ::std::string::String =
                    $crate::__private::serde::Deserialize::deserialize(deserializer)?;
                Self::from_wire(&s).map_err($crate::__private::serde::de::Error::custom)
            }
        }

        impl $crate::__private::schemars::JsonSchema for $name {
            fn schema_name() -> ::std::borrow::Cow<'static, str> {
                ::std::borrow::Cow::Borrowed(stringify!($name))
            }

            fn json_schema(
                _: &mut $crate::__private::schemars::SchemaGenerator,
            ) -> $crate::__private::schemars::Schema {
                $crate::__private::schemars::json_schema!({
                    "type": "string",
                    "maxLength": $crate::vocab::OPEN_VOCAB_MAX
                })
            }
        }
    };
}

open_vocab! {
    /// Example open vocabulary that exercises the schema pipeline.
    pub enum ExampleKind {
        Alpha = "alpha",
        Beta = "beta",
    }
}
