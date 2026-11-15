-- FIX-125: Admin › Audit. `scope=instance` lists `org_id IS NULL` rows newest first; `scope=all`
-- lists every row newest first. Neither is served by the (org_id, created_at) indexes.
CREATE INDEX audit_log_instance_created_idx ON audit_log (created_at DESC, id DESC) WHERE org_id IS NULL;
CREATE INDEX audit_log_created_idx ON audit_log (created_at DESC, id DESC);
