//! BYO agent governance (phase 2, RFE-025, D-59; 08 §11.9).
//!
//! Layers, outermost first: instance (`byo.enabled`, floor `byo.minimum_posture`) → organization
//! (`governance.byo`) → workspace (`workspaces.byo_posture`). The effective posture is the
//! strictest (`max`) of the layers, exactly like the open-lane posture; an unparsable stored
//! value fails closed (`disabled` / an empty source list).
use aurora_core::cidr::{Cidr, any_contains};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use utoipa::ToSchema;

/// BYO posture, ordered by strictness (`enabled` < `owners_only` < `disabled`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ByoPosture {
    Enabled,
    #[default]
    OwnersOnly,
    Disabled,
}

impl ByoPosture {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "enabled" => Some(Self::Enabled),
            "owners_only" => Some(Self::OwnersOnly),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::OwnersOnly => "owners_only",
            Self::Disabled => "disabled",
        }
    }
}

impl From<aurora_config::ByoPosture> for ByoPosture {
    fn from(p: aurora_config::ByoPosture) -> Self {
        match p {
            aurora_config::ByoPosture::Enabled => Self::Enabled,
            aurora_config::ByoPosture::OwnersOnly => Self::OwnersOnly,
            aurora_config::ByoPosture::Disabled => Self::Disabled,
        }
    }
}

/// Which layer decided a BYO value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ByoLevel {
    Instance,
    Org,
    Workspace,
    Agent,
}

impl ByoLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Instance => "instance",
            Self::Org => "org",
            Self::Workspace => "workspace",
            Self::Agent => "agent",
        }
    }
}

/// `organizations.governance.byo` (no column: `governance` is `jsonb`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct OrgByo {
    /// Absent = inherit the instance floor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub posture: Option<ByoPosture>,
    /// Org default source list (≤32 CIDRs, each inside the instance default when that is set).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub source_cidrs: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub require_context_credentials: bool,
}

impl OrgByo {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    /// Interprets a stored `governance.byo` value; anything unparsable fails closed.
    pub fn from_stored(v: Option<&serde_json::Value>) -> Self {
        let Some(v) = v else {
            return Self::default();
        };
        serde_json::from_value(v.clone()).unwrap_or_else(|e| {
            tracing::error!(error = %e, "malformed governance.byo; failing closed");
            Self::fail_closed()
        })
    }

    pub fn fail_closed() -> Self {
        Self {
            posture: Some(ByoPosture::Disabled),
            source_cidrs: vec![],
            require_context_credentials: true,
        }
    }
}

/// The instance level of BYO governance (`[byo]`, 15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByoInstance {
    pub enabled: bool,
    pub minimum_posture: ByoPosture,
    pub default_source_cidrs: Vec<Cidr>,
    pub require_context_credentials: bool,
}

impl Default for ByoInstance {
    fn default() -> Self {
        Self {
            enabled: true,
            minimum_posture: ByoPosture::OwnersOnly,
            default_source_cidrs: vec![],
            require_context_credentials: false,
        }
    }
}

impl ByoInstance {
    pub fn from_config(b: &aurora_config::Byo) -> Self {
        Self {
            enabled: b.enabled,
            minimum_posture: b.minimum_posture.into(),
            // Validated at startup, so every entry parses.
            default_source_cidrs: parse_list(&b.default_source_cidrs).unwrap_or_default(),
            require_context_credentials: b.require_context_credentials,
        }
    }
}

impl ByoInstance {
    /// From the raw (unvalidated) `[byo]` section; `None` when a value is invalid.
    pub fn from_raw(b: &aurora_config::ByoSection) -> Option<Self> {
        Some(Self {
            enabled: b.enabled,
            minimum_posture: ByoPosture::parse(&b.minimum_posture)?,
            default_source_cidrs: parse_list(&b.default_source_cidrs)?,
            require_context_credentials: b.require_context_credentials,
        })
    }
}

/// Parses a stored list; `None` when any entry is unparsable (callers fail closed).
pub fn parse_list(list: &[String]) -> Option<Vec<Cidr>> {
    list.iter().map(|s| Cidr::parse(s).ok()).collect()
}

/// Effective BYO controls of one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveByo {
    pub posture: ByoPosture,
    /// The layer whose value is the effective posture.
    pub posture_level: ByoLevel,
    /// The effective source list: `None` = unrestricted. `Some(vec![])` matches nothing
    /// (only produced by fail-closed handling of unparsable stored values).
    pub source_cidrs: Option<Vec<Cidr>>,
    pub source_level: Option<ByoLevel>,
    pub require_context: bool,
    pub require_context_level: Option<ByoLevel>,
}

