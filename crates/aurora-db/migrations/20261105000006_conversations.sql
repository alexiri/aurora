-- 03 §6 Conversations, turns, transcript, attachments, blobs; plus deferred FKs to conversations
-- from earlier files (action_approvals, peer_tasks, gateway_credentials).
-- Size caps tied to TURN_TEXT_MAX / transcript payload bounds are enforced by the repository.

CREATE TABLE conversations (
    id                    uuid PRIMARY KEY,
    agent_id              uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    workspace_id          uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    kind                  text NOT NULL CHECK (kind IN ('direct', 'group', 'schedule', 'webhook', 'watch', 'peer', 'api')),
    external_ref          text NOT NULL,
    direct_user_id        uuid REFERENCES users (id) ON DELETE CASCADE,
    title                 text,
    origin_conversation_id uuid REFERENCES conversations (id) ON DELETE SET NULL,
    harness_session_ref   text,
    harness_session_epoch bigint NOT NULL DEFAULT 0,
    last_seq              bigint NOT NULL DEFAULT 0,
    app_turn_streak       integer NOT NULL DEFAULT 0,
    last_activity_at      timestamptz NOT NULL, -- churn column
    created_at            timestamptz NOT NULL DEFAULT now(),
    updated_at            timestamptz NOT NULL DEFAULT now(),
    UNIQUE (agent_id, kind, external_ref)
);
CREATE UNIQUE INDEX conversations_direct_user_uq ON conversations (agent_id, direct_user_id) WHERE kind = 'direct';
CREATE INDEX conversations_workspace_idx ON conversations (workspace_id);
CREATE INDEX conversations_direct_user_idx ON conversations (direct_user_id) WHERE direct_user_id IS NOT NULL;
CREATE INDEX conversations_origin_idx ON conversations (origin_conversation_id) WHERE origin_conversation_id IS NOT NULL;

CREATE TABLE turns (
    id                    uuid PRIMARY KEY,
    conversation_id       uuid NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    agent_id              uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    role                  text NOT NULL CHECK (role IN ('turn', 'followup')),
    status                text NOT NULL CHECK (status IN ('queued', 'dispatched', 'running', 'done', 'failed', 'aborted', 'joining', 'joined', 'promoted')),
    parent_turn_id        uuid REFERENCES turns (id) ON DELETE CASCADE,
    door                  jsonb NOT NULL,
    speaker_kind          text NOT NULL CHECK (speaker_kind IN ('person', 'bot', 'agent', 'system')),
    speaker_user_id       uuid REFERENCES users (id) ON DELETE SET NULL,
    speaker_label         text NOT NULL,
    text                  text NOT NULL,
    source_event_id       text,
    abort_requested_at    timestamptz,
    send_now_requested_at timestamptz,
    error_code            text CHECK (char_length(error_code) <= 64),
    error_message         text,
    usage                 jsonb,
    dispatched_at         timestamptz,
    started_at            timestamptz,
    finished_at           timestamptz,
    last_progress_at      timestamptz, -- churn column
    dispatch_lease_until  timestamptz, -- claim clock (dispatch)
    revived_at            timestamptz,
    ceiling_warned_at     timestamptz,
    steer_delivered_at    timestamptz,
    promoted_at           timestamptz,
    boot_id               uuid,
    last_event_n          integer NOT NULL DEFAULT 0,
    created_at            timestamptz NOT NULL DEFAULT now(),
    updated_at            timestamptz NOT NULL DEFAULT now(),
    -- 03: `turn` rows use the first six statuses; `followup` rows use joining/joined/promoted.
    CONSTRAINT turns_role_status_ck CHECK (
        (role = 'turn' AND status IN ('queued', 'dispatched', 'running', 'done', 'failed', 'aborted'))
        OR (role = 'followup' AND status IN ('joining', 'joined', 'promoted'))
    )
);
-- One active turn per conversation.
CREATE UNIQUE INDEX turns_one_active_per_conversation_uq ON turns (conversation_id)
    WHERE role = 'turn' AND status IN ('queued', 'dispatched', 'running');
CREATE UNIQUE INDEX turns_source_event_uq ON turns (conversation_id, source_event_id) WHERE source_event_id IS NOT NULL;
CREATE INDEX turns_agent_idx ON turns (agent_id);
CREATE INDEX turns_parent_idx ON turns (parent_turn_id) WHERE parent_turn_id IS NOT NULL;
CREATE INDEX turns_speaker_user_idx ON turns (speaker_user_id) WHERE speaker_user_id IS NOT NULL;

CREATE TABLE transcript_events (
    conversation_id uuid NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    seq             bigint NOT NULL,
    turn_id         uuid,
    type            text NOT NULL,
    payload         jsonb NOT NULL, -- bounded (<= 64 KiB serialized); repository validates
    created_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (conversation_id, seq)
);

CREATE TABLE attachments (
    id                 uuid PRIMARY KEY,
    workspace_id       uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    conversation_id    uuid NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    turn_id            uuid REFERENCES turns (id) ON DELETE SET NULL,
    direction          text NOT NULL CHECK (direction IN ('in', 'out')),
    status             text NOT NULL CHECK (status IN ('pending', 'bound', 'failed')),
    filename           text NOT NULL,
    content_type       text NOT NULL,
    size_bytes         bigint NOT NULL,
    sha256             text NOT NULL,
    blob_backend       text NOT NULL,
    blob_key           text NOT NULL,
    caption            text,
    failure_reason     text,
    created_by_user_id uuid,
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX attachments_conversation_idx ON attachments (conversation_id);
CREATE INDEX attachments_turn_idx ON attachments (turn_id) WHERE turn_id IS NOT NULL;
CREATE INDEX attachments_workspace_idx ON attachments (workspace_id);

-- Phase 0 blob backend (`postgres`); content-addressed by key, immutable: no updated_at.
CREATE TABLE blobs (
    key        text PRIMARY KEY,
    content    bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- Deferred foreign keys to conversations.
ALTER TABLE action_approvals ADD CONSTRAINT action_approvals_conversation_id_fkey
    FOREIGN KEY (conversation_id) REFERENCES conversations (id) ON DELETE SET NULL;
ALTER TABLE peer_tasks ADD CONSTRAINT peer_tasks_origin_conversation_id_fkey
    FOREIGN KEY (origin_conversation_id) REFERENCES conversations (id) ON DELETE CASCADE;
ALTER TABLE peer_tasks ADD CONSTRAINT peer_tasks_task_conversation_id_fkey
    FOREIGN KEY (task_conversation_id) REFERENCES conversations (id) ON DELETE CASCADE;
ALTER TABLE gateway_credentials ADD CONSTRAINT gateway_credentials_conversation_id_fkey
    FOREIGN KEY (conversation_id) REFERENCES conversations (id) ON DELETE CASCADE;
CREATE INDEX action_approvals_conversation_idx ON action_approvals (conversation_id) WHERE conversation_id IS NOT NULL;
CREATE INDEX peer_tasks_origin_conversation_idx ON peer_tasks (origin_conversation_id);
CREATE INDEX peer_tasks_task_conversation_idx ON peer_tasks (task_conversation_id);
CREATE INDEX gateway_credentials_conversation_idx ON gateway_credentials (conversation_id) WHERE conversation_id IS NOT NULL;
