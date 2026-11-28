-- (FIX-288, D-509) Delegation chains (A -> B -> C).

-- The conversation an approval raised in peer task `task` is reported to: walk up the origins
-- while the task above was asked for by the same person. The walk ends at the first
-- conversation that is no task conversation, or where the requester changes. A chain longer than
-- `max_depth` tasks (only one opened before the limit existed) falls back to the task's own
-- origin; so does a walk that ends in a conversation of a workspace the person can no longer use
-- (a row of `effective_workspace_access` is `can_use`). The one statement of the rule: the
-- service, the card fan-out and the sweeps use it.
CREATE FUNCTION peer_task_card_home(task uuid, max_depth integer) RETURNS uuid
LANGUAGE sql STABLE AS $$
    WITH RECURSIVE up (requester, origin, depth) AS (
        SELECT t.requester_user_id, t.origin_conversation_id, 1
          FROM peer_tasks t WHERE t.id = task
        UNION ALL
        SELECT up.requester, p.origin_conversation_id, up.depth + 1
          FROM up JOIN peer_tasks p ON p.task_conversation_id = up.origin
         WHERE p.requester_user_id = up.requester AND up.depth <= max_depth
    ),
    walked AS (
        SELECT requester, origin FROM up
         ORDER BY CASE WHEN (SELECT max(depth) FROM up) > max_depth THEN depth ELSE -depth END
         LIMIT 1
    )
    SELECT CASE
        WHEN w.origin = own.origin THEN own.origin
        WHEN EXISTS (
            SELECT 1 FROM conversations c
              JOIN effective_workspace_access a
                ON a.workspace_id = c.workspace_id AND a.user_id = w.requester
             WHERE c.id = w.origin
        ) THEN w.origin
        ELSE own.origin
    END
      FROM walked w, up own
     WHERE own.depth = 1
$$;

-- Wake-ups of a delegating task held while its agent is frozen (the report of a task it opened,
-- or the outcome of an approval further down); the sweep delivers each once after the unfreeze.
CREATE TABLE peer_held_wakes (
    task_id    uuid NOT NULL REFERENCES peer_tasks (id) ON DELETE CASCADE,
    -- What happened, so the same event is held once: `approval:<id>` or `report:<task id>`.
    source     text NOT NULL,
    -- The task the event is about (the turn's door).
    about_task uuid NOT NULL,
    -- The wake turn's text.
    text       text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (task_id, source)
);
