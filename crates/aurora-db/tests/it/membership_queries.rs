#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! (P5-APIGAP-4, SEC-138) Guard: workspace membership is decided by the `effective_workspace_access`
//! view (through `aurora_db::authz`), which honours `workspace_access.status`. A query that reads
//! the raw `workspace_access` or `group_workspace_access` tables can grant access the view would
//! not (a suspended binding, or a group binding of a suspended person). Every file that does so
//! is listed below with the number of raw references and why each is safe; a new reader, or a
//! new reference in a listed file, fails here until it is reviewed: use the view (or
//! `authz::workspace_access`), or filter `status = 'active'` / exclude suspended people and
//! update the list.

use std::path::{Path, PathBuf};

/// (file relative to the repository root, raw references, why they are safe).
const REVIEWED: &[(&str, usize, &str)] = &[
    (
        "crates/aurora-db/src/authz.rs",
        2,
        "effective_access: the role comes from the view; the raw join only explains the sources",
    ),
    (
        "crates/aurora-db/src/repo/gateway.rs",
        3,
        "can_use: the view's law restated for the gateway role, suspension included",
    ),
    (
        "crates/aurora-db/src/repo/approval_notify.rs",
        3,
        "deciders: wa.status = 'active' and suspended people excluded from the group branch",
    ),
    (
        "crates/aurora-db/src/repo/groups.rs",
        17,
        "group management, impact (a person with any direct row is not counted as losing \
         access), derived_members (rows come from the view), via_groups (suspended excluded)",
    ),
    (
        "crates/aurora-db/src/repo/workspaces.rs",
        6,
        "binding writes, the suspend/restore lock and write, and the listing (status is \
         returned, the row stays listed); roles_of is placement's 'what is bound', never an \
         access decision",
    ),
    (
        "crates/aurora-db/src/repo/idp_roles.rs",
        5,
        "the IdP role sync writes roles and never the status",
    ),
    (
        "crates/aurora-db/src/repo/members.rs",
        1,
        "deletes a removed member's bindings",
    ),
    (
        "crates/aurora-db/src/repo/account.rs",
        2,
        "account deletion: a workspace someone else is bound to (suspended or not) is kept",
    ),
    (
        "crates/aurora-db/src/repo/placement.rs",
        3,
        "rebind_personal_owner: writes and reads only the person's own binding in their personal \
         workspace (raise + activate, or insert); it grants nothing beyond the binding, which \
         own_workspace_access gates on the active membership",
    ),
    (
        "crates/aurora-db/src/repo/scim.rs",
        1,
        "counts a group's bindings for the SCIM view",
    ),
    (
        "crates/aurora-db/src/grants.rs",
        2,
        "the gateway role's SELECT grants",
    ),
    (
        "services/aurora-api/src/domains/groups/service.rs",
        1,
        "change_members lists a group's bound workspaces and roles (and the personal marker) to \
         apply the binding rules; it decides no access — the bindings act through \
         effective_workspace_access at request time",
    ),
    (
        "services/aurora-api/src/domains/scim/service.rs",
        1,
        "personal_add_refusal reads a group's bound workspaces for the personal marker to apply \
         the binding rules (D-480/C-1161); it decides no access — it only refuses or skips an \
         SCIM add, and the bindings act through effective_workspace_access at request time",
    ),
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // Tests may set up any row they like; migrations define the view.
            if name != "tests" && name != "target" && name != "migrations" {
                rust_files(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Raw references to the binding tables on one line: `workspace_access` not preceded by
/// `effective_` and not a Rust path, function or audit action (`workspace_access(`,
/// `::workspace_access`, `workspace_access.set`, `workspace_access_…`); `group_workspace_access`
/// counts as a raw reference too.
fn raw_refs(line: &str) -> usize {
    let t = line.trim_start();
    if t.starts_with("//") {
        return 0;
    }
    let needle = "workspace_access";
    let mut n = 0;
    let mut from = 0;
    while let Some(i) = line[from..].find(needle) {
        let at = from + i;
        let end = at + needle.len();
        from = end;
        let before = &line[..at];
        let after = &line[end..];
        // `effective_workspace_access` and `own_workspace_access` (FIX-230) are the views.
        if before.ends_with("effective_")
            || before.ends_with("own_")
            || before.ends_with("::")
            || before.ends_with("fn ")
        {
            continue;
        }
        let next = after.chars().next();
        if matches!(next, Some('(' | '.' | '_')) {
            continue;
        }
        if before
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_alphanumeric() && !before.ends_with("group_"))
        {
            continue;
        }
        n += 1;
    }
    n
}

#[test]
fn every_raw_workspace_membership_query_is_reviewed() {
    let root = root();
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    rust_files(&root.join("services"), &mut files);
    rust_files(&root.join("connectors"), &mut files);
    let mut problems = Vec::new();
    let mut seen = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        let n: usize = text.lines().map(raw_refs).sum();
        if n == 0 {
            continue;
        }
        let rel = f
            .strip_prefix(&root)
            .unwrap_or(f)
            .to_string_lossy()
            .replace('\\', "/");
        let rel = rel.trim_start_matches("./").to_owned();
        seen.push(rel.clone());
        match REVIEWED.iter().find(|(p, _, _)| *p == rel) {
            None => problems.push(format!(
                "{rel}: {n} raw reference(s) to workspace_access/group_workspace_access; decide \
                 membership through effective_workspace_access (authz), or honour \
                 workspace_access.status and add the file to REVIEWED"
            )),
            Some((_, want, _)) if *want != n => problems.push(format!(
                "{rel}: {n} raw reference(s), {want} reviewed; check the new one honours \
                 workspace_access.status (a suspended binding grants nothing) and update REVIEWED"
            )),
            Some(_) => {}
        }
    }
    for (p, _, _) in REVIEWED {
        if !seen.iter().any(|s| s == p) {
            problems.push(format!(
                "{p}: listed in REVIEWED but has no raw reference any more"
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn the_scanner_counts_tables_and_ignores_functions_actions_and_the_view() {
    assert_eq!(raw_refs("FROM workspace_access wa"), 1);
    assert_eq!(raw_refs("JOIN group_workspace_access b ON"), 1);
    assert_eq!(raw_refs("FROM effective_workspace_access ew"), 0);
    assert_eq!(raw_refs("JOIN own_workspace_access ea ON"), 0);
    assert_eq!(raw_refs("authz::workspace_access(&s.pool, u, ws)"), 0);
    assert_eq!(raw_refs("event(&c, \"workspace_access.set\")"), 0);
    assert_eq!(raw_refs("// reads workspace_access"), 0);
    assert_eq!(raw_refs("pub async fn workspace_access<'e>"), 0);
}
