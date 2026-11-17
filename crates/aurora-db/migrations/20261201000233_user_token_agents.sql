-- P4-UTOK (D-283(a), C-855): the owner's grants of a token to single agents. No grant, no send; a
-- grant of one agent is no grant of another. A grant ends with the token or the agent.
CREATE TABLE user_token_agents (
    user_token_id uuid NOT NULL REFERENCES user_tokens (id) ON DELETE CASCADE,
    agent_id      uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    granted_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_token_id, agent_id)
);
CREATE INDEX user_token_agents_agent_idx ON user_token_agents (agent_id);
