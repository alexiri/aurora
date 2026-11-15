-- FIX-108 (REVIEW-5 AUTH-1): a removal marker per (org, user). An admin removal of a member
-- is a hard delete, so nothing told "removed" from "never joined" and the person's next
-- sign-in through a provider with a `jit` block restored the membership and bindings. The
-- marker is written by that removal, consulted by JIT, and cleared whenever a membership row
-- is created again (invitation, admin add, claim, SCIM). Existing removals leave no marker.
CREATE TABLE org_member_removals (
    org_id     uuid        NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    user_id    uuid        NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    removed_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, user_id)
);

-- FIX-108 (AUTH-5): the CLI approval screen shows where the login was requested from: a coarse
-- address (IPv4 /24, IPv6 /48) recorded at `POST /v1/auth/cli`; the start time is `created_at`.
ALTER TABLE cli_logins ADD COLUMN requested_addr text;
