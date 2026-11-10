-- (P4-GSL, D-148, D-149, SEC-83, 03 §skill_libraries / §skills library columns /
-- §skill_library_updates) Git-backed skill library: an org connects a repository through an
-- org-scoped app connection; skills are *added* (stored, pinned to a commit, read-only); a refresh
-- only writes proposals. Additive: no existing row changes meaning. The gateway database role
-- receives nothing on these tables (the gateway reads repositories, it never reads libraries).

CREATE TABLE skill_libraries (
    id                  uuid PRIMARY KEY,
    org_id              uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    label               text NOT NULL CHECK (char_length(label) BETWEEN 1 AND 80),
    -- The org-scoped connection (service-enforced at create and at every refresh); set null when
    -- the connection is deleted (status error, last_error connection_deleted).
    connection_id       uuid REFERENCES app_connections (id) ON DELETE SET NULL,
    app_id              text NOT NULL,
    repo                text NOT NULL CHECK (char_length(repo) BETWEEN 1 AND 200),
    ref                 text CHECK (ref IS NULL OR char_length(ref) BETWEEN 1 AND 200),
    path                text NOT NULL DEFAULT '' CHECK (char_length(path) <= 200),
    status              text NOT NULL DEFAULT 'active'
                        CHECK (status IN ('active', 'paused', 'error', 'over_budget')),
    last_head_sha       text,
    last_refresh_at     timestamptz,
    next_refresh_at     timestamptz NOT NULL DEFAULT now(),
    -- Consecutive failed refreshes: drives the 15 min -> 1 h -> 6 h backoff.
    refresh_failures    integer NOT NULL DEFAULT 0 CHECK (refresh_failures >= 0),
    last_error          text CHECK (last_error IS NULL OR char_length(last_error) <= 200),
    refresh_lease_until timestamptz,
    lease_owner         uuid,
    -- "Check now" (at most once per LIBRARY_CHECK_NOW_INTERVAL).
    check_requested_at  timestamptz,
    created_by_user_id  uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX skill_libraries_label_uq ON skill_libraries (org_id, label);
CREATE INDEX skill_libraries_org_idx ON skill_libraries (org_id);
CREATE INDEX skill_libraries_due_idx ON skill_libraries (next_refresh_at)
    WHERE status IN ('active', 'error', 'over_budget');

ALTER TABLE skills
    ADD COLUMN library_id uuid REFERENCES skill_libraries (id) ON DELETE SET NULL,
    ADD COLUMN library_label text,
    ADD COLUMN library_repo text,
    ADD COLUMN library_path text,
    ADD COLUMN library_commit_sha text,
    ADD COLUMN library_blobs jsonb,
    ADD COLUMN library_content_sha256 bytea,
    ADD COLUMN library_pinned_at timestamptz,
    ADD COLUMN library_pinned_by_user_id uuid REFERENCES users (id) ON DELETE SET NULL,
    ADD CONSTRAINT skills_library_cols CHECK (
        library_id IS NULL OR (
            library_path IS NOT NULL AND library_commit_sha IS NOT NULL
            AND library_blobs IS NOT NULL AND library_content_sha256 IS NOT NULL));
CREATE INDEX skills_library_idx ON skills (library_id) WHERE library_id IS NOT NULL;

CREATE TABLE skill_library_updates (
    id                 uuid PRIMARY KEY,
    skill_id           uuid NOT NULL REFERENCES skills (id) ON DELETE CASCADE,
    library_id         uuid REFERENCES skill_libraries (id) ON DELETE SET NULL,
    kind               text NOT NULL CHECK (kind IN ('changed', 'removed', 'invalid')),
    status             text NOT NULL CHECK (status IN ('pending', 'applied', 'rejected', 'superseded')),
    commit_sha         text NOT NULL,
    -- changed: { body, description, files: [{ path, content_b64, executable }], blobs }
    proposed           jsonb,
    -- What an approval must quote: SHA-256 of the canonical proposed state (changed), of the marker
    -- (removed) or of the blob set and reason (invalid). The same state hashes the same.
    content_sha256     bytea,
    reason             text CHECK (reason IS NULL OR char_length(reason) <= 300),
    -- Size of `proposed` for the per-org open-proposal budget.
    bytes              integer NOT NULL DEFAULT 0 CHECK (bytes >= 0),
    created_at         timestamptz NOT NULL DEFAULT now(),
    decided_at         timestamptz,
    decided_by_user_id uuid REFERENCES users (id) ON DELETE SET NULL
);
CREATE UNIQUE INDEX skill_library_updates_pending_uq ON skill_library_updates (skill_id)
    WHERE status = 'pending';
CREATE INDEX skill_library_updates_library_idx ON skill_library_updates (library_id);
CREATE INDEX skill_library_updates_decided_idx ON skill_library_updates (decided_at)
    WHERE status <> 'pending';
