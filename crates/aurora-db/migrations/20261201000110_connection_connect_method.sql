-- FIX-123 (D-195(a), D-214, 03 §app_connections): `connect_method` records which manifest
-- `connect[]` method created (or last reconnected) a connection, so that one app can offer several
-- ways to authenticate and the gateway's refresh/mint paths and the tool filter use the method's
-- own `token` / `connect_methods`. Null = unknown: the app's `token`, no tool hidden.
--
-- Backfill: from the stored manifest of the package version the connection recorded
-- (`app_version`), only where that app declares exactly one method (unambiguous). Rows of an
-- app with several methods stay null, as do rows whose package version is no longer stored.
-- Idempotent: only null rows are touched.
ALTER TABLE app_connections ADD COLUMN IF NOT EXISTS connect_method text
    CHECK (connect_method IS NULL OR char_length(connect_method) BETWEEN 1 AND 64);

UPDATE app_connections c
   SET connect_method = m.method
  FROM (
        SELECT p.package_id, p.version, a -> 'about' ->> 'id' AS app_id,
               a -> 'connect' -> 0 ->> 'id' AS method
          FROM plugin_packages p,
               LATERAL jsonb_array_elements(COALESCE(p.manifest -> 'apps', '[]'::jsonb)) a
         WHERE jsonb_typeof(a -> 'connect') = 'array'
           AND jsonb_array_length(a -> 'connect') = 1
       ) m
 WHERE c.connect_method IS NULL
   AND c.package_id = m.package_id
   AND c.app_version = m.version
   AND c.app_id = m.app_id
   AND m.method IS NOT NULL
   AND char_length(m.method) BETWEEN 1 AND 64;
