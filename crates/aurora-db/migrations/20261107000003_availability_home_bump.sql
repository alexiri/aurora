-- FIX-007 (docs/09 §App availability): the rendered `apps` honour the org's availability posture
-- and allow-list, so changing either bumps the affected agents' `home_sync_generation`.

CREATE FUNCTION trg_org_availability_home_bump() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE r org_app_availability;
BEGIN
    IF TG_OP = 'DELETE' THEN r := OLD; ELSE r := NEW; END IF;
    UPDATE agents SET home_sync_generation = home_sync_generation + 1
     WHERE org_id = r.org_id AND (r.workspace_id IS NULL OR workspace_id = r.workspace_id);
    RETURN NULL;
END $$;
CREATE TRIGGER org_app_availability_home_bump
    AFTER INSERT OR UPDATE OR DELETE ON org_app_availability
    FOR EACH ROW EXECUTE FUNCTION trg_org_availability_home_bump();

CREATE FUNCTION trg_org_posture_home_bump() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.app_posture IS DISTINCT FROM OLD.app_posture THEN
        UPDATE agents SET home_sync_generation = home_sync_generation + 1 WHERE org_id = NEW.id;
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER organizations_posture_home_bump AFTER UPDATE ON organizations
    FOR EACH ROW EXECUTE FUNCTION trg_org_posture_home_bump();
