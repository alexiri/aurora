-- P5-REFINE-6 (C-1070, D-407): the Test of a sign-in app (an app's OAuth client).
--
-- `POST /v1/oauth-clients/{id}/test` sends one token request with a made-up code and records how
-- the provider answered: `ok` (it accepted the client and refused the code), `rejected` (the
-- client id or secret is wrong), `unreachable` (network error, timeout or a destination-guard
-- refusal) or `unexpected`. `not_testable` is answered but never stored. No response text is
-- stored. Existing rows read "Ready (not tested)" until tested or used.
ALTER TABLE app_oauth_clients
    ADD COLUMN last_test_at timestamptz NULL,
    ADD COLUMN last_test_result text NULL
        CHECK (last_test_result IN ('ok', 'rejected', 'unreachable', 'unexpected'));
