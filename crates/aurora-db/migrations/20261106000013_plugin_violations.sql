-- BE-040: validation findings of `invalid` packages (API-003 PluginPackage.violations).
ALTER TABLE plugin_packages ADD COLUMN violations jsonb NOT NULL DEFAULT '[]';
