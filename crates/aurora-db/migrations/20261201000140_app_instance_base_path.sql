-- Self-hosted apps served under a path (FIX-145, D-231, docs/03 §app_instances). Additive: one
-- nullable column (existing rows are served at the host root, as before) and a wider unique key.

ALTER TABLE app_instances ADD COLUMN base_path text;

-- Defence in depth for SEC-59, like the host CHECK: starts with `/`, 1-8 segments of 1-64
-- unreserved characters, no `.` or `..` segment, no trailing `/`, at most 128 characters. The API
-- validates the same shape (and normalizes) before insert.
ALTER TABLE app_instances ADD CONSTRAINT app_instances_base_path_ck CHECK (
    base_path IS NULL OR (
        base_path ~ '^(/[A-Za-z0-9._~-]{1,64}){1,8}$'
        AND base_path !~ '(^|/)\.\.?(/|$)'
        AND char_length(base_path) <= 128
    )
);

ALTER TABLE app_instances DROP CONSTRAINT app_instances_scope_uq;
ALTER TABLE app_instances ADD CONSTRAINT app_instances_scope_uq
    UNIQUE NULLS NOT DISTINCT (org_id, app_id, host, base_path);
