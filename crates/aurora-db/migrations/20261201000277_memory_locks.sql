-- P6-MEMLOCK (C-1148, D-463, D-464, SEC-151): a person can lock a memory so the agent can read
-- it but not change or delete it; people keep editing, restoring, redacting and deleting it.
-- `locked_at` and `locked_by` are set when `locked` becomes true and cleared when it becomes
-- false (maintained by the memory service); `locked_by` is null for an API key without a user
-- and for a person who was later deleted. No index: locks are read with the memory row itself.

ALTER TABLE memories
    ADD COLUMN locked boolean NOT NULL DEFAULT false,
    ADD COLUMN locked_at timestamptz NULL,
    ADD COLUMN locked_by uuid NULL REFERENCES users (id) ON DELETE SET NULL;
