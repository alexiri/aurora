-- (P6-TPL-2, C-1108) An agent template version that reaches `pending_approval` is a decision
-- item (C-1022): one notice per active instance admin, exactly once. The kind joins the CHECK.

ALTER TABLE admin_attention_notices DROP CONSTRAINT admin_attention_notices_kind_check;
ALTER TABLE admin_attention_notices ADD CONSTRAINT admin_attention_notices_kind_check
    CHECK (kind IN
        ('connector_approval', 'package_approval', 'addon_key_changed', 'host_claim_conflict',
         'agent_template_version'));
