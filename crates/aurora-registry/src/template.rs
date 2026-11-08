//! Template language (09 §4): literal text with `{namespace.name}` tokens.
use std::collections::BTreeSet;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TemplateError {
    #[error("unbalanced braces in template")]
    Unbalanced,
    #[error("malformed token `{0}`")]
    BadToken(String),
    #[error("unknown namespace `{0}`")]
    UnknownNamespace(String),
}

impl TemplateError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unbalanced | Self::BadToken(_) => "template_syntax",
            Self::UnknownNamespace(_) => "template_unknown_namespace",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ns {
    Credentials,
    Metadata,
    /// `connection.tenant_host` (the only name).
    Connection,
    Context,
    Fields,
    /// `request.host` (the only name).
    Request,
    /// `path.N`
    Path,
    /// `query.x`
    Query,
    /// `field.Label`
    Field,
    /// `{instance}`: the host of the connection's app instance (D-74); a whole host only.
    Instance,
}

impl Ns {
    fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "credentials" => Self::Credentials,
            "metadata" => Self::Metadata,
            "connection" => Self::Connection,
            "context" => Self::Context,
            "fields" => Self::Fields,
            "request" => Self::Request,
            "path" => Self::Path,
            "query" => Self::Query,
            "field" => Self::Field,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Reference {
    pub ns: Ns,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Lit(String),
    Ref(Reference),
}

/// Where a template appears; decides which namespaces are allowed (09 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Inject,
    Finalize,
    RewriteHost,
    Transform,
    TokenUrl,
    TokenParams,
    /// Probe URL; `fields.*` only when the probe belongs to a `fields` connect.
    Probe {
        fields_connect: bool,
    },
    Link,
}

/// What the manifest declares, for [`Template::check`].
#[derive(Debug, Default)]
pub struct Declared<'a> {
    pub credentials: BTreeSet<&'a str>,
    pub metadata: BTreeSet<&'a str>,
    pub fields: BTreeSet<&'a str>,
    /// `permissions.context`.
    pub context: BTreeSet<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateIssue {
    pub code: &'static str,
    pub message: String,
}

pub const CONTEXT_VARS: &[&str] = &[
    "org_external_id",
    "org_id",
    "workspace_id",
    "instance_url",
    "agent_name",
    "agent_handle",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    raw: String,
    parts: Vec<Part>,
}

impl Serialize for Template {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.raw)
    }
}

