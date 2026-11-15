-- FIX-126: the Storage pages are served from a snapshot shared by every API replica, so a page
-- view never recomputes the instance breakdown (and is no longer an `export`-class request).
-- `scope` is `instance` or an organization id (text).

CREATE TABLE storage_snapshots (
    scope       text PRIMARY KEY,
    payload     jsonb NOT NULL,
    computed_at timestamptz NOT NULL DEFAULT now()
);
