-- (P6-TPL-3, D-457(f), C-1112, SEC-146) Agent-template provenance: which template and version an
-- agent was created from, with the checked input values, and one row per part the agent last
-- took from the template (`instructions`, each own `skill`, `helpers`, each `library_skill`).
-- Everything is text/keyed by the template id (no FK), so deleting a source or template changes
-- no agent: agents made from templates are copies. `content_sha256` is the C-1104 part hash of
-- what was last written from the template; for `library_skill` the hash of the ref key — a
-- library skill's content is never compared (the library's refresh proposes it, D-457(g)).
-- Written at creation and at each apply (C-1112); cleared by the detach (DELETE
-- /v1/agents/{agent}/template). The gateway role gets nothing on these columns and this table.

ALTER TABLE agents
    ADD COLUMN template_id      text,
    ADD COLUMN template_name    text,
    ADD COLUMN template_version text,
    ADD COLUMN template_inputs  jsonb;

CREATE TABLE agent_template_parts (
    agent_id       uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    kind           text NOT NULL CHECK (kind IN ('instructions', 'skill', 'helpers', 'library_skill')),
    key            text NOT NULL DEFAULT '',
    content_sha256 text NOT NULL,
    PRIMARY KEY (agent_id, kind, key)
);
