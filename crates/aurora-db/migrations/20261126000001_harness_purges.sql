-- P4-HAR (phase 4, D-122, D-153): a queued session purge remembers the harness that created the
-- session, so the adapter of that harness (not the agent's current one) deletes the files after a
-- harness switch. Additive and nullable: existing rows keep NULL = "the sandbox's own harness".

ALTER TABLE session_purges ADD COLUMN harness text;
