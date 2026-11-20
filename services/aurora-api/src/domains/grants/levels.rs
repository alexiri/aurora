//! Access levels (P5-UX-2, C-980, D-344): the shared expansion of a level into a grant, and the
//! connection's `access` derived from a set of levels. The connect journey (C-957) and the access
//! API (C-980, P5-UX-4) both call these two functions, so a client can never widen a level: the
//! tools come from the snapshot, never from the request.

use super::dto::{GrantMode, PutConnectionGrant};
use crate::domains::apps::dto::Tool;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// What one agent may do with one account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Read tools only.
    Read,
    /// Read tools allowed, changes ask first.
    Ask,
    /// Everything, without asking (mode `full`).
    Act,
}

fn is_read(t: &Tool) -> bool {
    t.group == "read"
}

/// (P5-REFINE-6, C-1067, D-419) An app with tools and none of them in group `read` (the Outgoing
/// webhook): it only sends, so `read` is never one of its levels. Derived, nothing stored.
#[must_use]
pub fn send_only(tools: &[Tool]) -> bool {
    !tools.is_empty() && !tools.iter().any(is_read)
}

/// (C-1067) Refuse a level the app does not offer, before it is expanded: `read` on a send-only
/// app would expand to an empty grant. `422 level_not_offered` with the levels that exist.
pub fn require_offered(
    level: Level,
    tools: &[Tool],
    custom_only: bool,
    app_id: &str,
) -> Result<(), aurora_core::ApiError> {
    if level == Level::Read && send_only(tools) {
        let offered: Vec<&str> = levels_for(tools, custom_only, None)
            .into_iter()
            .map(Level::as_str)
            .collect();
        return Err(aurora_core::ApiError::unprocessable(
            "level_not_offered",
            "This app only sends: choose Ask before each send or Send without asking.",
        )
        .with_details(serde_json::json!({"app_id": app_id, "offered": offered})));
    }
    Ok(())
}

/// Expand `level` over the app's tools (from [`crate::domains::apps::service::tools`]).
///
/// - `read` allows every read tool.
/// - `ask` allows the read tools and asks for the write tools.
/// - `act` is mode `full` (the app's `custom_only` restriction refuses it downstream, 422
///   `full_grant_forbidden`).
#[must_use]
pub fn expand(level: Level, tools: &[Tool]) -> PutConnectionGrant {
    let ids = |read: bool| -> Vec<String> {
        tools
            .iter()
            .filter(|t| is_read(t) == read)
            .map(|t| t.id.clone())
            .collect()
    };
    match level {
        Level::Act => PutConnectionGrant {
            mode: GrantMode::Full,
            allow_tools: Vec::new(),
            ask_tools: Vec::new(),
            ask_timeout_secs: None,
        },
        Level::Read => PutConnectionGrant {
            mode: GrantMode::Custom,
            allow_tools: ids(true),
            ask_tools: Vec::new(),
            ask_timeout_secs: None,
        },
        Level::Ask => PutConnectionGrant {
            mode: GrantMode::Custom,
            allow_tools: ids(true),
            ask_tools: ids(false),
            ask_timeout_secs: None,
        },
    }
}

/// The connection's `access` (`read` | `write`) for a set of agents' levels: `read` when every
/// level is `read`, `write` otherwise (with several agents, the highest any of them needs). An
/// empty set is `read`.
#[must_use]
pub fn access_for_levels(levels: &[Level]) -> &'static str {
    if levels.iter().all(|l| *l == Level::Read) {
        "read"
    } else {
        "write"
    }
}

/// The same for a saved grant: `full` and any custom grant that allows or asks a write tool need
/// `write`.
#[must_use]
pub fn access_for_grant(grant: &PutConnectionGrant, tools: &[Tool]) -> &'static str {
    match grant.mode {
        GrantMode::Full => "write",
        GrantMode::Custom => {
            let writes = |id: &String| tools.iter().any(|t| &t.id == id && !is_read(t));
            if grant.allow_tools.iter().chain(&grant.ask_tools).any(writes) {
                "write"
            } else {
                "read"
            }
        }
    }
}

impl Level {
    /// Order of the three choices: read < ask < act.
    #[must_use]
    pub fn rank(self) -> u8 {
        match self {
            Self::Read => 0,
            Self::Ask => 1,
            Self::Act => 2,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Ask => "ask",
            Self::Act => "act",
        }
    }
}

