-- P2-008 (RFE-008, D-52, SEC-32): `audit_log.critical` marks rows of the security-critical set
-- (aurora_core::audit::SECURITY_CRITICAL_ACTIONS), which are written in the same transaction as
-- the change. Additive; the default keeps every existing row valid. Existing successful rows of
-- the set are flagged so the auditor filter also covers history (the flag is derived data).
ALTER TABLE audit_log ADD COLUMN critical boolean NOT NULL DEFAULT false;

UPDATE audit_log SET critical = true
WHERE outcome = 'success' AND action IN (
    'admin.connector_approve', 'admin.connector_create', 'admin.connector_delete',
    'admin.connector_rotate', 'admin.runner_create', 'admin.runner_delete', 'admin.runner_rotate',
    'agent.byo_settings.update', 'agent.token_regenerate', 'api_key.create', 'api_key.revoke',
    'app_instance.create', 'app_instance.delete', 'app_instance.update', 'cli_login.approve', 'cli_login.consume',
    'connection.create', 'connection.delete', 'connection.disable', 'connection.enable', 'connection.reconnect',
    'connection.update', 'oauth_client.create', 'oauth_client.delete', 'oauth_client.update',
    'secret.accept', 'secret.create', 'secret.delete', 'secret.update', 'secret.value_update',
    'workspace.byo_posture.update', 'grant.create', 'grant.delete', 'policy.publish',
    'policy.rollback', 'plugin.approve', 'plugin.delete', 'plugin.disable', 'plugin.enable',
    'plugin.host_claim.decide', 'plugin.reject', 'trusted_key.create', 'trusted_key.revoke',
    'action_approval.decide', 'agent_link.policy', 'approval.decide',
    'reach_grant.decide', 'admin.user_update', 'invitation.accept', 'invitation.cancel',
    'invitation.create', 'member.add', 'member.remove', 'member.update',    'user.delete', 'workspace_access.remove', 'workspace_access.set', 'agent.freeze',
    'agent.unfreeze', 'org.freeze', 'org.unfreeze', 'workspace.freeze', 'workspace.unfreeze',
    'governance.update', 'org.app_availability.update', 'org.delete', 'org.update',
    'retention.update', 'workspace.delete', 'channel_integration.autolink_disable',
    'channel_integration.autolink_enable', 'channel_integration.create',
    'channel_integration.delete', 'channel_integration.update', 'user_link.create',
    'user_link.delete', 'automation.reassign', 'conversation.delete', 'memory.redact',
    'transcript_event.redact'
);

CREATE INDEX audit_log_org_critical_created_idx ON audit_log (org_id, critical, created_at DESC);
