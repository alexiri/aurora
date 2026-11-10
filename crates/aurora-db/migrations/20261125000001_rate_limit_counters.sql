-- (P4-CNT, D-119, SEC-62, 03 §rate_limit_windows / rate_limit_buckets)
-- Shared rate-limit counters for `ratelimit.counters.backend = postgres`. Both tables are
-- UNLOGGED: counters are not a system of record (a crash or a failover to a standby without
-- unlogged data resets windows, which at worst grants one extra window of quota). Created
-- whatever the configured backend is (empty, no cost). Keys hold ids and keyed hashes only.

CREATE UNLOGGED TABLE rate_limit_windows (
    key          text        NOT NULL CHECK (char_length(key) BETWEEN 1 AND 200),
    window_start timestamptz NOT NULL,
    count        bigint      NOT NULL CHECK (count >= 0),
    expires_at   timestamptz NOT NULL,
    PRIMARY KEY (key, window_start)
);
CREATE INDEX rate_limit_windows_expires_at_idx ON rate_limit_windows (expires_at);

CREATE UNLOGGED TABLE rate_limit_buckets (
    key        text        PRIMARY KEY CHECK (char_length(key) BETWEEN 1 AND 200),
    tat        timestamptz NOT NULL,
    expires_at timestamptz NOT NULL
);
CREATE INDEX rate_limit_buckets_expires_at_idx ON rate_limit_buckets (expires_at);
