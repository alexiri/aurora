-- P2-025 (RFE-025, D-59): BYO agent controls. Additive; existing rows take the defaults
-- (no source list, context credentials not required, workspace posture inherits).
ALTER TABLE agents
    ADD COLUMN byo_source_cidrs text[] NULL,
    ADD COLUMN byo_require_context boolean NOT NULL DEFAULT false,
    ADD CONSTRAINT agents_byo_source_cidrs_size_ck
        CHECK (byo_source_cidrs IS NULL OR cardinality(byo_source_cidrs) BETWEEN 1 AND 32),
    ADD CONSTRAINT agents_byo_only_ck
        CHECK (kind = 'byo' OR (byo_source_cidrs IS NULL AND NOT byo_require_context));

ALTER TABLE workspaces
    ADD COLUMN byo_posture text NULL
        CONSTRAINT workspaces_byo_posture_ck CHECK (byo_posture IN ('enabled', 'owners_only', 'disabled'));

-- The labelled run of a BYO context credential (08 §11.9 d): shown on request-log rows and
-- approval cards. Additive; null for everything that has no label.
ALTER TABLE request_log ADD COLUMN context_label text NULL;
ALTER TABLE approvals ADD COLUMN context_label text NULL;
