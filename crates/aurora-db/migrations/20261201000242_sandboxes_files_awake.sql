-- P5-FILES-1 (D-316; C-900): the Files keep-awake clock and its per-boot cap clock. Written by
-- the API at a Files request on a running sandbox (at most one stamp per sandbox per 30 s per
-- replica); read by the keep-awake predicate. `files_awake_since` is reset at every start claim
-- (the same shape as the terminal columns). The Files wake demand reuses `terminal_wake_at`.
ALTER TABLE sandboxes
    ADD COLUMN files_active_until timestamptz,
    ADD COLUMN files_awake_since  timestamptz;
