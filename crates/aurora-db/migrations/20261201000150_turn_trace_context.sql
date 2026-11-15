-- FIX-146 (D-232, C-732, SEC-88): request tracing across the turn queue. `turns.trace_context` is
-- the W3C traceparent of the span that inserted the turn (written once by the API, never taken from
-- a sandbox or supervisor); work on the turn in other processes is a child of it. The gateway role
-- has no grant on `turns`, so the value is copied onto the turn's context credential at mint.
-- Forward-only, nullable, no default, no backfill (ADD COLUMN without a default does not rewrite).
ALTER TABLE turns
    ADD COLUMN trace_context text
    CONSTRAINT turns_trace_context_chk CHECK (trace_context ~ '^00-[0-9a-f]{32}-[0-9a-f]{16}-[0-9a-f]{2}$');
ALTER TABLE gateway_credentials
    ADD COLUMN trace_context text
    CONSTRAINT gateway_credentials_trace_context_chk CHECK (trace_context ~ '^00-[0-9a-f]{32}-[0-9a-f]{16}-[0-9a-f]{2}$');
