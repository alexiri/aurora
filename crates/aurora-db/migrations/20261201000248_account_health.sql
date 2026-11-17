-- P5-UX-14 (D-335, D-350, C-944, C-985): account health.
--
-- `last_check_at` / `last_check_result` on both kinds of account. Written by `POST /v1/accounts/{type}/{id}/test`
-- (and the post-connect test) and by the gateway when real use shows a change of state (an upstream 401 on a
-- request with the account's credential, then a later success). Informational: nothing reads them to decide
-- what an agent may do, and recording never changes `status`. `not_testable` is answered but never stored.
-- The gateway role's `UPDATE (last_check_at, last_check_result)` grant is applied with the other role grants
-- (`grants.rs`).
ALTER TABLE app_connections
    ADD COLUMN last_check_at timestamptz NULL,
    ADD COLUMN last_check_result text NULL
        CONSTRAINT app_connections_last_check_result_check
        CHECK (last_check_result IN ('ok', 'rejected', 'unreachable', 'unexpected'));

ALTER TABLE secrets
    ADD COLUMN last_check_at timestamptz NULL,
    ADD COLUMN last_check_result text NULL
        CONSTRAINT secrets_last_check_result_check
        CHECK (last_check_result IN ('ok', 'rejected', 'unreachable', 'unexpected'));
