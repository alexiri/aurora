-- Preset instances (FIX-134, D-243, docs/03 §app_instances). Additive: one column. A preset
-- instance is a package's own ready-made instance (a vendor's region host) that the API seeds
-- from the manifest's `instances.presets` when the package version activates; existing rows
-- (registered by people) stay `false`.

ALTER TABLE app_instances ADD COLUMN builtin boolean NOT NULL DEFAULT false;

-- Preset rows are instance-wide: never org-scoped.
ALTER TABLE app_instances ADD CONSTRAINT app_instances_builtin_scope_ck
    CHECK (NOT builtin OR org_id IS NULL);
