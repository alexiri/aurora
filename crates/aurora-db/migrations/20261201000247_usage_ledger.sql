-- P5-USAGE-1 (D-359, D-360, C-1001, C-1002): workspace usage, tokens and time.
--
-- `usage_daily` is a ledger of turns, turn time and tokens per (workspace, agent, UTC day, model), added to
-- in the transaction that ends a turn and never decremented or deleted (no FKs: it outlives the
-- conversation, the agent, the workspace and the organization). `endpoint_id` is text, not uuid:
-- operator-configured endpoint ids are slugs (`03` `agents.model_endpoint_id`), the contract row said uuid.
-- Forward-only.

-- A tolerant integer reader for token counters in `turns.usage`: a non-negative integer of at most 18
-- digits, else 0 (the same reading as `model::counts_of`).
CREATE FUNCTION usage_counter(v jsonb) RETURNS bigint
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE WHEN jsonb_typeof(v) = 'number' AND v::text ~ '^[0-9]{1,18}$' THEN v::text::bigint ELSE 0 END
$$;

-- Whether a usage object carries a non-empty `by_model` array (CASE keeps the length call off non-arrays).
CREATE FUNCTION usage_has_breakdown(u jsonb) RETURNS boolean
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE WHEN jsonb_typeof(u -> 'by_model') = 'array' THEN jsonb_array_length(u -> 'by_model') > 0 ELSE false END
$$;

CREATE TABLE usage_daily (
    org_id             uuid   NOT NULL,
    workspace_id       uuid   NOT NULL,
    agent_id           uuid   NOT NULL,
    day                date   NOT NULL, -- UTC day of turns.finished_at
    provider           text   NULL,
    endpoint_id        text   NULL,
    model              text   NULL,
    turns              bigint NOT NULL DEFAULT 0,
    turn_seconds       bigint NOT NULL DEFAULT 0,
    input_tokens       bigint NOT NULL DEFAULT 0,
    output_tokens      bigint NOT NULL DEFAULT 0,
    cache_read_tokens  bigint NOT NULL DEFAULT 0,
    cache_write_tokens bigint NOT NULL DEFAULT 0,
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now(),
    UNIQUE NULLS NOT DISTINCT (workspace_id, agent_id, day, provider, endpoint_id, model)
);
CREATE INDEX usage_daily_workspace_day_idx ON usage_daily (workspace_id, day);
CREATE INDEX usage_daily_org_day_idx ON usage_daily (org_id, day);
CREATE INDEX usage_daily_day_idx ON usage_daily (day);

CREATE TABLE usage_rollup_state (
    singleton         boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    runtime_watermark timestamptz NULL,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now()
);
INSERT INTO usage_rollup_state (singleton) VALUES (true);

-- `runtime_daily` was never written; the hourly rollup rebuilds it from `runtime_intervals`.
TRUNCATE runtime_daily;
ALTER TABLE runtime_daily ADD COLUMN workspace_id uuid NOT NULL;
CREATE INDEX runtime_daily_workspace_day_idx ON runtime_daily (workspace_id, day);
CREATE INDEX runtime_daily_day_idx ON runtime_daily (day);

-- Backfill from every terminal turn still present and started, with the rules of the live write
-- (`aurora_db::repo::usage::record_turn` holds the same statement, restricted to one turn):
-- the turn and its seconds under the default model's key (`usage` stamp, else `model_basis`, else
-- nulls); tokens per `by_model` entry, or the totals under the default's key when there is no breakdown.
INSERT INTO usage_daily (org_id, workspace_id, agent_id, day, provider, endpoint_id, model, turns,
                         turn_seconds, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens)
SELECT org_id, workspace_id, agent_id, day, provider, endpoint_id, model, sum(turns), sum(turn_seconds),
       sum(input_tokens), sum(output_tokens), sum(cache_read_tokens), sum(cache_write_tokens)
FROM (
    SELECT a.org_id, c.workspace_id, t.agent_id,
           (t.finished_at AT TIME ZONE 'UTC')::date AS day,
           NULLIF(COALESCE(t.usage ->> 'provider', t.model_basis ->> 'provider'), '') AS provider,
           NULLIF(COALESCE(t.usage ->> 'endpoint_id', t.model_basis ->> 'endpoint_id'), '') AS endpoint_id,
           NULLIF(COALESCE(t.usage ->> 'model', t.model_basis ->> 'model'), '') AS model,
           1::bigint AS turns,
           GREATEST(floor(extract(epoch FROM t.finished_at - t.started_at)), 0)::bigint AS turn_seconds,
           CASE WHEN usage_has_breakdown(t.usage)
                THEN 0 ELSE usage_counter(t.usage -> 'input_tokens') END AS input_tokens,
           CASE WHEN usage_has_breakdown(t.usage)
                THEN 0 ELSE usage_counter(t.usage -> 'output_tokens') END AS output_tokens,
           CASE WHEN usage_has_breakdown(t.usage)
                THEN 0 ELSE usage_counter(t.usage -> 'cache_read_tokens') END AS cache_read_tokens,
           CASE WHEN usage_has_breakdown(t.usage)
                THEN 0 ELSE usage_counter(t.usage -> 'cache_write_tokens') END AS cache_write_tokens
    FROM turns t
    JOIN conversations c ON c.id = t.conversation_id
    JOIN agents a ON a.id = t.agent_id
    WHERE t.role = 'turn' AND t.status IN ('done', 'failed', 'aborted')
      AND t.started_at IS NOT NULL AND t.finished_at IS NOT NULL
    UNION ALL
    SELECT a.org_id, c.workspace_id, t.agent_id,
           (t.finished_at AT TIME ZONE 'UTC')::date,
           NULLIF(COALESCE(t.usage ->> 'provider', t.model_basis ->> 'provider'), ''),
           NULLIF(COALESCE(t.usage ->> 'endpoint_id', t.model_basis ->> 'endpoint_id'), ''),
           NULLIF(e ->> 'model', ''),
           0, 0,
           usage_counter(e -> 'input_tokens'), usage_counter(e -> 'output_tokens'),
           usage_counter(e -> 'cache_read_tokens'), usage_counter(e -> 'cache_write_tokens')
    FROM turns t
    JOIN conversations c ON c.id = t.conversation_id
    JOIN agents a ON a.id = t.agent_id
    CROSS JOIN LATERAL jsonb_array_elements(
        CASE WHEN jsonb_typeof(t.usage -> 'by_model') = 'array' THEN t.usage -> 'by_model' ELSE '[]'::jsonb END) AS e
    WHERE t.role = 'turn' AND t.status IN ('done', 'failed', 'aborted')
      AND t.started_at IS NOT NULL AND t.finished_at IS NOT NULL
      AND jsonb_typeof(e) = 'object'
) rows
GROUP BY org_id, workspace_id, agent_id, day, provider, endpoint_id, model;
