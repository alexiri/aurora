-- 03 §`instance_settings` (SET-1, D-236): instance-wide settings as application state, one row
-- per section. Rows are created on the first write to a section; the migration writes no data
-- (the first API start mirrors the configuration's locked keys, D-238(a)).

CREATE TABLE instance_settings (
    section          text PRIMARY KEY CHECK (section ~ '^[a-z][a-z_]{0,31}$'),
    schema_version   int NOT NULL DEFAULT 1,
    value            jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(value) = 'object'),
    generation       bigint NOT NULL DEFAULT 0,
    updated_by       uuid REFERENCES users (id) ON DELETE SET NULL,
    updated_by_label text,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now()
);
