-- FIX-005 (FUP-M4.1): the answer-rule buffer (text after the turn's last tool call) is durable,
-- so an API replica restart mid-turn cannot lose answer text. Written with each `turn.events`
-- batch under the turn lock; consumed (deleted) when the turn result emits the `text` events.
CREATE TABLE turn_answer_buffers (
    turn_id uuid PRIMARY KEY REFERENCES turns(id) ON DELETE CASCADE,
    msgs    jsonb NOT NULL DEFAULT '[]'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
