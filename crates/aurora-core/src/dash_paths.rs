//! The dashboard's page paths, one function per page (P5-UX-2, C-960, C-981).
//!
//! The API (`WhoCan.path`, links in refusals) and the gateway (its link builder, C-959) take
//! every path they emit from here, so a page that moves is changed in one place. The routes are
//! the table of `docs/14-web-dashboard.md` §Information architecture, "Topics per level". All
//! paths are relative (no scheme or host); the caller joins them to `external_url`.
//!
//! Slugs and ids are passed as they are stored: organization and workspace slugs and UUIDs
//! contain only URL-safe characters, and an app id is a package-defined slug (`[a-z0-9-]`).

/// A workspace's base: `/o/<org>/w/<ws>`.
fn ws(org: &str, workspace: &str) -> String {
    format!("/o/{org}/w/{workspace}")
}

// ---- Administration ----

#[must_use]
pub fn admin_apps(tab: &str) -> String {
    format!("/admin/apps?tab={tab}")
}

#[must_use]
pub fn admin_oauth_clients() -> String {
    admin_apps("oauth-clients")
}

#[must_use]
pub fn admin_self_hosted() -> String {
    admin_apps("self-hosted")
}

#[must_use]
pub fn admin_app_package(package_id: &str) -> String {
    format!("/admin/apps/packages/{package_id}")
}

#[must_use]
pub fn admin_people(tab: &str) -> String {
    format!("/admin/people?tab={tab}")
}

#[must_use]
pub fn admin_ai_models() -> String {
    "/admin/ai-models".into()
}

#[must_use]
pub fn admin_internet_access() -> String {
    "/admin/internet-access".into()
}

// ---- Organization ----

#[must_use]
pub fn org_overview(org: &str) -> String {
    format!("/o/{org}")
}

#[must_use]
pub fn org_apps(org: &str, tab: &str) -> String {
    format!("/o/{org}/apps?tab={tab}")
}

#[must_use]
pub fn org_oauth_clients(org: &str) -> String {
    org_apps(org, "oauth-clients")
}

#[must_use]
pub fn org_self_hosted(org: &str) -> String {
    org_apps(org, "self-hosted")
}

/// An app's setup page at the organization level.
#[must_use]
pub fn org_app_setup(org: &str, app: &str) -> String {
    format!("/o/{org}/apps/{app}")
}

#[must_use]
pub fn org_people(org: &str, tab: &str) -> String {
    format!("/o/{org}/people?tab={tab}")
}

#[must_use]
pub fn org_ai_models(org: &str) -> String {
    format!("/o/{org}/ai-models")
}

#[must_use]
pub fn org_internet_access(org: &str) -> String {
    format!("/o/{org}/internet-access")
}

#[must_use]
pub fn org_settings(org: &str, tab: &str) -> String {
    format!("/o/{org}/settings?tab={tab}")
}

// ---- Workspace ----

#[must_use]
pub fn ws_overview(org: &str, workspace: &str) -> String {
    ws(org, workspace)
}

#[must_use]
pub fn ws_agents(org: &str, workspace: &str) -> String {
    format!("{}/agents", ws(org, workspace))
}

/// An agent's page on its Access tab; the page accepts an id (FIX-153).
#[must_use]
pub fn ws_agent_access(org: &str, workspace: &str, agent_id: &str) -> String {
    format!("{}/agents/{agent_id}?tab=access", ws(org, workspace))
}

/// The journey's deep link (C-959): the agent's Access tab with the connect journey open.
#[must_use]
pub fn ws_agent_connect(org: &str, workspace: &str, agent_id: &str, app: &str) -> String {
    format!(
        "{}/agents/{agent_id}?tab=access&connect={app}",
        ws(org, workspace)
    )
}

#[must_use]
pub fn ws_apps(org: &str, workspace: &str, tab: &str) -> String {
    format!("{}/apps?tab={tab}", ws(org, workspace))
}

/// The workspace's accounts list.
#[must_use]
pub fn ws_accounts(org: &str, workspace: &str) -> String {
    ws_apps(org, workspace, "accounts")
}

/// The workspace-level connect link (C-959 `connect_url_workspace`).
#[must_use]
pub fn ws_connect(org: &str, workspace: &str, app: &str) -> String {
    format!("{}&connect={app}", ws_apps(org, workspace, "apps"))
}

/// The Reconnect dialog of one account on the workspace's accounts list (C-842, C-959 `reconnect_url`).
#[must_use]
pub fn ws_reconnect(org: &str, workspace: &str, connection: &str) -> String {
    format!("{}&reconnect={connection}", ws_accounts(org, workspace))
}

/// An app's setup page at the workspace level.
#[must_use]
pub fn ws_app_setup(org: &str, workspace: &str, app: &str) -> String {
    format!("{}/apps/{app}", ws(org, workspace))
}

#[must_use]
pub fn ws_people(org: &str, workspace: &str) -> String {
    format!("{}/people", ws(org, workspace))
}

#[must_use]
pub fn ws_settings(org: &str, workspace: &str, tab: &str) -> String {
    format!("{}/settings?tab={tab}", ws(org, workspace))
}

#[must_use]
pub fn ws_internet_access(org: &str, workspace: &str) -> String {
    format!("{}/internet-access", ws(org, workspace))
}

/// The workspace's Approvals page, optionally focused on one request (C-983 `focus=`).
#[must_use]
pub fn ws_approvals(org: &str, workspace: &str, focus: Option<&str>) -> String {
    match focus {
        Some(f) => format!("{}/approvals?focus={f}", ws(org, workspace)),
        None => format!("{}/approvals", ws(org, workspace)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_follow_the_information_architecture_table() {
        assert_eq!(admin_oauth_clients(), "/admin/apps?tab=oauth-clients");
        assert_eq!(org_oauth_clients("acme"), "/o/acme/apps?tab=oauth-clients");
        assert_eq!(org_app_setup("acme", "app1"), "/o/acme/apps/app1");
        assert_eq!(
            ws_app_setup("acme", "main", "app1"),
            "/o/acme/w/main/apps/app1"
        );
        assert_eq!(
            ws_accounts("acme", "main"),
            "/o/acme/w/main/apps?tab=accounts"
        );
        assert_eq!(ws_people("acme", "main"), "/o/acme/w/main/people");
        assert_eq!(
            ws_agent_connect("acme", "main", "a1", "app1"),
            "/o/acme/w/main/agents/a1?tab=access&connect=app1"
        );
        assert_eq!(
            ws_connect("acme", "main", "app1"),
            "/o/acme/w/main/apps?tab=apps&connect=app1"
        );
        assert_eq!(
            ws_approvals("acme", "main", Some("r1")),
            "/o/acme/w/main/approvals?focus=r1"
        );
    }
}
