-- FIX-005 (FUP-M4.5, docs/12 §Sync protocol): any change to brief, name/handle, timezone,
-- grants/connected apps, peers (agent links), schedules or the active plugin snapshot bumps the
-- affected agents' `home_sync_generation`. Done with triggers so every write path (including
-- cascades) is covered. Skills, memory, memory language and presences already bump explicitly.

CREATE FUNCTION home_bump_agent(a uuid) RETURNS void LANGUAGE sql AS
$$ UPDATE agents SET home_sync_generation = home_sync_generation + 1 WHERE id = a $$;

-- agents: name / handle / brief (render inputs `agent`, `brief`)
CREATE FUNCTION trg_agents_home_bump() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.home_sync_generation = OLD.home_sync_generation
       AND (NEW.name IS DISTINCT FROM OLD.name OR NEW.handle IS DISTINCT FROM OLD.handle
            OR NEW.brief IS DISTINCT FROM OLD.brief) THEN
        NEW.home_sync_generation := OLD.home_sync_generation + 1;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER agents_home_bump BEFORE UPDATE ON agents
    FOR EACH ROW EXECUTE FUNCTION trg_agents_home_bump();

-- workspaces: name / timezone (the workspace generation is part of every agent's desired value)
CREATE FUNCTION trg_workspaces_home_bump() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.home_sync_generation = OLD.home_sync_generation
       AND (NEW.name IS DISTINCT FROM OLD.name OR NEW.timezone IS DISTINCT FROM OLD.timezone) THEN
        NEW.home_sync_generation := OLD.home_sync_generation + 1;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER workspaces_home_bump BEFORE UPDATE ON workspaces
    FOR EACH ROW EXECUTE FUNCTION trg_workspaces_home_bump();

-- grants on connections (apps with an effective grant)
CREATE FUNCTION trg_grants_home_bump() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP IN ('UPDATE', 'DELETE') AND OLD.connection_id IS NOT NULL THEN
        PERFORM home_bump_agent(OLD.agent_id);
    END IF;
    IF TG_OP IN ('INSERT', 'UPDATE') AND NEW.connection_id IS NOT NULL
       AND (TG_OP = 'INSERT' OR NEW.agent_id IS DISTINCT FROM OLD.agent_id) THEN
        PERFORM home_bump_agent(NEW.agent_id);
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER grants_home_bump AFTER INSERT OR UPDATE OR DELETE ON grants
    FOR EACH ROW EXECUTE FUNCTION trg_grants_home_bump();

-- connection label / status / app (rendered accounts)
CREATE FUNCTION trg_connections_home_bump() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.label IS DISTINCT FROM OLD.label OR NEW.status IS DISTINCT FROM OLD.status
       OR NEW.app_id IS DISTINCT FROM OLD.app_id THEN
        UPDATE agents SET home_sync_generation = home_sync_generation + 1
         WHERE id IN (SELECT agent_id FROM grants WHERE connection_id = NEW.id);
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER connections_home_bump AFTER UPDATE ON app_connections
    FOR EACH ROW EXECUTE FUNCTION trg_connections_home_bump();

-- agent links: peers and their policies
CREATE FUNCTION trg_agent_links_home_bump() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP IN ('UPDATE', 'DELETE') THEN
        PERFORM home_bump_agent(OLD.agent_a_id);
        PERFORM home_bump_agent(OLD.agent_b_id);
    ELSE
        PERFORM home_bump_agent(NEW.agent_a_id);
        PERFORM home_bump_agent(NEW.agent_b_id);
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER agent_links_home_bump AFTER INSERT OR DELETE ON agent_links
    FOR EACH ROW EXECUTE FUNCTION trg_agent_links_home_bump();
CREATE TRIGGER agent_links_home_bump_upd AFTER UPDATE OF policy_a, policy_b ON agent_links
    FOR EACH ROW EXECUTE FUNCTION trg_agent_links_home_bump();

-- schedules summary
CREATE FUNCTION trg_schedules_home_bump() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM home_bump_agent(OLD.agent_id);
    ELSE
        PERFORM home_bump_agent(NEW.agent_id);
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER schedules_home_bump AFTER INSERT OR DELETE ON schedules
    FOR EACH ROW EXECUTE FUNCTION trg_schedules_home_bump();
CREATE TRIGGER schedules_home_bump_upd AFTER UPDATE OF name, cron, timezone, enabled ON schedules
    FOR EACH ROW EXECUTE FUNCTION trg_schedules_home_bump();

-- a new active plugin snapshot may change snippets / docs links / app names
CREATE FUNCTION trg_snapshots_home_bump() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE agents SET home_sync_generation = home_sync_generation + 1
     WHERE id IN (SELECT DISTINCT agent_id FROM grants WHERE connection_id IS NOT NULL);
    RETURN NULL;
END $$;
CREATE TRIGGER registry_snapshots_home_bump AFTER INSERT ON registry_snapshots
    FOR EACH STATEMENT EXECUTE FUNCTION trg_snapshots_home_bump();
