-- P5-UX-11 (D-353, C-990, SEC-131): the limit of an organization's account.
--
-- `limit_level` is one of the three access choices (`read`, `ask`, `act`) that no agent's access to the
-- account may exceed. It exists only on organization-scoped connections; the migration fills it from
-- `access` (`read` -> `read`, `write` -> `act`), which is what the limit was derived as until now.
-- `access` stays in step (`read` for `read`, `write` otherwise) and is still what the gateway enforces.
ALTER TABLE app_connections
    ADD COLUMN limit_level text NULL
        CONSTRAINT app_connections_limit_level_check CHECK (limit_level IN ('read', 'ask', 'act')),
    ADD CONSTRAINT app_connections_limit_level_scope_check
        CHECK (workspace_id IS NULL OR limit_level IS NULL);

UPDATE app_connections
   SET limit_level = CASE WHEN access = 'read' THEN 'read' ELSE 'act' END
 WHERE workspace_id IS NULL;
