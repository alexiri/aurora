-- P5-WEBHOOK-2 (D-320, C-909): the core-owned configuration of a send-mode secret-link connection
-- (an outgoing webhook): the template, slots, headers, method, rate limits and preset id. It holds
-- no secret: the URL, the secret values and the signing secret are sealed in the credential.
-- Non-null only for a send-mode connection. The gateway role reads it under its table-level SELECT.
ALTER TABLE app_connections
    ADD COLUMN config jsonb NULL
        CONSTRAINT app_connections_config_check CHECK (config IS NULL OR jsonb_typeof(config) = 'object');
