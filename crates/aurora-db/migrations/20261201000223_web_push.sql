-- P4-011B-3 (D-134, D-266, C-830..C-834, SEC-75, SEC-76): Web Push. The instance's VAPID keys, one
-- row per browser subscription (sealed endpoint and keys, bound to the session that registered it)
-- and the idempotent delivery claims. The gateway role has no access to any of these tables.

CREATE TABLE webpush_vapid_keys (
    id          uuid PRIMARY KEY,
    public_key  text NOT NULL,
    private_ref text NOT NULL,
    status      text NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    retired_at  timestamptz,
    CONSTRAINT webpush_vapid_keys_status_ck CHECK (status IN ('active', 'retired')),
    CONSTRAINT webpush_vapid_keys_retired_ck CHECK ((status = 'retired') = (retired_at IS NOT NULL))
);
-- Exactly one active key (C-833): the first replica to find none inserts it under an advisory lock.
CREATE UNIQUE INDEX webpush_vapid_keys_one_active ON webpush_vapid_keys (status) WHERE status = 'active';

CREATE TABLE push_subscriptions (
    id              uuid PRIMARY KEY,
    user_id         uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    session_id      uuid NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    vapid_key_id    uuid REFERENCES webpush_vapid_keys (id) ON DELETE RESTRICT,
    endpoint_hash   bytea NOT NULL UNIQUE,
    endpoint_host   text NOT NULL,
    secret_ref      text NOT NULL,
    label           text NOT NULL,
    show_details    boolean NOT NULL DEFAULT false,
    paused_at       timestamptz,
    expires_at      timestamptz,
    failure_count   integer NOT NULL DEFAULT 0,
    disabled_at     timestamptz,
    last_success_at timestamptz,
    last_failure_at timestamptz,
    created_at      timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT push_subscriptions_label_ck CHECK (char_length(label) <= 80),
    CONSTRAINT push_subscriptions_failures_ck CHECK (failure_count >= 0)
);
CREATE INDEX push_subscriptions_user_idx ON push_subscriptions (user_id);
CREATE INDEX push_subscriptions_session_idx ON push_subscriptions (session_id);
CREATE INDEX push_subscriptions_vapid_key_idx ON push_subscriptions (vapid_key_id) WHERE vapid_key_id IS NOT NULL;

CREATE TABLE approval_push_claims (
    approval_id uuid NOT NULL,
    user_id     uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    stage       text NOT NULL,
    kind        text NOT NULL,
    delivery    text NOT NULL,
    sent_at     timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (approval_id, user_id, stage),
    CONSTRAINT approval_push_claims_stage_ck CHECK (stage IN ('initial', 'reminder')),
    CONSTRAINT approval_push_claims_kind_ck CHECK (kind IN ('gateway', 'action')),
    CONSTRAINT approval_push_claims_delivery_ck CHECK (delivery IN ('push', 'batch', 'digest'))
);
CREATE INDEX approval_push_claims_user_sent_idx ON approval_push_claims (user_id, sent_at);
