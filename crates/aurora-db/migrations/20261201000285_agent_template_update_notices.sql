-- (FIX-251, D-488(a)) When a template version becomes the offered one, the people who can apply
-- an update to an agent made from that template are told — one in-app notice per workspace and
-- version, exactly once: the row IS the dedup (a withdrawal and a re-offer of the same version
-- never sends again). Keyed by the template id (no FK), like the provenance, so removing a
-- source or template changes no workspace. The gateway role gets nothing on this table.

CREATE TABLE agent_template_update_notices (
    workspace_id uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    template_id  text NOT NULL,
    version      text NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, template_id, version)
);
CREATE INDEX agent_template_update_notices_workspace_idx
    ON agent_template_update_notices (workspace_id, created_at DESC);