/// Inputs of one agent's resolution, as stored.
#[derive(Debug, Clone, Default)]
pub struct ByoInputs<'a> {
    pub org: OrgByo,
    /// `workspaces.byo_posture` (unparsable text → `disabled`).
    pub workspace_posture: Option<&'a str>,
    /// `agents.byo_source_cidrs`.
    pub agent_cidrs: Option<&'a [String]>,
    /// `agents.byo_require_context`.
    pub agent_require_context: bool,
}

pub fn effective_byo(inst: &ByoInstance, i: &ByoInputs<'_>) -> EffectiveByo {
    // Posture: master switch, then strictest of floor / org / workspace; ties go to the
    // outermost layer.
    let ws = i
        .workspace_posture
        .map(|s| ByoPosture::parse(s).unwrap_or(ByoPosture::Disabled));
    let (posture, posture_level) = if !inst.enabled {
        (ByoPosture::Disabled, ByoLevel::Instance)
    } else {
        let mut best = (inst.minimum_posture, ByoLevel::Instance);
        for (p, l) in [(i.org.posture, ByoLevel::Org), (ws, ByoLevel::Workspace)] {
            if let Some(p) = p
                && p > best.0
            {
                best = (p, l);
            }
        }
        best
    };

    // Source list: agent's own, else org default, else instance default, else unrestricted.
    let org_list = (!i.org.source_cidrs.is_empty()).then_some(i.org.source_cidrs.as_slice());
    let inst_list = (!inst.default_source_cidrs.is_empty()).then_some(&inst.default_source_cidrs);
    let (source_cidrs, source_level) = if let Some(a) = i.agent_cidrs {
        (
            Some(parse_list(a).unwrap_or_default()),
            Some(ByoLevel::Agent),
        )
    } else if let Some(o) = org_list {
        (Some(parse_list(o).unwrap_or_default()), Some(ByoLevel::Org))
    } else if let Some(l) = inst_list {
        (Some(l.clone()), Some(ByoLevel::Instance))
    } else {
        (None, None)
    };

    let (require_context, require_context_level) = if inst.require_context_credentials {
        (true, Some(ByoLevel::Instance))
    } else if i.org.require_context_credentials {
        (true, Some(ByoLevel::Org))
    } else if i.agent_require_context {
        (true, Some(ByoLevel::Agent))
    } else {
        (false, None)
    };
    EffectiveByo {
        posture,
        posture_level,
        source_cidrs,
        source_level,
        require_context,
        require_context_level,
    }
}

impl EffectiveByo {
    /// Source-list check: `true` when unrestricted or the address lies inside the list.
    pub fn permits_source(&self, ip: IpAddr) -> bool {
        self.source_cidrs
            .as_ref()
            .is_none_or(|l| any_contains(l, ip))
    }

    pub fn source_strings(&self) -> Option<Vec<String>> {
        self.source_cidrs
            .as_ref()
            .map(|l| l.iter().map(ToString::to_string).collect())
    }
}

