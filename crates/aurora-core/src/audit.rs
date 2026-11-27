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
    "admin.connector_host_token_issue",
    "admin.connector_host_token_revoke",
    "admin.runner_create",
    "admin.runner_delete",
    "admin.runner_rotate",
    "agent.byo_settings.update",
    "agent.token_regenerate",
    "api_key.create",
    "api_key.revoke",
    "app_instance.create",
    "app_instance.delete",
    // (FIX-132) an org registration made instance-wide: widens who can connect to the host
    "app_instance.promote",
    "app_instance.update",
    "cli_login.approve",
    "cli_login.consume",
    // (P4-011B-3, C-833) the instance's Web Push signing key rotated (CLI)
    "push.vapid_rotate",
    "connection.create",
    "connection.delete",
    "connection.disable",
    "connection.enable",
    "connection.reconnect",
    "connection.update",
    // (P5-WEBHOOK-2, C-909) An outgoing webhook's configuration, secret values and signing secret.
    "connection.webhook.update",
    "oauth_client.create",
    "oauth_client.delete",
    // (P5-REFINE-6, C-1070) a sign-in app's Test records its result with the row
    "oauth_client.test",
    "oauth_client.update",
    // (P4-SSH-1, D-239(b), C-790) a person's registered SSH public keys
    "ssh_key.create",
    "ssh_key.delete",
    // (P4-UTOK, D-284(b), C-858) a person's chat-platform token ends (owner, link, SCIM, delete)
    "user_token.revoke",
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
    // (FIX-123, D-215) Moves connections of a superseded app to the app that replaced it.
    "plugin.supersede",
    "trusted_key.create",
    "trusted_key.revoke",
    // Addon sources (phase 5, P5-ASRC, D-250, SEC-86)
    "addon.ingest",
    "addon_source.add",
    "addon_source.check",
    "addon_source.remove",
    "addon_source.update",
    // Agent templates (phase 6, P6-TPL-2, C-1113; approve/reject/update arrive with the
    // approval routes of P6-TPL-2 slice 4)
    "agent_template.approve",
    "agent_template.ingest",
    "agent_template.reject",
    "agent_template.update",
    "agent_template_source.add",
    "agent_template_source.update",
    "agent_template_source.remove",
    // Approval decisions
    "action_approval.decide",
    "agent_link.policy",
    // (P5-REFINE-10, D-411, SEC-142) a person links two agents, changes or removes the link
    "agent_link.create",
    "agent_link.delete",
    "agent_link.direction",
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
    // (P5-REFINE-6, C-1069, SEC-140) a re-issued invitation link replaces the token
    "invitation.link",
    "invitation.resend",
    "member.add",
    "member.remove",
    "member.update",
    // (FIX-108, D-143, SEC-78) a user admitted to a closed instance by the IdP's role mapping
    "user.admit_by_claim",
    "user.delete",
    "workspace_access.remove",
    // (P5-APIGAP-4, D-399, SEC-138) one person suspended in or restored to one workspace
    "workspace_access.restore",
    "workspace_access.set",
    "workspace_access.suspend",
    // (P5-UX-5, D-325, SEC-128) a workspace owner adds a person by e-mail: bound at once, or a pending row that a
    // sign-in applies (or the owner removes)
    "workspace.access.grant",
    "workspace.access.pending_apply",
    "workspace.access.pending_create",
    "workspace.access.pending_delete",
    // (P4-GRP-1, D-130, D-156) directory groups: membership, org-role mapping, bindings
    "group.create",
    "group.delete",
    "group.update",
    "group_access.remove",
    "group_access.set",
    "group_member.add",
    "group_member.remove",
    // (P4-ONB-1, D-140) the instance admin saves the new-user placement
    "placement.update",
    // Skill library (P4-GSL, D-148; emitted as written)
    "skill.library.add",
    "skill.library.detach",
    "skill.library.update_apply",
    "skill.library.update_reject",
    // (P5-SKILLS-1, D-499) an inherited skill turned off or back on for one agent
    "skill.agent_off",
    "skill.agent_on",
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
    // (FIX-236, D-478(d), C-1164, SEC-154) emergency read access: opening it, ending it early
    // and its expiry are security-critical; every conversation read through it writes
    // `conversation.emergency_view` atomically (a failed write refuses the read, C-1165), and
    // every other read of workspace content `workspace.emergency_read` (FIX-262, D-502).
    "conversation.emergency_view",
    "workspace.emergency_read",
    "workspace.emergency_access.end",
    "workspace.emergency_access.expire",
    "workspace.emergency_access.open",
    // Port previews (P4-PRV-1, D-144, D-145, SEC-80): share lifecycle and the org switch
    "org.previews.update",
    "preview.share.create",
    "preview.share.delete",
    "preview.share.update",
    // Governance and tenant settings
    "governance.update",
    "org.app_availability.update",
    "org.delete",
    "org.update",
    "retention.update",
    "workspace.delete",
    // (P5-ADMDEL, D-234) an instance admin deletes an organization or a workspace
    "admin.org.delete",
    "admin.workspace.delete",
    // Channels
    "channel_integration.autolink_disable",
    "channel_integration.autolink_enable",
    "channel_integration.create",
    "channel_integration.delete",
    "channel_integration.update",
    "user_link.create",
    "user_link.delete",
    // Instance settings (phase 5, SET-1, D-236, C-777, SEC-90): a dashboard/API write, an import,
    // and a CLI reset; `instance_settings.helm_sync` is a best-effort system row, not listed.
    "instance_settings.update",
    "instance_settings.import",
    "instance_settings.reset",
    // (FIX-124, C-747) a dashboard job that moves every managed value to another secret store
    "secret_stores.migrate_start",
    "secret_stores.migrate_finish",
    // Data erasure and reassignment
    "automation.reassign",
    "conversation.delete",
    "memory.redact",
    "transcript_event.redact",
    // SCIM provisioning (P4-GRP-2, D-132; docs/04 §SCIM): the membership and group changes go
    // through the existing `member.*` / `group*` actions above with `metadata.via = "scim"`.
    "scim.update",
    "scim_token.create",
    "scim_token.revoke",
    // (phase 5, P4-EVAL-1, C-863, C-873) the judge key an agent's evals send its answers to; every
    // settings write is atomic (a regression-notice toggle included: one action name, one class).
    "eval.settings.update",
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
