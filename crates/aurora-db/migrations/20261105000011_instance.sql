-- 03 §11 Instance. (`migrations` is managed by sqlx as _sqlx_migrations.)

CREATE TABLE gateway_instances (
    id            uuid PRIMARY KEY, -- per process start
    hostname      text NOT NULL,
    version       text NOT NULL,
    snapshot_hash text, -- churn column
    started_at    timestamptz NOT NULL,
    last_seen_at  timestamptz NOT NULL, -- churn column (every 15 s)
    stopped_at    timestamptz,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
);
