-- P5-MODELS-1 (D-302, D-303, C-876): model roles and the subagent default of an agent.
--
-- `model_roles` = { endpoint_id, large?, medium?, small? }, stamped with the endpoint it was chosen
-- for (08 §Model roles). `subagent_model` = which role subagents use when their definition names no
-- model. Both are checked here as well as in the API (SEC-122: model ids are inert data).
--
-- The instructions' Models line (10 §Agent instructions, C-884) is a home-sync render input, so a
-- change of the default model, its endpoint, the roles or the subagent default bumps the agent's
-- `home_sync_generation` like a change of name or brief does (FIX-005).

ALTER TABLE agents
    ADD COLUMN model_roles jsonb,
    ADD COLUMN subagent_model text NOT NULL DEFAULT 'inherit';

ALTER TABLE agents
    ADD CONSTRAINT agents_subagent_model_check
        CHECK (subagent_model IN ('inherit', 'large', 'medium', 'small')),
    ADD CONSTRAINT agents_model_roles_check CHECK (
        model_roles IS NULL OR (
            jsonb_typeof(model_roles) = 'object'
            AND jsonb_typeof(model_roles -> 'endpoint_id') = 'string'
            AND (model_roles - 'endpoint_id' - 'large' - 'medium' - 'small') = '{}'::jsonb
            AND (model_roles ->> 'large' IS NULL
                 OR model_roles ->> 'large' ~ '^[A-Za-z0-9._:/@-]{1,200}$')
            AND (model_roles ->> 'medium' IS NULL
                 OR model_roles ->> 'medium' ~ '^[A-Za-z0-9._:/@-]{1,200}$')
            AND (model_roles ->> 'small' IS NULL
                 OR model_roles ->> 'small' ~ '^[A-Za-z0-9._:/@-]{1,200}$')
        )
    );

CREATE OR REPLACE FUNCTION trg_agents_home_bump() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.home_sync_generation = OLD.home_sync_generation
       AND (NEW.name IS DISTINCT FROM OLD.name OR NEW.handle IS DISTINCT FROM OLD.handle
            OR NEW.brief IS DISTINCT FROM OLD.brief
            OR NEW.model_provider IS DISTINCT FROM OLD.model_provider
            OR NEW.model_endpoint_id IS DISTINCT FROM OLD.model_endpoint_id
            OR NEW.model_id IS DISTINCT FROM OLD.model_id
            OR NEW.model_roles IS DISTINCT FROM OLD.model_roles
            OR NEW.subagent_model IS DISTINCT FROM OLD.subagent_model) THEN
        NEW.home_sync_generation := OLD.home_sync_generation + 1;
    END IF;
    RETURN NEW;
END $$;
