//! Single integration-test binary for this crate (OPS-016): one link instead of one per file.
mod accounts;
mod action_approvals;
mod activity;
mod addons;
mod admin;
mod admin_delete;
mod admin_overview_previews;
mod agent_token_age;
mod agents;
mod api_ratelimit;
mod app_access;
mod app_instances;
mod app_setup;
mod approval_always;
mod approval_notify;
mod approval_park;
mod approval_park_revoke;
mod approval_stall;
mod approvals;
mod apps_connections;
mod apps_tools;
mod attachment_storage;
mod attachments;
mod audit;
mod audit_atomic;
mod audit_instance;
mod auth;
mod automation_continuity;
mod avatars;
mod byo;
mod calendar_invitations;
mod channels_cards;
mod channels_inbox;
mod channels_messaging;
mod channels_mgmt;
mod chat_reply;
mod chat_search;
mod cli;
mod common;
mod connect;
mod connect_access;
mod connector_ops;
mod connectors;
mod connectors_host_tokens;
mod connectors_managed;
mod conversation_lifecycle;
mod conversations;
mod counters;
mod direct_visibility;
mod evals_judge;
mod evals_questions;
mod evals_runs;
mod files;
mod fix_029;
mod foundation;
mod freeze;
mod governance;
mod grants_apps;
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
mod model_roles;
mod oidc;
mod oidc_roles;
mod onboarding;
mod org_limit;
mod peer_tasks;
mod peers_rig;
mod pending_access;
mod permission_matrix;
mod placement;
mod plugins;
mod plugins_lifecycle;
mod plugins_supersede;
mod plugins_trust;
mod plugins_upgrade;
mod policy;
mod preview_auth;
mod previews;
mod runner_plane;
mod sandbox_env;
mod schedules;
mod scim_auth;
mod scim_common;
mod scim_counters;
mod scim_deprovision;
mod scim_errors;
mod scim_groups;
mod scim_idp_replay;
mod scim_link;
mod scim_protocol;
mod scim_scope;
mod scim_settings_put;
mod secret_stores;
mod secrets;
mod secrets_migrate;
mod settings;
mod settings_governance;
mod settings_llm;
mod settings_ops;
mod settings_signin;
mod simulate_apps;
mod skill_library;
mod skills;
mod sse_revocation;
mod ssh;
mod ssh_sessions;
mod telemetry_ingest;
mod tenancy;
mod terminal;
mod tools;
mod trace_e2e;
mod turn_trace_context;
mod usage_by_model;
mod usage_ledger;
mod usage_routes;
mod user_tokens;
mod web_push;
mod web_push_ssrf;
mod webhooks;
mod who_can;

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
