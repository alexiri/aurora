-- P5-POLISH-3 (D-383, C-1033, SEC-129): real use and tests recorded apart.
--
-- Until now one `last_check_*` pair held both what an agent's real call showed (the gateway, C-985) and
-- what a Test said (C-943), so a passed Test could read as "Works". From here:
--   * `last_ok_at`, `last_error_at`, `last_error`: written by the gateway for agent calls only.
--   * `last_test_at`, `last_test_result`: written by the Test route only.
-- `last_check_at` / `last_check_result` are backfilled into the use columns below and then kept, no longer
-- written, until a later cleanup migration. Informational: nothing here decides what an agent may do.
-- The gateway role's `UPDATE (last_ok_at, last_error_at, last_error)` grant is applied with the other role
-- grants (`grants.rs`).
ALTER TABLE app_connections
    ADD COLUMN last_ok_at timestamptz NULL,
    ADD COLUMN last_error_at timestamptz NULL,
    ADD COLUMN last_error text NULL
        CONSTRAINT app_connections_last_error_check
        CHECK (last_error IN ('rejected', 'unreachable', 'unexpected', 'expired')),
    ADD COLUMN last_test_at timestamptz NULL,
    ADD COLUMN last_test_result text NULL
        CONSTRAINT app_connections_last_test_result_check
        CHECK (last_test_result IN ('ok', 'rejected', 'unreachable', 'unexpected'));

ALTER TABLE secrets
    ADD COLUMN last_ok_at timestamptz NULL,
    ADD COLUMN last_error_at timestamptz NULL,
    ADD COLUMN last_error text NULL
        CONSTRAINT secrets_last_error_check
        CHECK (last_error IN ('rejected', 'unreachable', 'unexpected', 'expired')),
    ADD COLUMN last_test_at timestamptz NULL,
    ADD COLUMN last_test_result text NULL
        CONSTRAINT secrets_last_test_result_check
        CHECK (last_test_result IN ('ok', 'rejected', 'unreachable', 'unexpected'));

UPDATE app_connections
   SET last_ok_at = CASE WHEN last_check_result = 'ok' THEN last_check_at END,
       last_error_at = CASE WHEN last_check_result <> 'ok' THEN last_check_at END,
       last_error = CASE WHEN last_check_result <> 'ok' THEN last_check_result END
 WHERE last_check_result IS NOT NULL;

UPDATE secrets
   SET last_ok_at = CASE WHEN last_check_result = 'ok' THEN last_check_at END,
       last_error_at = CASE WHEN last_check_result <> 'ok' THEN last_check_at END,
       last_error = CASE WHEN last_check_result <> 'ok' THEN last_check_result END
 WHERE last_check_result IS NOT NULL;