/// What the limit would be for a connection `access` (`read` → read, `write` → act): the value
/// the migration filled `limit_level` with, and what an API caller's `access` maps to (C-990).
#[must_use]
pub fn limit_from_access(access: &str) -> Level {
    if access == "read" {
        Level::Read
    } else {
        Level::Act
    }
}

impl Level {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "read" => Some(Self::Read),
            "ask" => Some(Self::Ask),
            "act" => Some(Self::Act),
            _ => None,
        }
    }
}

/// (P5-UX-11, C-990) The stored limit of an organization's account. A row without one (never
/// after the migration) reads as what its `access` gives.
#[must_use]
pub fn connection_limit(limit_level: Option<&str>, access: &str) -> Level {
    limit_level
        .and_then(Level::parse)
        .unwrap_or_else(|| limit_from_access(access))
}

/// The `access` the gateway enforces for a limit: `read` for `read`, `write` otherwise.
#[must_use]
pub fn access_for_limit(limit: Level) -> &'static str {
    if limit == Level::Read {
        "read"
    } else {
        "write"
    }
}

/// What a saved grant becomes under a lowered `limit` (C-990, SEC-131), or `None` when it
/// already fits. A level above the limit becomes the limit; under `ask`, changes allowed without
/// asking become asked; under `read`, change tools are removed. Returns `(full, allow, ask)`.
#[must_use]
pub fn lowered_grant(
    limit: Level,
    full: bool,
    allow: &[String],
    ask: &[String],
    tools: &[Tool],
) -> Option<(bool, Vec<String>, Vec<String>)> {
    if !exceeds_limit(limit, full, allow, ask, tools) {
        return None;
    }
    if full {
        let g = expand(limit, tools);
        return Some((false, g.allow_tools, g.ask_tools));
    }
    let is_write = |id: &String| tools.iter().any(|t| &t.id == id && !is_read(t));
    let (mut a, mut k) = (Vec::new(), Vec::new());
    for id in allow {
        match (limit, is_write(id)) {
            (Level::Read, true) => {}
            (Level::Ask, true) => k.push(id.clone()),
            _ => a.push(id.clone()),
        }
    }
    for id in ask {
        if !(limit == Level::Read && is_write(id)) && !k.contains(id) {
            k.push(id.clone());
        }
    }
    Some((false, a, k))
}

/// The choices that exist for an app: none when it has no tool that changes anything, no `read`
/// for a send-only app (C-1067), no `act` for a `custom_only` app (C-410), and nothing above the
/// account's `limit`.
#[must_use]
pub fn levels_for(tools: &[Tool], custom_only: bool, limit: Option<Level>) -> Vec<Level> {
    if !tools.iter().any(|t| !is_read(t)) {
        return Vec::new();
    }
    let sends_only = send_only(tools);
    [Level::Read, Level::Ask, Level::Act]
        .into_iter()
        .filter(|l| !(sends_only && *l == Level::Read))
        .filter(|l| !(custom_only && *l == Level::Act))
        .filter(|l| limit.is_none_or(|m| l.rank() <= m.rank()))
        .collect()
}

