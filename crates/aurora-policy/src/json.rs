//! One reading of an untrusted JSON body.
//!
//! A decision (a policy condition, an approval card, a model allow-list) is made on a parsed
//! body, and the receiver gets the bytes. They agree only when the bytes have one reading, and
//! `serde_json::Value` hides when they have two:
//!
//! - it keeps the **last** of two equal keys; another parser keeps the first, or merges a
//!   repeated object into the one it already filled;
//! - with the `raw_value` feature (on in the gateway's build) it reads an object whose member
//!   has one private name as the JSON written in that member's string, which no receiver does;
//! - it says nothing about names that differ only in case, which some receivers match as one;
//! - it gives `1e2`, `100.0` and `100` as different texts though they are one number.
//!
//! [`parse`] keeps every member as the bytes spell it. A reader then asks for what it reads
//! ([`Node::lookup`], [`Node::value`]) and is told when that has more than one reading.

use std::collections::HashSet;
use std::fmt;

use serde::de::{DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

/// A parsed document with every object member kept, in order, names unescaped and literal.
#[derive(Debug)]
pub enum Node {
    /// Null, a boolean, a number or a string.
    Scalar(Value),
    Array(Vec<Node>),
    Object(Vec<(String, Node)>),
}

/// What a JSON pointer leads to.
#[derive(Debug)]
pub enum Lookup<'a> {
    Found(&'a Node),
    Missing,
    /// On the way some object holds the name twice, or only or also in another case.
    Ambiguous,
}

/// `body` as one JSON value, or `None` when it is not (trailing data included).
#[must_use]
pub fn parse(body: &[u8]) -> Option<Node> {
    let mut de = serde_json::Deserializer::from_slice(body);
    let node = Seed.deserialize(&mut de).ok()?;
    de.end().ok()?;
    Some(node)
}

/// `body` as a JSON value that says what the bytes say, for a reader of the whole body.
///
/// # Errors
/// `Err(Some(key))`: an object at some depth repeats `key`. `Err(None)`: the body is not one
/// JSON value.
pub fn read_unrepeated(body: &[u8]) -> Result<Value, Option<String>> {
    let node = parse(body).ok_or(None)?;
    node.value().map_err(|key| Some(key.to_owned()))
}

/// A name the way a receiver that ignores case compares it: lower case, with the Kelvin sign
/// (U+212A) and the long s (U+017F), which such matching folds to `k` and `s`.
pub fn fold(s: &str) -> impl Iterator<Item = char> + '_ {
    s.chars().map(|c| match c {
        '\u{212A}' => 'k',
        '\u{017F}' => 's',
        c => c.to_lowercase().next().unwrap_or(c),
    })
}

/// The member `name` of an object's `members`, if the object has one reading of it.
fn member<'a>(members: &'a [(String, Node)], name: &str) -> Lookup<'a> {
    let mut found = None;
    for (k, v) in members {
        if fold(k).eq(fold(name)) {
            if found.is_some() || k != name {
                return Lookup::Ambiguous;
            }
            found = Some(v);
        }
    }
    found.map_or(Lookup::Missing, Lookup::Found)
}

/// An array index as RFC 6901 spells it.
fn index(token: &str) -> Option<usize> {
    if token.starts_with('+') || (token.starts_with('0') && token.len() > 1) {
        return None;
    }
    token.parse().ok()
}

impl Node {
    /// The member `name` of this object (`Missing` for anything but an object).
    #[must_use]
    pub fn get(&self, name: &str) -> Lookup<'_> {
        match self {
            Node::Object(members) => member(members, name),
            _ => Lookup::Missing,
        }
    }

    /// The node an RFC 6901 pointer names.
    #[must_use]
    pub fn lookup(&self, pointer: &str) -> Lookup<'_> {
        if pointer.is_empty() {
            return Lookup::Found(self);
        }
        let Some(rest) = pointer.strip_prefix('/') else {
            return Lookup::Missing;
        };
        let mut at = self;
        for token in rest.split('/') {
            let token = token.replace("~1", "/").replace("~0", "~");
            let next = match at {
                Node::Object(members) => member(members, &token),
                Node::Array(items) => index(&token)
                    .and_then(|i| items.get(i))
                    .map_or(Lookup::Missing, Lookup::Found),
                Node::Scalar(_) => Lookup::Missing,
            };
            match next {
                Lookup::Found(n) => at = n,
                other => return other,
            }
        }
        Lookup::Found(at)
    }

    /// This node as a `Value`.
    ///
    /// # Errors
    /// The first name some object at or below this node repeats.
    pub fn value(&self) -> Result<Value, &str> {
        Ok(match self {
            Node::Scalar(v) => v.clone(),
            Node::Array(items) => {
                Value::Array(items.iter().map(Node::value).collect::<Result<_, _>>()?)
            }
            Node::Object(members) => {
                let mut out = serde_json::Map::new();
                for (k, v) in members {
                    if out.contains_key(k) {
                        return Err(k);
                    }
                    out.insert(k.clone(), v.value()?);
                }
                Value::Object(out)
            }
        })
    }

    /// Whether some object at or below this node holds two names that are equal, or equal
    /// without case: for a reader that cannot say which names it reads.
    #[must_use]
    pub fn has_colliding_names(&self) -> bool {
        match self {
            Node::Scalar(_) => false,
            Node::Array(items) => items.iter().any(Node::has_colliding_names),
            Node::Object(members) => {
                let mut seen = HashSet::new();
                members.iter().any(|(k, v)| {
                    !seen.insert(fold(k).collect::<String>()) || v.has_colliding_names()
                })
            }
        }
    }

    /// Whether a number at or below this node is a whole number the bytes did not write as an
    /// integer (`1e2`, `100.0`, or one too large for 64 bits): its text is not its only one.
    #[must_use]
    pub fn has_whole_float(&self) -> bool {
        match self {
            Node::Scalar(Value::Number(n)) => {
                n.is_f64() && n.as_f64().is_some_and(|f| f.fract() == 0.0)
            }
            Node::Scalar(_) => false,
            Node::Array(items) => items.iter().any(Node::has_whole_float),
            Node::Object(members) => members.iter().any(|(_, v)| v.has_whole_float()),
        }
    }
}

/// Builds a [`Node`]. It takes every member name literally, unlike `Value`'s own parser.
struct Seed;

impl<'de> DeserializeSeed<'de> for Seed {
    type Value = Node;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Node, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Seed {
    type Value = Node;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_unit<E>(self) -> Result<Node, E> {
        Ok(Node::Scalar(Value::Null))
    }

    fn visit_bool<E>(self, v: bool) -> Result<Node, E> {
        Ok(Node::Scalar(v.into()))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Node, E> {
        Ok(Node::Scalar(v.into()))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Node, E> {
        Ok(Node::Scalar(v.into()))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Node, E> {
        Ok(Node::Scalar(v.into()))
    }

    fn visit_str<E>(self, v: &str) -> Result<Node, E> {
        Ok(Node::Scalar(v.into()))
    }

    fn visit_string<E>(self, v: String) -> Result<Node, E> {
        Ok(Node::Scalar(v.into()))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
        let mut out = Vec::new();
        while let Some(v) = seq.next_element_seed(Seed)? {
            out.push(v);
        }
        Ok(Node::Array(out))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let mut out = Vec::new();
        // The name arrives unescaped, so `a` and `a` are one name.
        while let Some(key) = map.next_key::<String>()? {
            out.push((key, map.next_value_seed(Seed)?));
        }
        Ok(Node::Object(out))
    }
}