/// `true` when the BYO posture lets a caller with these capabilities create a BYO agent /
/// regenerate its token. `Err(())` = refused (the caller picks `byo_disabled` vs
/// `byo_not_permitted` from the posture).
pub fn creation_allowed(posture: ByoPosture, can_use: bool, can_own: bool) -> bool {
    match posture {
        ByoPosture::Enabled => can_use,
        ByoPosture::OwnersOnly => can_own,
        ByoPosture::Disabled => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inst(enabled: bool, floor: ByoPosture) -> ByoInstance {
        ByoInstance {
            enabled,
            minimum_posture: floor,
            ..ByoInstance::default()
        }
    }

    #[test]
    fn posture_order_and_strictest_wins() {
        assert!(ByoPosture::Enabled < ByoPosture::OwnersOnly);
        assert!(ByoPosture::OwnersOnly < ByoPosture::Disabled);
        let i = inst(true, ByoPosture::Enabled);
        let org = |p| OrgByo {
            posture: p,
            ..OrgByo::default()
        };
        let e = effective_byo(&i, &ByoInputs::default());
        assert_eq!(
            (e.posture, e.posture_level),
            (ByoPosture::Enabled, ByoLevel::Instance)
        );
        let e = effective_byo(
            &i,
            &ByoInputs {
                org: org(Some(ByoPosture::OwnersOnly)),
                workspace_posture: Some("enabled"),
                ..ByoInputs::default()
            },
        );
        assert_eq!(
            (e.posture, e.posture_level),
            (ByoPosture::OwnersOnly, ByoLevel::Org)
        );
        let e = effective_byo(
            &i,
            &ByoInputs {
                org: org(Some(ByoPosture::OwnersOnly)),
                workspace_posture: Some("disabled"),
                ..ByoInputs::default()
            },
        );
        assert_eq!(
            (e.posture, e.posture_level),
            (ByoPosture::Disabled, ByoLevel::Workspace)
        );
        // Ties go to the outermost layer; the floor applies to everything.
        let e = effective_byo(
            &inst(true, ByoPosture::OwnersOnly),
            &ByoInputs {
                org: org(Some(ByoPosture::OwnersOnly)),
                ..ByoInputs::default()
            },
        );
        assert_eq!(e.posture_level, ByoLevel::Instance);
    }

    #[test]
    fn master_switch_overrides_everything_and_bad_values_fail_closed() {
        let e = effective_byo(
            &inst(false, ByoPosture::Enabled),
            &ByoInputs {
                workspace_posture: Some("enabled"),
                ..ByoInputs::default()
            },
        );
        assert_eq!(
            (e.posture, e.posture_level),
            (ByoPosture::Disabled, ByoLevel::Instance)
        );
        let e = effective_byo(
            &inst(true, ByoPosture::Enabled),
            &ByoInputs {
                workspace_posture: Some("garbage"),
                ..ByoInputs::default()
            },
        );
        assert_eq!(e.posture, ByoPosture::Disabled);
        let org = OrgByo::from_stored(Some(&serde_json::json!({"posture": 7})));
        assert_eq!(org.posture, Some(ByoPosture::Disabled));
        let e = effective_byo(
            &inst(true, ByoPosture::Enabled),
            &ByoInputs {
                agent_cidrs: Some(&["not-a-cidr".to_owned()]),
                ..ByoInputs::default()
            },
        );
        assert!(
            !e.permits_source("203.0.113.1".parse().unwrap()),
            "bad stored list matches nothing"
        );
    }

    #[test]
    fn source_list_resolution_and_context_or() {
        let mut i = inst(true, ByoPosture::Enabled);
        i.default_source_cidrs = vec![Cidr::parse("10.0.0.0/8").unwrap()];
        let ip = |s: &str| -> IpAddr { s.parse().unwrap() };
        let e = effective_byo(&i, &ByoInputs::default());
        assert_eq!(e.source_level, Some(ByoLevel::Instance));
        assert!(e.permits_source(ip("10.1.2.3")) && !e.permits_source(ip("11.0.0.1")));
        let org = OrgByo {
            source_cidrs: vec!["10.1.0.0/16".into()],
            require_context_credentials: true,
            ..OrgByo::default()
        };
        let agent = vec!["10.1.2.0/24".to_owned()];
        let e = effective_byo(
            &i,
            &ByoInputs {
                org: org.clone(),
                ..ByoInputs::default()
            },
        );
        assert_eq!(e.source_level, Some(ByoLevel::Org));
        assert!(e.require_context && e.require_context_level == Some(ByoLevel::Org));
        let e = effective_byo(
            &i,
            &ByoInputs {
                org,
                agent_cidrs: Some(&agent),
                ..ByoInputs::default()
            },
        );
        assert_eq!(e.source_level, Some(ByoLevel::Agent));
        assert!(e.permits_source(ip("10.1.2.9")) && !e.permits_source(ip("10.1.3.9")));
        assert!(e.permits_source(ip("::ffff:10.1.2.9")));
        let e = effective_byo(&inst(true, ByoPosture::Enabled), &ByoInputs::default());
        assert!(e.source_cidrs.is_none() && e.permits_source(ip("8.8.8.8")) && !e.require_context);
    }

    #[test]
    fn creation_matrix() {
        use ByoPosture::*;
        assert!(creation_allowed(Enabled, true, false));
        assert!(!creation_allowed(Enabled, false, false));
        assert!(!creation_allowed(OwnersOnly, true, false));
        assert!(creation_allowed(OwnersOnly, true, true));
        assert!(!creation_allowed(Disabled, true, true));
    }
}
