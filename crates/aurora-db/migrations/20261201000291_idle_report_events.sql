-- FIX-257: idle usage is counted by event, not by a high-water number, so a report that arrives
-- after a later one is still counted. One row per counted event of an anchor turn; the primary
-- key is what makes a redelivery count nothing. `turns.idle_usage_n` (20261201000287) is no longer
-- read or written; it stays for replicas of the previous release (expand, then contract).
CREATE TABLE idle_usage_events (
    turn_id uuid   NOT NULL REFERENCES turns (id) ON DELETE CASCADE,
    n       bigint NOT NULL,
    PRIMARY KEY (turn_id, n)
);

-- FIX-257: the duplicate check of an idle background list looks a notice up by the event's name.
-- Plain CREATE INDEX, not CONCURRENTLY: migrations run inside a transaction here. Partial: only
-- the notices that carry an event name are in it.
CREATE INDEX transcript_events_notice_event_idx
    ON transcript_events (conversation_id, (payload ->> 'event'))
    WHERE type = 'notice' AND (payload ->> 'event') IS NOT NULL;
