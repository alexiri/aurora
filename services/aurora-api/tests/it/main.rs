//! Single integration-test binary for this crate (OPS-016): one link instead of one per file.
mod action_approvals;
mod activity;
mod admin;
mod agent_token_age;
mod agents;
mod api_ratelimit;
mod app_instances;
mod approval_notify;
mod approval_stall;
mod approvals;
mod apps_connections;
mod apps_tools;
mod attachment_storage;
mod attachments;
mod audit;
mod audit_atomic;
mod auth;
mod automation_continuity;
mod avatars;
mod byo;
mod channels_cards;
mod channels_inbox;
mod channels_messaging;
mod channels_mgmt;
mod cli;
mod common;
mod connect;
mod connector_ops;
mod connectors;
mod connectors_managed;
mod conversation_lifecycle;
mod conversations;
mod counters;
mod direct_visibility;
mod fix_029;
mod foundation;
mod freeze;
mod governance;
mod grants_bulk;
mod grants_connections;
mod grants_custom_only;
mod groups;
mod groups_idp;
mod harness_selection;
mod home_checksum;
mod home_render_inputs;
mod hosted;
mod invitations;
mod jobs;
mod key_rings;
mod keys;
mod managed_support;
mod me;
mod memory;
mod message_idempotency;
mod model_readiness;
mod oidc;
mod oidc_roles;
mod onboarding;
mod peer_tasks;
mod peers_rig;
mod permission_matrix;
mod placement;
mod plugins;
mod plugins_lifecycle;
mod plugins_upgrade;
mod policy;
mod preview_auth;
mod previews;
mod runner_plane;
mod sandbox_env;
mod schedules;
mod scim_auth;
mod scim_common;
mod scim_deprovision;
mod scim_groups;
mod scim_idp_replay;
mod scim_link;
mod scim_protocol;
mod scim_scope;
mod secrets;
mod secrets_migrate;
mod skill_library;
mod skills;
mod sse_revocation;
mod tenancy;
mod terminal;
mod tools;
mod webhooks;

#[test]
fn init_scim_logs() {
    scim_test_logs();
}

/// SCIM debugging: surface the `tracing` output of the API code (the 500 paths log the
/// real cause) on stderr. A no-op unless `SCIM_LOG` is set.
fn scim_test_logs() {
    if std::env::var("SCIM_LOG").is_ok() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing_subscriber::filter::LevelFilter::DEBUG)
            .with_writer(std::io::stderr)
            .try_init();
    }
}
