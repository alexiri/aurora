-- P5-CHAT-1 (D-228, C-725): a message may quote an earlier event of the same conversation.
-- `reply_to_seq` is the target's `transcript_events.seq` (no FK: events are not keyed rows a turn
-- may point at; validated at insert). Forward-only, nullable, no backfill (ADD COLUMN with no
-- default does not rewrite the table). The partial index lets the redaction cascade find the
-- turns that quote a redacted event.
ALTER TABLE turns ADD COLUMN reply_to_seq bigint;
CREATE INDEX turns_reply_to_idx ON turns (conversation_id, reply_to_seq) WHERE reply_to_seq IS NOT NULL;
