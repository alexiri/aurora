-- FIX-043 (D-144, SEC-80): tombstones of spent preview hostname labels.
-- A share's label is never reused: `create` writes the minted label here in the same
-- transaction as the share row, and deleting the share never removes the tombstone, so the
-- mint's exclusion set (existing rows union tombstones) covers every label that has ever
-- existed (docs/11 §Port previews, docs/17 §Security). No FK on purpose: the tombstone
-- outlives the share it describes (a ledger, per docs/03 §Conventions).

CREATE TABLE preview_labels (
    -- The hostname label: `p` + 24 base32 lowercase chars, exactly as minted (the same value
    -- the share holds in preview_shares.label, under its unique index).
    label      citext PRIMARY KEY,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- Every label that exists is already spent: backfill the ledger in one statement.
INSERT INTO preview_labels (label)
    SELECT label FROM preview_shares;
