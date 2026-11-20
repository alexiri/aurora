-- P5-REFINE-10 (D-411, C-1085, SEC-142): people create agent links and say who may hand tasks
-- to whom.
--
-- `direction` is stored on the ordered pair (a = the lower id, as `agent_links_ordered_ck` orders
-- it): `both`, `a_to_b` (a hands tasks to b, b only works on what a sends) or `b_to_a`. Existing
-- links keep `both` (what they allowed until now).
--
-- `created_by_kind`: every link before this migration was created by an agent's first
-- `message_agent` (there was no other path), so existing rows are `agent`; the agent path keeps the
-- default and the people route writes `person`. `created_by_user_id` is the person (NULL for an
-- agent, and once that user is deleted). The gateway role has no grant on `agent_links`.
ALTER TABLE agent_links
    ADD COLUMN direction text NOT NULL DEFAULT 'both'
        CONSTRAINT agent_links_direction_ck CHECK (direction IN ('both', 'a_to_b', 'b_to_a')),
    ADD COLUMN created_by_kind text NOT NULL DEFAULT 'agent'
        CONSTRAINT agent_links_created_by_kind_ck CHECK (created_by_kind IN ('person', 'agent')),
    ADD COLUMN created_by_user_id uuid NULL REFERENCES users (id) ON DELETE SET NULL;
