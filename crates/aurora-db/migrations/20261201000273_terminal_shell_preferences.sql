-- (FIX-218, D-459, C-1145; 03 §terminal_shell_preferences) The person's chosen shell per agent,
-- stored on the server so it follows the person across browsers and into SSH (it used to live in
-- the browser's localStorage). One writer: `PUT /v1/agents/{agent}/terminal/shell`. The value is
-- the closed enum the supervisor maps to a fixed binary, never a path.
CREATE TABLE terminal_shell_preferences (
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    agent_id   uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    shell      text NOT NULL CHECK (shell IN ('bash', 'zsh')),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, agent_id)
);
