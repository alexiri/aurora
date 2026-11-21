-- (P5-SKLIB-2, D-449, C-1128; 03 §skill_libraries, §skill_library_org_state) Instance-level skill
-- libraries: `org_id` null = a library of the whole installation, managed by instance admins and
-- visible to every organization. The open-proposal budget is per organization, so "over budget"
-- moves from the library's status to a per-(library, organization) state row.
--
-- Additive for readers: no column is dropped; the `over_budget` value stays allowed in the status
-- CHECK for one release but is no longer written.

ALTER TABLE skill_libraries ALTER COLUMN org_id DROP NOT NULL;

-- Labels and addresses are unique per organization, and among the instance libraries (org_id null
-- counts as one scope: the nil UUID).
DROP INDEX skill_libraries_label_uq;
CREATE UNIQUE INDEX skill_libraries_label_uq
    ON skill_libraries (coalesce(org_id, '00000000-0000-0000-0000-000000000000'::uuid), label);
DROP INDEX skill_libraries_url_uq;
CREATE UNIQUE INDEX skill_libraries_url_uq
    ON skill_libraries (coalesce(org_id, '00000000-0000-0000-0000-000000000000'::uuid), url,
                        coalesce(ref, ''), path)
    WHERE url IS NOT NULL;

CREATE TABLE skill_library_org_state (
    library_id        uuid NOT NULL REFERENCES skill_libraries (id) ON DELETE CASCADE,
    org_id            uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    -- Since when this organization's open proposals of the library did not fit its budget.
    over_budget_since timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (library_id, org_id)
);
CREATE INDEX skill_library_org_state_org_idx ON skill_library_org_state (org_id);

-- A library that was over budget keeps showing it in its organization's view: its status goes
-- back to `active` and its organization gets the state row.
INSERT INTO skill_library_org_state (library_id, org_id)
SELECT id, org_id FROM skill_libraries WHERE status = 'over_budget' AND org_id IS NOT NULL;
UPDATE skill_libraries SET status = 'active', updated_at = now() WHERE status = 'over_budget';
