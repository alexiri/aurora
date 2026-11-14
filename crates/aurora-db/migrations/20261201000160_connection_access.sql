-- FIX-133 (D-240, 03 §app_connections, 07 §Read-only connections): `access` marks a connection
-- `read` (the gateway then allows only the app's read tools through it and blocks write tools and
-- untooled requests, whatever the grants say) or `write` (no restriction beyond the grants). Every
-- existing connection is `write`: nothing changes for them. Migration number taken from the
-- p5-apps block (`20261201000160` … `000169`); newest on master was `000110` (FIX-123).
ALTER TABLE app_connections ADD COLUMN IF NOT EXISTS access text NOT NULL DEFAULT 'write'
    CHECK (access IN ('read', 'write'));
