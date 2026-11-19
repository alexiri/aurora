-- P5-POLISH-4 (D-386, C-1030, SEC-135): a person asks the organization's owners and admins to make an app
-- ready (register a sign-in app, a server, or switch it on). A request gives nothing: it changes no
-- availability, OAuth client or access, and holds no free text. One open request per organization and app;
-- a second person's ask joins it. It closes itself when the app becomes ready for its askers
-- (`resolved`), when the last asker withdraws, or when an owner or admin declines it. Closed rows are
-- purged after 90 days by the retention sweep. The gateway role gets nothing on these tables.
CREATE TABLE app_setup_requests (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    app_id      text NOT NULL,
    status      text NOT NULL DEFAULT 'open'
        CHECK (status IN ('open', 'resolved', 'declined', 'withdrawn')),
    -- the C-1029 `needs` at the time of the last ask: a subset of oauth_client, instance, admin_material
    needs       jsonb NOT NULL DEFAULT '[]'::jsonb,
    -- the app is switched off for the organization (nothing in `needs` then)
    unavailable boolean NOT NULL DEFAULT false,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    closed_at   timestamptz NULL,
    -- the decliner; null for `resolved` (the system) and `withdrawn`
    closed_by   uuid NULL REFERENCES users (id) ON DELETE SET NULL
);

CREATE UNIQUE INDEX app_setup_requests_open
    ON app_setup_requests (org_id, app_id) WHERE status = 'open';
CREATE INDEX app_setup_requests_closed
    ON app_setup_requests (closed_at) WHERE status <> 'open';

CREATE TABLE app_setup_request_askers (
    request_id   uuid NOT NULL REFERENCES app_setup_requests (id) ON DELETE CASCADE,
    user_id      uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- where the person asked (for the resolution link)
    workspace_id uuid NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    asked_at     timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (request_id, user_id)
);

-- One row per (request, recipient, kind): the claims that keep the e-mail and the push exactly-once across
-- replicas, as `admin_attention_notices` does. kind: `asked` (to each owner and admin, when the request is
-- opened), `more:<yyyymmddhh>` (one digest per hour when others join), `resolved` and `declined` (to askers).
-- `title` and `detail` are fixed sentences built from the app name and display names, never free text.
CREATE TABLE app_setup_request_notices (
    request_id uuid NOT NULL REFERENCES app_setup_requests (id) ON DELETE CASCADE,
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    kind       text NOT NULL,
    title      text NOT NULL,
    detail     text NOT NULL,
    -- the dashboard path the notice links to (never a decision link)
    href       text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    mailed_at  timestamptz NULL,
    pushed_at  timestamptz NULL,
    PRIMARY KEY (request_id, user_id, kind)
);
CREATE INDEX app_setup_request_notices_open
    ON app_setup_request_notices (created_at) WHERE mailed_at IS NULL OR pushed_at IS NULL;
