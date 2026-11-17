-- P5-MODELS-2 (D-304, C-879, C-880): usage per model.
--
-- `request_log.llm_model`: the model id a request with an injected key on a governed LLM endpoint asked
-- for (`model_from_request`; null otherwise or when unknown). Audit only: no decision reads it. Added on
-- the partitioned parent, so every partition (and the default one) inherits it; existing rows stay null.
ALTER TABLE request_log ADD COLUMN llm_model text NULL;

-- `turns.model_basis`: what the dispatcher chose for the turn, `{ provider, endpoint_id, model, roles? }`
-- (`roles` = the resolved role model ids). The trust rule of `08` (*Error classification and usage*)
-- needs the default model and the roles at turn end, which the harness must not be able to claim.
-- Null for turns dispatched before this migration and for the scripted `fake` harness.
ALTER TABLE turns ADD COLUMN model_basis jsonb NULL;
