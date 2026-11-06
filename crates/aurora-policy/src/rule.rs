//! Rule model (07 §Rule anatomy; 03 `policy_rules` / `derived_rules`).
use crate::facts::{Credential, Level, RequestFacts, RequestIdentity, SecretKind};
use crate::host::HostPattern;
use crate::methods;
use crate::path::PathPattern;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Identity {
    Agent { id: Uuid },
    User { id: Uuid },
}

impl Identity {
    pub fn matches(&self, who: &RequestIdentity) -> bool {
        match self {
            Self::Agent { id } => *id == who.agent_id,
            Self::User { id } => who.principals.contains(id),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeSel {
    Workspace,
    Organization,
    Any,
}

impl ScopeSel {
    pub fn covers(self, l: Level) -> bool {
        matches!(
            (self, l),
            (Self::Any, _)
                | (Self::Workspace, Level::Workspace)
                | (Self::Organization, Level::Organization)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Target {
    App {
        app_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tools: Option<Vec<String>>,
    },
    Connection {
        id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tools: Option<Vec<String>>,
    },
    AppConnections {
        app_id: String,
        scope: ScopeSel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tools: Option<Vec<String>>,
    },
    Secret {
        id: Uuid,
    },
    Secrets {
        scope: ScopeSel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<SecretKind>,
    },
    Network {
        host: HostPattern,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<PathPattern>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        methods: Vec<String>,
    },
}

fn tools_ok(tools: &Option<Vec<String>>, f: &RequestFacts) -> bool {
    match tools {
        None => true,
        Some(t) if t.is_empty() => true,
        Some(t) => f.tool_id.as_ref().is_some_and(|id| t.contains(id)),
    }
}

impl Target {
    pub fn matches(&self, f: &RequestFacts) -> bool {
        match self {
            Self::App { app_id, tools } => f.app_id.as_ref() == Some(app_id) && tools_ok(tools, f),
            Self::Connection { id, tools } => {
                matches!(&f.winning_credential, Some(Credential::Connection { id: w, .. }) if w == id)
                    && tools_ok(tools, f)
            }
            Self::AppConnections {
                app_id,
                scope,
                tools,
            } => {
                matches!(&f.winning_credential,
                    Some(Credential::Connection { app_id: a, scope: l, .. }) if a == app_id && scope.covers(*l))
                    && tools_ok(tools, f)
            }
            Self::Secret { id } => {
                matches!(&f.winning_credential, Some(Credential::Secret { id: w, .. }) if w == id)
            }
            Self::Secrets { scope, kind } => {
                matches!(&f.winning_credential,
                    Some(Credential::Secret { kind: k, scope: l, .. })
                        if scope.covers(*l) && kind.is_none_or(|want| want == *k))
            }
            Self::Network {
                host,
                path,
                methods: ms,
            } => {
                host.matches(&f.host, f.port)
                    && path.as_ref().is_none_or(|p| p.matches(&f.path))
                    && methods::matches(ms, &f.method)
            }
        }
    }

    /// Does this target *name* the candidate credential (injection selection)?
    pub fn selects(&self, c: &Credential) -> bool {
        match (self, c) {
            (Self::Connection { id, .. }, Credential::Connection { id: cid, .. }) => id == cid,
            (
                Self::AppConnections { app_id, scope, .. },
                Credential::Connection {
                    app_id: a,
                    scope: l,
                    ..
                },
            ) => app_id == a && scope.covers(*l),
            (Self::Secret { id }, Credential::Secret { id: cid, .. }) => id == cid,
            (
                Self::Secrets { scope, kind },
                Credential::Secret {
                    kind: k, scope: l, ..
                },
            ) => scope.covers(*l) && kind.is_none_or(|want| want == *k),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Allow,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Per {
    Minute,
    Hour,
    Day,
}

impl Per {
    pub fn seconds(self) -> u64 {
        match self {
            Self::Minute => 60,
            Self::Hour => 3600,
            Self::Day => 86400,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimit {
    pub count: u32,
    pub per: Per,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionOn {
    Body,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionOp {
    Contains,
    NotContains,
    Equals,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Condition {
    pub on: ConditionOn,
    pub op: ConditionOp,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_pointer: Option<String>,
}

/// Three-valued condition result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tri {
    True,
    False,
    Unknown,
}

fn from_bool(b: bool) -> Tri {
    if b { Tri::True } else { Tri::False }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || hay.windows(needle.len()).any(|w| w == needle)
}

impl Condition {
    pub fn eval(&self, f: &RequestFacts) -> Tri {
        match &self.json_pointer {
            Some(ptr) => self.eval_json(ptr, f),
            None => self.eval_raw(f),
        }
    }

    fn eval_raw(&self, f: &RequestFacts) -> Tri {
        let (body, needle) = (f.body_prefix.as_slice(), self.value.as_bytes());
        match self.op {
            ConditionOp::Contains | ConditionOp::NotContains => {
                let found = contains(body, needle);
                let want_found = self.op == ConditionOp::Contains;
                if found {
                    from_bool(want_found)
                } else if f.truncated {
                    Tri::Unknown
                } else {
                    from_bool(!want_found)
                }
            }
            ConditionOp::Equals => {
                if !f.truncated {
                    from_bool(body == needle)
                } else if body.len() >= needle.len() || !needle.starts_with(body) {
                    // The full body is strictly longer than the prefix, or diverges.
                    Tri::False
                } else {
                    Tri::Unknown
                }
            }
        }
    }

    fn eval_json(&self, ptr: &str, f: &RequestFacts) -> Tri {
        if f.truncated {
            return Tri::Unknown;
        }
        let Ok(doc) = serde_json::from_slice::<Value>(&f.body_prefix) else {
            return Tri::Unknown;
        };
        let text = match doc.pointer(ptr) {
            None => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(v) => Some(v.to_string()),
        };
        let hit = text.is_some_and(|t| match self.op {
            ConditionOp::Equals => t == self.value,
            _ => t.contains(&self.value),
        });
        match self.op {
            ConditionOp::NotContains => from_bool(!hit),
            _ => from_bool(hit),
        }
    }
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// Columns (hand rules) or supplied by the caller (derived rules).
    #[serde(default)]
    pub logical_id: Uuid,
    #[serde(default)]
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub identities: Vec<Identity>,
    #[serde(default)]
    pub targets: Vec<Target>,
    pub action: Action,
    #[serde(default)]
    pub require_approval: bool,
    #[serde(default)]
    pub rate_limit: Option<RateLimit>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

impl Rule {
    pub fn identity_matches(&self, who: &RequestIdentity) -> bool {
        self.identities.is_empty() || self.identities.iter().any(|i| i.matches(who))
    }

    pub fn targets_match(&self, f: &RequestFacts) -> bool {
        self.targets.is_empty() || self.targets.iter().any(|t| t.matches(f))
    }

    /// Restrictive rules resolve unknown conditions as matched.
    pub fn is_restrictive(&self) -> bool {
        self.action == Action::Block || self.require_approval || self.rate_limit.is_some()
    }

    /// AND of conditions; any false => false; else unknown resolves by polarity.
    pub fn conditions_match(&self, f: &RequestFacts) -> (bool, bool) {
        let mut unknown = false;
        for c in &self.conditions {
            match c.eval(f) {
                Tri::False => return (false, false),
                Tri::Unknown => unknown = true,
                Tri::True => {}
            }
        }
        (!unknown || self.is_restrictive(), unknown)
    }
}
