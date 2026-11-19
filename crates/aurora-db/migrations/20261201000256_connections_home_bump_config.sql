-- P5-WEBHOOK-3 (C-911): the agent's `apps` part renders an outgoing webhook's name, note and slots
-- from `app_connections.config`, so a config-only edit (`PUT .../webhook`) must bump the
-- `home_sync_generation` of the agents granted on the connection, like a label or status change.
CREATE OR REPLACE FUNCTION trg_connections_home_bump() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.label IS DISTINCT FROM OLD.label OR NEW.status IS DISTINCT FROM OLD.status
       OR NEW.app_id IS DISTINCT FROM OLD.app_id OR NEW.config IS DISTINCT FROM OLD.config THEN
        UPDATE agents SET home_sync_generation = home_sync_generation + 1
         WHERE id IN (SELECT agent_id FROM grants WHERE connection_id = NEW.id);
    END IF;
    RETURN NULL;
END $$;
