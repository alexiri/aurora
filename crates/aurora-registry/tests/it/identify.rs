#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-159 (D-329, C-930): the shared winner rule and scope check the gateway and the simulator
//! both call. The simulate == gateway table tests (`aurora-api` `simulate_apps.rs`, `aurora-gateway`
//! `apps.rs::read_only_custom_grant_decides_the_simulate_table`) cover the end-to-end decisions.
use aurora_registry::identify::{Rank, narrow, scope_missing};

fn r(is_app: bool, path_limited: bool, workspace: bool) -> Rank {
    Rank {
        is_app,
        path_limited,
        workspace,
    }
}

#[test]
fn the_winner_rule_prefers_app_then_path_then_workspace() {
    // (candidates, indices that survive)
    let table: Vec<(Vec<Rank>, Vec<usize>)> = vec![
        (vec![], vec![]),
        (vec![r(false, false, false)], vec![0]),
        // An app connection beats a generic secret, whatever the scope.
        (vec![r(false, false, true), r(true, false, false)], vec![1]),
        // A path-limited secret beats a whole-host one; workspace beats organization after that.
        (vec![r(false, false, true), r(false, true, false)], vec![1]),
        (vec![r(true, false, false), r(true, false, true)], vec![1]),
        // Two of the same rank stay ambiguous.
        (vec![r(true, false, true), r(true, false, true)], vec![0, 1]),
        (
            vec![r(true, false, false), r(true, false, false)],
            vec![0, 1],
        ),
    ];
    for (cands, want) in table {
        assert_eq!(narrow(&cands), want, "{cands:?}");
    }
}

#[test]
fn a_tool_scope_is_missing_only_when_both_sides_are_known() {
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    assert!(scope_missing(&s(&["read:b"]), &s(&["read:a"])));
    assert!(!scope_missing(&s(&["read:a", "read:b"]), &s(&["read:a"])));
    // Unknown scopes never refuse; a tool with no requirement is never missing one.
    assert!(!scope_missing(&s(&["read:b"]), &[]));
    assert!(!scope_missing(&[], &s(&["read:a"])));
}
