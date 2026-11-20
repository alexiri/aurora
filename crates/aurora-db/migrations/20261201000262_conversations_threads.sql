-- P5-REFINE-7 (D-409; C-1073, C-1074, C-1079): several direct conversations per person and
-- agent, one thread per automation run, recorded duplicate webhook deliveries.
--
-- Additive except the dropped unique index (one direct conversation per (agent, person)) and the
-- widened `webhook_deliveries.status` CHECK; both are reversible by re-creating them (the old
-- unique index only while every person still has a single direct conversation per agent).

-- C-1073: several direct conversations. The first keeps `external_ref = user:<uid>`, later ones
-- `user:<uid>:<uuid>`; the (agent_id, kind, external_ref) unique constraint is unchanged.
DROP INDEX conversations_direct_user_uq;
CREATE INDEX conversations_direct_person_idx
    ON conversations (agent_id, direct_user_id, last_activity_at DESC) WHERE kind = 'direct';

-- Who set the title: `auto` (the first person message, or none yet), `person` (a rename),
-- `automation` (the schedule's or webhook's name, kept in sync).
ALTER TABLE conversations
    ADD COLUMN title_source text NOT NULL DEFAULT 'auto'
        CHECK (title_source IN ('auto', 'person', 'automation'));

-- C-1074: the automation a run thread belongs to (no foreign key: the thread outlives the
-- schedule or webhook, marked by `automation_deleted_at`).
ALTER TABLE conversations
    ADD COLUMN automation_kind text NULL CHECK (automation_kind IN ('schedule', 'webhook')),
    ADD COLUMN automation_id uuid NULL,
    ADD COLUMN automation_deleted_at timestamptz NULL,
    ADD CONSTRAINT conversations_automation_pair_ck
        CHECK ((automation_kind IS NULL) = (automation_id IS NULL));
CREATE INDEX conversations_automation_idx
    ON conversations (automation_kind, automation_id, last_activity_at DESC)
    WHERE automation_id IS NOT NULL;

-- Backfill: today's one conversation per schedule (`schedule:<id>`) and per webhook
-- (`webhook:<id>`) becomes the oldest thread of its automation.
UPDATE conversations
   SET automation_kind = kind,
       automation_id = substr(external_ref, length(kind) + 2)::uuid,
       title_source = 'automation'
 WHERE kind IN ('schedule', 'webhook')
   AND external_ref ~ '^(schedule|webhook):[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
   AND split_part(external_ref, ':', 1) = kind;

-- The title follows the automation's current name (a rename before this change left it stale).
UPDATE conversations c SET title = s.name
  FROM schedules s
 WHERE c.automation_kind = 'schedule' AND s.id = c.automation_id;
UPDATE conversations c SET title = w.name
  FROM webhooks w
 WHERE c.automation_kind = 'webhook' AND w.id = c.automation_id;

-- Threads whose schedule or webhook is gone are marked deleted (their transcripts stay).
UPDATE conversations c
   SET automation_deleted_at = now()
 WHERE c.automation_kind = 'schedule'
   AND NOT EXISTS (SELECT 1 FROM schedules s WHERE s.id = c.automation_id);
UPDATE conversations c
   SET automation_deleted_at = now()
 WHERE c.automation_kind = 'webhook'
   AND NOT EXISTS (SELECT 1 FROM webhooks w WHERE w.id = c.automation_id);

-- C-1079: a repeated delivery is recorded (`reason` = the original delivery's id).
ALTER TABLE webhook_deliveries DROP CONSTRAINT webhook_deliveries_status_check;
ALTER TABLE webhook_deliveries
    ADD CONSTRAINT webhook_deliveries_status_check
    CHECK (status IN ('accepted', 'rejected', 'paused', 'duplicate'));
