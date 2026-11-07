-- RFE-015 (phase 2): attachment retention (bytes purged, row kept) and the per-org storage
-- counter. Additive only: two nullable columns and one new table; no existing data is rewritten.

ALTER TABLE attachments
    ADD COLUMN purged_at timestamptz,
    ADD COLUMN purge_reason text CHECK (purge_reason IN ('retention', 'deleted'));
-- The retention sweep scans live blobs by age.
CREATE INDEX attachments_live_created_idx ON attachments (created_at)
    WHERE purged_at IS NULL AND status <> 'failed';

CREATE TABLE org_storage_usage (
    org_id           uuid PRIMARY KEY REFERENCES organizations (id) ON DELETE CASCADE,
    attachment_bytes bigint NOT NULL DEFAULT 0,
    updated_at       timestamptz NOT NULL DEFAULT now()
);

-- Counter maintenance lives in the database so that every path that inserts, purges or deletes
-- attachment rows (including cascades from conversation/workspace/agent deletes) keeps it exact
-- inside the same transaction. A row counts while it holds bytes: not purged and not `failed`
-- (a `failed` row carries no blob). The daily sweep reconciles any residual drift.
CREATE FUNCTION attachments_usage_delta() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    d bigint := 0;
    o uuid;
BEGIN
    IF TG_OP IN ('UPDATE', 'DELETE') AND OLD.purged_at IS NULL AND OLD.status <> 'failed' THEN
        d := d - OLD.size_bytes;
    END IF;
    IF TG_OP IN ('INSERT', 'UPDATE') AND NEW.purged_at IS NULL AND NEW.status <> 'failed' THEN
        d := d + NEW.size_bytes;
    END IF;
    IF d = 0 THEN
        RETURN NULL;
    END IF;
    SELECT org_id INTO o FROM workspaces
     WHERE id = CASE WHEN TG_OP = 'DELETE' THEN OLD.workspace_id ELSE NEW.workspace_id END;
    IF o IS NULL THEN
        RETURN NULL; -- the workspace is going away in a cascade
    END IF;
    INSERT INTO org_storage_usage (org_id, attachment_bytes, updated_at)
        SELECT id, d, now() FROM organizations WHERE id = o
    ON CONFLICT (org_id) DO UPDATE
        SET attachment_bytes = org_storage_usage.attachment_bytes + d, updated_at = now();
    RETURN NULL;
END $$;

CREATE TRIGGER attachments_usage_ins AFTER INSERT ON attachments
    FOR EACH ROW EXECUTE FUNCTION attachments_usage_delta();
CREATE TRIGGER attachments_usage_upd AFTER UPDATE OF size_bytes, status, purged_at, workspace_id
    ON attachments FOR EACH ROW EXECUTE FUNCTION attachments_usage_delta();
CREATE TRIGGER attachments_usage_del AFTER DELETE ON attachments
    FOR EACH ROW EXECUTE FUNCTION attachments_usage_delta();

-- Counter for existing data.
INSERT INTO org_storage_usage (org_id, attachment_bytes, updated_at)
    SELECT w.org_id, COALESCE(sum(a.size_bytes), 0), now()
      FROM attachments a JOIN workspaces w ON w.id = a.workspace_id
     WHERE a.status <> 'failed'
     GROUP BY w.org_id;
