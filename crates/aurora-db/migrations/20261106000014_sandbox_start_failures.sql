-- S-10: consecutive failed starts (backoff); reset on running.
ALTER TABLE sandboxes ADD COLUMN start_failures int NOT NULL DEFAULT 0;
