-- P5-CALINV (D-274, C-844, SEC-108): calendar invitations an agent sends to the person it works for.
--
-- One row per invitation (a `UID`) so that update, cancel and list can act on it: ids, the iCalendar
-- `UID` and `SEQUENCE`, and the times. The description and the location are never stored; they exist
-- only in the e-mail that was sent. A row follows the agent, the person and the organization
-- (cascade) and the daily retention pass. `calendar_invitation_sends` is the rate-limit ledger: one
-- row per message sent (an invite, an update or a cancel), so a cancelled or updated invitation still
-- counts. The gateway role gets nothing on either table.
CREATE TABLE calendar_invitations (
    id              uuid PRIMARY KEY,
    org_id          uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    workspace_id    uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    agent_id        uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    person_id       uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    conversation_id uuid NULL REFERENCES conversations (id) ON DELETE SET NULL,
    uid             text NOT NULL,
    sequence        integer NOT NULL DEFAULT 0 CHECK (sequence >= 0),
    status          text NOT NULL CHECK (status IN ('sent', 'cancelled')),
    title           text NOT NULL,
    starts_at       timestamptz NOT NULL,
    ends_at         timestamptz NOT NULL,
    timezone        text NOT NULL,
    all_day         boolean NOT NULL DEFAULT false,
    repeat          jsonb NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX calendar_invitations_person_agent
    ON calendar_invitations (agent_id, person_id, created_at DESC);
CREATE INDEX calendar_invitations_org ON calendar_invitations (org_id);
CREATE INDEX calendar_invitations_person ON calendar_invitations (person_id);
CREATE INDEX calendar_invitations_retention ON calendar_invitations (updated_at);

CREATE TABLE calendar_invitation_sends (
    id        uuid PRIMARY KEY,
    org_id    uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    agent_id  uuid NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    person_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    sent_at   timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX calendar_invitation_sends_person ON calendar_invitation_sends (person_id, sent_at DESC);
CREATE INDEX calendar_invitation_sends_agent ON calendar_invitation_sends (agent_id, person_id, sent_at DESC);
CREATE INDEX calendar_invitation_sends_org ON calendar_invitation_sends (org_id);
