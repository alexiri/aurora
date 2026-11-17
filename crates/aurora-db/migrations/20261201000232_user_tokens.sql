-- P4-UTOK (D-281..D-284, SEC-112, SEC-113; C-853, C-855): a person's own chat-platform token, which
-- lets an agent they chose post as them after they approve each post. Owned by the person; one
-- integration per token. `token_ref` is a SecretRef (purpose `user_token`, owner = this row's id)
-- and is always set while the token is `active` (the CHECK). A platform user has one live token per
-- integration (partial unique index: a revoked token no longer counts). `token_ref` stays set
-- on an ended (`invalid` or `revoked`) token only until its sealed value is discarded, right
-- after the ending transaction commits or by the sweep, and is then null. The gateway database
-- role gets nothing on this table.
CREATE TABLE user_tokens (
    id                uuid PRIMARY KEY,
    user_id           uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    integration_id    uuid NOT NULL REFERENCES channel_integrations (id) ON DELETE CASCADE,
    -- The platform user the verify op returned; equal to the owner's link on the integration.
    platform_user_ref text NOT NULL,
    -- The handle the verify op returned, shown to the owner (never the platform user id).
    platform_user_label text NOT NULL,
    label           text CHECK (label IS NULL OR char_length(label) BETWEEN 1 AND 80),
    token_ref         text,
    status            text NOT NULL CHECK (status IN ('active', 'invalid', 'revoked')),
    last_used_at      timestamptz,
    revoked_at        timestamptz,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    CHECK (status <> 'active' OR token_ref IS NOT NULL)
);
CREATE UNIQUE INDEX user_tokens_live_per_platform_user_idx
    ON user_tokens (integration_id, platform_user_ref) WHERE status <> 'revoked';
CREATE INDEX user_tokens_user_idx ON user_tokens (user_id, created_at DESC, id DESC);
CREATE INDEX user_tokens_integration_idx ON user_tokens (integration_id);

-- Revocation finds the open sends of one token (pending approvals, queued posts) by this key.
CREATE INDEX action_approvals_send_as_me_idx
    ON action_approvals ((payload ->> 'token_id'))
    WHERE action_key = 'send_as_me' AND status = 'pending';
CREATE INDEX channel_ops_user_token_idx
    ON channel_ops ((payload -> 'sender' ->> 'user_token'))
    WHERE kind = 'message.post' AND status IN ('pending', 'claimed')
      AND payload -> 'sender' ? 'user_token';