/// Whether a grant allows what `limit` does not (C-980, SEC-131): `full` above `act`; a write
/// under `read`; a write allowed without asking under `ask`. Custom choices are checked tool by
/// tool, so a hand-built tool list cannot get past the limit.
#[must_use]
pub fn exceeds_limit(
    limit: Level,
    full: bool,
    allow: &[String],
    ask: &[String],
    tools: &[Tool],
) -> bool {
    let is_write = |id: &String| tools.iter().any(|t| &t.id == id && !is_read(t));
    match limit {
        Level::Act => false,
        _ if full => true,
        Level::Read => allow.iter().chain(ask).any(is_write),
        Level::Ask => allow.iter().any(is_write),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    Allow,
    Ask,
    Block,
}

fn level_decision(level: Level, t: &Tool) -> Decision {
    match (level, is_read(t)) {
        (Level::Act, _) | (_, true) => Decision::Allow,
        (Level::Ask, false) => Decision::Ask,
        (Level::Read, false) => Decision::Block,
    }
}

/// A saved grant as the dashboard reads it (C-917).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Level(Level),
    /// Not a level: the nearest level and how many tools differ from it.
    Custom {
        base: Level,
        differs: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classified {
    pub choice: Choice,
    /// Under `ask`: the changes the saved choice allows without asking (tool ids).
    pub always_allowed: Vec<String>,
}

/// Read a saved grant through the levels. `full` is `act`; reads allowed and no change blocked
/// is `ask` (the changes it allows are `always_allowed`); reads allowed and every change blocked
/// is `read`; every tool allowed is `act`; anything else is `custom` with its nearest level.
#[must_use]
pub fn classify(full: bool, allow: &[String], ask: &[String], tools: &[Tool]) -> Classified {
    let level = |l| Classified {
        choice: Choice::Level(l),
        always_allowed: Vec::new(),
    };
    if full {
        return level(Level::Act);
    }
    let decide = |t: &Tool| {
        if allow.contains(&t.id) {
            Decision::Allow
        } else if ask.contains(&t.id) {
            Decision::Ask
        } else {
            Decision::Block
        }
    };
    let reads_allowed = tools
        .iter()
        .filter(|t| is_read(t))
        .all(|t| decide(t) == Decision::Allow);
    let writes: Vec<(&Tool, Decision)> = tools
        .iter()
        .filter(|t| !is_read(t))
        .map(|t| (t, decide(t)))
        .collect();
    // (C-1067) A send-only app has no `read` level: a grant that blocks every send is not one of
    // its levels (the writers refuse it; a lowered limit can leave it behind).
    let sends_only = send_only(tools);
    if reads_allowed {
        if writes.iter().all(|(_, d)| *d == Decision::Block) && !sends_only {
            return level(Level::Read);
        }
        if writes.iter().all(|(_, d)| *d != Decision::Block) {
            if writes.iter().all(|(_, d)| *d == Decision::Allow) {
                return level(Level::Act);
            }
            return Classified {
                choice: Choice::Level(Level::Ask),
                always_allowed: writes
                    .iter()
                    .filter(|(_, d)| *d == Decision::Allow)
                    .map(|(t, _)| t.id.clone())
                    .collect(),
            };
        }
    }
    let (base, differs) = [Level::Read, Level::Ask, Level::Act]
        .into_iter()
        .filter(|l| !(sends_only && *l == Level::Read))
        .map(|l| {
            let n = tools
                .iter()
                .filter(|t| level_decision(l, t) != decide(t))
                .count();
            (l, n)
        })
        .min_by_key(|(_, n)| *n)
        .unwrap_or((Level::Read, 0));
    Classified {
        choice: Choice::Custom { base, differs },
        always_allowed: Vec::new(),
    }
}

/// `access_for_grant` over a saved row's parts.
#[must_use]
pub fn access_for_saved(
    full: bool,
    allow: &[String],
    ask: &[String],
    tools: &[Tool],
) -> &'static str {
    access_for_grant(
        &PutConnectionGrant {
            mode: if full {
                GrantMode::Full
            } else {
                GrantMode::Custom
            },
            allow_tools: allow.to_vec(),
            ask_tools: ask.to_vec(),
            ask_timeout_secs: None,
        },
        tools,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(id: &str, group: &str) -> Tool {
        Tool {
            id: id.into(),
            name: id.into(),
            description: None,
            group: group.into(),
            wildcard: false,
            host: "h".into(),
            path: "/".into(),
            methods: vec![],
            connect_methods: vec![],
            scopes: vec![],
            graphql: None,
            git: None,
            approval: None,
        }
    }

    fn catalogue() -> Vec<Tool> {
        vec![
            tool("list", "read"),
            tool("get", "read"),
            tool("create", "write"),
        ]
    }

    #[test]
    fn each_level_expands_from_the_catalogue() {
        let t = catalogue();
        let r = expand(Level::Read, &t);
        assert_eq!(r.mode, GrantMode::Custom);
        assert_eq!(
            (r.allow_tools, r.ask_tools),
            (vec!["list".into(), "get".into()], vec![])
        );
        let a = expand(Level::Ask, &t);
        assert_eq!(a.mode, GrantMode::Custom);
        assert_eq!(
            (a.allow_tools, a.ask_tools),
            (vec!["list".into(), "get".into()], vec!["create".into()])
        );
        let f = expand(Level::Act, &t);
        assert_eq!(f.mode, GrantMode::Full);
        assert!(f.allow_tools.is_empty() && f.ask_tools.is_empty());
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    fn two_writes() -> Vec<Tool> {
        vec![
            tool("list", "read"),
            tool("create", "write"),
            tool("comment", "write"),
        ]
    }

    #[test]
    fn a_saved_grant_reads_as_a_level_or_custom() {
        let t = two_writes();
        let lvl = |c: &Classified| match c.choice {
            Choice::Level(l) => Some(l),
            Choice::Custom { .. } => None,
        };
        for l in [Level::Read, Level::Ask, Level::Act] {
            let g = expand(l, &t);
            let c = classify(g.mode == GrantMode::Full, &g.allow_tools, &g.ask_tools, &t);
            assert_eq!(lvl(&c), Some(l));
            assert!(c.always_allowed.is_empty());
        }
        // Ask before changes with one change allowed: still `ask`, the change always allowed.
        let c = classify(false, &ids(&["list", "comment"]), &ids(&["create"]), &t);
        assert_eq!(c.choice, Choice::Level(Level::Ask));
        assert_eq!(c.always_allowed, ids(&["comment"]));
        // A blocked read is not a level (and the allowed change differs from asking, too).
        let c = classify(false, &ids(&["comment"]), &ids(&["create"]), &t);
        assert_eq!(
            c.choice,
            Choice::Custom {
                base: Level::Ask,
                differs: 2
            }
        );
        // A blocked change under otherwise-asked changes is not a level either (equally near
        // Read only and Ask before changes: the lower one is the base).
        let c = classify(false, &ids(&["list"]), &ids(&["create"]), &t);
        assert_eq!(
            c.choice,
            Choice::Custom {
                base: Level::Read,
                differs: 1
            }
        );
        // Every tool allowed in a custom grant takes effect as act.
        let c = classify(false, &ids(&["list", "create", "comment"]), &[], &t);
        assert_eq!(c.choice, Choice::Level(Level::Act));
    }

    #[test]
    fn limits_cut_the_choices_and_refuse_wider_grants() {
        let t = two_writes();
        assert_eq!(
            levels_for(&t, false, None),
            vec![Level::Read, Level::Ask, Level::Act]
        );
        assert_eq!(levels_for(&t, true, None), vec![Level::Read, Level::Ask]);
        assert_eq!(levels_for(&t, false, Some(Level::Read)), vec![Level::Read]);
        assert!(levels_for(&[tool("list", "read")], false, None).is_empty());
        let (a, k) = (ids(&["list", "create"]), ids(&[]));
        assert!(exceeds_limit(Level::Read, false, &a, &k, &t));
        assert!(exceeds_limit(Level::Ask, false, &a, &k, &t));
        assert!(!exceeds_limit(
            Level::Ask,
            false,
            &ids(&["list"]),
            &ids(&["create"]),
            &t
        ));
        assert!(exceeds_limit(
            Level::Read,
            false,
            &ids(&["list"]),
            &ids(&["create"]),
            &t
        ));
        assert!(exceeds_limit(Level::Ask, true, &[], &[], &t));
        assert!(!exceeds_limit(Level::Act, true, &[], &[], &t));
        assert_eq!(limit_from_access("read"), Level::Read);
        assert_eq!(limit_from_access("write"), Level::Act);
    }

    #[test]
    fn a_lowered_limit_keeps_what_fits_and_cuts_the_rest() {
        let t = two_writes();
        let (a, k) = (ids(&["list", "create"]), ids(&["comment"]));
        // Within the limit: untouched.
        assert_eq!(lowered_grant(Level::Act, false, &a, &k, &t), None);
        assert_eq!(
            lowered_grant(Level::Ask, false, &ids(&["list"]), &ids(&["create"]), &t),
            None
        );
        // Under `ask`, an allowed change is asked; the rest stays.
        assert_eq!(
            lowered_grant(Level::Ask, false, &a, &k, &t),
            Some((false, ids(&["list"]), ids(&["create", "comment"])))
        );
        // Under `read`, changes are removed, allowed or asked.
        assert_eq!(
            lowered_grant(Level::Read, false, &a, &k, &t),
            Some((false, ids(&["list"]), ids(&[])))
        );
        // A full grant becomes the limit's level.
        assert_eq!(
            lowered_grant(Level::Ask, true, &[], &[], &t),
            Some((false, ids(&["list"]), ids(&["create", "comment"])))
        );
        assert_eq!(
            lowered_grant(Level::Read, true, &[], &[], &t),
            Some((false, ids(&["list"]), ids(&[])))
        );
        assert_eq!(connection_limit(Some("ask"), "write"), Level::Ask);
        assert_eq!(connection_limit(None, "read"), Level::Read);
        assert_eq!(access_for_limit(Level::Ask), "write");
    }

    /// The Outgoing webhook: one write tool `send`, no read tool.
    fn webhook() -> Vec<Tool> {
        vec![tool("send", "write")]
    }

    #[test]
    fn a_send_only_app_offers_ask_and_act_never_read() {
        let w = webhook();
        assert!(send_only(&w));
        assert!(!send_only(&two_writes()));
        assert!(!send_only(&[]));
        assert!(!send_only(&[tool("list", "read")]));
        assert_eq!(levels_for(&w, false, None), vec![Level::Ask, Level::Act]);
        assert_eq!(levels_for(&w, true, None), vec![Level::Ask]);
        assert_eq!(levels_for(&w, false, Some(Level::Ask)), vec![Level::Ask]);
        assert!(levels_for(&w, false, Some(Level::Read)).is_empty());
        // An app with reads and writes is unchanged; a custom-only one drops `act`.
        assert_eq!(
            levels_for(&two_writes(), false, None),
            vec![Level::Read, Level::Ask, Level::Act]
        );
        assert_eq!(
            levels_for(&two_writes(), true, None),
            vec![Level::Read, Level::Ask]
        );
        // `ask` is "ask for every send", a valid custom grant.
        let a = expand(Level::Ask, &w);
        assert_eq!(a.mode, GrantMode::Custom);
        assert_eq!((a.allow_tools, a.ask_tools), (vec![], ids(&["send"])));
        assert_eq!(expand(Level::Act, &w).mode, GrantMode::Full);
        // `read` is refused before it is expanded.
        let e = require_offered(Level::Read, &w, false, "webhook").unwrap_err();
        assert_eq!(e.code(), "level_not_offered");
        assert_eq!(
            e.details().unwrap()["offered"],
            serde_json::json!(["ask", "act"])
        );
        assert!(require_offered(Level::Ask, &w, false, "webhook").is_ok());
        assert!(require_offered(Level::Read, &two_writes(), false, "gh").is_ok());
    }

    #[test]
    fn a_send_only_saved_grant_reads_as_ask_or_act_never_read() {
        let w = webhook();
        let lvl = |full: bool, allow: &[&str], ask: &[&str]| {
            classify(full, &ids(allow), &ids(ask), &w).choice
        };
        assert_eq!(lvl(false, &[], &["send"]), Choice::Level(Level::Ask));
        assert_eq!(lvl(true, &[], &[]), Choice::Level(Level::Act));
        assert_eq!(lvl(false, &["send"], &[]), Choice::Level(Level::Act));
        // Every send blocked (only a lowered limit leaves that behind) is not `read`.
        assert_eq!(
            lvl(false, &[], &[]),
            Choice::Custom {
                base: Level::Ask,
                differs: 1
            }
        );
        for l in [Level::Ask, Level::Act] {
            let g = expand(l, &w);
            let c = classify(g.mode == GrantMode::Full, &g.allow_tools, &g.ask_tools, &w);
            assert_eq!(c.choice, Choice::Level(l));
        }
    }

    #[test]
    fn access_follows_the_levels_and_the_grant() {
        use Level::{Act, Ask, Read};
        assert_eq!(access_for_levels(&[]), "read");
        assert_eq!(access_for_levels(&[Read, Read]), "read");
        assert_eq!(access_for_levels(&[Read, Ask]), "write");
        assert_eq!(access_for_levels(&[Act]), "write");
        let t = catalogue();
        for (l, want) in [(Read, "read"), (Ask, "write"), (Act, "write")] {
            assert_eq!(access_for_grant(&expand(l, &t), &t), want, "{l:?}");
        }
    }
}
