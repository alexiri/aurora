-- P5-PERSON (D-402, C-1052..C-1055, SEC-139): the agent knows who it is working for.
-- `users.timezone`: the person's own IANA time zone (validated with chrono-tz by the API, as
-- `workspaces.timezone` is); null = use the workspace's. The turn context block's clock (C-1053).
-- `user_links.handle`: the platform handle last seen for a linked platform user in a `user.lookup`
-- result (C-1055's cache); null until a lookup returns it. No backfill.
-- The gateway reads neither column (it sees users only through `gateway_users`), so no grant.
ALTER TABLE users ADD COLUMN timezone text NULL;
ALTER TABLE user_links ADD COLUMN handle text NULL;
