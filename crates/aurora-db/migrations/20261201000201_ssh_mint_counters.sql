-- P4-SSH-1 (D-239(c); C-791): the certificate-mint rate limit, a ledger of one counter per
-- (person, agent, hour). Its own table rather than `aurora-counters`: the limit must be exact
-- across API replicas whatever `ratelimit.counters.backend` says and must never fail open. No
-- foreign keys (a ledger, like `preview_grant_uses`); the retention sweep prunes rows older than
-- 24 h. The gateway database role gets nothing.
CREATE TABLE ssh_mint_counters (
    user_id      uuid NOT NULL,
    agent_id     uuid NOT NULL,
    window_start timestamptz NOT NULL,
    count        int NOT NULL DEFAULT 0,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, agent_id, window_start)
);
CREATE INDEX ssh_mint_counters_window_idx ON ssh_mint_counters (window_start);
