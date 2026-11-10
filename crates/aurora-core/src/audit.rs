//! The security-critical audit set (docs/04 §Audit, RFE-008, D-52, SEC-32).
//!
//! Actions in [`SECURITY_CRITICAL_ACTIONS`] are audited **atomically**: the `audit_log` row is
//! written in the same transaction as the change and a failed write fails the action (503
//! `audit_unavailable`). `audit_log.critical` is derived from this list when a row with
//! `outcome = success` is inserted, so the flag can never disagree with the set.
//!
//! The names are the action strings the API emits. Where `04 §Audit` uses a descriptive name
//! (`grant.put`, `runner.provision`, `org_member.role_change`), the emitted name is listed in the
//! mapping of D-83.

/// Every security-critical action, grouped by area as in `04 §Audit`. Add an action here *and* make its service function
/// write the row with `audit::commit_critical`; the coverage test fails otherwise.
pub const SECURITY_CRITICAL_ACTIONS: &[&str] = &[
    // Keys and credentials
    "admin.connector_approve",
    "admin.connector_create",
    // (P4-MCON, C-451, C-453) managed connector create and settings/secrets/enable changes
    "connector.created",
    "connector.updated",
    "admin.connector_delete",
    "admin.connector_rotate",
    "admin.runner_create",
    "admin.runner_delete",
    "admin.runner_rotate",
    "agent.byo_settings.update",
    "agent.token_regenerate",
    "api_key.create",
    "api_key.revoke",
    "app_instance.create",
    "app_instance.delete",
    "app_instance.update",
    "cli_login.approve",
    "cli_login.consume",
    "connection.create",
    "connection.delete",
    "connection.disable",
    "connection.enable",
    "connection.reconnect",
    "connection.update",
    "oauth_client.create",
    "oauth_client.delete",
    "oauth_client.update",
    "secret.accept",
    "secret.create",
    "secret.delete",
    "secret.update",
    "secret.value_update",
    "workspace.byo_posture.update",
    // Grants
    "grant.converted_by_app",
    "grant.create",
    "grant.delete",
    // Policy
    "policy.publish",
    "policy.rollback",
    // Plugins and registry
    "plugin.approve",
    "plugin.delete",
    "plugin.disable",
    "plugin.enable",
    "plugin.host_claim.decide",
    "plugin.reject",
    "trusted_key.create",
    "trusted_key.revoke",
    // Approval decisions
    "action_approval.decide",
    "agent_link.policy",
    "approval.decide",
    "reach_grant.decide",
    // Roles and membership
    "admin.user_update",
    // (P4-IDP-1, D-139) instance admin granted or revoked by the IdP-claim sync
    "instance_admin.grant",
    "instance_admin.revoke",
    "invitation.accept",
    "invitation.cancel",
    "invitation.create",
    "member.add",
    "member.remove",
    "member.update",
    "user.delete",
    "workspace_access.remove",
    "workspace_access.set",
    // Skill library (P4-GSL, D-148; emitted as written)
    "skill.library.add",
    "skill.library.detach",
    "skill.library.update_apply",
    "skill.library.update_reject",
    "skill_library.create",
    "skill_library.delete",
    "skill_library.update",
    // Freeze
    "agent.freeze",
    "agent.unfreeze",
    "org.freeze",
    "org.unfreeze",
    "workspace.freeze",
    "workspace.unfreeze",
    // Governance and tenant settings
    "governance.update",
    "org.app_availability.update",
    "org.delete",
    "org.update",
    "retention.update",
    "workspace.delete",
    // Channels
    "channel_integration.autolink_disable",
    "channel_integration.autolink_enable",
    "channel_integration.create",
    "channel_integration.delete",
    "channel_integration.update",
    "user_link.create",
    "user_link.delete",
    // Data erasure and reassignment
    "automation.reassign",
    "conversation.delete",
    "memory.redact",
    "transcript_event.redact",
];

/// Is `action` in the security-critical set?
#[must_use]
pub fn is_critical(action: &str) -> bool {
    SECURITY_CRITICAL_ACTIONS.contains(&action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_has_no_duplicates() {
        let mut v = SECURITY_CRITICAL_ACTIONS.to_vec();
        v.sort_unstable();
        assert!(v.windows(2).all(|w| w[0] != w[1]));
    }

    #[test]
    fn membership() {
        assert!(is_critical("policy.publish"));
        assert!(!is_critical("policy.draft_update"));
        assert!(!is_critical("approval.expire"));
    }
}
