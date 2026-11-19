-- (FIX-167 Part B, D-370, C-1022..C-1024, 03 §admin_attention_notices, 07 §E-mail, §Web Push)
-- One row per (decision item, instance admin): written in the transaction of the state change that
-- creates the item, so a retried or replayed registration never notifies twice. The sweeper claims
-- `mailed_at` / `pushed_at` (claim, then send) and honours the opt-out and the device opt-in.
CREATE TABLE admin_attention_notices (
    kind text NOT NULL CHECK (kind IN
        ('connector_approval', 'package_approval', 'addon_key_changed', 'host_claim_conflict')),
    -- text: connector ids are text, package and source ids are uuids
    subject_id text NOT NULL,
    -- the manifest digest of a connector, the version of a package, the new key fingerprint of a source
    subject_version text NOT NULL,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    title text NOT NULL,
    detail text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    mailed_at timestamptz,
    pushed_at timestamptz,
    PRIMARY KEY (kind, subject_id, subject_version, user_id)
);
CREATE INDEX admin_attention_notices_open_idx ON admin_attention_notices (created_at)
    WHERE mailed_at IS NULL OR pushed_at IS NULL;

-- Account › Notifications (instance admins only in the UI): e-mail me new decision items. Push
-- follows the device's own opt-in.
ALTER TABLE users ADD COLUMN admin_attention_email_enabled boolean NOT NULL DEFAULT true;