impl Template {
    pub fn parse(raw: &str) -> Result<Self, TemplateError> {
        let mut parts = Vec::new();
        let mut lit = String::new();
        let mut rest = raw;
        while let Some(i) = rest.find(['{', '}']) {
            lit.push_str(&rest[..i]);
            if rest.as_bytes()[i] == b'}' {
                return Err(TemplateError::Unbalanced);
            }
            let after = &rest[i + 1..];
            let end = after.find(['{', '}']).ok_or(TemplateError::Unbalanced)?;
            if after.as_bytes()[end] == b'{' {
                return Err(TemplateError::Unbalanced);
            }
            let r = parse_token(&after[..end])?;
            if !lit.is_empty() {
                parts.push(Part::Lit(std::mem::take(&mut lit)));
            }
            parts.push(Part::Ref(r));
            rest = &after[end + 1..];
        }
        lit.push_str(rest);
        if !lit.is_empty() {
            parts.push(Part::Lit(lit));
        }
        Ok(Self {
            raw: raw.to_owned(),
            parts,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    pub fn is_literal(&self) -> bool {
        self.parts.iter().all(|p| matches!(p, Part::Lit(_)))
    }

    pub fn references(&self) -> impl Iterator<Item = &Reference> {
        self.parts.iter().filter_map(|p| match p {
            Part::Ref(r) => Some(r),
            Part::Lit(_) => None,
        })
    }

    /// Check every reference against the allowed namespaces of `place` and
    /// the declared vocabularies.
    pub fn check(&self, place: Place, d: &Declared<'_>) -> Vec<TemplateIssue> {
        let mut out = Vec::new();
        let issue = |code, message: String| TemplateIssue { code, message };
        for r in self.references() {
            let shown = format!("{:?}.{}", r.ns, r.name);
            let allowed = matches!(
                (place, r.ns),
                (Place::Inject, Ns::Credentials | Ns::Metadata | Ns::Context)
                    | (Place::Finalize, Ns::Credentials | Ns::Metadata)
                    | (Place::RewriteHost, Ns::Metadata | Ns::Connection)
                    | (Place::Transform, Ns::Context)
                    | (Place::TokenUrl, Ns::Connection)
                    | (Place::TokenParams, Ns::Credentials)
                    | (
                        Place::Probe { .. },
                        Ns::Metadata | Ns::Connection | Ns::Instance
                    )
                    | (
                        Place::Probe {
                            fields_connect: true,
                        },
                        Ns::Fields,
                    )
                    | (
                        Place::Link,
                        Ns::Metadata | Ns::Request | Ns::Path | Ns::Query | Ns::Field
                    )
            );
            if !allowed {
                out.push(issue(
                    "template_ns_not_allowed",
                    format!("`{shown}` is not allowed here"),
                ));
                continue;
            }
            match r.ns {
                Ns::Credentials if !d.credentials.contains(r.name.as_str()) => out.push(issue(
                    "undeclared_credential",
                    format!("credential `{}` is not declared", r.name),
                )),
                Ns::Metadata if !d.metadata.contains(r.name.as_str()) => out.push(issue(
                    "undeclared_metadata",
                    format!("metadata `{}` is not declared", r.name),
                )),
                Ns::Fields if !d.fields.contains(r.name.as_str()) => out.push(issue(
                    "undeclared_field",
                    format!("form field `{}` is not declared", r.name),
                )),
                Ns::Context => {
                    if !d.context.contains(r.name.as_str()) {
                        out.push(issue(
                            "context_not_permitted",
                            format!(
                                "context variable `{}` is not in permissions.context",
                                r.name
                            ),
                        ));
                    } else if r.name.starts_with("agent_") && place != Place::Transform {
                        out.push(issue(
                            "context_not_permitted",
                            format!("`{}` is only available in request-path transforms", r.name),
                        ));
                    }
                }
                _ => {}
            }
        }
        out
    }
}

fn parse_token(t: &str) -> Result<Reference, TemplateError> {
    let bad = || TemplateError::BadToken(t.to_owned());
    if t == "instance" {
        return Ok(Reference {
            ns: Ns::Instance,
            name: "instance".to_owned(),
        });
    }
    let (ns_s, name) = t.split_once('.').ok_or_else(bad)?;
    let ns = Ns::from_str(ns_s).ok_or_else(|| TemplateError::UnknownNamespace(ns_s.to_owned()))?;
    let ident = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };
    let ok = match ns {
        Ns::Connection => name == "tenant_host",
        Ns::Request => name == "host",
        Ns::Path => !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()),
        Ns::Field => !name.trim().is_empty() && name.chars().all(|c| !c.is_control()),
        Ns::Credentials | Ns::Metadata | Ns::Context | Ns::Fields | Ns::Query => ident(name),
        Ns::Instance => false,
    };
    if !ok {
        return Err(bad());
    }
    Ok(Reference {
        ns,
        name: name.to_owned(),
    })
}

impl<'de> Deserialize<'de> for Template {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        Self::parse(&s).map_err(|e| serde::de::Error::custom(format!("{}: {e}", e.code())))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn valid_tokens() {
        let t = Template::parse("Bearer {credentials.access_token}/{metadata.org}x")
            .unwrap_or_else(|e| panic!("{e}"));
        let r: Vec<_> = t.references().map(|r| (r.ns, r.name.as_str())).collect();
        assert_eq!(
            r,
            [(Ns::Credentials, "access_token"), (Ns::Metadata, "org")]
        );
        assert!(Template::parse("plain").is_ok_and(|t| t.is_literal()));
        assert!(
            Template::parse("{path.4}{field.To Whom}{connection.tenant_host}{request.host}")
                .is_ok()
        );
    }

    #[test]
    fn bad_templates() {
        assert_eq!(
            Template::parse("{nope.x}"),
            Err(TemplateError::UnknownNamespace("nope".into()))
        );
        for s in [
            "{credentials.a",
            "a}b",
            "{{credentials.a}}",
            "{credentials.a}}",
            "{}",
            "{credentials}",
            "{connection.other}",
            "{path.x}",
        ] {
            assert!(Template::parse(s).is_err(), "{s}");
        }
    }

    #[test]
    fn namespace_places() {
        let mut d = Declared::default();
        d.credentials.insert("tok");
        d.context.insert("org_id");
        d.context.insert("agent_name");
        let ok = |s: &str, p| {
            Template::parse(s)
                .map(|t| t.check(p, &d))
                .unwrap_or_default()
        };
        assert!(ok("{credentials.tok}", Place::Inject).is_empty());
        assert_eq!(
            ok("{fields.x}", Place::Inject)[0].code,
            "template_ns_not_allowed"
        );
        assert_eq!(
            ok("{credentials.zz}", Place::Inject)[0].code,
            "undeclared_credential"
        );
        assert_eq!(
            ok("{context.workspace_id}", Place::Inject)[0].code,
            "context_not_permitted"
        );
        assert_eq!(
            ok("{context.agent_name}", Place::Inject)[0].code,
            "context_not_permitted"
        );
        assert!(ok("{context.agent_name}", Place::Transform).is_empty());
        assert_eq!(
            ok("{credentials.tok}", Place::Link)[0].code,
            "template_ns_not_allowed"
        );
    }
}
