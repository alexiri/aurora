#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! FIX-159 (D-329, C-930): the shared winner rule and scope check the gateway and the simulator
//! both call. The simulate == gateway table tests (`aurora-api` `simulate_apps.rs`, `aurora-gateway`
//! `apps.rs::read_only_custom_grant_decides_the_simulate_table`) cover the end-to-end decisions.
use aurora_registry::Manifest;
use aurora_registry::identify::{Rank, identify_app, identify_instance_app, narrow, scope_missing};
use aurora_registry::index::Snapshot;
use aurora_registry::snapshot::{ActivePackage, compile};

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

/// (FIX-215, SEC-04/SEC-59) The shipped atlassian package: a host entry's `path_prefix` covers
/// whole path segments, so no credential is selected on a sibling path of the same host.
#[test]
fn atlassian_path_prefixes_cover_whole_segments_only() {
    let path = format!(
        "{}/../../plugins/builtin/atlassian/plugin.yaml",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut m = Manifest::parse(&std::fs::read_to_string(&path).unwrap(), "9.9.9").unwrap();
    // The agent guides are digested from the stored archive; identification never reads them.
    for a in &mut m.apps {
        a.agent = None;
    }
    let p = ActivePackage {
        manifest: m,
        digest: "d".into(),
        publisher: "aurora".into(),
        hooks_digest: None,
    };
    let snap = Snapshot::from_slice(&compile(&[p], &[], &[], 1).unwrap().json).unwrap();
    let app_at = |path: &str| {
        identify_app(&snap, "api.atlassian.com", 443, path).map(|m| m.app.app_id.clone())
    };
    assert_eq!(
        app_at("/ex/jira/c1/rest/api/3/myself").as_deref(),
        Some("jira")
    );
    assert_eq!(app_at("/ex/jira").as_deref(), Some("jira"));
    assert_eq!(app_at("/ex/jiraxyz/c1"), None);
    assert_eq!(
        app_at("/ex/confluence/c1/wiki/api/v2/pages").as_deref(),
        Some("confluence")
    );
    assert_eq!(app_at("/ex/confluencex"), None);
    assert_eq!(app_at("/oauth/tokenx"), None);
    // A registered Jira Data Center instance: `/rest` covers the REST API, not `/restricted`.
    let inst = |path: &str| {
        identify_instance_app(&snap, "jira-dc", path).map(|m| m.entry.path_prefix.clone())
    };
    assert_eq!(inst("/rest/api/2/issue"), Some(Some("/rest".to_owned())));
    assert_eq!(inst("/rest"), Some(Some("/rest".to_owned())));
    assert_eq!(inst("/restricted"), None);
    assert_eq!(inst("/restricted/api/2/issue"), None);
}
