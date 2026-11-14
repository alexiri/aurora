-- FIX-116 (REVIEW-5 PLAT-4): the fresh-instance setup gate is a persisted marker, not "no
-- organization exists yet" (the first admin's own sign-in created one and closed the gate).
--
-- `instance_setup.placement_pending` is true on an instance installed with this release (no
-- organization and no saved placement when the migration runs) and stays true until a placement
-- is first saved. An instance that already has organizations or a placement is not pending: it
-- behaves as it always did.

CREATE TABLE instance_setup (
    singleton         boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    placement_pending boolean NOT NULL,
    updated_at        timestamptz NOT NULL DEFAULT now()
);

INSERT INTO instance_setup (singleton, placement_pending)
SELECT true,
       NOT EXISTS (SELECT 1 FROM organizations) AND NOT EXISTS (SELECT 1 FROM instance_placement);
