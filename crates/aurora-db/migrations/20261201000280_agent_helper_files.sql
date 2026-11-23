-- (P6-TPL-6, D-456(e), D-457(d), C-1143) A template's helper files on an agent: stored with the
-- agent and projected read-only at `.aurora/helpers/<path>` by the supervisor. Written only by
-- `agent_helpers::replace_all` / `delete` in the API (P6-TPL-3 from templates, the DELETE route
-- here); no agent tool exists, the agent's computer only ever reads the projection. The path is
-- the C-1141 helper path below `helpers/` (checked by the writer against the aurora-template
-- rules); `sha256` is the hex digest of `content`. The gateway role gets nothing on this table.

CREATE TABLE agent_helper_files (
    agent_id   uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    path       text NOT NULL,
    content    bytea NOT NULL,
    executable boolean NOT NULL,
    sha256     text NOT NULL,
    size       integer NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (agent_id, path)
);
