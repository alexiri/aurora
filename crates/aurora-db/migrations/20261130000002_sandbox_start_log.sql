-- FIX-029 (D-160, C-612): a per-boot start log on the sandbox. Additive; existing rows keep
-- '[]'. The ring (≤ 30 entries, ≤ 8 KiB, oldest dropped, detail ≤ 300 chars, sanitised at
-- write) is maintained by the API; every new boot resets it with the other per-boot fields.
ALTER TABLE sandboxes
    ADD COLUMN start_log jsonb NOT NULL DEFAULT '[]'::jsonb;
