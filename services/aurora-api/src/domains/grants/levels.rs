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
